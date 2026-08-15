//! Real hyper 1.x HTTP/1.1 proxy path.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::body::{Bytes, Incoming as IncomingBody};
// Reuse the cross-cell 64 MiB request-body cap by IMPORT; do NOT redefine it.
use crate::h2_proxy::MAX_REQUEST_BODY_BYTES;
// CF-DEDUP-1: the H1→H2 leg speaks the h2_proxy `ProxyErr` (the shared
// graceful-drop driver's type), NOT the h1_proxy-local one.
use crate::h2_proxy::{H2_ABORT_OBSERVE_TIMEOUT, ProxyErr as H2ProxyErr, drive_h2_upstream_send};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use lb_health::{AttemptOutcome, HealthRegistry, UpstreamErrorClass};
use lb_io::http2_pool::Http2Pool;
use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;
use tokio::io::{AsyncRead, AsyncWrite};

use lb_security::{ConnId, SmuggleDetector, SmuggleMode, Watchdog};

use crate::security_hooks::{DynSecurityHooks, NoopHooks, SecurityReject};
use crate::stripped_request::{StrippedRequest, strip_hop_by_hop as strip_into_newtype};
use crate::upstream::{BackendInfoPicker, SingleProtoPicker, UpstreamBackend, UpstreamProto};
use crate::ws_proxy::{self, WsProxy, build_handshake_response_headers, is_h1_upgrade_request};

/// Hop-by-hop headers per RFC 9110 §7.6.1.
///
/// PROTO-2-08: `"trailers"` is NOT a field name (only a `TE:` value-token) and
/// the real `Trailer:` header is end-to-end — do not re-add it here.
static HOP_BY_HOP: [HeaderName; 8] = [
    HeaderName::from_static("connection"),
    HeaderName::from_static("proxy-connection"),
    HeaderName::from_static("keep-alive"),
    HeaderName::from_static("proxy-authenticate"),
    HeaderName::from_static("proxy-authorization"),
    HeaderName::from_static("te"),
    HeaderName::from_static("transfer-encoding"),
    HeaderName::from_static("upgrade"),
];

/// Client-facing response body. Boxed error rather than `hyper::Error` (no
/// public ctor) so a streaming response can inject [`H1PumpAbort`].
pub(crate) type ClientRespBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

/// Bounded in-flight channel depth; `DEPTH × CHUNK_MAX` is the body-size-
/// INDEPENDENT retained-memory ceiling the R8 memory proof asserts.
const H1_REQ_CHANNEL_DEPTH: usize = 8;

const H1_REQ_CHUNK_MAX: usize = 8 * 1024;

/// F-MD-4 — request-smuggling guard, H1 mirror of `h2_proxy::PumpAbort`.
/// Dropping the body channel reads as a CLEAN EOF, so hyper would emit the
/// chunked terminator and the upstream would see a truncated request as
/// COMPLETE. `hyper::Error` has no public ctor, so the pump sends
/// `Err(H1PumpAbort)` and hyper aborts without a terminator.
#[derive(Debug)]
struct H1PumpAbort;

impl std::fmt::Display for H1PumpAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("inbound H1 request body aborted before clean end-of-body")
    }
}

impl std::error::Error for H1PumpAbort {}

/// Reject framing/routing fields in inbound H1 request trailers.
///
/// THE CATCH: hyper's chunked-trailer decoder
/// (`proto/h1/decode.rs::decode_trailers`) validates SYNTAX only and inserts
/// EVERY field, so forwarding trailers verbatim would also relay a
/// `Transfer-Encoding` / `Content-Length` — a desync primitive at the next hop
/// (RFC 7230 §4.1.2 / RFC 9110 §6.5.1).
fn validate_h1_request_trailers(trailers: &hyper::HeaderMap) -> Result<(), ProxyErr> {
    use hyper::header::{CONNECTION, CONTENT_LENGTH, HOST, TE, TRAILER, TRANSFER_ENCODING};
    for name in trailers.keys() {
        if name == CONTENT_LENGTH
            || name == TRANSFER_ENCODING
            || name == HOST
            || name == TRAILER
            || name == TE
            || name == CONNECTION
            || HOP_BY_HOP.iter().any(|h| h == name)
        {
            return Err(ProxyErr::BadRequest(format!(
                "forbidden field `{}` in request trailers (RFC 7230 §4.1.2)",
                name.as_str()
            )));
        }
    }
    Ok(())
}

/// `Alt-Svc` advertisement injected into responses.
#[derive(Debug, Clone, Copy)]
pub struct AltSvcConfig {
    /// UDP port of the advertised H3 listener.
    pub h3_port: u16,
    /// `max-age` in seconds.
    pub max_age: u32,
}

impl AltSvcConfig {
    /// Render `h3=":<h3_port>"; ma=<max_age>`.
    #[must_use]
    pub fn header_value(self) -> String {
        format!("h3=\":{}\"; ma={}", self.h3_port, self.max_age)
    }
}

/// Per-listener HTTP timeouts.
#[derive(Debug, Clone, Copy)]
pub struct HttpTimeouts {
    /// Slowloris deadline for the request line + header section; also the WS
    /// upgrade-dial budget. NOT the whole-request deadline — that is `total`.
    pub header: Duration,
    /// Phase-A no-forward-progress idle deadline for response / request-body IO.
    pub body: Duration,
    /// Hard upper bound on a single connection's total lifetime.
    pub total: Duration,
    /// Phase-B fixed cap on the post-upload head wait.
    pub head: Duration,
}

impl Default for HttpTimeouts {
    fn default() -> Self {
        Self {
            header: Duration::from_secs(10),
            body: Duration::from_secs(30),
            total: Duration::from_secs(60),
            head: Duration::from_secs(60),
        }
    }
}

/// ROUND8-L7-05 — policy for `_` in inbound header names. Duplicated from
/// `lb_config` to avoid a proxy → config dep edge; the wiring crate maps them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeaderUnderscorePolicy {
    /// Reject with `400 Bad Request` if any header name contains `_`. Default.
    #[default]
    Reject,
    /// Silently strip underscore-bearing headers (nginx default).
    Drop,
    /// Pass underscore-bearing headers through verbatim.
    Allow,
}

/// Picks the next backend address; runs on every inbound request, so an impl
/// must be cheap and lock-free or fine-grained.
pub trait BackendPicker: Send + Sync {
    /// Next backend to dial, or `None` if none can serve the request.
    fn pick(&self) -> Option<SocketAddr>;
}

/// Round-robin picker over a fixed address list.
pub struct RoundRobinAddrs {
    addrs: Vec<SocketAddr>,
    counter: parking_lot::Mutex<usize>,
}

impl RoundRobinAddrs {
    /// Create a picker over `addrs`; `None` if `addrs` is empty.
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
    ws: Option<Arc<WsProxy>>,
    h2_upstream: Option<Arc<Http2Pool>>,
    h3_upstream: Option<Arc<QuicUpstreamPool>>,
    hooks: Arc<dyn DynSecurityHooks>,
    watchdog: Option<Watchdog>,
    /// Combined with the peer IP as the [`Watchdog`] key, so two concurrent
    /// NAT-egress connections stay distinct.
    conn_seq: Arc<parking_lot::Mutex<u64>>,
    smuggle_strict: bool,
    header_underscore_policy: HeaderUnderscorePolicy,
    expected_sni: Option<String>,
    /// `0` disables; the cap-th response carries `Connection: close`.
    max_keepalive_requests: u32,
    /// An atomic, not a metric handle — lb-l7 has no metrics-registry dep.
    keepalive_cap_terminations: Arc<std::sync::atomic::AtomicU64>,
    /// G5 passive ejection sink. `None` (the default) leaves the proxy exactly as it was before
    /// ejection existed — only the binary attaches one, so every test that builds a proxy directly
    /// keeps its pre-G5 behaviour.
    health: Option<Arc<HealthRegistry>>,
}

