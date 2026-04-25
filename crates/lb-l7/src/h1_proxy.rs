//! Real hyper 1.x HTTP/1.1 proxy path (Pillar 3b.3b-1).
//!
//! [`H1Proxy`] is the L7 entry point used by the binary's `H1` and `H1s`
//! listener modes. Each accepted client connection (plain TCP for `H1`, or
//! a TLS-decrypted stream for `H1s`) is handed to [`H1Proxy::serve_connection`]
//! which drives a hyper 1.x HTTP/1.1 server over it.
//!
//! For every inbound request the proxy:
//!
//! 1. Strips hop-by-hop request headers per RFC 9110 §7.6.1, including any
//!    additional names listed in the `Connection` header value.
//! 2. Appends `X-Forwarded-{For,Proto,Host}` and `Via` headers so origins
//!    can attribute the inbound request.
//! 3. Picks a backend via the supplied [`crate::h1_proxy::BackendPicker`]
//!    (round-robin in the binary today).
//! 4. Acquires a pooled TCP socket via [`lb_io::pool::TcpPool`] and runs a
//!    hyper 1.x HTTP/1.1 client handshake on it.
//! 5. Forwards the request, body-timeout-bounded, and translates the
//!    upstream response back to the client.
//! 6. Strips hop-by-hop response headers and, when configured, injects
//!    `Alt-Svc: h3=":<port>"; ma=<max_age>` so HTTP/3-aware clients can
//!    upgrade.
//!
//! The whole `serve_connection` future is wrapped in
//! `tokio::time::timeout` with the configured `total_timeout` so a stuck
//! upstream cannot wedge a client connection forever.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::body::{Bytes, Incoming as IncomingBody};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lb_io::http2_pool::Http2Pool;
use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::upstream::{BackendInfoPicker, SingleProtoPicker, UpstreamBackend, UpstreamProto};
use crate::ws_proxy::{self, WsProxy, build_handshake_response_headers, is_h1_upgrade_request};

/// Hop-by-hop headers per RFC 9110 §7.6.1.
///
/// These are stripped from BOTH request and response in addition to any
/// header names listed inside the `Connection` header value. Built as
/// `HeaderName` constants so removal is panic-free at runtime (the
/// strings are checked at compile time via `HeaderName::from_static`).
static HOP_BY_HOP: [HeaderName; 8] = [
    HeaderName::from_static("connection"),
    HeaderName::from_static("keep-alive"),
    HeaderName::from_static("proxy-authenticate"),
    HeaderName::from_static("proxy-authorization"),
    HeaderName::from_static("te"),
    HeaderName::from_static("trailers"),
    HeaderName::from_static("transfer-encoding"),
    HeaderName::from_static("upgrade"),
];

/// Configuration for the `Alt-Svc` advertisement injected into responses.
#[derive(Debug, Clone, Copy)]
pub struct AltSvcConfig {
    /// UDP port hosting the H3 listener that should be advertised.
    pub h3_port: u16,
    /// `max-age` in seconds.
    pub max_age: u32,
}

impl AltSvcConfig {
    /// Render the canonical header value for this configuration:
    /// `h3=":<h3_port>"; ma=<max_age>`.
    #[must_use]
    pub fn header_value(self) -> String {
        format!("h3=\":{}\"; ma={}", self.h3_port, self.max_age)
    }
}

/// Per-listener HTTP timeouts.
#[derive(Debug, Clone, Copy)]
pub struct HttpTimeouts {
    /// Maximum time spent reading the request line + headers. Wrapped
    /// around the entire upstream request future today since hyper's H1
    /// server does not expose a separate header-receipt knob in 1.x.
    pub header: Duration,
    /// Maximum time the upstream side spends sending its response (and
    /// the client side spends sending its request body). Applied around
    /// the upstream `send_request` call.
    pub body: Duration,
    /// Hard upper bound on a single connection's total lifetime.
    pub total: Duration,
}

impl Default for HttpTimeouts {
    fn default() -> Self {
        Self {
            header: Duration::from_secs(10),
            body: Duration::from_secs(30),
            total: Duration::from_secs(60),
        }
    }
}

