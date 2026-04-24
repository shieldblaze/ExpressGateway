//! gRPC upstream path (Item 3 / PROMPT.md §13).
//!
//! `GrpcProxy` is a *capability* attached to [`crate::h2_proxy::H2Proxy`]:
//! incoming H2 streams whose `content-type` matches
//! `application/grpc[+ext]` are peeled off the regular H2 request path
//! and driven through this module instead.
//!
//! What happens on a gRPC stream:
//!
//! 1. The request `content-type` is matched against
//!    `^application/grpc(\+\w+)?$` — case insensitive.
//! 2. If the path is `/grpc.health.v1.Health/Check` and the listener
//!    config allows it, the proxy answers `SERVING` locally without
//!    ever dialing a backend (saves the liveness signal from being
//!    coupled to backend availability).
//! 3. Otherwise the proxy parses `grpc-timeout` via
//!    [`lb_grpc::GrpcDeadline::parse_timeout`], clamps it at
//!    [`GrpcConfig::max_deadline`], rewrites the header, and preserves
//!    `TE: trailers` (RFC 9113 §8.2.2 forbids stripping it for gRPC).
//! 4. The request is forwarded upstream over a fresh H2 client
//!    connection (gRPC REQUIRES HTTP/2). Body and trailers pass
//!    through verbatim — gRPC carries `grpc-status`, `grpc-message`,
//!    and `grpc-status-details-bin` in trailers.
//! 5. On gateway-side deadline elapse, the client receives a synthetic
//!    `200 OK` with trailers `grpc-status: 4 DEADLINE_EXCEEDED`.
//! 6. On non-200 upstream HTTP status, the proxy synthesises trailers
//!    from the HTTP code via [`lb_grpc::GrpcStatus::from_http_status`]
//!    — preserving client-visible gRPC semantics even when the origin
//!    blurts back a bare HTTP error.
//!
//! Compression negotiation, gRPC-Web, server reflection, and upstream
//! mTLS are deliberately post-v1.

use std::sync::Arc;
use std::time::Duration;

use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::body::{Bytes, Frame, Incoming as IncomingBody};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use lb_io::pool::TcpPool;

use lb_grpc::{GrpcDeadline, GrpcStatus};

/// Per-listener gRPC knobs.
#[derive(Debug, Clone, Copy)]
pub struct GrpcConfig {
    /// Master switch. Default `true` when the block is present.
    pub enabled: bool,
    /// Upper bound on an accepted `grpc-timeout` value. Client-supplied
    /// values exceeding this are clamped before forwarding; the clamp
    /// also bounds the gateway-side timeout used to emit
    /// `DEADLINE_EXCEEDED`. Default 300 s per gRPC spec guidance.
    pub max_deadline: Duration,
    /// When true, `/grpc.health.v1.Health/Check` is served locally
    /// without forwarding — a gateway liveness signal independent of
    /// backend health. Default true.
    pub health_synthesized: bool,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_deadline: Duration::from_secs(300),
            health_synthesized: true,
        }
    }
}

/// gRPC reverse proxy. Cheap to clone via [`Arc`].
pub struct GrpcProxy {
    cfg: GrpcConfig,
    pool: TcpPool,
}

impl GrpcProxy {
    /// Construct a [`GrpcProxy`] consuming the backend [`TcpPool`].
    #[must_use]
    pub const fn new(cfg: GrpcConfig, pool: TcpPool) -> Self {
        Self { cfg, pool }
    }

    /// Return the [`GrpcConfig`] in effect.
    #[must_use]
    pub const fn config(&self) -> GrpcConfig {
        self.cfg
    }