impl H1Proxy {
    /// Construct over a single-protocol H1 backend pool; use
    /// [`Self::with_multi_proto`] for H2/H3 backends.
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
            hooks: Arc::new(NoopHooks::new()),
            watchdog: None,
            conn_seq: Arc::new(parking_lot::Mutex::new(0)),
            smuggle_strict: false,
            header_underscore_policy: HeaderUnderscorePolicy::Reject,
            expected_sni: None,
            max_keepalive_requests: 100,
            keepalive_cap_terminations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            health: None,
        }
    }

    /// Construct an [`H1Proxy`] backed by a multi-protocol picker.
    #[must_use]
    pub fn with_multi_proto(
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
            hooks: Arc::new(NoopHooks::new()),
            watchdog: None,
            conn_seq: Arc::new(parking_lot::Mutex::new(0)),
            smuggle_strict: false,
            header_underscore_policy: HeaderUnderscorePolicy::Reject,
            expected_sni: None,
            max_keepalive_requests: 100,
            keepalive_cap_terminations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            health: None,
        }
    }

    /// Attach a security-hooks impl. Without this call the proxy falls back to
    /// [`NoopHooks`] and the production smuggle / cap / watchdog checks are off.
    #[must_use]
    pub fn with_hooks(mut self, hooks: Arc<dyn DynSecurityHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Attach an [`lb_security::Watchdog`]. Only the header-byte estimate is
    /// recorded as progress; per-chunk body progress is NOT wired.
    #[must_use]
    pub fn with_watchdog(mut self, watchdog: Watchdog) -> Self {
        self.watchdog = Some(watchdog);
        self
    }

    /// G5 — attach the passive-ejection registry this proxy feeds. The SAME registry must back the
    /// [`crate::upstream::HealthFilteredPicker`] wrapping this proxy's picker, or outcomes are
    /// recorded against a gate nobody consults.
    #[must_use]
    pub fn with_health(mut self, health: Arc<HealthRegistry>) -> Self {
        self.health = Some(health);
        self
    }

    /// Feed one upstream attempt to the registry. Takes `&self` and returns `()`, and every call
    /// site invokes it AFTER the response value is built — so it cannot alter a response (R3).
    fn record_health(&self, addr: SocketAddr, outcome: AttemptOutcome) {
        if let Some(health) = self.health.as_ref() {
            health.record(addr, outcome);
        }
    }

    /// Enable strict-TE policy ([`SmuggleMode::H1Strict`]); default lenient.
    #[must_use]
    pub const fn with_smuggle_strict(mut self, strict: bool) -> Self {
        self.smuggle_strict = strict;
        self
    }

    /// ROUND8-L7-05: set the header-name underscore policy.
    #[must_use]
    pub const fn with_header_underscore_policy(mut self, policy: HeaderUnderscorePolicy) -> Self {
        self.header_underscore_policy = policy;
        self
    }

    /// Default expected SNI. TLS listeners prefer
    /// [`Self::serve_connection_with_cancel_sni`], which uses the live SNI.
    #[must_use]
    pub fn with_expected_sni(mut self, sni: Option<String>) -> Self {
        self.expected_sni = sni;
        self
    }

    /// Attach an H2 upstream pool for [`UpstreamProto::H2`] backends.
    #[must_use]
    pub fn with_h2_upstream(mut self, pool: Arc<Http2Pool>) -> Self {
        self.h2_upstream = Some(pool);
        self
    }

    /// Attach an H3 upstream pool for [`UpstreamProto::H3`] backends.
    #[must_use]
    pub fn with_h3_upstream(mut self, pool: Arc<QuicUpstreamPool>) -> Self {
        self.h3_upstream = Some(pool);
        self
    }

    /// Has an H2 upstream pool been wired?
    #[must_use]
    pub const fn has_h2_upstream(&self) -> bool {
        self.h2_upstream.is_some()
    }

    /// Has an H3 upstream pool been wired?
    #[must_use]
    pub const fn has_h3_upstream(&self) -> bool {
        self.h3_upstream.is_some()
    }

    /// ROUND8-L7-06: set the per-connection request cap; `0` disables.
    #[must_use]
    pub fn with_max_keepalive_requests(mut self, cap: u32) -> Self {
        self.max_keepalive_requests = cap;
        self
    }

    /// Shared handle to the cap-triggered-close counter, for the wiring crate.
    #[must_use]
    pub fn keepalive_cap_termination_counter(&self) -> Arc<std::sync::atomic::AtomicU64> {
        Arc::clone(&self.keepalive_cap_terminations)
    }

    /// Enable WebSocket upgrade handling on this proxy.
    #[must_use]
    pub fn with_websocket(mut self, ws: Arc<WsProxy>) -> Self {
        self.ws = Some(ws);
        self
    }

    /// Drive HTTP/1.1 server logic over `io` until the connection closes,
    /// bounded by [`HttpTimeouts::total`]. Per-request upstream errors become
    /// 502/504 responses and do NOT terminate the connection.
    ///
    /// # Errors
    /// I/O errors and timeouts.
    pub async fn serve_connection<IO>(self: Arc<Self>, io: IO, peer: SocketAddr) -> io::Result<()>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        self.serve_connection_with_cancel(io, peer, tokio_util::sync::CancellationToken::new())
            .await
    }

    /// PROTO-2-11 — H1 half of the drain-on-cancel contract.
    ///
    /// PROTO-2-16 CAVEAT: hyper-1's `http1::graceful_shutdown` only calls
    /// `disable_keep_alive()`, and `Connection: close` is serialised only onto
    /// a head not yet flushed — a cancel landing after the head is on the wire
    /// leaves the FIN as the only close signal (RFC 9110 §7.6.1 permits this).
    ///
    /// # Errors
    /// Same as [`Self::serve_connection`], plus `TimedOut` past
    /// [`HttpTimeouts::total`].
    pub async fn serve_connection_with_cancel<IO>(
        self: Arc<Self>,
        io: IO,
        peer: SocketAddr,
        cancel: tokio_util::sync::CancellationToken,
    ) -> io::Result<()>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let sni = self.expected_sni.clone();
        self.serve_connection_with_cancel_sni(io, peer, cancel, sni)
            .await
    }

    /// H1 entry point threading the per-connection TLS SNI into the hot path so
    /// the authority check runs against the OBSERVED SNI; `None` disables it.
    ///
    /// # Errors
    /// Same as [`Self::serve_connection_with_cancel`].
    pub async fn serve_connection_with_cancel_sni<IO>(
        self: Arc<Self>,
        io: IO,
        peer: SocketAddr,
        cancel: tokio_util::sync::CancellationToken,
        sni: Option<String>,
    ) -> io::Result<()>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let total = self.timeouts.total;
        // ROUND8-L7-06: shared across hyper's per-request service clones.
        let cap = self.max_keepalive_requests;
        let close_signal = Arc::new(tokio::sync::Notify::new());
        let svc = ProxyService {
            inner: Arc::clone(&self),
            peer,
            expected_sni: sni,
            served: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            cap,
            close_signal: Arc::clone(&close_signal),
        };
        // F-RES-1: `header_read_timeout` is INERT unless a `Timer` is wired —
        // hyper silently ignores it and only `total` bounds the header phase.
        let conn = hyper::server::conn::http1::Builder::new()
            .keep_alive(true)
            .timer(TokioTimer::new())
            .header_read_timeout(self.timeouts.header)
            .serve_connection(TokioIo::new(io), svc)
            .with_upgrades();
        tokio::pin!(conn);
        let cancel_fut = cancel.cancelled();
        tokio::pin!(cancel_fut);
        let timer = tokio::time::sleep(total);
        tokio::pin!(timer);
        let cap_close = close_signal.notified();
        tokio::pin!(cap_close);
        tokio::select! {
            // biased: cancel wins ties so a SIGTERM mid-request still drains.
            biased;
            () = &mut cancel_fut => {
                conn.as_mut().graceful_shutdown();
                match tokio::time::timeout(total, conn).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(io::Error::other(format!("h1 graceful shutdown: {e}"))),
                    Err(_) => Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "h1 graceful shutdown timeout",
                    )),
                }
            }
            res = &mut conn => match res {
                Ok(()) => Ok(()),
                Err(e) => Err(io::Error::other(format!("h1 server: {e}"))),
            },
            () = &mut timer => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "total connection timeout",
            )),
            // ROUND8-L7-06: cap reached. A clean completion is `Ok(())` — the
            // cap close is the INTENDED terminal state, unlike a total timeout.
            () = &mut cap_close => {
                conn.as_mut().graceful_shutdown();
                match tokio::time::timeout(total, conn).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(io::Error::other(format!(
                        "h1 keepalive-cap shutdown: {e}"
                    ))),
                    Err(_) => Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "h1 keepalive-cap shutdown timeout",
                    )),
                }
            }
        }
    }
}

/// Service carrying the [`H1Proxy`] plus the peer address. hyper clones it per
/// request; the `Arc`-held connection-scoped state is shared across clones.
#[derive(Clone)]
struct ProxyService {
    inner: Arc<H1Proxy>,
    peer: SocketAddr,
    expected_sni: Option<String>,
    served: Arc<std::sync::atomic::AtomicU32>,
    cap: u32,
    /// Notified once at the cap so the driver issues `graceful_shutdown`.
    close_signal: Arc<tokio::sync::Notify>,
}

impl hyper::service::Service<Request<IncomingBody>> for ProxyService {
    type Response = Response<ClientRespBody>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<IncomingBody>) -> Self::Future {
        let inner = Arc::clone(&self.inner);
        let peer = self.peer;
        let sni = self.expected_sni.clone();
        // `fetch_add` returns the prior value, so `count` is 1-based.
        let cap = self.cap;
        let count = self
            .served
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let force_close = cap > 0 && count >= cap;
        let close_signal = Arc::clone(&self.close_signal);
        let cap_counter = Arc::clone(&inner.keepalive_cap_terminations);
        Box::pin(async move {
            let mut resp = Box::pin(inner.handle(req, peer, sni.as_deref())).await;
            if force_close {
                // Advertise close on the head (RFC 9110 §7.6.1); the driver's
                // `graceful_shutdown` follows. `count == cap` fires once.
                resp.headers_mut()
                    .insert(hyper::header::CONNECTION, HeaderValue::from_static("close"));
                if count == cap {
                    cap_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                close_signal.notify_one();
            }
            Ok(resp)
        })
    }
}

impl H1Proxy {
    /// Span-opening wrapper. Deliberately `.instrument()`-ed rather than
    /// holding a `tracing::span::Entered` across an `.await` — that leaks the
    /// span onto whatever the executor polls next, and only bites under load.
    async fn handle(
        &self,
        req: Request<IncomingBody>,
        peer: SocketAddr,
        expected_sni: Option<&str>,
    ) -> Response<ClientRespBody> {
        use tracing::Instrument;
        let listener_label = if self.is_https { "h1s" } else { "h1" };
        let req_trace = crate::trace_ctx::RequestTrace::open(
            req.headers(),
            "h1",
            req.method().as_str(),
            req.uri()
                .path_and_query()
                .map_or("/", http::uri::PathAndQuery::as_str),
            listener_label,
            expected_sni,
        );
        let span = req_trace.span.clone();
        let resp = self
            .handle_inner(req, peer, expected_sni, req_trace)
            .instrument(span.clone())
            .await;
        span.record("http.status_code", resp.status().as_u16());
        resp
    }