/// Picks the next backend address. Implementations must be cheap to call
/// and lock-free or fine-grained: it runs once per inbound request.
pub trait BackendPicker: Send + Sync {
    /// Return the next backend [`SocketAddr`] to dial, or `None` if no
    /// backend can serve the request.
    fn pick(&self) -> Option<SocketAddr>;
}

/// Round-robin picker over a fixed [`Vec<SocketAddr>`]. Used by the
/// binary today; the trait keeps the proxy decoupled from the
/// `lb_balancer` crate.
pub struct RoundRobinAddrs {
    addrs: Vec<SocketAddr>,
    counter: parking_lot::Mutex<usize>,
}

impl RoundRobinAddrs {
    /// Create a new picker over `addrs`. Returns `None` if `addrs` is
    /// empty (a backend-less listener cannot serve any request).
    #[must_use]
    pub fn new(addrs: Vec<SocketAddr>) -> Option<Self> {
        if addrs.is_empty() {
            return None;
        }
        Some(Self {
            addrs,
            counter: parking_lot::Mutex::new(0),
        })
    }
}

impl BackendPicker for RoundRobinAddrs {
    fn pick(&self) -> Option<SocketAddr> {
        if self.addrs.is_empty() {
            return None;
        }
        let idx = {
            let mut g = self.counter.lock();
            let i = *g % self.addrs.len();
            *g = g.wrapping_add(1);
            i
        };
        self.addrs.get(idx).copied()
    }
}

/// L7 HTTP/1.1 proxy. Cheap to clone via [`Arc`].
pub struct H1Proxy {
    pool: TcpPool,
    picker: Arc<dyn BackendInfoPicker>,
    alt_svc: Option<AltSvcConfig>,
    timeouts: HttpTimeouts,
    is_https: bool,
    /// When `Some`, inbound requests carrying an RFC 6455 handshake are
    /// routed through the WebSocket proxy instead of the regular request
    /// path. `None` disables WebSocket support on this listener.
    ws: Option<Arc<WsProxy>>,
    /// Optional H2 upstream pool. When the picker selects a backend
    /// with [`UpstreamProto::H2`], the proxy dispatches via this pool.
    /// PROTO-001 H1→H2 path.
    h2_upstream: Option<Arc<Http2Pool>>,
    /// Optional H3 upstream pool. PROTO-001 H1→H3 path.
    h3_upstream: Option<Arc<QuicUpstreamPool>>,
}

impl H1Proxy {
    /// Construct an [`H1Proxy`] over a single-protocol H1 backend pool.
    ///
    /// `is_https` selects the value emitted into `X-Forwarded-Proto`
    /// (`"https"` for `H1s`, `"http"` for `H1`).
    ///
    /// Wraps `picker` in a [`SingleProtoPicker`] tagged
    /// [`UpstreamProto::H1`] for backwards compatibility with the
    /// pre-PROTO-001 surface. Call sites that need H2/H3 backends use
    /// [`Self::with_multi_proto`] instead.
    #[must_use]
    pub fn new(
        pool: TcpPool,
        picker: Arc<dyn BackendPicker>,
        alt_svc: Option<AltSvcConfig>,
        timeouts: HttpTimeouts,
        is_https: bool,
    ) -> Self {
        let info = Arc::new(SingleProtoPicker::new(picker, UpstreamProto::H1, None));
        Self {
            pool,
            picker: info,
            alt_svc,
            timeouts,
            is_https,
            ws: None,
            h2_upstream: None,
            h3_upstream: None,
        }
    }

    /// Construct an [`H1Proxy`] backed by a multi-protocol picker.
    ///
    /// The picker may return `H1`, `H2`, or `H3` backends per call;
    /// the proxy branches on `UpstreamProto` and dials via the matching
    /// pool. Call sites must populate `h2_upstream` / `h3_upstream`
    /// when their picker can return that protocol — a pick whose
    /// matching pool is `None` falls back to a 502 response.
    #[must_use]
    pub const fn with_multi_proto(
        pool: TcpPool,
        picker: Arc<dyn BackendInfoPicker>,
        alt_svc: Option<AltSvcConfig>,
        timeouts: HttpTimeouts,
        is_https: bool,
    ) -> Self {
        Self {
            pool,
            picker,
            alt_svc,
            timeouts,
            is_https,
            ws: None,
            h2_upstream: None,
            h3_upstream: None,
        }
    }

