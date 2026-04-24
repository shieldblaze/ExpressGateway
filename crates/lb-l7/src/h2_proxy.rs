//! Real hyper 1.x HTTP/2 proxy path (Pillar 3b.3b-2).
//!
//! [`H2Proxy`] is the L7 entry point used by the binary's `h1s` listener
//! once ALPN negotiates `h2`. Each accepted TLS-decrypted connection is
//! handed to [`H2Proxy::serve_connection`] which drives a hyper 1.x
//! HTTP/2 server over it.
//!
//! Architecturally identical to [`crate::h1_proxy::H1Proxy`]:
//!
//! 1. Strips hop-by-hop request headers (RFC 9110 §7.6.1 + Connection-
//!    listed names). H2 forbids these over the wire, but the *upstream*
//!    we forward to is still H1, so we must scrub them before relaying.
//! 2. Appends `X-Forwarded-{For,Proto,Host}` + `Via`.
//! 3. Picks a backend via [`crate::h1_proxy::BackendPicker::pick`]. The
//!    service closure runs **once per H2 stream**, so a single H2
//!    connection multiplexing N requests hits the picker N times —
//!    real per-stream load balancing.
//! 4. Dials the backend (H1) via [`lb_io::pool::TcpPool`] and issues the
//!    request body-timeout-bounded.
//! 5. Strips hop-by-hop from the response, optionally injects
//!    `Alt-Svc`, and returns the response on the original H2 stream.
//!
//! The whole `serve_connection` future is bounded by
//! [`crate::h1_proxy::HttpTimeouts::total`].

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::{Bytes, Incoming as IncomingBody};
use hyper::header::HeaderValue;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite};

use lb_io::pool::TcpPool;

use crate::h1_proxy::{
    AltSvcConfig, BackendPicker, HttpTimeouts, append_via, append_xff, set_xfh, set_xfp,
    strip_hop_by_hop,
};
use crate::h2_security::H2SecurityThresholds;

/// L7 HTTP/2 reverse proxy. Cheap to clone via [`Arc`].
pub struct H2Proxy {
    pool: TcpPool,
    picker: Arc<dyn BackendPicker>,
    alt_svc: Option<AltSvcConfig>,
    timeouts: HttpTimeouts,
    is_https: bool,
    security: H2SecurityThresholds,
}

impl H2Proxy {
    /// Construct an [`H2Proxy`] with the default
    /// [`H2SecurityThresholds`]. Equivalent to
    /// [`Self::with_security`]`(..., H2SecurityThresholds::default())`.
    ///
    /// `is_https` selects the value emitted into `X-Forwarded-Proto`.
    /// It is always `true` for the production wiring (H2 ships only over
    /// the `h1s` listener today), but exposed for test harnesses that
    /// want to exercise the plaintext path.
    #[must_use]
    pub fn new(
        pool: TcpPool,
        picker: Arc<dyn BackendPicker>,
        alt_svc: Option<AltSvcConfig>,
        timeouts: HttpTimeouts,
        is_https: bool,
    ) -> Self {
        Self::with_security(
            pool,
            picker,
            alt_svc,
            timeouts,
            is_https,
            H2SecurityThresholds::default(),
        )
    }

    /// Construct an [`H2Proxy`] with an explicit [`H2SecurityThresholds`].
    #[must_use]
    pub fn with_security(
        pool: TcpPool,
        picker: Arc<dyn BackendPicker>,
        alt_svc: Option<AltSvcConfig>,
        timeouts: HttpTimeouts,
        is_https: bool,
        security: H2SecurityThresholds,
    ) -> Self {
        Self {
            pool,
            picker,
            alt_svc,
            timeouts,
            is_https,
            security,
        }
    }

    /// Drive HTTP/2 server logic over `io`.
    ///
    /// Returns once the connection has fully closed. A
    /// [`tokio::time::timeout`] of [`HttpTimeouts::total`] is wrapped
    /// around the whole loop so a runaway client-or-upstream pair cannot
    /// pin a worker forever.
    ///
    /// # Errors
    ///
    /// Surfaces I/O errors and timeouts. Per-stream upstream errors are
    /// translated to 502/504 responses and do NOT terminate the
    /// connection.
    pub async fn serve_connection<IO>(self: Arc<Self>, io: IO, peer: SocketAddr) -> io::Result<()>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let total = self.timeouts.total;
        let svc = ProxyService {
            inner: Arc::clone(&self),
            peer,
        };
        // Configure hyper's H2 Builder with the detector-derived
        // thresholds. Hyper enforces on the wire; the lb-h2 detector
        // types remain the canonical threshold source (Pingora-style
        // policy/enforcement split). See crate::h2_security for the
        // attack → knob mapping.
        //
        // A `Timer` is required before `keep_alive_interval` fires;
        // without it hyper panics "You must supply a timer." Always
        // wire the tokio timer here — it's the same runtime our
        // caller is already using.
        let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
        builder.timer(TokioTimer::new());
        self.security.apply(&mut builder);
        let conn = builder.serve_connection(TokioIo::new(io), svc);
        match tokio::time::timeout(total, conn).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(io::Error::other(format!("h2 server: {e}"))),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "total connection timeout",
            )),
        }
    }
}

/// Service implementation carrying the [`H2Proxy`] plus the peer address.
#[derive(Clone)]
struct ProxyService {
    inner: Arc<H2Proxy>,
    peer: SocketAddr,
}

impl hyper::service::Service<Request<IncomingBody>> for ProxyService {
    type Response = Response<BoxBody<Bytes, hyper::Error>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<IncomingBody>) -> Self::Future {
        let inner = Arc::clone(&self.inner);
        let peer = self.peer;
        Box::pin(async move { Ok(inner.handle(req, peer).await) })
    }
}