    async fn handle_inner(
        &self,
        req: Request<IncomingBody>,
        peer: SocketAddr,
        expected_sni: Option<&str>,
        req_trace: crate::trace_ctx::RequestTrace,
    ) -> Response<ClientRespBody> {
        // ROUND8-L7-09 — authority-validation CHOKE POINT. MUST stay the FIRST
        // statement: the WS-upgrade fork below reached `pick_info()`
        // unvalidated before this was hoisted here.
        if let Err((bad, err)) = crate::authority::validate_request(&req) {
            tracing::warn!(
                peer = %peer,
                authority = %bad,
                error = ?err,
                "ROUND8-L7-09: H1 authority rejected (choke point)"
            );
            return error_response(StatusCode::BAD_REQUEST, "invalid authority (ROUND8-L7-09)");
        }

        // gRPC needs H2 framing, so reject it on an H1 listener with 415 rather
        // than let a backend answer 502. The trailing `+` deliberately leaves
        // `application/grpc-web` (plain HTTP, forwards transparently) out.
        if req
            .headers()
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| {
                let media_type = s
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                media_type == "application/grpc" || media_type.starts_with("application/grpc+")
            })
        {
            return error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "gRPC requires HTTP/2; this listener is HTTP/1.1",
            );
        }

        if self
            .ws
            .as_ref()
            .is_some_and(|w| w.config().enabled && is_h1_upgrade_request(&req))
        {
            return self.handle_ws_upgrade(req, req_trace).await;
        }

        let (mut parts, body) = req.into_parts();

        // ROUND8-L7-05 header-name underscore policy. SEC-2-01: strict smuggle
        // mode FORCES `Reject` regardless of operator config — opting out of
        // underscore rejection requires opting out of strict-TE mode too.
        let effective_policy = if self.smuggle_strict {
            HeaderUnderscorePolicy::Reject
        } else {
            self.header_underscore_policy
        };
        match effective_policy {
            HeaderUnderscorePolicy::Reject => {
                if parts
                    .headers
                    .iter()
                    .any(|(n, _)| n.as_str().as_bytes().contains(&b'_'))
                {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "header name contains underscore (ROUND8-L7-05)",
                    );
                }
            }
            HeaderUnderscorePolicy::Drop => {
                let to_drop: Vec<hyper::header::HeaderName> = parts
                    .headers
                    .iter()
                    .filter_map(|(n, _)| {
                        if n.as_str().as_bytes().contains(&b'_') {
                            Some(n.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                for name in to_drop {
                    parts.headers.remove(name);
                }
            }
            HeaderUnderscorePolicy::Allow => {}
        }

        // Hooks run BEFORE the strip + upstream-acquire so a rejected request
        // never spends a pool slot.
        let inspect_req = {
            let mut b = Request::builder()
                .method(parts.method.clone())
                .uri(parts.uri.clone())
                .version(parts.version);
            for (n, v) in &parts.headers {
                b = b.header(n.clone(), v.clone());
            }
            b.body(()).unwrap_or_else(|_| Request::new(()))
        };
        if let Err(rej) = self.hooks.inspect_request(&inspect_req, peer.ip()) {
            return reject_to_response(&rej);
        }

        // ROUND8-L7-09 authority validation already ran at the choke point.

        // PROTO-2-18 — SNI ↔ Host agreement (RFC 9110 §15.5.20). Loopback peers
        // skip it (sec-r5): the vector is L7 routing/authz, and probe scripts
        // use IP-literal Host headers that cannot match the cert's SNI.
        if !peer.ip().is_loopback() {
            let authority = parts
                .headers
                .get(hyper::header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if let Err(mismatch) =
                crate::sni_authority::check_sni_authority(expected_sni, authority)
            {
                tracing::warn!(
                    peer = %peer,
                    sni = %mismatch.sni,
                    authority = %mismatch.authority,
                    "PROTO-2-18: H1 SNI/Host mismatch — emitting 421 Misdirected Request"
                );
                let (status, body) = crate::sni_authority::misdirected_response();
                return error_response(status, body);
            }
        }

        // SEC-2-01 defense-in-depth: this site fires regardless of the wired
        // `DynSecurityHooks` impl, so the detector is never dead code.
        let header_pairs: Vec<(String, String)> = parts
            .headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_owned(), s.to_owned()))
            })
            .collect();
        let smuggle_mode = if self.smuggle_strict {
            SmuggleMode::H1Strict
        } else {
            SmuggleMode::H1
        };
        if let Err(e) = SmuggleDetector::check_all_mode(&header_pairs, smuggle_mode) {
            tracing::warn!(error = %e, peer = %peer, "h1 smuggle rejected");
            return error_response(StatusCode::BAD_REQUEST, "request smuggling");
        }

        let watch_id = self.watchdog.as_ref().map(|wd| {
            let seq = {
                let mut g = self.conn_seq.lock();
                *g = g.wrapping_add(1);
                *g
            };
            let id = ConnId::new(peer.ip(), seq);
            let deadline = std::time::Instant::now() + self.timeouts.header;
            wd.register(id, deadline);
            // The detector treats progress as cumulative bytes-read.
            let header_bytes: u64 = parts
                .headers
                .iter()
                .map(|(n, v)| n.as_str().len() as u64 + v.len() as u64 + 4)
                .sum();
            if let Err(e) = wd.progress(id, header_bytes) {
                tracing::warn!(error = %e, peer = %peer, "h1 watchdog evicted at header phase");
            }
            id
        });

        let host = parts
            .headers
            .get(hyper::header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        // PROTO-2-07 — the newtype factory makes the strip a type-level
        // guarantee for the downstream proxy_* methods.
        let req_pre_strip = Request::from_parts(parts, body);
        let mut stripped = strip_into_newtype(req_pre_strip);
        {
            let headers = stripped.headers_mut();
            append_xff(headers, peer);
            set_xfp(headers, self.is_https);
            if let Some(h) = host.as_deref() {
                set_xfh(headers, h);
            }
            append_via(headers);
        }

        let Some(backend) = self.picker.pick_info() else {
            return error_response(StatusCode::BAD_GATEWAY, "no backend available");
        };

        // G5: every arm yields the response AND the health verdict for `backend.addr`, so the
        // whole attempt is classified once, here, rather than per protocol leg.
        let (resp, outcome) = match backend.proto {
            UpstreamProto::H1 => match self.proxy_request(backend.addr, stripped).await {
                Ok(resp) => (self.finalize_response(resp), AttemptOutcome::Success),
                Err(e) => {
                    let outcome = e.error_class().outcome();
                    let resp = match e {
                        ProxyErr::Upstream(s) => error_response(StatusCode::BAD_GATEWAY, &s),
                        ProxyErr::Timeout => {
                            error_response(StatusCode::GATEWAY_TIMEOUT, "upstream timeout")
                        }
                        // Client's fault → 400; the backend response is never relayed.
                        ProxyErr::BadRequest(s) => error_response(StatusCode::BAD_REQUEST, &s),
                        ProxyErr::BodyTooLarge => error_response(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "request body exceeds maximum allowed size",
                        ),
                    };
                    (resp, outcome)
                }
            },
            UpstreamProto::H2 => Box::pin(self.proxy_h1_to_h2(backend.addr, stripped)).await,
            UpstreamProto::H3 => Box::pin(self.proxy_h1_to_h3(&backend, stripped)).await,
        };
        self.record_health(backend.addr, outcome);
        // The sweeper covers the abandoned-future case.
        if let (Some(wd), Some(id)) = (self.watchdog.as_ref(), watch_id) {
            wd.deregister(id);
        }
        resp
    }

    /// Forward an H1 inbound request to an H1 upstream over a single-use TCP
    /// stream.
    ///
    /// **ROUND8-L7-10 — take-and-discard upstream stream pattern.**
    /// `pooled.take_stream()` consumes the wrapper WITHOUT its return-to-pool
    /// `Drop`, so the upstream socket is never reused. Pingora paid for this
    /// twice (0.6.0 / 0.8.0 CHANGELOG): an upstream sending fewer or more body
    /// bytes than its declared Content-Length corrupts the next pipelined
    /// request on a reused connection — an upstream smuggling primitive.
    ///
    /// **Refactor warning.** Pooling H1 upstream connections MUST first compare
    /// the response body to `Content-Length` and call
    /// [`lb_io::pool::PooledTcp::set_reusable(false)`](lb_io::pool::PooledTcp::set_reusable)
    /// on mismatch before letting the wrapper drop.
    async fn proxy_request(
        &self,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Result<Response<IncomingBody>, ProxyErr> {
        use hyper::body::Frame;

        let req = req.into_inner();
        let (mut parts, mut body) = req.into_parts();

        // F-MD-1 — force HTTP/1.1 and STRIP `content-length`/`transfer-encoding`
        // so hyper's encoder picks the framing for the unknown-length
        // `StreamBody`; a stale CL makes it truncate or emit an empty body and
        // never poll the pump. Framing correctness, not a security check.
        parts.version = hyper::Version::HTTP_11;
        parts.headers.remove(hyper::header::CONTENT_LENGTH);
        parts.headers.remove(hyper::header::TRANSFER_ENCODING);

        // S9 — Branch-B-only (no lookahead): H1 ingress has no
        // validate-before-forward ordering requirement, so dial first and
        // forward-as-it-arrives through a bounded window. No `collect()`.
        let pooled = self
            .pool
            .acquire_async(backend_addr)
            .await
            .map_err(|e| ProxyErr::Upstream(format!("backend connect {backend_addr}: {e}")))?;

        // ROUND8-L7-10 (fn doc): `take_stream` defeats the return-to-pool Drop.
        // Do not remove without first implementing the body-length guard.
        let stream = pooled
            .take_stream()
            .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;

        // F-MD-4: a CONSTRUCTIBLE body error so the pump can INJECT an error;
        // dropping the channel instead reads as a smuggled-complete request.
        let (mut sender, conn) = hyper::client::conn::http1::handshake::<
            _,
            BoxBody<Bytes, H1PumpAbort>,
        >(TokioIo::new(stream))
        .await
        .map_err(|e| ProxyErr::Upstream(format!("h1 client handshake: {e}")))?;

        let conn_handle = tokio::spawn(async move {
            let _ = conn.await;
        });

        // R8 backpressure chain: backend write stalls → hyper stops pulling →
        // the channel fills → the pump stops polling the inbound body.
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, H1PumpAbort>>(H1_REQ_CHANNEL_DEPTH);

        // F-MD-3 — ACTUAL live in-flight occupancy, not a constant.
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight_body = std::sync::Arc::clone(&in_flight_bytes);

        let stream_body =
            http_body_util::StreamBody::new(futures_util::stream::poll_fn(move |cx| {
                let polled = rx.poll_recv(cx);
                if let std::task::Poll::Ready(Some(Ok(ref frame))) = polled {
                    if let Some(d) = frame.data_ref() {
                        in_flight_body.fetch_sub(d.len(), std::sync::atomic::Ordering::Relaxed);
                    }
                }
                polled
            }))
            .boxed();
        let req = Request::from_parts(parts, stream_body);

        // The response head is gated on the pump's terminal verdict.
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), ProxyErr>>();

        // S14 — `idle_bounded_send` switches from the Phase-A idle deadline to
        // the Phase-B head cap once `upload_complete` is set.
        let last_progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let upload_complete = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let epoch = tokio::time::Instant::now();
        let last_progress_pump = std::sync::Arc::clone(&last_progress);
        let upload_complete_pump = std::sync::Arc::clone(&upload_complete);
        let epoch_pump = epoch;

        let pump = tokio::spawn(async move {
            // Relaxed: the helper re-arms next tick if a bump lands late.
            let bump = || {
                let dt = tokio::time::Instant::now().saturating_duration_since(epoch_pump);
                let ms = u64::try_from(dt.as_millis()).unwrap_or(u64::MAX);
                last_progress_pump.store(ms, std::sync::atomic::Ordering::Relaxed);
            };
            // Release pairs with the helper's Acquire load so the FINAL
            // `last_progress` bump is visible before `upload_complete` is.
            let set_complete = || {
                upload_complete_pump.store(true, std::sync::atomic::Ordering::Release);
            };

            // The 64 MiB total-body cap applies in the streaming regime too.
            let mut forwarded_total: usize = 0;

            // `ReceiverGone` = hyper dropped the request body. Do NOT
            // manufacture a 413 (F-MD-2) — drain-and-validate instead.
            enum SendOutcome {
                ReceiverGone,
            }

            macro_rules! send_chunked {
                ($bytes:expr) => {{
                    let mut data: Bytes = $bytes;
                    let mut outcome: Result<(), SendOutcome> = Ok(());
                    while !data.is_empty() {
                        let take = data.len().min(H1_REQ_CHUNK_MAX);
                        let chunk = data.split_to(take);
                        let clen = chunk.len();
                        in_flight_bytes.fetch_add(clen, std::sync::atomic::Ordering::Relaxed);
                        #[cfg(any(test, feature = "test-gauges"))]
                        record_retained_h1(
                            in_flight_bytes.load(std::sync::atomic::Ordering::Relaxed),
                        );
                        if tx.send(Ok(Frame::data(chunk))).await.is_err() {
                            // Never entered hyper's buffer — back the counter out.
                            in_flight_bytes.fetch_sub(clen, std::sync::atomic::Ordering::Relaxed);
                            outcome = Err(SendOutcome::ReceiverGone);
                            break;
                        }
                        bump();
                    }
                    outcome
                }};
            }

            // F-MD-2 drain-and-validate: the backend stopped reading, but the
            // inbound body MUST still reach a validated terminal state or a
            // truncated request would relay the backend's response.
            macro_rules! drain_and_validate {
                () => {{
                    loop {
                        match body.frame().await {
                            // F-MD-4 (H1): `None` IS the confirmed clean end;
                            // a truncation surfaces as `Some(Err)` instead.
                            None => break Ok(()),
                            Some(Ok(frame)) => {
                                if frame.is_trailers() {
                                    if let Some(t) = frame.trailers_ref() {
                                        break validate_h1_request_trailers(t);
                                    }
                                    break Ok(());
                                }
                                if let Some(d) = frame.data_ref() {
                                    forwarded_total = forwarded_total.saturating_add(d.len());
                                    if forwarded_total > MAX_REQUEST_BODY_BYTES {
                                        break Err(ProxyErr::BodyTooLarge);
                                    }
                                }
                                // discard the data frame — bounded memory.
                            }
                            Some(Err(e)) => {
                                break Err(ProxyErr::BadRequest(format!(
                                    "inbound H1 request body incomplete: {e}"
                                )));
                            }
                        }
                    }
                }};
            }

            loop {
                match body.frame().await {
                    None => {
                        // F-MD-4 (H1 — MIRROR-IMAGE of H2; do NOT copy the H2
                        // `is_end_stream` logic here). The inbound H1 body is
                        // hyper's `Kind::Chan`, so `frame()==None` IS the
                        // positively-confirmed clean end; a premature mid-body
                        // half-close arrives as `Some(Err)` instead (hyper emits
                        // `IncompleteBody` on early EOF for both chunked
                        // decode.rs ~L162 and Content-Length ~L504). And do NOT
                        // consult `is_end_stream()`: for `Kind::Chan` it returns
                        // `content_length == ZERO`, unreliable for chunked.
                        set_complete();
                        let _ = verdict_tx.send(Ok(()));
                        return;
                    }
                    Some(Ok(frame)) => {
                        if frame.is_trailers() {
                            // Q-H3: validate BEFORE forwarding — hyper's
                            // trailer decoder does NOT reject framing fields.
                            let verdict = frame
                                .trailers_ref()
                                .map_or(Ok(()), validate_h1_request_trailers);
                            match verdict {
                                Ok(()) => {
                                    let _ = tx.send(Ok(frame)).await;
                                    bump();
                                    set_complete();
                                    let _ = verdict_tx.send(Ok(()));
                                    return;
                                }
                                Err(e) => {
                                    // FIFO Err-before-close: the body error
                                    // FIRST (dropping tx alone = clean EOF =
                                    // smuggled-complete), THEN the verdict.
                                    let _ = tx.send(Err(H1PumpAbort)).await;
                                    let _ = verdict_tx.send(Err(e));
                                    return;
                                }
                            }
                        }
                        if let Ok(data) = frame.into_data() {
                            forwarded_total = forwarded_total.saturating_add(data.len());
                            if forwarded_total > MAX_REQUEST_BODY_BYTES {
                                // FIFO Err-before-close: the body error first
                                // (the upstream ends abruptly), then the 413.
                                let _ = tx.send(Err(H1PumpAbort)).await;
                                let _ = verdict_tx.send(Err(ProxyErr::BodyTooLarge));
                                return;
                            }
                            if let Err(SendOutcome::ReceiverGone) = send_chunked!(data) {
                                // F-MD-2 drain-and-validate, NOT a 413.
                                let _ = verdict_tx.send(drain_and_validate!());
                                return;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        // F-MD-4 (H1): a premature mid-body close surfaces HERE
                        // as `Some(Err)`, not a clean `None`. FIFO
                        // Err-before-close so hyper aborts WITHOUT a `0\r\n\r\n`
                        // terminator, then the 400 verdict.
                        let _ = tx.send(Err(H1PumpAbort)).await;
                        let _ = verdict_tx.send(Err(ProxyErr::BadRequest(format!(
                            "inbound H1 request body incomplete: {e}"
                        ))));
                        return;
                    }
                }
            }
        });

        // Drive the send concurrently with the pump (hyper must pull the channel
        // for the pump to progress under backpressure), but do NOT relay the
        // response until the verdict lands. S14: `idle_bounded_send` replaces a
        // fixed wall-clock deadline with idle (Phase A) + head cap (Phase B).
        let send_fut = sender.send_request(req);
        let resp = match lb_io::idle_send::idle_bounded_send(
            send_fut,
            std::sync::Arc::clone(&last_progress),
            std::sync::Arc::clone(&upload_complete),
            epoch,
            self.timeouts.body,
            self.timeouts.head,
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                // F-CAP-1 — the pump's verdict is the AUTHORITATIVE cause; a
                // `send_request` error is only its downstream effect, and 502
                // here would mask a real 413/400. Bounded by `timeouts.body`,
                // and do NOT `pump.abort()` before this await.
                let classified = match tokio::time::timeout(self.timeouts.body, verdict_rx).await {
                    Ok(Ok(Err(ve @ (ProxyErr::BodyTooLarge | ProxyErr::BadRequest(_))))) => {
                        Some(ve)
                    }
                    // Anything else → a genuine upstream failure; 502.
                    _ => None,
                };
                pump.abort();
                conn_handle.abort();
                return Err(
                    classified.unwrap_or_else(|| ProxyErr::Upstream(format!("send_request: {e}")))
                );
            }
            Err(idle_err) => {
                tracing::warn!(error = %idle_err, "h1→h1 idle/head deadline fired");
                pump.abort();
                conn_handle.abort();
                return Err(ProxyErr::Timeout);
            }
        };

        // The response head only relays once the inbound body has validated.
        match verdict_rx.await {
            Ok(Ok(())) => {
                // Do NOT await `conn_handle` — response-body streaming still
                // needs the driver task running. Detach it.
                drop(conn_handle);
                Ok(resp)
            }
            Ok(Err(e)) => {
                // Malformed/truncated or over-cap: abort the upstream conn (do
                // NOT pool it) and NEVER relay its response.
                pump.abort();
                conn_handle.abort();
                Err(e)
            }
            Err(_) => {
                // No verdict — never leak the backend response.
                conn_handle.abort();
                Err(ProxyErr::BadRequest(
                    "inbound H1 request pump terminated without a verdict".to_owned(),
                ))
            }
        }
    }

    /// STREAMING H1→H2 request leg. MIRROR of
    /// [`crate::h2_proxy::H2Proxy::proxy_h2_to_h2_request`]: bounded lookahead →
    /// Branch A (fits the window → buffered send) / Branch B (streaming pump →
    /// the SHARED [`drive_h2_upstream_send`]). H1 deltas: `frame()==None` IS the
    /// confirmed clean end (never `is_end_stream()`), and [`H1PumpAbort`] lets
    /// the pump INJECT a body error instead of a spurious clean END_STREAM.
    ///
    /// Returns the h2_proxy [`H2ProxyErr`] — the type the shared driver returns.
    async fn proxy_h1_to_h2_request(
        &self,
        h2_pool: &Http2Pool,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Result<Response<IncomingBody>, H2ProxyErr> {
        // DELTA: no `is_end_stream()` for H1 — `None` is the clean-end signal.
        use hyper::body::Frame;
        use lb_io::http2_pool::{H2ReqBody, Http2PoolError};

        let req = req.into_inner();
        let (parts, mut body) = req.into_parts();

        // DELTA vs `proxy_request`: do NOT force HTTP/1.1 or strip CL/TE —
        // those were H1-framing fixes; H2 framing is hyper's encoder's job.
        let upstream_parts = match build_h1_to_h2_upstream_parts(&parts) {
            Ok(p) => p,
            Err(e) => return Err(H2ProxyErr::Upstream(e)),
        };

        let mut lookahead: Vec<Bytes> = Vec::new();
        let mut buffered: usize = 0;
        let mut trailers_map: Option<hyper::HeaderMap> = None;
        let mut reached_eof = false;

        loop {
            #[cfg(any(test, feature = "test-gauges"))]
            record_retained_h1(buffered);

            if buffered > H1_REQ_CHANNEL_DEPTH * H1_REQ_CHUNK_MAX {
                break;
            }

            match body.frame().await {
                None => {
                    // F-MD-4 (H1): `None` IS the confirmed clean end (never
                    // `is_end_stream()`); in-window clean end → Branch A.
                    reached_eof = true;
                    break;
                }
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        buffered = buffered.saturating_add(data.len());
                        if buffered > MAX_REQUEST_BODY_BYTES {
                            return Err(H2ProxyErr::BodyTooLarge);
                        }
                    }
                    if frame.is_data() {
                        lookahead.push(frame.into_data().unwrap_or_default());
                    } else if frame.is_trailers() {
                        trailers_map = frame.into_trailers().ok();
                        reached_eof = true;
                        break;
                    }
                }
                Some(Err(e)) => {
                    // F-MD-4 (H1): a within-window truncation surfaces BEFORE
                    // any pool contact — zero-dial, validate-before-dial intact.
                    return Err(H2ProxyErr::BadRequest(format!(
                        "inbound H1 request body incomplete: {e}"
                    )));
                }
            }
        }

        if reached_eof {
            // ── Branch A: the whole request fit the window; zero pool contact.
            if let Some(tm) = trailers_map.as_ref() {
                validate_h1_request_trailers(tm).map_err(h1_to_h2_proxy_err)?;
            }
            let trailers_vec: Vec<(String, String)> = trailers_map
                .as_ref()
                .map(|tm| {
                    tm.iter()
                        .filter_map(|(n, v)| {
                            v.to_str()
                                .ok()
                                .map(|s| (n.as_str().to_owned(), s.to_owned()))
                        })
                        .collect()
                })
                .unwrap_or_default();

            let body_bytes = concat_h1_chunks(&lookahead, buffered);
            let upstream_body: H2ReqBody = build_body_with_trailers(body_bytes, &trailers_vec)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                .boxed();
            let upstream_req = Request::from_parts(upstream_parts, upstream_body);

            return match h2_pool.send_request(backend_addr, upstream_req).await {
                Ok(resp) => Ok(resp),
                Err(Http2PoolError::Timeout) => Err(H2ProxyErr::Timeout),
                // G5: `Send` may never have left this process — see
                // `h2_proxy::ProxyErr::UpstreamUnattributable`. Formatting the WHOLE
                // error keeps the 502 body byte-identical to the pre-G5 text.
                Err(e @ Http2PoolError::Send(_)) => Err(H2ProxyErr::UpstreamUnattributable(
                    format!("h2 upstream: {e}"),
                )),
                Err(e) => Err(H2ProxyErr::Upstream(format!("h2 upstream: {e}"))),
            };
        }

        // ── Branch B: stream with the bounded in-flight window.
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, H1PumpAbort>>(H1_REQ_CHANNEL_DEPTH);

        // F-MD-3: genuine retained-memory gauge (live in-flight occupancy).
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight_body = std::sync::Arc::clone(&in_flight_bytes);

        let stream_body: H2ReqBody =
            http_body_util::StreamBody::new(futures_util::stream::poll_fn(move |cx| {
                let polled = rx.poll_recv(cx);
                if let std::task::Poll::Ready(Some(Ok(ref frame))) = polled {
                    if let Some(d) = frame.data_ref() {
                        in_flight_body.fetch_sub(d.len(), std::sync::atomic::Ordering::Relaxed);
                    }
                }
                polled
            }))
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            .boxed();
        let upstream_req = Request::from_parts(upstream_parts, stream_body);

        // The response head is gated on the pump's terminal verdict.
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), H2ProxyErr>>();
        let drained: Vec<Bytes> = std::mem::take(&mut lookahead);

        let last_progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let upload_complete = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let epoch = tokio::time::Instant::now();
        let last_progress_pump = std::sync::Arc::clone(&last_progress);
        let upload_complete_pump = std::sync::Arc::clone(&upload_complete);
        let epoch_pump = epoch;

        let pump = tokio::spawn(async move {
            let bump = || {
                let dt = tokio::time::Instant::now().saturating_duration_since(epoch_pump);
                let ms = u64::try_from(dt.as_millis()).unwrap_or(u64::MAX);
                last_progress_pump.store(ms, std::sync::atomic::Ordering::Relaxed);
            };
            let set_complete = || {
                upload_complete_pump.store(true, std::sync::atomic::Ordering::Release);
            };

            let mut forwarded_total: usize = buffered;
            let mut lookahead_remaining: usize = buffered;

            enum SendOutcome {
                ReceiverGone,
            }

            macro_rules! send_chunked {
                ($bytes:expr, $is_lookahead:expr) => {{
                    let mut data: Bytes = $bytes;
                    let mut outcome: Result<(), SendOutcome> = Ok(());
                    while !data.is_empty() {
                        let take = data.len().min(H1_REQ_CHUNK_MAX);
                        let chunk = data.split_to(take);
                        let clen = chunk.len();
                        in_flight_bytes.fetch_add(clen, std::sync::atomic::Ordering::Relaxed);
                        if $is_lookahead {
                            lookahead_remaining = lookahead_remaining.saturating_sub(clen);
                        }
                        #[cfg(any(test, feature = "test-gauges"))]
                        record_retained_h1(
                            lookahead_remaining
                                + in_flight_bytes.load(std::sync::atomic::Ordering::Relaxed),
                        );
                        if tx.send(Ok(Frame::data(chunk))).await.is_err() {
                            in_flight_bytes.fetch_sub(clen, std::sync::atomic::Ordering::Relaxed);
                            outcome = Err(SendOutcome::ReceiverGone);
                            break;
                        }
                        bump();
                    }
                    outcome
                }};
            }

            macro_rules! drain_and_validate {
                () => {{
                    loop {
                        match body.frame().await {
                            // F-MD-4 (H1): clean end = `None`, truncation = Err.
                            None => break Ok(()),
                            Some(Ok(frame)) => {
                                if frame.is_trailers() {
                                    if let Some(t) = frame.trailers_ref() {
                                        break validate_h1_request_trailers(t)
                                            .map_err(h1_to_h2_proxy_err);
                                    }
                                    break Ok(());
                                }
                                if let Some(d) = frame.data_ref() {
                                    forwarded_total = forwarded_total.saturating_add(d.len());
                                    if forwarded_total > MAX_REQUEST_BODY_BYTES {
                                        break Err(H2ProxyErr::BodyTooLarge);
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                break Err(H2ProxyErr::BadRequest(format!(
                                    "inbound H1 request body incomplete: {e}"
                                )));
                            }
                        }
                    }
                }};
            }

            // F-MD-4 — inject `Err(H1PumpAbort)` and HOLD the sender open until
            // hyper OBSERVES it. FIFO forces hyper to poll the error BEFORE any
            // channel-close `None`, so it RESETS the stream rather than emitting
            // a spurious clean END_STREAM. Bounded so a wedged driver can't hang.
            macro_rules! inject_abort {
                () => {{
                    let _ = tx.send(Err(H1PumpAbort)).await;
                    let _ = tokio::time::timeout(H2_ABORT_OBSERVE_TIMEOUT, tx.closed()).await;
                }};
            }

            for chunk in drained {
                if let Err(SendOutcome::ReceiverGone) = send_chunked!(chunk, true) {
                    let _ = verdict_tx.send(drain_and_validate!());
                    return;
                }
            }
            loop {
                match body.frame().await {
                    None => {
                        // F-MD-4 (H1 — do NOT copy the H2 `is_end_stream`
                        // logic). `None` = clean end → drop tx → the upstream
                        // sees a COMPLETE request. NO inject_abort here.
                        set_complete();
                        let _ = verdict_tx.send(Ok(()));
                        return;
                    }
                    Some(Ok(frame)) => {
                        if frame.is_trailers() {
                            let verdict = frame
                                .trailers_ref()
                                .map_or(Ok(()), validate_h1_request_trailers);
                            match verdict {
                                Ok(()) => {
                                    let _ = tx.send(Ok(frame)).await;
                                    bump();
                                    set_complete();
                                    let _ = verdict_tx.send(Ok(()));
                                    return;
                                }
                                Err(e) => {
                                    inject_abort!();
                                    let _ = verdict_tx.send(Err(h1_to_h2_proxy_err(e)));
                                    return;
                                }
                            }
                        }
                        if let Ok(data) = frame.into_data() {
                            forwarded_total = forwarded_total.saturating_add(data.len());
                            if forwarded_total > MAX_REQUEST_BODY_BYTES {
                                inject_abort!();
                                let _ = verdict_tx.send(Err(H2ProxyErr::BodyTooLarge));
                                return;
                            }
                            if let Err(SendOutcome::ReceiverGone) = send_chunked!(data, false) {
                                let _ = verdict_tx.send(drain_and_validate!());
                                return;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        // F-MD-4 (H1): premature close → inject the body error
                        // FIRST so hyper aborts without a clean terminator.
                        inject_abort!();
                        let _ = verdict_tx.send(Err(H2ProxyErr::BadRequest(format!(
                            "inbound H1 request body incomplete: {e}"
                        ))));
                        return;
                    }
                }
            }
        });

        // F-MD-4: the SHARED driver owns the detached send task (biased
        // verdict-vs-head race, `reset_peer` on abort, the F-CAP-1 caller arm).
        drive_h2_upstream_send(
            h2_pool,
            backend_addr,
            upstream_req,
            verdict_rx,
            pump,
            last_progress,
            upload_complete,
            epoch,
            self.timeouts.body,
            self.timeouts.head,
            self.timeouts.body,
        )
        .await
    }

    /// Forward an H1 inbound request to an H2 backend; dispatch shim over the
    /// streaming [`Self::proxy_h1_to_h2_request`]. BOTH legs stream.
    ///
    /// Returns the health verdict alongside the response: the error→status mapping stays exactly
    /// where it was, and the caller records the outcome (G5).
    async fn proxy_h1_to_h2(
        &self,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> (Response<ClientRespBody>, AttemptOutcome) {
        let Some(h2_pool) = self.h2_upstream.as_ref() else {
            // A missing pool is OUR mis-wiring, not the backend's fault — record nothing.
            return (
                error_response(
                    StatusCode::BAD_GATEWAY,
                    "H2 backend selected but no Http2Pool wired",
                ),
                UpstreamErrorClass::Misconfigured.outcome(),
            );
        };
        match self
            .proxy_h1_to_h2_request(h2_pool, backend_addr, req)
            .await
        {
            Ok(resp) => (
                upstream_response_to_h1(resp, self.alt_svc),
                AttemptOutcome::Success,
            ),
            Err(e) => {
                let outcome = e.error_class().outcome();
                let resp = match e {
                    // Same arm for both: `UpstreamUnattributable` differs ONLY in health
                    // attribution, never on the wire.
                    H2ProxyErr::Upstream(s) | H2ProxyErr::UpstreamUnattributable(s) => {
                        error_response(StatusCode::BAD_GATEWAY, &s)
                    }
                    H2ProxyErr::Timeout => {
                        error_response(StatusCode::GATEWAY_TIMEOUT, "upstream H2 timeout")
                    }
                    H2ProxyErr::BodyTooLarge => error_response(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "request body exceeds maximum",
                    ),
                    H2ProxyErr::BadRequest(s) => error_response(StatusCode::BAD_REQUEST, &s),
                };
                (resp, outcome)
            }
        }
    }

    /// Forward an H1 inbound request to an H3 backend — FULLY STREAMING on both
    /// legs, via [`lb_quic::stream_request_to_h3_upstream`].
    ///
    /// `frame()==None` → `End`; a truncation / forbidden trailer / over-cap →
    /// `Reset`, so the connector RESETs the QUIC stream WITHOUT a FIN and a
    /// truncated inbound is NEVER presented as complete.
    ///
    /// CF-RESP-1: a streamed H1 response cannot pre-declare `Trailer:`, so a
    /// late `Trailers` event rides the terminal frame and hyper-1 may drop it.
    ///
    /// F-CAP-1: a PRE-DATA over-cap synthesizes `Head{413}`; a MID-BODY one
    /// becomes `Reset` (never a 413 — response-splitting guard).
    async fn proxy_h1_to_h3(
        &self,
        backend: &UpstreamBackend,
        req: StrippedRequest<IncomingBody>,
    ) -> (Response<ClientRespBody>, AttemptOutcome) {
        use hyper::body::Frame;
        use lb_quic::h3_bridge::{H3_BODY_CHUNK_MAX, ReqBodyEvent};

        let Some(h3_pool) = self.h3_upstream.as_ref() else {
            // Mis-wiring, not a backend fault.
            return (
                error_response(
                    StatusCode::BAD_GATEWAY,
                    "H3 backend selected but no QuicUpstreamPool wired",
                ),
                UpstreamErrorClass::Misconfigured.outcome(),
            );
        };
        let sni = backend.sni.as_deref().unwrap_or("").to_owned();
        let addr = backend.addr;

        let inner = req.into_inner();
        let (parts, mut body) = inner.into_parts();
        let headers = match build_h1_to_h3_fieldlist(&parts, &sni, /* https = */ true) {
            Ok(h) => h,
            // The INBOUND request could not be expressed as an H3 field list — nothing was dialed.
            Err(s) => {
                return (
                    error_response(StatusCode::BAD_GATEWAY, &s),
                    UpstreamErrorClass::ClientRequest.outcome(),
                );
            }
        };

        // Backpressure: a slow QUIC upstream → the connector stops draining →
        // this channel fills → the pump stops polling.
        let (body_tx, body_rx) =
            tokio::sync::mpsc::channel::<ReqBodyEvent>(lb_quic::conn_actor::H3_BODY_CHANNEL_DEPTH);
        let (resp_tx, mut resp_rx) = tokio::sync::mpsc::channel::<lb_quic::H3RespEvent>(
            lb_quic::h3_bridge::H3_RESP_CHANNEL_DEPTH,
        );

        // F-MD-3 gauge: in-flight bytes, bounded independent of body size.
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let pump_in_flight = std::sync::Arc::clone(&in_flight_bytes);
        let pump = tokio::spawn(async move {
            // The request-body cap is OUR job (the connector caps the RESPONSE).
            // Timing-critical: over-cap BEFORE any chunk → `Reset` first →
            // connector inline-413; after ≥1 chunk → RESET-without-FIN, no 413.
            let mut forwarded_total: usize = 0;

            // Err(()) = connector gone.
            macro_rules! send_chunked {
                ($bytes:expr) => {{
                    let mut data: Bytes = $bytes;
                    let mut ok = true;
                    while !data.is_empty() {
                        let take = data.len().min(H3_BODY_CHUNK_MAX);
                        let chunk = data.split_to(take);
                        let clen = chunk.len();
                        pump_in_flight.fetch_add(clen, std::sync::atomic::Ordering::Relaxed);
                        #[cfg(any(test, feature = "test-gauges"))]
                        record_retained_h1(
                            pump_in_flight.load(std::sync::atomic::Ordering::Relaxed),
                        );
                        let send_res = body_tx.send(ReqBodyEvent::Chunk(chunk)).await;
                        pump_in_flight.fetch_sub(clen, std::sync::atomic::Ordering::Relaxed);
                        if send_res.is_err() {
                            ok = false;
                            break;
                        }
                    }
                    if ok { Ok(()) } else { Err(()) }
                }};
            }

            loop {
                match body.frame().await {
                    None => {
                        // F-MD-4 (H1): `None` is the confirmed clean end → FIN.
                        let _ = body_tx
                            .send(ReqBodyEvent::End {
                                trailers: Vec::new(),
                            })
                            .await;
                        return;
                    }
                    Some(Ok(frame)) => {
                        if frame.is_trailers() {
                            // Validate BEFORE forwarding; forbidden → `Reset`,
                            // never a clean `End` (desync primitive).
                            let verdict = frame
                                .trailers_ref()
                                .map_or(Ok(()), validate_h1_request_trailers);
                            match verdict {
                                Ok(()) => {
                                    let tvec: Vec<(String, String)> = frame
                                        .trailers_ref()
                                        .map(|tm| {
                                            tm.iter()
                                                .filter_map(|(n, v)| {
                                                    v.to_str().ok().map(|s| {
                                                        (n.as_str().to_owned(), s.to_owned())
                                                    })
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    let _ =
                                        body_tx.send(ReqBodyEvent::End { trailers: tvec }).await;
                                    return;
                                }
                                Err(_) => {
                                    let _ = body_tx.send(ReqBodyEvent::Reset).await;
                                    return;
                                }
                            }
                        }
                        if let Ok(data) = frame.into_data() {
                            // No over-cap byte is forwarded either way.
                            if forwarded_total.saturating_add(data.len()) > MAX_REQUEST_BODY_BYTES {
                                let _ = body_tx.send(ReqBodyEvent::Reset).await;
                                return;
                            }
                            forwarded_total = forwarded_total.saturating_add(data.len());
                            if send_chunked!(data).is_err() {
                                return;
                            }
                        }
                    }
                    Some(Err(_e)) => {
                        // F-MD-4 (H1): premature close surfaces as `Some(Err)`;
                        // `Reset` → no FIN → never a complete truncated request.
                        let _ = body_tx.send(ReqBodyEvent::Reset).await;
                        return;
                    }
                }
            }
        });

        let sink = lb_quic::H3RespOut::Decoded {
            tx: resp_tx,
            total: 0,
            cap: lb_quic::h3_bridge::MAX_RESPONSE_BODY_BYTES,
        };
        let pool = std::sync::Arc::clone(h3_pool);
        let connector_handle = tokio::spawn(async move {
            let _ = lb_quic::stream_request_to_h3_upstream(
                headers, /* forward_req_trailers = */ true, addr, &sni, &pool, body_rx, sink,
            )
            .await;
        });

        // The FIRST event determines the head; `Reset`/closed before it → 502.
        let first = resp_rx.recv().await;
        match first {
            Some(lb_quic::H3RespEvent::Head { status, headers }) => {
                let st = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
                let builder = h3_decoded_resp_head_builder(st, &headers, self.alt_svc);

                // `Reset` → inject a body error so hyper emits no clean
                // terminator (response-splitting guard); `End` → drop the sender.
                let (btx, brx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, H1PumpAbort>>(
                    lb_quic::h3_bridge::H3_RESP_CHANNEL_DEPTH,
                );
                tokio::spawn(async move {
                    while let Some(ev) = resp_rx.recv().await {
                        match ev {
                            lb_quic::H3RespEvent::Body(b) => {
                                if btx.send(Ok(Frame::data(b))).await.is_err() {
                                    break;
                                }
                            }
                            lb_quic::H3RespEvent::Trailers(t) => {
                                let mut tm = hyper::HeaderMap::new();
                                for (n, v) in &t {
                                    if let (Ok(name), Ok(val)) = (
                                        HeaderName::from_bytes(n.as_bytes()),
                                        HeaderValue::from_str(v),
                                    ) {
                                        tm.append(name, val);
                                    }
                                }
                                let _ = btx.send(Ok(Frame::trailers(tm))).await;
                            }
                            lb_quic::H3RespEvent::End => break,
                            lb_quic::H3RespEvent::Reset => {
                                // Truncate WITHOUT a clean terminator.
                                let _ = btx.send(Err(H1PumpAbort)).await;
                                break;
                            }
                            // A second Head is malformed — abort.
                            lb_quic::H3RespEvent::Head { .. } => {
                                let _ = btx.send(Err(H1PumpAbort)).await;
                                break;
                            }
                        }
                    }
                    drop(connector_handle);
                });

                let mut brx = brx;
                let stream_body =
                    http_body_util::StreamBody::new(futures_util::stream::poll_fn(move |cx| {
                        brx.poll_recv(cx)
                    }))
                    // F-MD-4 (response leg): the relay SENDS `H1PumpAbort`
                    // (never a clean drop), so hyper aborts the chunked response
                    // WITHOUT a `0\r\n\r\n` — never a smuggled-complete one.
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
                let _ = &pump; // pump is detached; its task owns the request leg
                // A head arrived: the backend answered. Its STATUS is not the detector's business
                // (see `UpstreamErrorClass`).
                (
                    build_h1_streaming_response(builder, stream_body.boxed()),
                    AttemptOutcome::Success,
                )
            }
            None | Some(lb_quic::H3RespEvent::Reset) => {
                pump.abort();
                connector_handle.abort();
                (
                    error_response(
                        StatusCode::BAD_GATEWAY,
                        "H3 upstream produced no response head",
                    ),
                    UpstreamErrorClass::Transport.outcome(),
                )
            }
            // Body/Trailers/End before a Head is a connector contract violation.
            Some(_) => {
                pump.abort();
                connector_handle.abort();
                (
                    error_response(StatusCode::BAD_GATEWAY, "H3 upstream response head missing"),
                    UpstreamErrorClass::Transport.outcome(),
                )
            }
        }
    }

    /// Handle an RFC 6455 handshake request.
    ///
    /// **ROUND8-L7-01 (Pingora GHSA-xq2h-p299-vjwv / Envoy
    /// GHSA-rj35-4m94-77jh, both CVSS 9.3):** `101 Switching Protocols` is
    /// emitted ONLY after the upstream handshake succeeds. The pre-fix code
    /// returned `101` synchronously and dialed in a detached task, so anything
    /// pipelined after the upgrade request entered an unread upgraded
    /// byte-stream — the smuggling primitive both references paid for. On
    /// upstream failure the wire is still in H1 mode, so `502`/`504` is
    /// returned and the client connection stays keep-alive-eligible.
    ///
    /// ROUND8-OPS-06: the child `traceparent` is injected onto the upstream
    /// handshake and the splice task is `.instrument()`-ed with the request span.
    async fn handle_ws_upgrade(
        &self,
        mut req: Request<IncomingBody>,
        req_trace: crate::trace_ctx::RequestTrace,
    ) -> Response<ClientRespBody> {
        let Some(ws_proxy) = self.ws.clone() else {
            return error_response(StatusCode::BAD_GATEWAY, "websocket disabled");
        };
        let Some(handshake_headers) = build_handshake_response_headers(&req) else {
            return error_response(StatusCode::BAD_REQUEST, "invalid websocket handshake");
        };
        let Some(backend) = self.picker.pick_info() else {
            return error_response(StatusCode::BAD_GATEWAY, "no backend available");
        };
        // WS upgrade supports H1 backends only; H2/H3 here is a misconfig.
        if backend.proto != UpstreamProto::H1 {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "WebSocket upgrade requires H1 backend",
            );
        }
        let backend_addr = backend.addr;

        let path_and_query = req
            .uri()
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string);
        let forwarded_protocols = req
            .headers()
            .get(&WS_PROTOCOL)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        // ROUND8-L7-01 — drive the upstream handshake BEFORE any client-visible
        // response, bounded by `HttpTimeouts::header`. Timeout → 504, else 502.
        let child_traceparent = req_trace.child_traceparent();
        let tracestate = req_trace.tracestate.clone();
        let upstream_dial = dial_upstream_ws(
            self.pool.clone(),
            backend_addr,
            path_and_query,
            forwarded_protocols,
            child_traceparent,
            tracestate,
            ws_proxy.clone(),
        );
        let backend_ws = match tokio::time::timeout(self.timeouts.header, upstream_dial).await {
            Ok(Ok(ws)) => {
                self.record_health(backend_addr, AttemptOutcome::Success);
                ws
            }
            Ok(Err(ProxyErr::Upstream(msg))) => {
                self.record_health(backend_addr, UpstreamErrorClass::Transport.outcome());
                tracing::debug!(backend = %backend_addr, error = %msg, "ws: upstream handshake refused — returning 502 (no 101 emitted)");
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "websocket upstream handshake failed",
                );
            }
            Ok(Err(ProxyErr::Timeout)) => {
                self.record_health(backend_addr, UpstreamErrorClass::Timeout.outcome());
                tracing::debug!(backend = %backend_addr, "ws: upstream dial timeout — returning 504 (no 101 emitted)");
                return error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "websocket upstream dial timeout",
                );
            }
            // The WS path never runs the body pump; map defensively to 502.
            Ok(Err(ProxyErr::BadRequest(_) | ProxyErr::BodyTooLarge)) => {
                self.record_health(backend_addr, UpstreamErrorClass::ClientRequest.outcome());
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "websocket upstream handshake failed",
                );
            }
            Err(_elapsed) => {
                // The BUDGET elapsed, not an inner deadline — still the backend failing to answer.
                self.record_health(backend_addr, UpstreamErrorClass::Timeout.outcome());
                tracing::debug!(backend = %backend_addr, "ws: upstream handshake budget elapsed — returning 504 (no 101 emitted)");
                return error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "websocket upstream handshake timeout",
                );
            }
        };

        // Upstream handshake succeeded: ONLY NOW arm the upgrade and build `101`.
        let upgrade_fut = hyper::upgrade::on(&mut req);
        tokio::spawn(tracing::Instrument::instrument(
            run_h1_ws_splice_task(upgrade_fut, backend_ws, ws_proxy),
            req_trace.span.clone(),
        ));

        // v1 mirrors the first offered sub-protocol verbatim.
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

    fn finalize_response(&self, resp: Response<IncomingBody>) -> Response<ClientRespBody> {
        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        if let Some(alt) = self.alt_svc {
            // Insert (not append) so an older origin cannot shadow ours.
            if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
                parts.headers.insert(hyper::header::ALT_SVC, value);
            }
        }
        Response::from_parts(
            parts,
            body.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                .boxed(),
        )
    }
}

/// Upstream / timeout / inbound-fault discriminant, mapped to an HTTP status.
enum ProxyErr {
    Upstream(String),
    Timeout,
    /// Malformed inbound body (F-MD-4 truncation, Q-H3 forbidden trailer) →
    /// `400`; the upstream response is NEVER relayed.
    BadRequest(String),
    /// Over [`MAX_REQUEST_BODY_BYTES`] mid-stream → `413`. DISTINCT from an
    /// upstream receiver-drop (F-MD-2), which is NOT a 413.
    BodyTooLarge,
}

impl ProxyErr {
    /// Adapt to the shared health taxonomy. This is a type mapping only — whether a class counts
    /// as a failure is decided once, in [`UpstreamErrorClass::outcome`].
    const fn error_class(&self) -> UpstreamErrorClass {
        match self {
            Self::Upstream(_) => UpstreamErrorClass::Transport,
            Self::Timeout => UpstreamErrorClass::Timeout,
            Self::BadRequest(_) | Self::BodyTooLarge => UpstreamErrorClass::ClientRequest,
        }
    }
}

/// ROUND8-L7-01 — dial and drive the RFC 6455 client-side handshake **before**
/// the client sees `101`. The caller maps the error to `502` / `504`.
async fn dial_upstream_ws(
    pool: TcpPool,
    backend_addr: SocketAddr,
    path_and_query: String,
    forwarded_protocols: Option<String>,
    child_traceparent: String,
    tracestate: Option<String>,
    ws_proxy: Arc<WsProxy>,
) -> Result<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, ProxyErr> {
    // A dial failure is `502` — but the client has NOT yet seen `101`.
    let pooled = pool
        .acquire_async(backend_addr)
        .await
        .map_err(|e| ProxyErr::Upstream(format!("backend dial failed: {e}")))?;
    let upstream_stream = pooled
        .take_stream()
        .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;

    let uri = format!("ws://{backend_addr}{path_and_query}")
        .parse()
        .map_err(|e| ProxyErr::Upstream(format!("upstream uri build failed: {e}")))?;
    let mut builder = tokio_tungstenite::tungstenite::client::ClientRequestBuilder::new(uri);
    if let Some(protocols) = forwarded_protocols.as_deref() {
        for p in protocols.split(',') {
            let p = p.trim();
            if !p.is_empty() {
                builder = builder.with_sub_protocol(p);
            }
        }
    }
    builder = builder.with_header(
        lb_observability::tracing_propagation::TRACEPARENT_HEADER,
        child_traceparent,
    );
    if let Some(ts) = tracestate {
        builder = builder.with_header(lb_observability::tracing_propagation::TRACESTATE_HEADER, ts);
    }

    let ws_cfg = ws_proxy.config();
    let (backend_ws, _resp) = tokio_tungstenite::client_async_with_config(
        builder,
        upstream_stream,
        Some(ws_cfg.tungstenite_config()),
    )
    .await
    .map_err(|e| ProxyErr::Upstream(format!("upstream handshake failed: {e}")))?;
    Ok(backend_ws)
}

/// ROUND8-L7-01 — splice-only task; by now `101` is already on the wire.
async fn run_h1_ws_splice_task(
    upgrade_fut: hyper::upgrade::OnUpgrade,
    backend_ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    ws_proxy: Arc<WsProxy>,
) {
    let upgraded = match upgrade_fut.await {
        Ok(u) => u,
        Err(e) => {
            // `backend_ws`'s Drop closes the pooled socket, so we do not leak it.
            tracing::debug!(error = %e, "ws: hyper upgrade failed after upstream established");
            return;
        }
    };
    let ws_cfg = ws_proxy.config();
    let client_ws = ws_proxy::server_ws(TokioIo::new(upgraded), &ws_cfg).await;
    if let Err(e) = ws_proxy.proxy_frames(client_ws, backend_ws).await {
        tracing::debug!(error = %e, "ws: frame proxy ended with error");
    }
}

fn error_response(status: StatusCode, msg: &str) -> Response<ClientRespBody> {
    let body = Full::new(Bytes::from(msg.to_owned()))
        .map_err(|never| match never {})
        .boxed();
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    resp
}

/// Map a [`SecurityReject`] to a response: `Smuggle`/`SlowHandshake` → `400`;
/// `RateLimited`/`OverCap` → `503` + `Retry-After: 1` (leaks no internals).
pub(crate) fn reject_to_response(rej: &SecurityReject) -> Response<ClientRespBody> {
    match rej {
        SecurityReject::Smuggle(_) => error_response(StatusCode::BAD_REQUEST, "request smuggling"),
        SecurityReject::SlowHandshake => error_response(StatusCode::BAD_REQUEST, "slow handshake"),
        SecurityReject::RateLimited | SecurityReject::OverCap(_) => {
            let mut resp = error_response(StatusCode::SERVICE_UNAVAILABLE, "over capacity");
            resp.headers_mut()
                .insert(hyper::header::RETRY_AFTER, HeaderValue::from_static("1"));
            resp
        }
    }
}

/// Strip hop-by-hop headers (RFC 9110 §7.6.1) plus any name listed inside the
/// `Connection` value. `pub` so integration tests can pin the exact behaviour.
pub fn strip_hop_by_hop(headers: &mut hyper::HeaderMap) {
    // Collect Connection-token names BEFORE removing `Connection` itself.
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

/// Append the peer's IP to `X-Forwarded-For`, iterating EVERY existing value:
/// `HeaderMap::get` returns only the first and `insert` clobbers the rest — the
/// silent-drop class of **Envoy GHSA-ghc4-35x6-crw5**. Mirrored on [`append_via`].
pub(crate) fn append_xff(headers: &mut hyper::HeaderMap, peer: SocketAddr) {
    let peer_ip = peer.ip().to_string();
    let mut joined = String::new();
    for v in headers.get_all(&XFF_NAME) {
        if let Ok(s) = v.to_str() {
            if !joined.is_empty() {
                joined.push_str(", ");
            }
            joined.push_str(s);
        }
    }
    if !joined.is_empty() {
        joined.push_str(", ");
    }
    joined.push_str(&peer_ip);
    if let Ok(v) = HeaderValue::from_str(&joined) {
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

/// Append `HTTP/1.1 expressgateway` to `Via`, merging every existing value —
/// `Via` is list-valued (RFC 9110 §7.6.3); see [`append_xff`].
pub(crate) fn append_via(headers: &mut hyper::HeaderMap) {
    const VIA_TOKEN: &str = "HTTP/1.1 expressgateway";
    let mut joined = String::new();
    for v in headers.get_all(hyper::header::VIA) {
        if let Ok(s) = v.to_str() {
            if !joined.is_empty() {
                joined.push_str(", ");
            }
            joined.push_str(s);
        }
    }
    if !joined.is_empty() {
        joined.push_str(", ");
    }
    joined.push_str(VIA_TOKEN);
    if let Ok(v) = HeaderValue::from_str(&joined) {
        headers.insert(hyper::header::VIA, v);
    }
}

static XFF_NAME: HeaderName = HeaderName::from_static("x-forwarded-for");
static XFP_NAME: HeaderName = HeaderName::from_static("x-forwarded-proto");
static XFH_NAME: HeaderName = HeaderName::from_static("x-forwarded-host");
static WS_PROTOCOL: HeaderName = HeaderName::from_static("sec-websocket-protocol");

/// HEAD-ONLY preamble for the streaming H1→H2 request leg: the
/// `create_bridge(Http1, Http2)` codec mints the H2 pseudo-header set over a
/// body-LESS request. DELTA: no forced HTTP/1.1 and no CL/TE strip — H2 framing
/// is hyper's encoder's job. The body is attached SEPARATELY by the caller.
fn build_h1_to_h2_upstream_parts(
    parts: &http::request::Parts,
) -> Result<http::request::Parts, String> {
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
        body: Bytes::new(),
        scheme: Some("http".to_owned()),
        trailers: Vec::new(),
    };
    let translated = bridge
        .bridge_request(&bridge_in)
        .map_err(|e| format!("h1->h2 bridge: {e}"))?;

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
    // hyper's H2 client builds the pseudo-headers itself from URI and method.
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }
    // Empty body purely to validate method/uri/headers; return its `Parts`.
    let (head, ()) = builder
        .body(())
        .map_err(|e| format!("build h2 req: {e}"))?
        .into_parts();
    Ok(head)
}

/// Map the local [`ProxyErr`] into the [`H2ProxyErr`] the H1→H2 leg speaks.
fn h1_to_h2_proxy_err(e: ProxyErr) -> H2ProxyErr {
    match e {
        ProxyErr::Upstream(s) => H2ProxyErr::Upstream(s),
        ProxyErr::Timeout => H2ProxyErr::Timeout,
        ProxyErr::BadRequest(s) => H2ProxyErr::BadRequest(s),
        ProxyErr::BodyTooLarge => H2ProxyErr::BodyTooLarge,
    }
}

/// Concatenate lookahead DATA chunks; `total` presizes the one allocation.
fn concat_h1_chunks(chunks: &[Bytes], total: usize) -> Bytes {
    if let [single] = chunks {
        return single.clone();
    }
    let mut out = bytes::BytesMut::with_capacity(total);
    for c in chunks {
        out.extend_from_slice(c);
    }
    out.freeze()
}

/// Build the STREAMING H1 response head from a decoded H3 `Head` event; shares
/// the pseudo/`RESPONSE_HOP_BY_HOP` strip with [`upstream_response_to_h1`].
/// CF-RESP-1: it CANNOT pre-declare `Trailer:` — the names are unknown here.
fn h3_decoded_resp_head_builder(
    status: StatusCode,
    headers: &[(String, String)],
    alt_svc: Option<AltSvcConfig>,
) -> hyper::http::response::Builder {
    let mut builder = Response::builder().status(status);
    for (n, v) in headers {
        if n.starts_with(':') {
            continue;
        }
        let lower = n.to_lowercase();
        if crate::h2_to_h1::RESPONSE_HOP_BY_HOP.contains(&lower.as_str()) {
            continue;
        }
        builder = builder.header(lower.as_str(), v.as_str());
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    builder
}

/// Finalize a streaming H1 response, centralizing the build-failure fallback.
fn build_h1_streaming_response(
    builder: hyper::http::response::Builder,
    body: ClientRespBody,
) -> Response<ClientRespBody> {
    builder.body(body).unwrap_or_else(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "build h1 streaming response failed",
        )
    })
}

/// PROTO-2-12 — data bytes followed by a trailer frame; an empty `trailers`
/// list emits no trailer frame.
fn build_body_with_trailers(
    body_bytes: Bytes,
    trailers: &[(String, String)],
) -> BoxBody<Bytes, hyper::Error> {
    use http_body_util::StreamBody;
    use hyper::HeaderMap;
    use hyper::body::Frame;

    if trailers.is_empty() {
        return Full::new(body_bytes)
            .map_err(|never| match never {})
            .boxed();
    }
    let mut tmap = HeaderMap::new();
    for (n, v) in trailers {
        if let (Ok(name), Ok(value)) = (
            hyper::header::HeaderName::try_from(n.as_str()),
            HeaderValue::from_str(v),
        ) {
            tmap.append(name, value);
        }
    }
    let frames: Vec<Result<Frame<Bytes>, hyper::Error>> = if body_bytes.is_empty() {
        vec![Ok(Frame::trailers(tmap))]
    } else {
        vec![Ok(Frame::data(body_bytes)), Ok(Frame::trailers(tmap))]
    };
    StreamBody::new(futures_util::stream::iter(frames)).boxed()
}

/// Convert an upstream H2 `Response<Incoming>` into the H1 response the
/// listener emits, STREAMING the body by construction.
///
/// CF-RESP-1 TRAILERS: a streamed relay cannot pre-declare the head `Trailer:`
/// names — they arrive in the body's TERMINAL frame — and re-adding a
/// `collect()` to capture them is the exact R8 violation this path removed. If
/// hyper-1's encoder will not flush an undeclared trailer frame, streamed H1←H2
/// responses simply do not forward trailers: matches nginx, documented, NOT a
/// silent regression.
fn upstream_response_to_h1(
    resp: Response<IncomingBody>,
    alt_svc: Option<AltSvcConfig>,
) -> Response<ClientRespBody> {
    let (parts, body) = resp.into_parts();
    let mut builder = Response::builder().status(parts.status);
    for (n, v) in &parts.headers {
        let name = n.as_str();
        if name.starts_with(':') {
            continue;
        }
        let lower = name.to_lowercase();
        if crate::h2_to_h1::RESPONSE_HOP_BY_HOP.contains(&lower.as_str()) {
            continue;
        }
        builder = builder.header(lower.as_str(), v);
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    // R8: stream the `Incoming` by construction; trailers ride its last frame.
    builder
        .body(
            body.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                .boxed(),
        )
        .unwrap_or_else(|_| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "build h1 response failed",
            )
        })
}

/// Assemble the final H1 wire response from a translated
/// [`crate::BridgeResponse`] — the authoritative trailer-aware H1 head shape.
///
/// With non-empty trailers this injects `Transfer-Encoding: chunked` + a
/// `Trailer:` declaration and drops any incoming `Content-Length` /
/// `Transfer-Encoding` / `Trailer`. hyper-1's H1 encoder requires BOTH to flush
/// a `Frame::trailers` (`proto/h1/encode.rs:163-213`); without them the trailer
/// fields silently disappear.
pub fn build_h1_response_with_trailers(
    translated: crate::BridgeResponse,
    alt_svc: Option<AltSvcConfig>,
) -> Response<ClientRespBody> {
    let status = StatusCode::from_u16(translated.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    let has_trailers = !translated.trailers.is_empty();
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        // PROTO-2-19: drop pre-existing framing/trailer declarations (both
        // re-injected below) so the proxy's authoritative list wins.
        if has_trailers
            && (n.eq_ignore_ascii_case("transfer-encoding")
                || n.eq_ignore_ascii_case("content-length")
                || n.eq_ignore_ascii_case("trailer"))
        {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }
    if has_trailers {
        // RFC 9112 §7.1: trailers require chunked TE; RFC 9110 §6.5 forbids CL.
        let trailer_names: Vec<&str> = translated
            .trailers
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        let trailer_header = trailer_names.join(", ");
        if let Ok(v) = HeaderValue::from_str(&trailer_header) {
            builder = builder.header(hyper::header::TRAILER, v);
        }
        builder = builder.header(
            hyper::header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    let body = build_body_with_trailers(translated.body, &translated.trailers)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        .boxed();
    builder.body(body).unwrap_or_else(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "build h1 response failed",
        )
    })
}

/// Build the H1→H3 request FIELD-LIST from the HEAD only; body and trailers
/// stream through the connector.
fn build_h1_to_h3_fieldlist(
    parts: &hyper::http::request::Parts,
    sni: &str,
    is_https: bool,
) -> Result<Vec<(String, String)>, String> {
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
        body: Bytes::new(),
        scheme: Some(scheme.to_owned()),
        trailers: Vec::new(),
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
    Ok(field_list)
}

/// Max instantaneous inbound-request memory the H1 ingress pump retains.
///
/// A GENUINE gauge, not a constant: the pump increments before each push and
/// DECREMENTS when hyper pulls, so a buffering variant would grow with request
/// size and trip the ceiling the memory proof asserts.
#[cfg(any(test, feature = "test-gauges"))]
pub static H1_REQ_MAX_RETAINED_BODY_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Lock-free CAS-max update for [`H1_REQ_MAX_RETAINED_BODY_BYTES`].
#[cfg(any(test, feature = "test-gauges"))]
pub fn record_retained_h1(n: usize) {
    use std::sync::atomic::Ordering;
    let mut cur = H1_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    while n > cur {
        match H1_REQ_MAX_RETAINED_BODY_BYTES.compare_exchange_weak(
            cur,
            n,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => cur = observed,
        }
    }
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

    /// ROUND8-L7-04 — two `X-Forwarded-For` LINES must survive comma-joined
    /// (the Envoy GHSA-ghc4-35x6-crw5 silent-drop class).
    #[test]
    fn x_forwarded_for_two_lines_preserved() {
        let mut h = HeaderMap::new();
        h.append(
            HeaderName::from_static("x-forwarded-for"),
            HeaderValue::from_static("1.1.1.1"),
        );
        h.append(
            HeaderName::from_static("x-forwarded-for"),
            HeaderValue::from_static("2.2.2.2"),
        );
        let peer: SocketAddr = "9.9.9.9:1".parse().unwrap();
        append_xff(&mut h, peer);
        let all: Vec<&str> = h
            .get_all(HeaderName::from_static("x-forwarded-for"))
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        assert_eq!(
            all.len(),
            1,
            "expected canonical single header line, got {all:?}",
        );
        let first = all.first().copied().unwrap_or("");
        let parts: Vec<&str> = first.split(',').map(str::trim).collect();
        assert_eq!(parts, vec!["1.1.1.1", "2.2.2.2", "9.9.9.9"]);
    }

    /// ROUND8-L7-04 — same shape for `Via`.
    #[test]
    fn via_two_lines_preserved() {
        let mut h = HeaderMap::new();
        h.append(hyper::header::VIA, HeaderValue::from_static("1.1 gw1"));
        h.append(hyper::header::VIA, HeaderValue::from_static("1.1 gw2"));
        append_via(&mut h);
        let all: Vec<&str> = h
            .get_all(hyper::header::VIA)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        assert_eq!(all.len(), 1, "expected canonical Via, got {all:?}");
        let first = all.first().copied().unwrap_or("");
        let parts: Vec<&str> = first.split(',').map(str::trim).collect();
        assert_eq!(parts, vec!["1.1 gw1", "1.1 gw2", "HTTP/1.1 expressgateway"]);
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
    fn hop_by_hop_response_strips_te_and_transfer_encoding_keeps_trailer() {
        let mut h = map_with(&[
            ("content-type", "text/plain"),
            ("transfer-encoding", "chunked"),
            ("te", "trailers"),
            // PROTO-2-08: `Trailer:` is end-to-end and must NOT be stripped.
            ("trailer", "X-Foo"),
        ]);
        strip_hop_by_hop(&mut h);
        assert!(h.get("transfer-encoding").is_none());
        assert!(h.get("te").is_none());
        assert_eq!(h.get("trailer").unwrap(), "X-Foo");
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

    // PROTO-2-11 H1 half: a busy-loop or a held-open conn times out.
    #[tokio::test(flavor = "current_thread")]
    async fn test_sigterm_h1_graceful_shutdown_resolves() {
        use std::time::Duration;
        use tokio_util::sync::CancellationToken;

        let pool = lb_io::pool::TcpPool::new(
            lb_io::pool::PoolConfig::default(),
            lb_io::sockopts::BackendSockOpts::default(),
            lb_io::Runtime::new(),
        );
        let addrs: Vec<SocketAddr> = vec!["127.0.0.1:1".parse().unwrap()];
        let picker = RoundRobinAddrs::new(addrs).unwrap();
        let proxy = Arc::new(H1Proxy::new(
            pool,
            Arc::new(picker),
            None,
            HttpTimeouts::default(),
            false,
        ));
        let (server_io, client) = tokio::io::duplex(8 * 1024);
        drop(client);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let peer: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let r = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.serve_connection_with_cancel(server_io, peer, cancel),
        )
        .await;
        assert!(
            r.is_ok(),
            "h1 serve_connection_with_cancel hung past 5 s deadline — graceful shutdown is broken"
        );
    }
}