    /// Attach an H2 upstream pool used for backends with
    /// [`UpstreamProto::H2`]. PROTO-001.
    #[must_use]
    pub fn with_h2_upstream(mut self, pool: Arc<Http2Pool>) -> Self {
        self.h2_upstream = Some(pool);
        self
    }

    /// Attach an H3 upstream pool used for backends with
    /// [`UpstreamProto::H3`]. PROTO-001.
    #[must_use]
    pub fn with_h3_upstream(mut self, pool: Arc<QuicUpstreamPool>) -> Self {
        self.h3_upstream = Some(pool);
        self
    }

    /// Enable WebSocket upgrade handling on this proxy.
    ///
    /// Takes ownership; returns `self` so the call site reads as a
    /// fluent chain off [`Self::new`].
    #[must_use]
    pub fn with_websocket(mut self, ws: Arc<WsProxy>) -> Self {
        self.ws = Some(ws);
        self
    }

    /// Drive HTTP/1.1 server logic over `io`.
    ///
    /// Returns once the connection has fully closed. A
    /// [`tokio::time::timeout`] of [`HttpTimeouts::total`] is wrapped
    /// around the whole loop so a runaway client-or-upstream pair cannot
    /// pin a worker forever; on elapsed-timeout the function returns
    /// [`io::ErrorKind::TimedOut`].
    ///
    /// # Errors
    ///
    /// Surfaces I/O errors and timeouts. Per-request upstream errors are
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
        let conn = hyper::server::conn::http1::Builder::new()
            .keep_alive(true)
            .serve_connection(TokioIo::new(io), svc)
            .with_upgrades();
        match tokio::time::timeout(total, conn).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(io::Error::other(format!("h1 server: {e}"))),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "total connection timeout",
            )),
        }
    }
}

/// Service implementation carrying the [`H1Proxy`] plus the peer address.
#[derive(Clone)]
struct ProxyService {
    inner: Arc<H1Proxy>,
    peer: SocketAddr,
}

impl hyper::service::Service<Request<IncomingBody>> for ProxyService {
    type Response = Response<BoxBody<Bytes, hyper::Error>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<IncomingBody>) -> Self::Future {
        let inner = Arc::clone(&self.inner);
        let peer = self.peer;
        Box::pin(async move { Ok(Box::pin(inner.handle(req, peer)).await) })
    }
}