    /// Serve a gRPC request.
    ///
    /// The caller (`H2Proxy`'s hyper service fn) is responsible for the
    /// `is_grpc_request` predicate; this entry point assumes the
    /// decision has already been made.
    ///
    /// # Errors
    ///
    /// Never — errors are translated into gRPC trailer blocks so the
    /// client observes them as proper gRPC failures rather than
    /// connection resets.
    pub async fn handle(
        self: Arc<Self>,
        req: Request<IncomingBody>,
        backend_addr: std::net::SocketAddr,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        if self.cfg.health_synthesized
            && req.method() == Method::POST
            && req.uri().path() == HEALTH_CHECK_PATH
        {
            return handle_health_check();
        }
        self.forward(req, backend_addr).await
    }

    /// Forward the gRPC request over a fresh H2 client connection.
    ///
    /// Deadline clamping: if the request carries `grpc-timeout`, the
    /// value is parsed, clamped at `max_deadline`, and re-emitted
    /// before forwarding. The gateway also wraps the upstream call in
    /// `tokio::time::timeout` with the clamped deadline so it can
    /// synthesise `DEADLINE_EXCEEDED` when the backend stalls.
    async fn forward(
        &self,
        req: Request<IncomingBody>,
        backend_addr: std::net::SocketAddr,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let (mut parts, body) = req.into_parts();

        let deadline_ms = clamp_grpc_timeout(&mut parts.headers, self.cfg.max_deadline);

        // gRPC requires `TE: trailers` per RFC 9113 §8.2.2. H2 forbids
        // the generic hop-by-hop strip from touching it; we defensively
        // re-insert here so future middlewares that accidentally strip
        // it do not break gRPC.
        parts
            .headers
            .insert(TE_NAME.clone(), HeaderValue::from_static("trailers"));

        // hyper's H2 client requires an absolute URI (scheme +
        // authority). H2 server-side requests arrive with a
        // path-only URI because :scheme/:authority live as separate
        // pseudo-headers on that side. Rewrite before forwarding.
        if let Some(new_uri) = rewrite_uri_for_upstream(&parts.uri, backend_addr) {
            parts.uri = new_uri;
        }

        // Box the body so hyper's H2 client accepts it as
        // `impl Body<Data = Bytes, Error = hyper::Error>`. Passing
        // `IncomingBody` directly triggers subtle Send/Sync bound
        // mismatches inside hyper's generic machinery.
        let upstream_body: BoxBody<Bytes, hyper::Error> = body.map_err(hyper::Error::from).boxed();
        let upstream_req = Request::from_parts(parts, upstream_body);

        let pool = self.pool.clone();
        let acquire = tokio::task::spawn_blocking(move || pool.acquire(backend_addr)).await;
        let pooled = match acquire {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                return grpc_error_response(
                    GrpcStatus::Unavailable,
                    &format!("backend dial failed: {e}"),
                );
            }
            Err(e) => {
                return grpc_error_response(GrpcStatus::Internal, &format!("dial join: {e}"));
            }
        };
        let Some(upstream_io) = pooled.take_stream() else {
            return grpc_error_response(GrpcStatus::Internal, "pooled stream missing");
        };

        let (mut sender, conn) = match hyper::client::conn::http2::handshake::<
            _,
            _,
            BoxBody<Bytes, hyper::Error>,
        >(TokioExecutor::new(), TokioIo::new(upstream_io))
        .await
        {
            Ok(pair) => pair,
            Err(e) => {
                return grpc_error_response(
                    GrpcStatus::Unavailable,
                    &format!("h2 client handshake: {e}"),
                );
            }
        };
        let conn_handle = tokio::spawn(async move {
            let _ = conn.await;
        });

        let send_fut = sender.send_request(upstream_req);
        let upstream_result = if let Some(ms) = deadline_ms {
            let timed = tokio::time::timeout(Duration::from_millis(ms), send_fut).await;
            if let Ok(r) = timed {
                r
            } else {
                conn_handle.abort();
                return grpc_error_response(GrpcStatus::DeadlineExceeded, "gateway deadline");
            }
        } else {
            send_fut.await
        };

