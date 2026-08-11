//! gRPC upstream path — a capability attached to
//! [`crate::h2_proxy::H2Proxy`]: inbound H2 streams whose `content-type`
//! matches `application/grpc[+ext]` are peeled off the regular request path.
//!
//! `/grpc.health.v1.Health/Check` is answered locally when configured, so the
//! liveness signal is not coupled to backend availability. Otherwise
//! `grpc-timeout` is parsed, clamped at [`GrpcConfig::max_deadline`] and
//! rewritten, and `TE: trailers` is preserved (RFC 9113 §8.2.2 forbids
//! stripping it for gRPC). Body and trailers pass through verbatim — gRPC
//! carries `grpc-status` / `grpc-message` / `grpc-status-details-bin` in
//! trailers. On gateway deadline elapse, or a non-200 upstream status, the
//! client still receives a `200 OK` with synthesised gRPC trailers, because
//! gRPC clients do not understand bare HTTP errors.
//!
//! Compression negotiation, gRPC-Web, server reflection and upstream mTLS are
//! deliberately post-v1.

use std::sync::Arc;
use std::time::Duration;

use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::body::{Bytes, Frame, Incoming as IncomingBody};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use lb_io::pool::TcpPool;

use lb_grpc::{DEFAULT_MAX_MESSAGE_SIZE, GrpcDeadline, GrpcStatus, decode_grpc_frame};

/// Per-listener gRPC knobs.
#[derive(Debug, Clone, Copy)]
pub struct GrpcConfig {
    /// Master switch. Default `true` when the block is present.
    pub enabled: bool,
    /// Upper bound on an accepted `grpc-timeout`. Client values above it are
    /// clamped before forwarding, and the clamp also bounds the gateway-side
    /// `DEADLINE_EXCEEDED` timer. Default 300 s per gRPC spec guidance.
    pub max_deadline: Duration,
    /// When true, `/grpc.health.v1.Health/Check` is served locally — a gateway
    /// liveness signal independent of backend health. Default true.
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

/// Default upstream-side `max_header_list_size` (GRPC-001). Mirrors the
/// listener default (64 KiB) so a malicious backend cannot transit oversize
/// trailers through the gateway before hyper rejects them.
pub const DEFAULT_UPSTREAM_MAX_HEADER_LIST_SIZE: u32 = 64 * 1024;

/// gRPC reverse proxy. Cheap to clone via [`Arc`].
pub struct GrpcProxy {
    cfg: GrpcConfig,
    pool: TcpPool,
    /// Max decoded HPACK header-list size accepted from the upstream H2 client;
    /// aligned with the listener's thresholds by
    /// [`crate::h2_proxy::H2Proxy::with_grpc`].
    pub(crate) max_header_list_size: u32,
}

impl GrpcProxy {
    /// Construct a [`GrpcProxy`] consuming the backend [`TcpPool`].
    #[must_use]
    pub const fn new(cfg: GrpcConfig, pool: TcpPool) -> Self {
        Self {
            cfg,
            pool,
            max_header_list_size: DEFAULT_UPSTREAM_MAX_HEADER_LIST_SIZE,
        }
    }

    /// Override the upstream H2 client's `max_header_list_size`, aligning it
    /// with the listener's cap (GRPC-001).
    #[must_use]
    pub const fn with_max_header_list_size(mut self, bytes: u32) -> Self {
        self.max_header_list_size = bytes;
        self
    }

    /// Return the [`GrpcConfig`] in effect.
    #[must_use]
    pub const fn config(&self) -> GrpcConfig {
        self.cfg
    }

    /// The upstream H2 client's `max_header_list_size` (bytes). Exposed for
    /// diagnostics and tests.
    #[must_use]
    pub const fn max_header_list_size(&self) -> u32 {
        self.max_header_list_size
    }

    /// Serve a gRPC request. The caller owns the `is_grpc_request` predicate;
    /// this entry point assumes the decision is already made.
    ///
    /// # Errors
    ///
    /// Never — errors become gRPC trailer blocks so the client observes proper
    /// gRPC failures rather than connection resets.
    pub async fn handle(
        self: Arc<Self>,
        req: Request<IncomingBody>,
        backend_addr: std::net::SocketAddr,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        if self.cfg.health_synthesized
            && req.method() == Method::POST
            && req.uri().path() == HEALTH_CHECK_PATH
        {
            return handle_health_check(req).await;
        }
        self.forward(req, backend_addr).await
    }