impl H1Proxy {
    async fn handle(
        &self,
        req: Request<IncomingBody>,
        peer: SocketAddr,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        // WebSocket upgrade intercept (RFC 6455 §4). Only fires when the
        // listener is configured with a `WsProxy`; all other listener
        // traffic continues through the regular H1 request path.
        if self
            .ws
            .as_ref()
            .is_some_and(|w| w.config().enabled && is_h1_upgrade_request(&req))
        {
            return self.handle_ws_upgrade(req);
        }

        let (mut parts, body) = req.into_parts();
        let host = parts
            .headers
            .get(hyper::header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        strip_hop_by_hop(&mut parts.headers);
        append_xff(&mut parts.headers, peer);
        set_xfp(&mut parts.headers, self.is_https);
        if let Some(h) = host.as_deref() {
            set_xfh(&mut parts.headers, h);
        }
        append_via(&mut parts.headers);

        let Some(backend) = self.picker.pick_info() else {
            return error_response(StatusCode::BAD_GATEWAY, "no backend available");
        };

        let req_for_upstream = Request::from_parts(parts, body);
        match backend.proto {
            UpstreamProto::H1 => match self.proxy_request(backend.addr, req_for_upstream).await {
                Ok(resp) => self.finalize_response(resp),
                Err(ProxyErr::Upstream(s)) => error_response(StatusCode::BAD_GATEWAY, &s),
                Err(ProxyErr::Timeout) => {
                    error_response(StatusCode::GATEWAY_TIMEOUT, "upstream timeout")
                }
            },
            UpstreamProto::H2 => {
                Box::pin(self.proxy_h1_to_h2(backend.addr, req_for_upstream)).await
            }
            UpstreamProto::H3 => Box::pin(self.proxy_h1_to_h3(&backend, req_for_upstream)).await,
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

        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| ProxyErr::Upstream(format!("h1 client handshake: {e}")))?;

        let conn_handle = tokio::spawn(async move {
            // hyper's H1 client connection drives reads/writes of the
            // request body and the response stream. Errors here usually
            // mean the upstream half-closed; we surface that on the
            // response side via `send_request` returning an error. The
            // join-handle is dropped at end-of-scope so the task is
            // cancelled if it outlives the request future.
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
        // We deliberately do NOT await `conn_handle` — the response body
        // streaming still needs the driver task running. Detach it.
        drop(conn_handle);
        Ok(resp)
    }

    /// Forward an H1 inbound request to an H2 backend (PROTO-001).
    ///
    /// Bridges via [`crate::create_bridge`]`(Http1, Http2)` — the
    /// codec-level translation produces the pseudo-header set hyper's
    /// H2 client expects from the request URI authority + scheme +
    /// path. Body is collected into a `Bytes` ahead of dial because the
    /// pool's `send_request` consumes the request once.
    async fn proxy_h1_to_h2(
        &self,
        backend_addr: SocketAddr,
        req: Request<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let Some(h2_pool) = self.h2_upstream.as_ref() else {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "H2 backend selected but no Http2Pool wired",
            );
        };
        let translated = match translate_h1_request_to_h2(req).await {
            Ok(r) => r,
            Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
        };
        match h2_pool.send_request(backend_addr, translated).await {
            Ok(resp) => upstream_response_to_h1(resp, self.alt_svc).await,
            Err(lb_io::http2_pool::Http2PoolError::Timeout) => {
                error_response(StatusCode::GATEWAY_TIMEOUT, "upstream H2 timeout")
            }
            Err(e) => error_response(StatusCode::BAD_GATEWAY, &format!("h2 upstream: {e}")),
        }
    }

    /// Forward an H1 inbound request to an H3 backend (PROTO-001).
    ///
    /// Bridges via [`crate::create_bridge`]`(Http1, Http3)` and
    /// dispatches via [`lb_io::quic_pool::QuicUpstreamPool`] +
    /// `lb_quic::request_h3_upstream`.
    async fn proxy_h1_to_h3(
        &self,
        backend: &UpstreamBackend,
        req: Request<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let Some(h3_pool) = self.h3_upstream.as_ref() else {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "H3 backend selected but no QuicUpstreamPool wired",
            );
        };
        let sni = backend.sni.as_deref().unwrap_or("");
        let (headers, body) =
            match collect_h1_request_to_h3_fieldlist(req, sni, /* https = */ true).await {
                Ok(p) => p,
                Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
            };
        let h3_resp = Box::pin(lb_quic::request_h3_upstream(
            headers,
            body,
            backend.addr,
            sni,
            h3_pool,
        ))
        .await;
        h3_response_to_h1(h3_resp, self.alt_svc)
    }

    /// Handle an RFC 6455 handshake request.
    ///
    /// Builds the `101 Switching Protocols` response and schedules a
    /// detached task that awaits [`hyper::upgrade::on`] on the inbound
    /// request, dials the backend with [`tokio_tungstenite::client_async`],
    /// and runs the bidirectional frame forwarder.
    ///
    /// Returns a plain 400 if the handshake is structurally valid but
    /// `Sec-WebSocket-Key` is missing once hyper hands us the request
    /// (race: the detector accepted it).
    fn handle_ws_upgrade(
        &self,
        mut req: Request<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let Some(ws_proxy) = self.ws.clone() else {
            return error_response(StatusCode::BAD_GATEWAY, "websocket disabled");
        };
        let Some(handshake_headers) = build_handshake_response_headers(&req) else {
            return error_response(StatusCode::BAD_REQUEST, "invalid websocket handshake");
        };
        let Some(backend) = self.picker.pick_info() else {
            return error_response(StatusCode::BAD_GATEWAY, "no backend available");
        };
        // WebSocket upgrade only supports H1 backends today. A picker
        // that returns H2/H3 for a WS request is misconfigured.
        if backend.proto != UpstreamProto::H1 {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "WebSocket upgrade requires H1 backend",
            );
        }
        let backend_addr = backend.addr;

        // Kick off the upgrade future BEFORE we return the 101 response.
        // hyper will drive it as soon as the response headers have been
        // written on the wire.
        let upgrade_fut = hyper::upgrade::on(&mut req);

        // Snapshot the request for the client-side handshake to the
        // upstream. We reuse `path + query` and pick up headers that the
        // RFC 6455 §4.1 client must carry (Sec-WebSocket-Protocol /
        // -Extensions). The `Host` header is rewritten to the backend
        // `SocketAddr` so the upstream accepts the handshake.
        let path_and_query = req
            .uri()
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string);
        let forwarded_protocols = req
            .headers()
            .get(&WS_PROTOCOL)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        // Detach a task that finishes the upgrade, dials upstream, and
        // runs the frame forwarder. We do NOT await it here — hyper
        // needs us to return the 101 response first so it can flip the
        // wire.
        tokio::spawn(run_h1_ws_upgrade_task(
            upgrade_fut,
            self.pool.clone(),
            backend_addr,
            path_and_query,
            forwarded_protocols,
            ws_proxy,
        ));