        let upstream_resp = match upstream_result {
            Ok(r) => r,
            Err(e) => {
                conn_handle.abort();
                return grpc_error_response(GrpcStatus::Unavailable, &format!("send_request: {e}"));
            }
        };
        drop(conn_handle);

        finalize_upstream(upstream_resp)
    }
}

/// Case-insensitive content-type check against `application/grpc(+ext)?`.
#[must_use]
pub fn is_grpc_request<B>(req: &Request<B>) -> bool {
    req.headers()
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| {
            let s = s.trim().to_ascii_lowercase();
            let core = s.split(';').next().unwrap_or(&s).trim();
            if core == "application/grpc" {
                return true;
            }
            let Some(rest) = core.strip_prefix("application/grpc+") else {
                return false;
            };
            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Rebuild the request URI so hyper's H2 client accepts it.
///
/// The client demands `:scheme` + `:authority`; we pick `http` because
/// v1 upstream is always plaintext TCP (upstream TLS is a follow-up
/// pillar), and we use the backend's `SocketAddr` as authority.
fn rewrite_uri_for_upstream(
    uri: &hyper::Uri,
    backend_addr: std::net::SocketAddr,
) -> Option<hyper::Uri> {
    let path_and_query = uri
        .path_and_query()
        .map_or_else(|| "/".to_owned(), std::string::ToString::to_string);
    let rebuilt = format!("http://{backend_addr}{path_and_query}");
    rebuilt.parse().ok()
}

/// Parse and clamp the `grpc-timeout` header in place.
///
/// Returns the effective deadline in milliseconds when a valid header
/// is present (even if zero); returns `None` when the header is absent
/// or unparseable (treated as "no deadline").
fn clamp_grpc_timeout(headers: &mut HeaderMap, max: Duration) -> Option<u64> {
    let raw = headers.get(&GRPC_TIMEOUT).and_then(|v| v.to_str().ok())?;
    let parsed_ms = GrpcDeadline::parse_timeout(raw).ok()?;
    let max_ms = u64::try_from(max.as_millis()).unwrap_or(u64::MAX);
    let effective = parsed_ms.min(max_ms);
    let rewritten = GrpcDeadline::format_timeout(effective);
    if let Ok(hv) = HeaderValue::from_str(&rewritten) {
        headers.insert(GRPC_TIMEOUT.clone(), hv);
    }
    Some(effective)
}

/// Serve the synthesized `/grpc.health.v1.Health/Check` response.
///
/// Returns a `200 OK` gRPC response with body = gRPC-framed
/// `HealthCheckResponse { status: SERVING }` (a two-byte protobuf
/// message `0x08 0x01`) and trailers `grpc-status: 0`.
///
/// The protobuf wire format for `HealthCheckResponse` is tag=1 wire=0
/// (0x08) followed by varint 1 (0x01). We hand-encode it so the
/// gateway does not pull `prost` at runtime.
fn handle_health_check() -> Response<BoxBody<Bytes, hyper::Error>> {
    // gRPC frame header: compressed=0, length=2 (BE u32), then the
    // two-byte protobuf message `0x08 0x01`.
    let mut frame = Vec::with_capacity(7);
    frame.push(0u8);
    frame.extend_from_slice(&2u32.to_be_bytes());
    frame.push(0x08);
    frame.push(0x01);
    let data_frame: Frame<Bytes> = Frame::data(Bytes::from(frame));
    let mut trailers = HeaderMap::new();
    trailers.insert(GRPC_STATUS.clone(), HeaderValue::from_static("0"));
    trailers.insert(GRPC_MESSAGE.clone(), HeaderValue::from_static(""));
    let trailer_frame: Frame<Bytes> = Frame::trailers(trailers);

    let stream = futures_util::stream::iter(vec![
        Ok::<_, hyper::Error>(data_frame),
        Ok::<_, hyper::Error>(trailer_frame),
    ]);
    let body = http_body_util::StreamBody::new(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "application/grpc+proto")
        .body(BoxBody::new(body))
        .unwrap_or_else(|_| empty_fallback())
}

fn empty_fallback() -> Response<BoxBody<Bytes, hyper::Error>> {
    Response::new(
        Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed(),
    )
}

/// Build a `200 OK` response whose only body frame is a gRPC trailer
/// block carrying the given status. Used for gateway-origin errors
/// (deadline exceeded, backend unreachable) so the client observes
/// them as proper gRPC failures rather than a bare HTTP code.
fn grpc_error_response(status: GrpcStatus, msg: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
    let mut trailers = HeaderMap::new();
    let code = status as u32;
    if let Ok(hv) = HeaderValue::from_str(&code.to_string()) {
        trailers.insert(GRPC_STATUS.clone(), hv);
    }
    if let Ok(hv) = HeaderValue::from_str(msg) {
        trailers.insert(GRPC_MESSAGE.clone(), hv);
    }
    let stream = futures_util::stream::iter(vec![Ok::<_, hyper::Error>(Frame::<Bytes>::trailers(
        trailers,
    ))]);
    let body = http_body_util::StreamBody::new(stream);
    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "application/grpc")
        .body(BoxBody::new(body))
        .unwrap_or_else(|_| empty_fallback())
}

/// Translate an upstream response into the downstream shape.
///
/// If the upstream HTTP status is 200, we forward the body + trailers
/// as-is (gRPC's own `grpc-status` trailer is the source of truth). If
/// it's non-200 the gateway synthesises a `200 OK` + gRPC trailers via
/// the HTTP→gRPC status mapping — gRPC clients do not understand bare
/// HTTP errors.
fn finalize_upstream(resp: Response<IncomingBody>) -> Response<BoxBody<Bytes, hyper::Error>> {
    let (parts, body) = resp.into_parts();
    if parts.status == StatusCode::OK {
        let body = body.map_err(hyper::Error::from).boxed();
        let mut builder = Response::builder().status(parts.status);
        if let Some(hdrs) = builder.headers_mut() {
            for (k, v) in &parts.headers {
                hdrs.insert(k, v.clone());
            }
        }
        return builder.body(body).unwrap_or_else(|_| empty_fallback());
    }
    let code = GrpcStatus::from_http_status(parts.status.as_u16());
    grpc_error_response(code, &format!("upstream http {}", parts.status.as_u16()))
}

// ── header names ────────────────────────────────────────────────────────

static GRPC_TIMEOUT: HeaderName = HeaderName::from_static("grpc-timeout");
static GRPC_STATUS: HeaderName = HeaderName::from_static("grpc-status");
static GRPC_MESSAGE: HeaderName = HeaderName::from_static("grpc-message");
static TE_NAME: HeaderName = HeaderName::from_static("te");

const HEALTH_CHECK_PATH: &str = "/grpc.health.v1.Health/Check";

// Make the `IncomingBody` type alias usable in tests without exporting it.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    fn req_with_ct(ct: &str) -> Request<Empty<Bytes>> {
        Request::builder()
            .method("POST")
            .uri("/svc/Method")
            .header(hyper::header::CONTENT_TYPE, ct)
            .body(Empty::<Bytes>::new())
            .unwrap()
    }

    #[test]
    fn is_grpc_request_matches_application_grpc() {
        assert!(is_grpc_request(&req_with_ct("application/grpc")));
    }

    #[test]
    fn is_grpc_request_matches_application_grpc_plus_proto() {
        assert!(is_grpc_request(&req_with_ct("application/grpc+proto")));
    }

    #[test]
    fn is_grpc_request_matches_case_insensitive() {
        assert!(is_grpc_request(&req_with_ct("APPLICATION/GRPC+JSON")));
    }

    #[test]
    fn is_grpc_request_matches_with_charset_parameter() {
        // `application/grpc; charset=utf-8` is legal; strip the params.
        assert!(is_grpc_request(&req_with_ct(
            "application/grpc; charset=utf-8"
        )));
    }

    #[test]
    fn is_grpc_request_rejects_application_json() {
        assert!(!is_grpc_request(&req_with_ct("application/json")));
    }

    #[test]
    fn is_grpc_request_rejects_empty_extension() {
        // `application/grpc+` — the grammar requires at least one
        // codec character after the plus.
        assert!(!is_grpc_request(&req_with_ct("application/grpc+")));
    }

    #[test]
    fn grpc_timeout_parse_and_clamp_at_max() {
        // Client says "600S" (600 s); max_deadline = 300 s. The header
        // is rewritten in place. `GrpcDeadline::format_timeout` prefers
        // the coarsest unit that evenly divides the milliseconds, so
        // 300_000 ms renders as "5M"; the value is still the same
        // deadline, just expressed in minutes.
        let mut h = HeaderMap::new();
        h.insert(GRPC_TIMEOUT.clone(), HeaderValue::from_static("600S"));
        let ms = clamp_grpc_timeout(&mut h, Duration::from_secs(300)).unwrap();
        assert_eq!(ms, 300_000);
        let rewritten = h.get(&GRPC_TIMEOUT).unwrap().to_str().unwrap().to_owned();
        // Re-parse to prove the round-trip: whatever format was chosen,
        // it must still decode back to 300_000 ms.
        assert_eq!(GrpcDeadline::parse_timeout(&rewritten).unwrap(), 300_000);
    }

    #[test]
    fn grpc_timeout_below_max_is_preserved() {
        let mut h = HeaderMap::new();
        h.insert(GRPC_TIMEOUT.clone(), HeaderValue::from_static("5S"));
        let ms = clamp_grpc_timeout(&mut h, Duration::from_secs(300)).unwrap();
        assert_eq!(ms, 5_000);
        assert_eq!(h.get(&GRPC_TIMEOUT).unwrap().to_str().unwrap(), "5S");
    }

    #[test]
    fn grpc_timeout_absent_returns_none() {
        let mut h = HeaderMap::new();
        assert!(clamp_grpc_timeout(&mut h, Duration::from_secs(300)).is_none());
    }

    #[tokio::test]
    async fn health_check_response_well_formed() {
        let resp = handle_health_check();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(hyper::header::CONTENT_TYPE).unwrap(),
            "application/grpc+proto"
        );
        let collected = resp.into_body().collect().await.unwrap();
        let trailers = collected.trailers().cloned().unwrap_or_default();
        assert_eq!(trailers.get("grpc-status").unwrap(), "0");
        let body_bytes = collected.to_bytes();
        // gRPC frame: 0x00 0x00 0x00 0x00 0x02 0x08 0x01
        assert_eq!(
            body_bytes.as_ref(),
            &[0x00, 0x00, 0x00, 0x00, 0x02, 0x08, 0x01]
        );
    }

    #[test]
    fn http_non_200_translates_to_grpc_status() {
        // 404 → Unimplemented (12); 401 → Unauthenticated (16).
        assert_eq!(
            GrpcStatus::from_http_status(404) as u32,
            GrpcStatus::Unimplemented as u32
        );
        assert_eq!(
            GrpcStatus::from_http_status(401) as u32,
            GrpcStatus::Unauthenticated as u32
        );
        assert_eq!(
            GrpcStatus::from_http_status(503) as u32,
            GrpcStatus::Unavailable as u32
        );
    }

    #[tokio::test]
    async fn grpc_error_response_carries_trailer_status() {
        let resp = grpc_error_response(GrpcStatus::DeadlineExceeded, "gateway deadline");
        assert_eq!(resp.status(), StatusCode::OK);
        let collected = resp.into_body().collect().await.unwrap();
        let trailers = collected.trailers().cloned().unwrap_or_default();
        assert_eq!(trailers.get("grpc-status").unwrap(), "4");
        assert_eq!(trailers.get("grpc-message").unwrap(), "gateway deadline");
    }
}