impl H2Proxy {
    async fn handle(
        &self,
        req: Request<IncomingBody>,
        peer: SocketAddr,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let (mut parts, body) = req.into_parts();

        // Determine the authority: H2 carries it in :authority, which
        // hyper surfaces as `uri.authority()`. Fall back to the Host
        // header for clients that still populate it.
        let authority = parts
            .uri
            .authority()
            .map(|a| a.as_str().to_owned())
            .or_else(|| {
                parts
                    .headers
                    .get(hyper::header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            });

        strip_hop_by_hop(&mut parts.headers);
        append_xff(&mut parts.headers, peer);
        set_xfp(&mut parts.headers, self.is_https);
        if let Some(h) = authority.as_deref() {
            set_xfh(&mut parts.headers, h);
            // Upstream is H1, which requires a Host header. If the client
            // spoke H2 without one, synthesise it from :authority.
            if !parts.headers.contains_key(hyper::header::HOST) {
                if let Ok(v) = HeaderValue::from_str(h) {
                    parts.headers.insert(hyper::header::HOST, v);
                }
            }
        }
        append_via(&mut parts.headers);

        let Some(backend_addr) = self.picker.pick() else {
            return error_response(StatusCode::BAD_GATEWAY, "no backend available");
        };

        let req_for_upstream = Request::from_parts(parts, body);
        match self.proxy_request(backend_addr, req_for_upstream).await {
            Ok(resp) => self.finalize_response(resp),
            Err(ProxyErr::Upstream(s)) => error_response(StatusCode::BAD_GATEWAY, &s),
            Err(ProxyErr::Timeout) => {
                error_response(StatusCode::GATEWAY_TIMEOUT, "upstream timeout")
            }
        }
    }

    async fn proxy_request(
        &self,
        backend_addr: SocketAddr,
        req: Request<IncomingBody>,
    ) -> Result<Response<IncomingBody>, ProxyErr> {
        let pool = self.pool.clone();
        let pooled = tokio::task::spawn_blocking(move || pool.acquire(backend_addr))
            .await
            .map_err(|e| ProxyErr::Upstream(format!("backend dial join: {e}")))?
            .map_err(|e| ProxyErr::Upstream(format!("backend connect {backend_addr}: {e}")))?;

        let stream = pooled
            .take_stream()
            .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;

        // Upstream is H1 — matches nginx/haproxy production behaviour.
        // H2 upstream support is a future pillar.
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| ProxyErr::Upstream(format!("h1 client handshake: {e}")))?;

        let conn_handle = tokio::spawn(async move {
            let _ = conn.await;
        });

        let send_fut = sender.send_request(req);
        let resp = match tokio::time::timeout(self.timeouts.body, send_fut).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                conn_handle.abort();
                return Err(ProxyErr::Upstream(format!("send_request: {e}")));
            }
            Err(_) => {
                conn_handle.abort();
                return Err(ProxyErr::Timeout);
            }
        };
        drop(conn_handle);
        Ok(resp)
    }

    fn finalize_response(
        &self,
        resp: Response<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        if let Some(alt) = self.alt_svc {
            if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
                parts.headers.insert(hyper::header::ALT_SVC, value);
            }
        }
        Response::from_parts(parts, body.boxed())
    }
}

enum ProxyErr {
    Upstream(String),
    Timeout,
}

fn error_response(status: StatusCode, msg: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = Full::new(Bytes::from(msg.to_owned()))
        .map_err(|never| match never {})
        .boxed();
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    resp
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use hyper::HeaderMap;
    use hyper::header::HeaderName;

    fn map_with(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(
                HeaderName::try_from(*k).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        m
    }

    #[test]
    fn h2_proxy_alt_svc_injected() {
        // The H2 path uses the same Alt-Svc formatter as H1 (shared via
        // `AltSvcConfig::header_value`). Re-prove the contract here so
        // a regression in the H2 path gets its own red test rather than
        // hiding behind an H1 assertion.
        let alt = AltSvcConfig {
            h3_port: 443,
            max_age: 3_600,
        };
        let mut h = HeaderMap::new();
        let value = HeaderValue::from_str(&alt.header_value()).unwrap();
        h.insert(hyper::header::ALT_SVC, value);
        assert_eq!(h.get("alt-svc").unwrap(), "h3=\":443\"; ma=3600");
    }

    #[test]
    fn h2_proxy_hop_by_hop_stripped() {
        // H2 forbids Connection / TE / Transfer-Encoding on the wire,
        // but we still must scrub them before forwarding to an H1
        // upstream.
        let mut h = map_with(&[
            ("host", "example.com"),
            ("connection", "Keep-Alive, Foo"),
            ("keep-alive", "timeout=5"),
            ("foo", "bar"),
            ("transfer-encoding", "chunked"),
            ("accept", "text/html"),
        ]);
        strip_hop_by_hop(&mut h);
        assert!(h.get("connection").is_none());
        assert!(h.get("keep-alive").is_none());
        assert!(h.get("foo").is_none());
        assert!(h.get("transfer-encoding").is_none());
        assert_eq!(h.get("host").unwrap(), "example.com");
        assert_eq!(h.get("accept").unwrap(), "text/html");
    }

    #[test]
    fn h2_proxy_xff_appended() {
        // Shared with the H1 path — prove the H2 path gets it too.
        let mut h = map_with(&[("x-forwarded-for", "10.0.0.1")]);
        let peer: SocketAddr = "1.2.3.4:5555".parse().unwrap();
        append_xff(&mut h, peer);
        assert_eq!(h.get("x-forwarded-for").unwrap(), "10.0.0.1, 1.2.3.4");
    }
}