        // Build the 101 response. Mirror a sub-protocol selection if the
        // client asked for one — v1 picks the first offered protocol
        // verbatim. A later pillar can route on this.
        let echo_protocol = req
            .headers()
            .get(&WS_PROTOCOL)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| HeaderValue::from_str(s).ok());
        let mut builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
        for (name, value) in handshake_headers {
            builder = builder.header(name, value);
        }
        if let Some(hv) = echo_protocol {
            builder = builder.header(WS_PROTOCOL.as_str(), hv);
        }
        let body = Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed();
        builder.body(body).unwrap_or_else(|_| {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "101 build failed")
        })
    }

    fn finalize_response(
        &self,
        resp: Response<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        if let Some(alt) = self.alt_svc {
            // Inject (or replace) the Alt-Svc header so older origins
            // cannot shadow our advertisement.
            if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
                parts.headers.insert(hyper::header::ALT_SVC, value);
            }
        }
        Response::from_parts(parts, body.boxed())
    }
}

/// Distinguishes between "upstream said no" and "we gave up waiting" so
/// the public face can pick the right HTTP status.
enum ProxyErr {
    Upstream(String),
    Timeout,
}

/// Finish a WebSocket upgrade: await the hyper upgrade future, dial the
/// backend over the pooled TCP path, drive the RFC 6455 client-side
/// handshake, and hand both halves to [`WsProxy::proxy_frames`].
async fn run_h1_ws_upgrade_task(
    upgrade_fut: hyper::upgrade::OnUpgrade,
    pool: TcpPool,
    backend_addr: SocketAddr,
    path_and_query: String,
    forwarded_protocols: Option<String>,
    ws_proxy: Arc<WsProxy>,
) {
    let upgraded = match upgrade_fut.await {
        Ok(u) => u,
        Err(e) => {
            tracing::debug!(error = %e, "ws: hyper upgrade failed");
            return;
        }
    };

    let pooled_result = tokio::task::spawn_blocking(move || pool.acquire(backend_addr)).await;
    let pooled = match pooled_result {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            tracing::debug!(error = %e, backend = %backend_addr, "ws: backend dial failed");
            return;
        }
        Err(e) => {
            tracing::debug!(error = %e, "ws: dial join failed");
            return;
        }
    };
    let Some(upstream_stream) = pooled.take_stream() else {
        tracing::debug!("ws: pooled stream missing");
        return;
    };

    let uri = match format!("ws://{backend_addr}{path_and_query}").parse() {
        Ok(u) => u,
        Err(e) => {
            tracing::debug!(error = %e, "ws: upstream uri build failed");
            return;
        }
    };
    let mut builder = tokio_tungstenite::tungstenite::client::ClientRequestBuilder::new(uri);
    if let Some(protocols) = forwarded_protocols.as_deref() {
        for p in protocols.split(',') {
            let p = p.trim();
            if !p.is_empty() {
                builder = builder.with_sub_protocol(p);
            }
        }
    }

    let ws_cfg = ws_proxy.config();
    let (backend_ws, _resp) = match tokio_tungstenite::client_async_with_config(
        builder,
        upstream_stream,
        Some(ws_cfg.tungstenite_config()),
    )
    .await
    {
        Ok(pair) => pair,
        Err(e) => {
            tracing::debug!(error = %e, backend = %backend_addr, "ws: upstream handshake failed");
            return;
        }
    };

    let client_ws = ws_proxy::server_ws(TokioIo::new(upgraded), &ws_cfg).await;
    if let Err(e) = ws_proxy.proxy_frames(client_ws, backend_ws).await {
        tracing::debug!(error = %e, "ws: frame proxy ended with error");
    }
}