    /// Forward the gRPC request over a fresh H2 client connection. A present
    /// `grpc-timeout` is clamped at `max_deadline` and re-emitted, and the
    /// gateway wraps the upstream call in that budget so it can synthesise
    /// `DEADLINE_EXCEEDED` when the backend stalls.
    async fn forward(
        &self,
        req: Request<IncomingBody>,
        backend_addr: std::net::SocketAddr,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let (mut parts, body) = req.into_parts();

        // GRPC-002: distinguish header-absent (no deadline, forward) from
        // header-present-and-malformed (respond `INVALID_ARGUMENT` WITHOUT
        // dialing the backend).
        let deadline_ms =
            match parse_and_clamp_grpc_timeout(&mut parts.headers, self.cfg.max_deadline) {
                ParsedTimeout::Absent => None,
                ParsedTimeout::Ok(ms) => Some(ms),
                ParsedTimeout::Malformed(raw) => {
                    return grpc_error_response(
                        GrpcStatus::InvalidArgument,
                        &format!("malformed grpc-timeout: {raw}"),
                    );
                }
            };

        // gRPC requires `TE: trailers` (RFC 9113 §8.2.2). Re-insert defensively
        // so a future middleware that strips it cannot break gRPC.
        parts
            .headers
            .insert(TE_NAME.clone(), HeaderValue::from_static("trailers"));

        // hyper's H2 client requires an absolute URI; H2 server-side requests
        // arrive path-only because `:scheme`/`:authority` are separate
        // pseudo-headers on that side.
        if let Some(new_uri) = rewrite_uri_for_upstream(&parts.uri, backend_addr) {
            parts.uri = new_uri;
        }

        // Box the body: passing `IncomingBody` directly trips subtle Send/Sync
        // bound mismatches inside hyper's generic machinery.
        let upstream_body: BoxBody<Bytes, hyper::Error> = body.map_err(hyper::Error::from).boxed();
        let upstream_req = Request::from_parts(parts, upstream_body);

        let pooled = match self.pool.acquire_async(backend_addr).await {
            Ok(p) => p,
            Err(e) => {
                return grpc_error_response(
                    GrpcStatus::Unavailable,
                    &format!("backend dial failed: {e}"),
                );
            }
        };
        let Some(upstream_io) = pooled.take_stream() else {
            return grpc_error_response(GrpcStatus::Internal, "pooled stream missing");
        };

        // GRPC-001: cap the upstream client's `max_header_list_size` at the
        // listener-derived value so a malicious backend cannot blast oversize
        // trailers through the gateway before hyper rejects them.
        let mut h2_builder = hyper::client::conn::http2::Builder::new(TokioExecutor::new());
        h2_builder.max_header_list_size(self.max_header_list_size);
        let (mut sender, conn) = match h2_builder
            .handshake::<_, BoxBody<Bytes, hyper::Error>>(TokioIo::new(upstream_io))
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

/// Rebuild the request URI so hyper's H2 client accepts it (it demands
/// `:scheme` + `:authority`). Scheme is `http` because v1 upstream is always
/// plaintext TCP; the backend `SocketAddr` is the authority.
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

/// Outcome of parsing `grpc-timeout` (GRPC-002): absent (forward without a
/// deadline), OK (clamp + forward), or malformed — which the gateway must
/// answer with `grpc-status: 3 INVALID_ARGUMENT` per the gRPC spec.
#[derive(Debug)]
enum ParsedTimeout {
    /// `grpc-timeout` header not present (or non-UTF-8). Forward.
    Absent,
    /// Parsed successfully; the header was rewritten to the clamped value.
    Ok(u64),
    /// Present but not matching `Timeout = 1*DIGIT TimeUnit`; carries the raw
    /// value for the diagnostic `grpc-message` echo.
    Malformed(String),
}

/// Parse and clamp the `grpc-timeout` header in place, distinguishing
/// absent / malformed / OK cases (auditor-delta finding GRPC-002).
fn parse_and_clamp_grpc_timeout(headers: &mut HeaderMap, max: Duration) -> ParsedTimeout {
    let Some(hv) = headers.get(&GRPC_TIMEOUT) else {
        return ParsedTimeout::Absent;
    };
    let Ok(raw) = hv.to_str() else {
        // Non-UTF-8 header is malformed by definition.
        return ParsedTimeout::Malformed(String::from("<non-utf-8>"));
    };
    let raw_owned = raw.to_owned();
    let Ok(parsed_ms) = GrpcDeadline::parse_timeout(raw) else {
        return ParsedTimeout::Malformed(raw_owned);
    };
    let max_ms = u64::try_from(max.as_millis()).unwrap_or(u64::MAX);
    let effective = parsed_ms.min(max_ms);
    let rewritten = GrpcDeadline::format_timeout(effective);
    if let Ok(hv) = HeaderValue::from_str(&rewritten) {
        headers.insert(GRPC_TIMEOUT.clone(), hv);
    }
    ParsedTimeout::Ok(effective)
}

/// Test-only wrapper around [`parse_and_clamp_grpc_timeout`] returning
/// `Some(ms)` only for a valid header. Production branches on
/// [`ParsedTimeout`] directly so the malformed case can surface as
/// `INVALID_ARGUMENT` per gRPC spec (GRPC-002).
#[cfg(test)]
fn clamp_grpc_timeout(headers: &mut HeaderMap, max: Duration) -> Option<u64> {
    match parse_and_clamp_grpc_timeout(headers, max) {
        ParsedTimeout::Ok(ms) => Some(ms),
        ParsedTimeout::Absent | ParsedTimeout::Malformed(_) => None,
    }
}

/// Serve the synthesized `/grpc.health.v1.Health/Check` response, per the gRPC
/// health-check spec: an empty `service` is the overall server probe →
/// `SERVING`; a non-empty one is not registered with this gateway (no
/// per-service registry in v1) → `grpc-status: 5 NOT_FOUND` (GRPC-003).
/// Decode is hand-rolled so the gateway stays prost-free.
async fn handle_health_check(req: Request<IncomingBody>) -> Response<BoxBody<Bytes, hyper::Error>> {
    // A zero-length body or a decode error is treated as "empty service" — the
    // overall health probe, and we are by design always SERVING.
    let body_bytes = (req.into_body().collect().await)
        .map_or_else(|_| Bytes::new(), http_body_util::Collected::to_bytes);
    let service = decode_health_check_service(&body_bytes);

    if service.is_empty() {
        return health_check_serving_response();
    }
    grpc_error_response(
        GrpcStatus::NotFound,
        &format!("service not registered: {service}"),
    )
}

/// Build the `200 OK` SERVING response: a gRPC frame carrying the two-byte
/// protobuf message `0x08 0x01`, plus `grpc-status: 0` trailers.
fn health_check_serving_response() -> Response<BoxBody<Bytes, hyper::Error>> {
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

/// Hand-decode `HealthCheckRequest { string service = 1; }` without pulling
/// prost: a 5-byte gRPC frame header, then protobuf fields, where `service` is
/// `tag=1 wire=2` (the single varint byte `0x0A`, a varint length, then UTF-8
/// bytes). Returns `""` if absent or malformed — the "overall health" branch,
/// the safest default and what the spec asks for.
fn decode_health_check_service(body: &[u8]) -> String {
    // Empty body = overall health probe.
    if body.is_empty() {
        return String::new();
    }
    // Decode the gRPC envelope to get the raw protobuf message.
    let Ok((frame, _consumed)) = decode_grpc_frame(body, DEFAULT_MAX_MESSAGE_SIZE) else {
        return String::new();
    };
    // Compressed health-check requests are not in the spec; treat as overall.
    if frame.compressed {
        return String::new();
    }
    let payload = frame.data;

    // Only field #1 (`service`) is meaningful; other tags are skipped by wire
    // type.
    let mut i = 0usize;
    while i < payload.len() {
        let Some((tag, n)) = read_varint(&payload, i) else {
            return String::new();
        };
        i += n;
        let field_number = tag >> 3;
        let wire_type = tag & 0x07;
        match (field_number, wire_type) {
            (1, 2) => {
                // Length-delimited string for `service`.
                let Some((len, n)) = read_varint(&payload, i) else {
                    return String::new();
                };
                i += n;
                let Ok(len) = usize::try_from(len) else {
                    return String::new();
                };
                let Some(end) = i.checked_add(len) else {
                    return String::new();
                };
                if end > payload.len() {
                    return String::new();
                }
                let Some(bytes) = payload.get(i..end) else {
                    return String::new();
                };
                let Ok(s) = std::str::from_utf8(bytes) else {
                    return String::new();
                };
                return s.to_owned();
            }
            (_, 0) => {
                // Skip varint value.
                let Some((_, n)) = read_varint(&payload, i) else {
                    return String::new();
                };
                i += n;
            }
            (_, 2) => {
                // Skip length-delimited value (length varint + bytes).
                let Some((len, n)) = read_varint(&payload, i) else {
                    return String::new();
                };
                i += n;
                let Ok(len) = usize::try_from(len) else {
                    return String::new();
                };
                let Some(end) = i.checked_add(len) else {
                    return String::new();
                };
                if end > payload.len() {
                    return String::new();
                }
                i = end;
            }
            (_, 5) => i = i.saturating_add(4), // fixed32
            (_, 1) => i = i.saturating_add(8), // fixed64
            _ => return String::new(),         // unknown / SGROUP / EGROUP
        }
    }
    String::new()
}

/// Read a base-128 varint, returning `(value, consumed_bytes)`. `None` on
/// truncation or on a varint longer than 10 bytes (the 64-bit maximum).
fn read_varint(buf: &[u8], start: usize) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut i = 0;
    while i < 10 {
        let idx = start.checked_add(i)?;
        let byte = *buf.get(idx)?;
        result |= u64::from(byte & 0x7F).checked_shl(shift)?;
        i += 1;
        if byte & 0x80 == 0 {
            return Some((result, i));
        }
        shift = shift.checked_add(7)?;
    }
    None
}

fn empty_fallback() -> Response<BoxBody<Bytes, hyper::Error>> {
    Response::new(
        Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed(),
    )
}

/// Build a `200 OK` whose only body frame is a gRPC trailer block carrying
/// `status`. Used for gateway-origin errors (deadline exceeded, backend
/// unreachable) so the client observes a proper gRPC failure, not a bare HTTP
/// code.
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

/// Translate an upstream response into the downstream shape. A 200 forwards
/// body + trailers as-is (gRPC's own `grpc-status` trailer is the source of
/// truth); a non-200 becomes a synthesised `200 OK` + gRPC trailers via the
/// HTTP→gRPC status mapping, because gRPC clients do not understand bare HTTP
/// errors.
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
        // Client says 600S; max is 300 s. `format_timeout` prefers the coarsest
        // unit that divides evenly, so 300_000 ms renders as "5M" — the same
        // deadline, expressed in minutes.
        let mut h = HeaderMap::new();
        h.insert(GRPC_TIMEOUT.clone(), HeaderValue::from_static("600S"));
        let ms = clamp_grpc_timeout(&mut h, Duration::from_secs(300)).unwrap();
        assert_eq!(ms, 300_000);
        let rewritten = h.get(&GRPC_TIMEOUT).unwrap().to_str().unwrap().to_owned();
        // Re-parse to prove the round-trip regardless of the chosen format.
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
    async fn health_check_serving_response_well_formed() {
        let resp = health_check_serving_response();
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
    fn decode_health_check_service_empty_body_returns_empty() {
        assert_eq!(decode_health_check_service(&[]), "");
    }

    #[test]
    fn decode_health_check_service_empty_message_returns_empty() {
        // gRPC frame whose payload is a zero-byte protobuf message.
        let buf = [0u8, 0, 0, 0, 0];
        assert_eq!(decode_health_check_service(&buf), "");
    }

    #[test]
    fn decode_health_check_service_decodes_string_field() {
        // protobuf: field 1, wire 2 (string), value "foo.Bar"
        // Byte layout: 0x0A, 0x07, 'f','o','o','.','B','a','r'
        let pb: Vec<u8> = vec![0x0A, 0x07, b'f', b'o', b'o', b'.', b'B', b'a', b'r'];
        // gRPC frame header: compressed=0, BE u32 length=9, then payload.
        let mut buf = Vec::new();
        buf.push(0u8);
        buf.extend_from_slice(&u32::try_from(pb.len()).unwrap().to_be_bytes());
        buf.extend_from_slice(&pb);
        assert_eq!(decode_health_check_service(&buf), "foo.Bar");
    }

    #[test]
    fn decode_health_check_service_skips_unknown_field() {
        // Field 99, wire 0 (varint), value 7. Field 1 absent.
        // Build the bytes via varint: tag = (99 << 3) | 0 (wire 0) = 792.
        let mut pb = Vec::new();
        let tag: u64 = 99 << 3; // wire type 0 contributes nothing
        write_varint(&mut pb, tag);
        write_varint(&mut pb, 7); // varint value 7
        let mut buf = Vec::new();
        buf.push(0u8);
        buf.extend_from_slice(&u32::try_from(pb.len()).unwrap().to_be_bytes());
        buf.extend_from_slice(&pb);
        // No field-1 anywhere → "" (overall health).
        assert_eq!(decode_health_check_service(&buf), "");
    }

    #[test]
    fn parse_and_clamp_grpc_timeout_malformed_yields_invalid_argument() {
        // GRPC-002: malformed value surfaces as ParsedTimeout::Malformed.
        let mut h = HeaderMap::new();
        h.insert(GRPC_TIMEOUT.clone(), HeaderValue::from_static("foo"));
        match parse_and_clamp_grpc_timeout(&mut h, Duration::from_secs(300)) {
            ParsedTimeout::Malformed(raw) => assert_eq!(raw, "foo"),
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn parse_and_clamp_grpc_timeout_absent_yields_absent() {
        let mut h = HeaderMap::new();
        match parse_and_clamp_grpc_timeout(&mut h, Duration::from_secs(300)) {
            ParsedTimeout::Absent => {}
            other => panic!("expected Absent, got {other:?}"),
        }
    }

    fn write_varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push(((v & 0x7F) as u8) | 0x80);
            v >>= 7;
        }
        out.push((v & 0x7F) as u8);
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