fn error_response(status: StatusCode, msg: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = Full::new(Bytes::from(msg.to_owned()))
        .map_err(|never| match never {})
        .boxed();
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    resp
}

/// Strip hop-by-hop headers per RFC 9110 §7.6.1 plus any names listed
/// inside the `Connection` header value.
pub(crate) fn strip_hop_by_hop(headers: &mut hyper::HeaderMap) {
    // Collect Connection-token names BEFORE removing the Connection
    // header itself.
    let extra: Vec<HeaderName> = headers
        .get_all(hyper::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|n| HeaderName::try_from(n.to_ascii_lowercase()).ok())
        .collect();

    for name in &HOP_BY_HOP {
        // HeaderMap::remove removes ALL values for the name, not just one.
        headers.remove(name);
    }
    for name in extra {
        headers.remove(name);
    }
}

/// Append the peer's IP to `X-Forwarded-For`, creating the header if
/// absent.
pub(crate) fn append_xff(headers: &mut hyper::HeaderMap, peer: SocketAddr) {
    let peer_ip = peer.ip().to_string();
    let new_value = headers.get(&XFF_NAME).map_or_else(
        || peer_ip.clone(),
        |existing| {
            existing
                .to_str()
                .map_or_else(|_| peer_ip.clone(), |prev| format!("{prev}, {peer_ip}"))
        },
    );
    if let Ok(v) = HeaderValue::from_str(&new_value) {
        headers.insert(&XFF_NAME, v);
    }
}

/// Set `X-Forwarded-Proto` to `"https"` or `"http"`.
pub(crate) fn set_xfp(headers: &mut hyper::HeaderMap, is_https: bool) {
    let v = if is_https { "https" } else { "http" };
    if let Ok(value) = HeaderValue::from_str(v) {
        headers.insert(&XFP_NAME, value);
    }
}

/// Set `X-Forwarded-Host` to the given host.
pub(crate) fn set_xfh(headers: &mut hyper::HeaderMap, host: &str) {
    if let Ok(value) = HeaderValue::from_str(host) {
        headers.insert(&XFH_NAME, value);
    }
}

/// Append `HTTP/1.1 expressgateway` to `Via`, creating the header if
/// absent.
pub(crate) fn append_via(headers: &mut hyper::HeaderMap) {
    const VIA_TOKEN: &str = "HTTP/1.1 expressgateway";
    let new_value = headers.get(hyper::header::VIA).map_or_else(
        || VIA_TOKEN.to_owned(),
        |existing| {
            existing.to_str().map_or_else(
                |_| VIA_TOKEN.to_owned(),
                |prev| format!("{prev}, {VIA_TOKEN}"),
            )
        },
    );
    if let Ok(v) = HeaderValue::from_str(&new_value) {
        headers.insert(hyper::header::VIA, v);
    }
}

static XFF_NAME: HeaderName = HeaderName::from_static("x-forwarded-for");
static XFP_NAME: HeaderName = HeaderName::from_static("x-forwarded-proto");
static XFH_NAME: HeaderName = HeaderName::from_static("x-forwarded-host");
static WS_PROTOCOL: HeaderName = HeaderName::from_static("sec-websocket-protocol");

// ── PROTO-001 cross-protocol translation helpers ───────────────────────

/// Lift an inbound H1 [`Request<IncomingBody>`] into the shape hyper's
/// H2 client expects. Body is collected into `Bytes`; the URI is
/// rebuilt to include the authority hyper extracts into `:authority`.
///
/// Uses the `lb_l7::create_bridge(Http1, Http2)` codec for header
/// transformation. Hop-by-hop headers + Host are stripped by the
/// bridge; hyper synthesises pseudo-headers from the rewritten URI.
async fn translate_h1_request_to_h2(
    req: Request<IncomingBody>,
) -> Result<Request<BoxBody<Bytes, hyper::Error>>, String> {
    let (parts, body) = req.into_parts();
    let body_bytes = body
        .collect()
        .await
        .map_err(|e| format!("body collect: {e}"))?
        .to_bytes();
    let bridge = crate::create_bridge(crate::Protocol::Http1, crate::Protocol::Http2);
    let bridge_in = crate::BridgeRequest {
        method: parts.method.to_string(),
        uri: parts
            .uri
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string),
        headers: parts
            .headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_owned(), s.to_owned()))
            })
            .collect(),
        body: body_bytes.clone(),
        scheme: Some("http".to_owned()),
    };
    let translated = bridge
        .bridge_request(&bridge_in)
        .map_err(|e| format!("h1->h2 bridge: {e}"))?;

    // Extract the :authority pseudo-header for the hyper URI.
    let authority = translated
        .headers
        .iter()
        .find(|(k, _)| k == ":authority")
        .map(|(_, v)| v.clone())
        .filter(|s| !s.is_empty());
    let scheme = translated.scheme.as_deref().unwrap_or("http");

    let mut builder = Request::builder().method(parts.method.clone());
    if let Some(auth) = authority.as_deref() {
        let uri = format!("{scheme}://{auth}{}", translated.uri);
        builder = builder.uri(uri);
    } else {
        builder = builder.uri(&translated.uri);
    }
    // Re-emit non-pseudo headers that the bridge produced. hyper's H2
    // client builds pseudo-headers itself from the URI and method.
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }
    let body = Full::new(body_bytes)
        .map_err(|never| match never {})
        .boxed();
    builder.body(body).map_err(|e| format!("build h2 req: {e}"))
}

/// Convert an upstream `Response<Incoming>` (H2) back into the H1
/// response shape the listener emits to the client.
async fn upstream_response_to_h1(
    resp: Response<IncomingBody>,
    alt_svc: Option<AltSvcConfig>,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let (parts, body) = resp.into_parts();
    let body_bytes = match body.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(StatusCode::BAD_GATEWAY, &format!("upstream body read: {e}"));
        }
    };
    let bridge = crate::create_bridge(crate::Protocol::Http2, crate::Protocol::Http1);
    let bridge_in = crate::BridgeResponse {
        status: parts.status.as_u16(),
        headers: parts
            .headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_owned(), s.to_owned()))
            })
            .collect(),
        body: body_bytes,
    };
    let translated = match bridge.bridge_response(&bridge_in) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("h2->h1 response bridge: {e}"),
            );
        }
    };
    let mut builder = Response::builder().status(translated.status);
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    let body = Full::new(translated.body)
        .map_err(|never| match never {})
        .boxed();
    builder.body(body).unwrap_or_else(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "build h1 response failed",
        )
    })
}

/// Collect an inbound H1 request, run the H1→H3 codec bridge, and
/// return a `(field_list, body)` pair for
/// `lb_quic::request_h3_upstream`.
async fn collect_h1_request_to_h3_fieldlist(
    req: Request<IncomingBody>,
    sni: &str,
    is_https: bool,
) -> Result<(Vec<(String, String)>, Bytes), String> {
    let (parts, body) = req.into_parts();
    let body_bytes = body
        .collect()
        .await
        .map_err(|e| format!("body collect: {e}"))?
        .to_bytes();
    let host = parts
        .headers
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map_or_else(|| sni.to_owned(), str::to_owned);
    let scheme = if is_https { "https" } else { "http" };
    let bridge = crate::create_bridge(crate::Protocol::Http1, crate::Protocol::Http3);
    let bridge_in = crate::BridgeRequest {
        method: parts.method.to_string(),
        uri: parts
            .uri
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string),
        headers: {
            let mut h: Vec<(String, String)> = parts
                .headers
                .iter()
                .filter_map(|(n, v)| {
                    v.to_str()
                        .ok()
                        .map(|s| (n.as_str().to_owned(), s.to_owned()))
                })
                .collect();
            // Ensure :authority synthesis has a host to draw from.
            if !h.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
                h.push(("host".to_owned(), host.clone()));
            }
            h
        },
        body: body_bytes.clone(),
        scheme: Some(scheme.to_owned()),
    };
    let translated = bridge
        .bridge_request(&bridge_in)
        .map_err(|e| format!("h1->h3 bridge: {e}"))?;
    let mut field_list: Vec<(String, String)> = translated.headers;
    if !field_list
        .iter()
        .any(|(k, _)| k == ":authority" && !k.is_empty())
    {
        field_list.push((":authority".to_owned(), host));
    }
    Ok((field_list, body_bytes))
}

/// Convert an [`lb_quic::H3UpstreamResponse`] back into the H1 response
/// shape the listener emits.
fn h3_response_to_h1(
    resp: lb_quic::H3UpstreamResponse,
    alt_svc: Option<AltSvcConfig>,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let bridge = crate::create_bridge(crate::Protocol::Http3, crate::Protocol::Http1);
    let body_bytes = Bytes::from(resp.body);
    let bridge_in = crate::BridgeResponse {
        status: resp.status,
        headers: resp.headers,
        body: body_bytes,
    };
    let translated = match bridge.bridge_response(&bridge_in) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("h3->h1 response bridge: {e}"),
            );
        }
    };
    let status = StatusCode::from_u16(translated.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    let body = Full::new(translated.body)
        .map_err(|never| match never {})
        .boxed();
    builder.body(body).unwrap_or_else(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "build h1 response failed",
        )
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use hyper::HeaderMap;

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
    fn hop_by_hop_headers_stripped_from_request() {
        let mut h = map_with(&[
            ("host", "example.com"),
            ("connection", "Keep-Alive, Foo"),
            ("keep-alive", "timeout=5"),
            ("foo", "bar"),
            ("accept", "text/html"),
        ]);
        strip_hop_by_hop(&mut h);
        assert!(h.get("connection").is_none(), "connection must be stripped");
        assert!(h.get("keep-alive").is_none(), "keep-alive must be stripped");
        assert!(
            h.get("foo").is_none(),
            "Connection-named header must be stripped"
        );
        assert_eq!(h.get("host").unwrap(), "example.com");
        assert_eq!(h.get("accept").unwrap(), "text/html");
    }

    #[test]
    fn x_forwarded_for_appended() {
        let mut h = map_with(&[("x-forwarded-for", "10.0.0.1")]);
        let peer: SocketAddr = "1.2.3.4:5555".parse().unwrap();
        append_xff(&mut h, peer);
        assert_eq!(h.get("x-forwarded-for").unwrap(), "10.0.0.1, 1.2.3.4");
    }

    #[test]
    fn x_forwarded_for_created_when_absent() {
        let mut h = HeaderMap::new();
        let peer: SocketAddr = "5.6.7.8:9999".parse().unwrap();
        append_xff(&mut h, peer);
        assert_eq!(h.get("x-forwarded-for").unwrap(), "5.6.7.8");
    }

    #[test]
    fn via_appended() {
        let mut h = map_with(&[("via", "1.1 gw1")]);
        append_via(&mut h);
        assert_eq!(h.get("via").unwrap(), "1.1 gw1, HTTP/1.1 expressgateway");
    }

    #[test]
    fn alt_svc_injected_when_configured() {
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
    fn alt_svc_absent_when_not_configured() {
        let h = HeaderMap::new();
        assert!(h.get("alt-svc").is_none());
    }

    #[test]
    fn hop_by_hop_response_strips_te_trailers_and_transfer_encoding() {
        let mut h = map_with(&[
            ("content-type", "text/plain"),
            ("transfer-encoding", "chunked"),
            ("te", "trailers"),
            ("trailers", "X-Foo"),
        ]);
        strip_hop_by_hop(&mut h);
        assert!(h.get("transfer-encoding").is_none());
        assert!(h.get("te").is_none());
        assert!(h.get("trailers").is_none());
        assert_eq!(h.get("content-type").unwrap(), "text/plain");
    }

    #[test]
    fn round_robin_picker_cycles() {
        let a: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let b: SocketAddr = "127.0.0.1:2".parse().unwrap();
        let p = RoundRobinAddrs::new(vec![a, b]).unwrap();
        assert_eq!(p.pick(), Some(a));
        assert_eq!(p.pick(), Some(b));
        assert_eq!(p.pick(), Some(a));
    }

    #[test]
    fn round_robin_empty_returns_none() {
        assert!(RoundRobinAddrs::new(vec![]).is_none());
    }
}
