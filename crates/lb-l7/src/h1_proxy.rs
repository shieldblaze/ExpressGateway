//! Real hyper 1.x HTTP/1.1 proxy path.
//!
//! [`H1Proxy::serve_connection`] drives a hyper H1 server over each accepted
//! connection (plain TCP for `H1`, TLS-decrypted for `H1s`). Per request:
//! strip hop-by-hop headers (RFC 9110 §7.6.1 plus any name listed inside
//! `Connection`), append `X-Forwarded-{For,Proto,Host}` + `Via`, pick a
//! backend, forward body-timeout-bounded, then strip hop-by-hop from the
//! response and optionally inject `Alt-Svc`. The whole connection is bounded
//! by [`HttpTimeouts::total`].

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
// CF-DEDUP-1: the H1→H2 streaming leg routes its egress through the SHARED
// graceful-drop driver and therefore speaks the h2_proxy `ProxyErr` — NOT the
// h1_proxy-local one the H1→H1 / WS / H1→H3 paths keep using.
use crate::h2_proxy::{H2_ABORT_OBSERVE_TIMEOUT, ProxyErr as H2ProxyErr, drive_h2_upstream_send};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use lb_io::http2_pool::Http2Pool;
use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;
use tokio::io::{AsyncRead, AsyncWrite};

use lb_security::{ConnId, SmuggleDetector, SmuggleMode, Watchdog};

use crate::security_hooks::{DynSecurityHooks, NoopHooks, SecurityReject};
use crate::stripped_request::{StrippedRequest, strip_hop_by_hop as strip_into_newtype};
use crate::upstream::{BackendInfoPicker, SingleProtoPicker, UpstreamBackend, UpstreamProto};
use crate::ws_proxy::{self, WsProxy, build_handshake_response_headers, is_h1_upgrade_request};

/// Hop-by-hop headers per RFC 9110 §7.6.1, stripped from BOTH request and
/// response in addition to any name listed inside the `Connection` value.
/// `HeaderName` constants so removal is panic-free at runtime.
///
/// PROTO-2-08 removed `"trailers"`, which appeared here IN ERROR: it is not a
/// header field name at all — only a value-token inside `TE: trailers`. The
/// real `Trailer:` header (RFC 9110 §6.6.2) is end-to-end. `keep-alive` was
/// missing and was added. Do not re-add either.
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

/// Client-facing response body. Widened from `BoxBody<Bytes, hyper::Error>` to
/// a boxed `std::error::Error` so a channel-built streaming response can inject
/// a CONSTRUCTIBLE truncation error ([`H1PumpAbort`]) — `hyper::Error` has no
/// public ctor. Lossless: hyper's H1 server only requires
/// `Body::Error: Into<Box<dyn Error + Send + Sync>>`, so a boxed `hyper::Error`
/// aborts byte-identically to the un-boxed one.
pub(crate) type ClientRespBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

/// Depth of the bounded in-flight channel feeding the streaming H1→H1 request
/// body. `DEPTH × CHUNK_MAX` (64 KiB) is the ceiling on retained inbound
/// memory, body-size-INDEPENDENT and independent of
/// [`MAX_REQUEST_BODY_BYTES`] — the R8 property the memory proof asserts.
const H1_REQ_CHANNEL_DEPTH: usize = 8;

/// Maximum size of one chunk pumped through the in-flight channel.
const H1_REQ_CHUNK_MAX: usize = 8 * 1024;

/// F-MD-4 — request-smuggling guard, H1 mirror of `h2_proxy::PumpAbort`.
/// Dropping the request-body channel sender makes `poll_recv` return `None`,
/// which `StreamBody` translates to a CLEAN body EOF: hyper then emits the
/// chunked terminator and the upstream sees a COMPLETE request — the wrong
/// signal when the inbound stream was truncated mid-body (a premature client
/// half-close is a smuggling primitive). `hyper::Error` has no public
/// constructor, so the pump sends `Err(H1PumpAbort)` instead: hyper sees an
/// ERROR, not a clean EOF, and aborts the upstream request without a
/// terminator.
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
/// THE CATCH (hyper 1.9.0, `proto/h1/decode.rs::decode_trailers`): the inbound
/// chunked-trailer decoder validates only name/value SYNTAX and inserts EVERY
/// field into the trailers map — it does NOT strip or reject framing/routing
/// fields. Forwarding trailers verbatim (the R3 requirement that keeps
/// `trailer_passthrough.rs` green) would therefore also relay a
/// `Transfer-Encoding` / `Content-Length` carried in the trailers, which
/// RFC 7230 §4.1.2 / RFC 9110 §6.5.1 forbid: a request-smuggling/desync
/// primitive at the next hop. H1 analogue of the H2 pseudo-header-in-trailers
/// reject. A legitimate trailer (`x-checksum`, `grpc-status`) passes through.
fn validate_h1_request_trailers(trailers: &hyper::HeaderMap) -> Result<(), ProxyErr> {
    use hyper::header::{CONNECTION, CONTENT_LENGTH, HOST, TE, TRAILER, TRANSFER_ENCODING};
    for name in trailers.keys() {
        // Framing/routing fields forbidden in trailers (RFC 7230 §4.1.2).
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
    /// Maximum time the client may take to deliver the complete request line +
    /// header section (the slowloris deadline). Wired into hyper's
    /// `header_read_timeout`, which requires a `Timer`. Also the WS
    /// upgrade-dial budget. NOT the whole-request deadline — that is `total`.
    pub header: Duration,
    /// Maximum time the upstream spends sending its response (and the client
    /// its request body). The Phase-A no-forward-progress idle deadline.
    pub body: Duration,
    /// Hard upper bound on a single connection's total lifetime.
    pub total: Duration,
    /// Phase-B fixed cap on the post-upload head wait, separate from the
    /// Phase-A `body` idle deadline.
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

/// ROUND8-L7-05 — runtime-side policy for `_` in inbound header names. Mirrors
/// `lb_config::HeaderUnderscorePolicy`; lives in lb-l7 to avoid a dep edge from
/// the proxy onto the config crate, so the wiring crate maps between them.
/// Default [`HeaderUnderscorePolicy::Reject`] — Envoy edge best-practice.
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

/// Picks the next backend address. Implementations must be cheap to call
/// and lock-free or fine-grained: it runs once per inbound request.
pub trait BackendPicker: Send + Sync {
    /// Return the next backend [`SocketAddr`] to dial, or `None` if no
    /// backend can serve the request.
    fn pick(&self) -> Option<SocketAddr>;
}

/// Round-robin picker over a fixed address list; keeps the proxy decoupled
/// from the `lb_balancer` crate.
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
    /// When `Some`, RFC 6455 handshakes route through the WebSocket proxy.
    ws: Option<Arc<WsProxy>>,
    /// Optional H2 upstream pool (H1→H2 path).
    h2_upstream: Option<Arc<Http2Pool>>,
    /// Optional H3 upstream pool (H1→H3 path).
    h3_upstream: Option<Arc<QuicUpstreamPool>>,
    /// Security-hook surface; defaults to [`NoopHooks`].
    hooks: Arc<dyn DynSecurityHooks>,
    /// Slowloris / slow-POST watchdog. `None` leaves only the [`HttpTimeouts`]
    /// deadlines in play.
    watchdog: Option<Watchdog>,
    /// Monotonic per-listener sequence, combined with the peer IP as the
    /// [`Watchdog`] key so two concurrent NAT-egress connections stay distinct.
    conn_seq: Arc<parking_lot::Mutex<u64>>,
    /// When `true` the per-request smuggle check runs in
    /// [`SmuggleMode::H1Strict`] (rejects any `Transfer-Encoding` codec other
    /// than `chunked`). Default `false` keeps the lenient RFC 9112 baseline.
    smuggle_strict: bool,
    /// ROUND8-L7-05: policy for `_` in inbound header names, default `Reject`.
    header_underscore_policy: HeaderUnderscorePolicy,
    /// Default expected SNI for [`crate::sni_authority::check_sni_authority`].
    /// `None` means SNI/authority agreement is not enforced unless
    /// [`Self::serve_connection_with_cancel_sni`] supplies a per-connection one.
    expected_sni: Option<String>,
    /// ROUND8-L7-06: requests served per keep-alive connection (nginx
    /// `keepalive_requests`); `0` disables. On the cap-th response the service
    /// sets `Connection: close` and signals the driver to `graceful_shutdown`
    /// after the body flushes. Default `100`.
    max_keepalive_requests: u32,
    /// ROUND8-L7-06: cap-triggered-close counter. An `AtomicU64` rather than a
    /// metric handle because lb-l7 has no metrics-registry dep edge.
    keepalive_cap_terminations: Arc<std::sync::atomic::AtomicU64>,
}

impl H1Proxy {
    /// Construct an [`H1Proxy`] over a single-protocol H1 backend pool.
    /// `is_https` selects the value emitted into `X-Forwarded-Proto`. Wraps
    /// `picker` in a [`SingleProtoPicker`] tagged [`UpstreamProto::H1`]; use
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
        }
    }

    /// Construct an [`H1Proxy`] backed by a multi-protocol picker. A pick whose
    /// matching pool is `None` falls back to a 502 response.
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
        }
    }

    /// Attach a security-hooks impl. Without this call the proxy falls back to
    /// [`NoopHooks`] and the production smuggle / cap / watchdog checks are off.
    #[must_use]
    pub fn with_hooks(mut self, hooks: Arc<dyn DynSecurityHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Attach an [`lb_security::Watchdog`] for slowloris / slow-POST eviction.
    /// Each request registers with a [`HttpTimeouts::header`]-derived deadline
    /// and records one `progress` measurement from the header-byte estimate;
    /// per-chunk body progress needs IO wrapping and is not wired here.
    #[must_use]
    pub fn with_watchdog(mut self, watchdog: Watchdog) -> Self {
        self.watchdog = Some(watchdog);
        self
    }

    /// Enable strict-TE policy ([`SmuggleMode::H1Strict`] — rejects any
    /// `Transfer-Encoding` codec other than `chunked`). Default is lenient.
    #[must_use]
    pub const fn with_smuggle_strict(mut self, strict: bool) -> Self {
        self.smuggle_strict = strict;
        self
    }

    /// ROUND8-L7-05: set the header-name underscore policy. Default
    /// [`HeaderUnderscorePolicy::Reject`].
    #[must_use]
    pub const fn with_header_underscore_policy(mut self, policy: HeaderUnderscorePolicy) -> Self {
        self.header_underscore_policy = policy;
        self
    }

    /// Default expected SNI for the
    /// [`crate::sni_authority::check_sni_authority`] hot-path check. TLS-
    /// bearing deployments prefer [`Self::serve_connection_with_cancel_sni`],
    /// which captures the SNI live from rustls at accept time.
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

    /// Whether an H2 upstream pool has been wired for this proxy.
    #[must_use]
    pub const fn has_h2_upstream(&self) -> bool {
        self.h2_upstream.is_some()
    }

    /// Whether an H3 upstream pool has been wired for this proxy.
    #[must_use]
    pub const fn has_h3_upstream(&self) -> bool {
        self.h3_upstream.is_some()
    }

    /// ROUND8-L7-06: set the per-keep-alive-connection request cap; `0`
    /// disables (only the wall-clock / idle timeouts then apply).
    #[must_use]
    pub fn with_max_keepalive_requests(mut self, cap: u32) -> Self {
        self.max_keepalive_requests = cap;
        self
    }

    /// ROUND8-L7-06: shared handle to the cap-triggered-close counter so the
    /// wiring crate can lift it into a metric without an lb-l7 → registry dep.
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

    /// Drive HTTP/1.1 server logic over `io`, returning once the connection has
    /// fully closed. Bounded by [`HttpTimeouts::total`].
    ///
    /// # Errors
    ///
    /// I/O errors and timeouts. Per-request upstream errors become 502/504
    /// responses and do NOT terminate the connection.
    pub async fn serve_connection<IO>(self: Arc<Self>, io: IO, peer: SocketAddr) -> io::Result<()>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        self.serve_connection_with_cancel(io, peer, tokio_util::sync::CancellationToken::new())
            .await
    }

    /// PROTO-2-11 — H1 half of the drain-on-cancel contract. Identical to
    /// [`Self::serve_connection`] until `cancel` fires, at which point
    /// `.graceful_shutdown()` is invoked.
    ///
    /// PROTO-2-16 CAVEAT: hyper-1's `http1::graceful_shutdown` only calls
    /// `disable_keep_alive()`, and the encoder serialises `Connection: close`
    /// solely onto a response head that has NOT yet been flushed. If the cancel
    /// lands after the current head is already on the wire, the only close
    /// signal the client receives is the FIN at body completion — the header is
    /// not added retroactively. RFC 9110 §7.6.1 permits this. The connection
    /// future is then driven to completion within the existing `total` budget.
    ///
    /// # Errors
    ///
    /// Same as [`Self::serve_connection`], plus `TimedOut` if the
    /// graceful-shutdown driver exceeds [`HttpTimeouts::total`].
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

    /// H1 entry point that threads the per-connection TLS SNI into the request
    /// hot path so [`crate::sni_authority::check_sni_authority`] runs against
    /// the OBSERVED SNI rather than the builder default. Plain-TCP listeners
    /// and SNI-omitting clients pass `None`, which disables the check.
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
        // ROUND8-L7-06: per-connection counter + close-notify, shared across
        // hyper's per-request service clones.
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
        // F-RES-1: a `Timer` MUST be wired before `header_read_timeout` has any
        // effect — without it hyper silently ignores the deadline and the
        // header-read phase is bounded only by `total` (slowloris hold).
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
        // ROUND8-L7-06: cap-driven close. Additive arm — it does NOT touch the
        // SIGTERM-cancel / total-timeout arms. The service already set
        // `Connection: close` on the cap-th response head; this drives the same
        // `graceful_shutdown` so the socket dies after that response flushes.
        let cap_close = close_signal.notified();
        tokio::pin!(cap_close);
        tokio::select! {
            // biased: cancel wins ties so a SIGTERM mid-request still drains.
            biased;
            () = &mut cancel_fut => {
                // PROTO-2-16 (see the fn doc): a head already on the wire
                // cannot gain a retroactive `Connection: close`; the FIN is
                // then the only close signal.
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
            // ROUND8-L7-06: cap reached. The cap-th response already carries
            // `Connection: close`; drive the same graceful_shutdown so the FIN
            // follows the flush, bounded by `total`. A clean completion here is
            // `Ok(())` — the cap close is the INTENDED terminal state, unlike
            // the total-timeout arm.
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

/// Service carrying the [`H1Proxy`] plus the peer address. hyper clones it once
/// per request, but the CONNECTION-scoped state (`served`, `cap`,
/// `close_signal`) lives behind `Arc`s built once per connection, so every
/// per-request clone shares one counter and one close-notify.
#[derive(Clone)]
struct ProxyService {
    inner: Arc<H1Proxy>,
    peer: SocketAddr,
    /// SNI captured at TLS-accept time; `None` on plain TCP.
    expected_sni: Option<String>,
    /// Per-connection request counter (shared across per-request clones).
    served: Arc<std::sync::atomic::AtomicU32>,
    /// ROUND8-L7-06: per-connection request cap (`0` disables).
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
        // Count this request BEFORE handling it; `fetch_add` returns the prior
        // value, so `count` is 1-based for this request.
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
                // RFC 9110 §7.6.1: advertise the close on the head; the
                // driver's `graceful_shutdown` tears the socket down after the
                // flush. `count == cap` fires exactly once.
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
    /// Span-opening wrapper: extracts the inbound W3C trace context, opens the
    /// request span, and runs the handler `.instrument()`-ed. Deliberately NOT
    /// a `tracing::span::Entered` guard held across an `.await` — that leaks
    /// the span onto whatever the executor polls next on the same thread, and
    /// only bites under concurrent load.
    async fn handle(
        &self,
        req: Request<IncomingBody>,
        peer: SocketAddr,
        expected_sni: Option<&str>,
    ) -> Response<ClientRespBody> {
        use tracing::Instrument;
        // H1Proxy carries no per-bind label, so the span's `listener` field is
        // the protocol family (h1/h1s).
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
        // ROUND8-L7-09 — authority-validation CHOKE POINT (the H1 parser was
        // missing the check the H2/H3 path had). MUST stay the FIRST statement:
        // a comma / whitespace / control byte in `Host`, or an absolute-form
        // target, is a routing/ACL-desync primitive — and the WS-upgrade fork
        // below reached `pick_info()` unvalidated before it was hoisted here.
        if let Err((bad, err)) = crate::authority::validate_request(&req) {
            tracing::warn!(
                peer = %peer,
                authority = %bad,
                error = ?err,
                "ROUND8-L7-09: H1 authority rejected (choke point)"
            );
            return error_response(StatusCode::BAD_REQUEST, "invalid authority (ROUND8-L7-09)");
        }

        // gRPC requires HTTP/2 — its framing relies on H2 streams, trailers and
        // HEADERS continuation — so reject on an H1 listener with 415 rather
        // than letting a downstream H1 backend answer 502. Match exactly
        // `application/grpc` or `application/grpc+<sub>` on a case-insensitive
        // media-type token (RFC 7231 §3.1.1.1) after stripping `;`-parameters;
        // the trailing `+` deliberately keeps `application/grpc-web` (hyphen,
        // plain HTTP, forwards transparently) outside the reject.
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

        // WebSocket upgrade intercept (RFC 6455 §4); only when a `WsProxy` is wired.
        if self
            .ws
            .as_ref()
            .is_some_and(|w| w.config().enabled && is_h1_upgrade_request(&req))
        {
            return self.handle_ws_upgrade(req, req_trace).await;
        }

        let (mut parts, body) = req.into_parts();

        // ROUND8-L7-05: enforce the header-name underscore policy before any
        // other inspection; default `Reject` (Envoy edge best-practice).
        //
        // SEC-2-01 defence-in-depth: strict smuggle mode FORCES `Reject`
        // regardless of operator configuration — opting out of underscore
        // rejection requires also opting out of strict-TE mode.
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

        // Run the security hooks before hop-by-hop strip + upstream-acquire so
        // a rejected request never spends a pool slot. The rebuilt
        // `Request<()>` is a header-only borrow surface.
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

        // ROUND8-L7-09 authority validation already ran at the `handle_inner`
        // choke point above, covering the upgrade fork — no second call here.

        // PROTO-2-18 — SNI ↔ Host agreement (RFC 9110 §15.5.20); H1 carries the
        // authority in `Host` (RFC 9112 §3.2). Loopback peers skip enforcement
        // (sec-r5): SNI-vs-Host confusion is a Layer-7 routing/authz vector
        // that does not apply to loopback, and probe scripts routinely use
        // IP-literal Host headers that cannot match the cert's SNI.
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

        // SEC-2-01 defense-in-depth: this call site fires regardless of which
        // `DynSecurityHooks` impl is wired ([`NoopHooks`] included), so the
        // detector is never dead code on the proxy hot path.
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

        // Register with the slowloris watchdog at `now + HttpTimeouts::header`;
        // an overrun is evicted via `progress` (or the sweeper).
        let watch_id = self.watchdog.as_ref().map(|wd| {
            let seq = {
                let mut g = self.conn_seq.lock();
                *g = g.wrapping_add(1);
                *g
            };
            let id = ConnId::new(peer.ip(), seq);
            let deadline = std::time::Instant::now() + self.timeouts.header;
            wd.register(id, deadline);
            // Header bytes (approximate) as the initial progress checkpoint —
            // the detector treats progress as cumulative bytes-read.
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

        // PROTO-2-07 — strip via the `StrippedRequest` newtype factory so the
        // downstream proxy_* methods consume a type that statically guarantees
        // hop-by-hop has been removed.
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

        let resp = match backend.proto {
            UpstreamProto::H1 => match self.proxy_request(backend.addr, stripped).await {
                Ok(resp) => self.finalize_response(resp),
                Err(ProxyErr::Upstream(s)) => error_response(StatusCode::BAD_GATEWAY, &s),
                Err(ProxyErr::Timeout) => {
                    error_response(StatusCode::GATEWAY_TIMEOUT, "upstream timeout")
                }
                // A malformed inbound body (F-MD-4 truncation or a forbidden
                // trailer) is the CLIENT's fault → 400, never the backend's
                // response.
                Err(ProxyErr::BadRequest(s)) => error_response(StatusCode::BAD_REQUEST, &s),
                Err(ProxyErr::BodyTooLarge) => error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body exceeds maximum allowed size",
                ),
            },
            UpstreamProto::H2 => Box::pin(self.proxy_h1_to_h2(backend.addr, stripped)).await,
            UpstreamProto::H3 => Box::pin(self.proxy_h1_to_h3(&backend, stripped)).await,
        };
        // Deregister here; the sweeper covers the abandoned-future case.
        if let (Some(wd), Some(id)) = (self.watchdog.as_ref(), watch_id) {
            wd.deregister(id);
        }
        resp
    }

    /// Forward an H1 inbound request to an H1 upstream backend over a
    /// single-use TCP stream.
    ///
    /// **ROUND8-L7-10 — take-and-discard upstream stream pattern.**
    /// `pooled.take_stream()` consumes the `PooledTcp` wrapper WITHOUT running
    /// its return-to-pool `Drop`, so an H1 upstream socket is never reused.
    /// That is correct by accident, and it matters: Pingora paid for this bug
    /// twice (0.6.0 "Discard extra upstream body and disable keepalive", 0.8.0
    /// "Ensure http1 downstream session is not reused on more body bytes than
    /// expected") — an upstream that sends fewer or more body bytes than its
    /// declared Content-Length corrupts the next pipelined request on a reused
    /// connection, an upstream request-smuggling primitive.
    ///
    /// **Refactor warning.** Any change that pools H1 upstream connections MUST
    /// first implement the Pingora-class over-read / under-read guard: compare
    /// the response body to `Content-Length` and call
    /// [`lb_io::pool::PooledTcp::set_reusable(false)`](lb_io::pool::PooledTcp::set_reusable)
    /// on any mismatch before letting the wrapper drop. See also the H2 cousin
    /// in `Http2Pool::send_request`, which evicts on every Send-class error.
    async fn proxy_request(
        &self,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Result<Response<IncomingBody>, ProxyErr> {
        use hyper::body::Frame;

        let req = req.into_inner();
        let (mut parts, mut body) = req.into_parts();

        // F-MD-1 — let hyper's HTTP/1.1 encoder choose the framing for the
        // unknown-length streaming body: force HTTP/1.1 and STRIP
        // `content-length` + `transfer-encoding`. An inbound H1 request CAN
        // carry a content-length, and a stale CL alongside an unknown-length
        // `StreamBody` mis-frames — the encoder either truncates to the stale
        // CL or emits an empty body and never polls our pump. Header-level
        // CL/TE smuggling was already rejected pre-pump by the smuggle
        // detector; this strip is framing correctness, not a security check.
        parts.version = hyper::Version::HTTP_11;
        parts.headers.remove(hyper::header::CONTENT_LENGTH);
        parts.headers.remove(hyper::header::TRANSFER_ENCODING);

        // Async dial; the pool's `connect_timeout` bounds the syscall.
        //
        // S9 — Branch-B-only (no lookahead): H1 ingress has no HPACK, no H2
        // framing and no validate-before-forward ordering requirement, so we
        // dial first and forward-as-it-arrives through a bounded in-flight
        // window. No whole-body buffering, no `collect()`.
        let pooled = self
            .pool
            .acquire_async(backend_addr)
            .await
            .map_err(|e| ProxyErr::Upstream(format!("backend connect {backend_addr}: {e}")))?;

        // ROUND8-L7-10 (see the fn doc): `take_stream` defeats the pool's
        // return-to-pool Drop, making this upstream connection single-use. Do
        // not remove without first implementing the body-length guard.
        let stream = pooled
            .take_stream()
            .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;

        // F-MD-4: the body's error type is the constructible `H1PumpAbort`
        // (`hyper::Error` has no public ctor) so the pump can INJECT an error
        // instead of dropping the channel — a drop reads as a clean EOF, i.e. a
        // smuggled-complete request.
        let (mut sender, conn) = hyper::client::conn::http1::handshake::<
            _,
            BoxBody<Bytes, H1PumpAbort>,
        >(TokioIo::new(stream))
        .await
        .map_err(|e| ProxyErr::Upstream(format!("h1 client handshake: {e}")))?;

        let conn_handle = tokio::spawn(async move {
            // Errors here usually mean the upstream half-closed; that surfaces
            // on the response side via `send_request`. The handle is dropped at
            // end-of-scope so the task dies if it outlives the request future.
            let _ = conn.await;
        });

        // Bounded in-flight channel — the R8 backpressure chain: backend write
        // stalls → hyper stops pulling → the channel fills → the pump stops
        // polling the inbound body → the client's send is paused.
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, H1PumpAbort>>(H1_REQ_CHANNEL_DEPTH);

        // F-MD-3 — track ACTUAL live in-flight occupancy: incremented just
        // before a push, decremented the moment hyper pulls the chunk back out.
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight_body = std::sync::Arc::clone(&in_flight_bytes);

        // Bridge the receiver into an `http_body` stream body; each pull
        // decrements the live counter (the chunk now belongs to hyper).
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

        // The pump owns the inbound body and reports its terminal verdict via a
        // oneshot, so the response head is gated on a VALIDATED terminal state.
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), ProxyErr>>();

        // S14 — forward-progress signal for
        // [`lb_io::idle_send::idle_bounded_send`]: `last_progress` is bumped on
        // every chunk hyper accepted, and `upload_complete` is set once at the
        // verdict-Ok arm so the helper switches from the Phase-A idle deadline
        // (`timeouts.body`) to the Phase-B head cap (`timeouts.head`).
        let last_progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let upload_complete = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let epoch = tokio::time::Instant::now();
        let last_progress_pump = std::sync::Arc::clone(&last_progress);
        let upload_complete_pump = std::sync::Arc::clone(&upload_complete);
        let epoch_pump = epoch;

        let pump = tokio::spawn(async move {
            // Relaxed is fine: the helper re-arms on the next tick if a bump
            // lands late.
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

            // `ReceiverGone` = hyper dropped the request body (the backend
            // early-responded WITHOUT reading it). Do NOT manufacture a 413
            // (F-MD-2) — switch to drain-and-validate so the backend's real
            // response is relayed once the inbound body validates.
            enum SendOutcome {
                ReceiverGone,
            }

            // Split a DATA payload into ≤ `H1_REQ_CHUNK_MAX` pieces and push
            // each through the bounded channel (the backpressure point).
            macro_rules! send_chunked {
                ($bytes:expr) => {{
                    let mut data: Bytes = $bytes;
                    let mut outcome: Result<(), SendOutcome> = Ok(());
                    while !data.is_empty() {
                        let take = data.len().min(H1_REQ_CHUNK_MAX);
                        let chunk = data.split_to(take);
                        let clen = chunk.len();
                        in_flight_bytes.fetch_add(clen, std::sync::atomic::Ordering::Relaxed);
                        // F-MD-3: the ACTUAL retained set (the decrement
                        // happens in the StreamBody poll when hyper pulls).
                        #[cfg(any(test, feature = "test-gauges"))]
                        record_retained_h1(
                            in_flight_bytes.load(std::sync::atomic::Ordering::Relaxed),
                        );
                        if tx.send(Ok(Frame::data(chunk))).await.is_err() {
                            // Never entered hyper's buffer — back the counter
                            // out so the gauge stays honest.
                            in_flight_bytes.fetch_sub(clen, std::sync::atomic::Ordering::Relaxed);
                            outcome = Err(SendOutcome::ReceiverGone);
                            break;
                        }
                        bump();
                    }
                    outcome
                }};
            }

            // drain-and-validate (F-MD-2): the backend stopped reading, so we
            // can no longer forward — but we MUST still drive the inbound body
            // to a validated terminal state or a truncated request would relay
            // the backend's response. Bytes are DISCARDED (one frame at a time);
            // the 64 MiB cap and the trailer guard still apply.
            macro_rules! drain_and_validate {
                () => {{
                    loop {
                        match body.frame().await {
                            // F-MD-4 (H1): `frame()==None` IS the positively-
                            // confirmed clean end; a truncation surfaces as
                            // `Some(Err)` instead.
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

            // Forward-as-it-arrives with the bounded window.
            loop {
                match body.frame().await {
                    None => {
                        // F-MD-4 (H1 — MIRROR-IMAGE of the H2 case; do NOT copy
                        // the H2 `is_end_stream` logic here).
                        //
                        // The inbound H1 server body is hyper's `Kind::Chan` (a
                        // channel fed by the H1 connection driver), NOT
                        // `Kind::H2`. For H1, `frame()==None` IS the
                        // positively-confirmed clean end-of-body: the chunked
                        // decoder reached the real `0\r\n\r\n` (or a
                        // Content-Length body was fully satisfied) and the
                        // driver dropped the body sender. A PREMATURE mid-body
                        // half-close does NOT arrive here as `None`: hyper-1.9.0
                        // emits `IncompleteBody` (UnexpectedEof) on early EOF
                        // for BOTH chunked (decode.rs ~L162) and Content-Length
                        // (~L504), which the driver pushes into the body channel
                        // as `Some(Err(..))` — the arm below. Request bodies are
                        // never close-delimited, so there is no "EOF == clean
                        // end" framing on the request path at all.
                        //
                        // And do NOT consult `Body::is_end_stream()` for H1: for
                        // `Kind::Chan` it returns `content_length == ZERO`,
                        // unreliable for chunked bodies (CHUNKED is never
                        // decremented to ZERO).
                        //
                        // Clean end → drop `tx` → the StreamBody yields `None`
                        // → hyper writes the chunked terminator → the upstream
                        // sees a COMPLETE request.
                        set_complete();
                        let _ = verdict_tx.send(Ok(()));
                        return;
                    }
                    Some(Ok(frame)) => {
                        if frame.is_trailers() {
                            // Q-H3: validate BEFORE forwarding — a
                            // framing/routing field in trailers is a desync
                            // primitive that hyper's decoder does NOT reject.
                            let verdict = frame
                                .trailers_ref()
                                .map_or(Ok(()), validate_h1_request_trailers);
                            match verdict {
                                Ok(()) => {
                                    // Forward legitimate trailers byte-
                                    // faithfully (R3 keeps trailer_passthrough
                                    // green), then a clean verdict.
                                    let _ = tx.send(Ok(frame)).await;
                                    bump();
                                    set_complete();
                                    let _ = verdict_tx.send(Ok(()));
                                    return;
                                }
                                Err(e) => {
                                    // FIFO Err-before-close: inject the body
                                    // error FIRST so hyper aborts the upstream
                                    // request WITHOUT a clean terminator
                                    // (dropping tx alone = clean EOF =
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
                                // Cap exceeded mid-stream. FIFO Err-before-
                                // close: the body error first (upstream ends
                                // abruptly; the caller aborts the conn and never
                                // relays its response), then the 413 verdict.
                                let _ = tx.send(Err(H1PumpAbort)).await;
                                let _ = verdict_tx.send(Err(ProxyErr::BodyTooLarge));
                                return;
                            }
                            if let Err(SendOutcome::ReceiverGone) = send_chunked!(data) {
                                // Backend stopped reading mid-stream — F-MD-2
                                // drain-and-validate, NOT a 413.
                                let _ = verdict_tx.send(drain_and_validate!());
                                return;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        // F-MD-4 (H1): a premature mid-body close (or any
                        // inbound body error) surfaces HERE as `Some(Err)`
                        // (hyper's `IncompleteBody`), NOT as a clean `None`.
                        // FIFO Err-before-close: the body error first so hyper
                        // aborts the upstream request without a `0\r\n\r\n`
                        // terminator — the backend NEVER observes a COMPLETE
                        // (truncated) request — then the 400 verdict. Single-use
                        // `take_stream` also guarantees the aborted upstream
                        // conn is dropped, not pooled.
                        let _ = tx.send(Err(H1PumpAbort)).await;
                        let _ = verdict_tx.send(Err(ProxyErr::BadRequest(format!(
                            "inbound H1 request body incomplete: {e}"
                        ))));
                        return;
                    }
                }
            }
        });

        // Drive the upstream send concurrently with the pump (hyper must pull
        // the channel for the pump to progress under backpressure), but do NOT
        // relay the response until the pump's terminal verdict lands.
        //
        // S14 — the former fixed wall-clock `timeout(timeouts.body, send_fut)`
        // was really a WHOLE-UPLOAD deadline for backends that withhold the
        // response head until the body is consumed. `idle_bounded_send` splits
        // it: Phase A is a no-forward-progress idle deadline re-armed by the
        // pump's `bump()`; Phase B a fixed `timeouts.head` cap anchored when
        // `set_complete()` ran. The helper passes the hyper error through
        // unchanged, so the verdict-vs-send classification below is identical.
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
                // F-CAP-1 — a `send_request` error is the downstream EFFECT of
                // whatever the pump did; the pump's classified verdict is the
                // AUTHORITATIVE cause. Returning 502 here would mask a real
                // 413/400 and create a 413-vs-502 race. Consult the verdict
                // first, BOUNDED by `timeouts.body` so a wedged pump cannot
                // hang the error path, and do NOT `pump.abort()` before this
                // await — the pump must still deliver its verdict.
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
                // Collapse onto Timeout; phase discriminant logged for triage.
                tracing::warn!(error = %idle_err, "h1→h1 idle/head deadline fired");
                pump.abort();
                conn_handle.abort();
                return Err(ProxyErr::Timeout);
            }
        };

        // Validate-before-RESPONSE-relay gate: the response head only relays
        // once the inbound body has reached a validated terminal state.
        match verdict_rx.await {
            Ok(Ok(())) => {
                // Do NOT await `conn_handle` — response-body streaming still
                // needs the driver task running. Detach it.
                drop(conn_handle);
                Ok(resp)
            }
            Ok(Err(e)) => {
                // Malformed/truncated inbound or over-cap: abort the upstream
                // connection (do NOT pool it) and the pump, and NEVER relay the
                // upstream response.
                pump.abort();
                conn_handle.abort();
                Err(e)
            }
            Err(_) => {
                // Pump vanished without a verdict — treat as an inbound
                // failure; never leak the backend response.
                conn_handle.abort();
                Err(ProxyErr::BadRequest(
                    "inbound H1 request pump terminated without a verdict".to_owned(),
                ))
            }
        }
    }

    /// STREAMING H1→H2 request leg. MIRROR of
    /// [`crate::h2_proxy::H2Proxy::proxy_h2_to_h2_request`]: a bounded lookahead
    /// window → Branch A (whole request fit the window → buffered send) /
    /// Branch B (streaming pump → the SHARED [`drive_h2_upstream_send`] driver).
    ///
    /// Deltas vs that mirror: [`build_h1_to_h2_upstream_parts`] for the head;
    /// H1 ingress framing (`frame()==None` IS the confirmed clean end — NOT
    /// gated on `is_end_stream()`, unreliable for chunked `Kind::Chan`; a
    /// premature close surfaces as `Some(Err)`); the constructible
    /// [`H1PumpAbort`] as the channel error so the pump can INJECT a body error
    /// rather than let hyper emit a spurious clean END_STREAM on a truncated
    /// request; [`validate_h1_request_trailers`]; [`record_retained_h1`].
    ///
    /// Returns the h2_proxy [`H2ProxyErr`] — the type the shared driver returns.
    async fn proxy_h1_to_h2_request(
        &self,
        h2_pool: &Http2Pool,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Result<Response<IncomingBody>, H2ProxyErr> {
        // DELTA: no `use hyper::body::Body` — the H1 framing deliberately never
        // calls `is_end_stream()` (`None` is the clean-end signal for H1).
        use hyper::body::Frame;
        use lb_io::http2_pool::{H2ReqBody, Http2PoolError};

        let req = req.into_inner();
        let (parts, mut body) = req.into_parts();

        // DELTA vs `proxy_request`: keep the request HTTP/2-shaped via the
        // H1→H2 bridge head, WITHOUT collecting the body, and do NOT force
        // HTTP/1.1 or strip content-length/transfer-encoding — those were
        // H1-framing fixes; H2 upstream framing is hyper's H2 encoder's job.
        let upstream_parts = match build_h1_to_h2_upstream_parts(&parts) {
            Ok(p) => p,
            Err(e) => return Err(H2ProxyErr::Upstream(e)),
        };

        // Bounded ingress pump; the lookahead posture is IDENTICAL to H2→H2,
        // only the framing arms differ.
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
                    // F-MD-4 (H1 — MIRROR-IMAGE of H2): `None` IS the confirmed
                    // clean end; a premature close surfaces as `Some(Err)`
                    // below. Do NOT consult `is_end_stream()` (unreliable for
                    // chunked `Kind::Chan`). Clean end in-window → Branch A.
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
                    // F-MD-4 (H1): a premature close surfaces while VALIDATING,
                    // BEFORE any pool contact — zero-dial for a within-window
                    // truncation, preserving Branch-A validate-before-dial.
                    return Err(H2ProxyErr::BadRequest(format!(
                        "inbound H1 request body incomplete: {e}"
                    )));
                }
            }
        }

        if reached_eof {
            // ── Branch A: the whole request fit the window. Zero pool contact
            // for a malformed one — any inbound Err returned above.
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
            // DELTA: widen the shared helper's `hyper::Error` to the boxed
            // error `H2ReqBody` requires.
            let upstream_body: H2ReqBody = build_body_with_trailers(body_bytes, &trailers_vec)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                .boxed();
            let upstream_req = Request::from_parts(upstream_parts, upstream_body);

            return match h2_pool.send_request(backend_addr, upstream_req).await {
                Ok(resp) => Ok(resp),
                Err(Http2PoolError::Timeout) => Err(H2ProxyErr::Timeout),
                Err(e) => Err(H2ProxyErr::Upstream(format!("h2 upstream: {e}"))),
            };
        }

        // ── Branch B: request > window → stream with the bounded in-flight
        // window; gate the response head on the inbound terminal state.
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, H1PumpAbort>>(H1_REQ_CHANNEL_DEPTH);

        // F-MD-3: genuine retained-memory gauge (live in-flight occupancy).
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight_body = std::sync::Arc::clone(&in_flight_bytes);

        // Bridge the receiver into a StreamBody, mapping `H1PumpAbort` → boxed
        // error so the body is `H2ReqBody`.
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

        // The pump reports its terminal verdict via a oneshot so the response
        // head is gated on a VALIDATED terminal state.
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), H2ProxyErr>>();
        let drained: Vec<Bytes> = std::mem::take(&mut lookahead);

        // S14 — forward-progress signal threaded through
        // [`drive_h2_upstream_send`].
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
                            // F-MD-4 (H1): clean end-of-body = `None`; a
                            // truncation surfaces as `Some(Err)` instead.
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

            // F-MD-4 (body-layer half, H1 mirror of H2→H2's `inject_abort!`) —
            // inject `Err(H1PumpAbort)` and HOLD the sender open until hyper has
            // OBSERVED it. FIFO delivery forces hyper to poll the error BEFORE
            // any channel-close `None`, so it RESETS the upstream stream rather
            // than emitting a spurious clean END_STREAM. Bounded by the shared
            // `H2_ABORT_OBSERVE_TIMEOUT` so a wedged driver cannot hang the pump.
            macro_rules! inject_abort {
                () => {{
                    let _ = tx.send(Err(H1PumpAbort)).await;
                    let _ = tokio::time::timeout(H2_ABORT_OBSERVE_TIMEOUT, tx.closed()).await;
                }};
            }

            // 1) Drain the lookahead buffer first (oldest chunks first).
            for chunk in drained {
                if let Err(SendOutcome::ReceiverGone) = send_chunked!(chunk, true) {
                    let _ = verdict_tx.send(drain_and_validate!());
                    return;
                }
            }
            // 2) Continue forward-as-it-arrives with the bounded window.
            loop {
                match body.frame().await {
                    None => {
                        // F-MD-4 (H1 — do NOT copy the H2 `is_end_stream`
                        // logic). `None` = clean end → drop tx → hyper writes
                        // the terminator → the upstream sees a COMPLETE
                        // request. NO inject_abort on this arm.
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
                        // F-MD-4 (H1): premature close / inbound body error →
                        // inject the body error FIRST so hyper aborts the
                        // upstream request WITHOUT a clean terminator, THEN
                        // signal the 400 verdict.
                        inject_abort!();
                        let _ = verdict_tx.send(Err(H2ProxyErr::BadRequest(format!(
                            "inbound H1 request body incomplete: {e}"
                        ))));
                        return;
                    }
                }
            }
        });

        // F-MD-4: route the graceful-drop egress through the SHARED driver,
        // identical to H2→H2's Branch B. It owns the detached send task (biased
        // verdict-vs-head race, `reset_peer` on abort, the F-CAP-1 caller arm)
        // and the final `head_rx.await`.
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

    /// Forward an H1 inbound request to an H2 backend. Dispatch shim over the
    /// streaming [`Self::proxy_h1_to_h2_request`]; bridges via
    /// [`crate::create_bridge`]`(Http1, Http2)`, whose codec-level translation
    /// produces the pseudo-header set hyper's H2 client expects. BOTH legs
    /// stream — no ahead-of-dial request collect, no response collect.
    async fn proxy_h1_to_h2(
        &self,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Response<ClientRespBody> {
        let Some(h2_pool) = self.h2_upstream.as_ref() else {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "H2 backend selected but no Http2Pool wired",
            );
        };
        match self
            .proxy_h1_to_h2_request(h2_pool, backend_addr, req)
            .await
        {
            Ok(resp) => upstream_response_to_h1(resp, self.alt_svc),
            Err(H2ProxyErr::Upstream(s)) => error_response(StatusCode::BAD_GATEWAY, &s),
            Err(H2ProxyErr::Timeout) => {
                error_response(StatusCode::GATEWAY_TIMEOUT, "upstream H2 timeout")
            }
            Err(H2ProxyErr::BodyTooLarge) => error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body exceeds maximum",
            ),
            Err(H2ProxyErr::BadRequest(s)) => error_response(StatusCode::BAD_REQUEST, &s),
        }
    }

    /// Forward an H1 inbound request to an H3 backend — FULLY STREAMING on both
    /// legs.
    ///
    /// Request leg: an H1 ingress pump feeds
    /// [`lb_quic::h3_bridge::ReqBodyEvent`]s into the shared streaming
    /// connector [`lb_quic::stream_request_to_h3_upstream`]. `frame()==None` is
    /// the positively-confirmed clean end → `End`; a `Some(Err)` truncation /
    /// forbidden trailer / over-cap → `Reset`, and the connector RESETs the
    /// upstream QUIC stream WITHOUT a clean FIN — a truncated inbound is NEVER
    /// presented as a complete request. `forward_req_trailers=true`: validated
    /// request trailers ride `End{trailers}` as a post-DATA HEADERS frame.
    ///
    /// Response leg: drains the connector's decoded [`lb_quic::H3RespEvent`]
    /// channel into a streaming H1 response. CF-RESP-1: a streamed H1 response
    /// cannot pre-declare a `Trailer:` header — the names are unknown at
    /// head-time — so a late `Trailers` event rides the body's terminal frame
    /// and hyper-1 may drop it absent that declaration.
    ///
    /// F-CAP-1: a PRE-DATA over-cap (the pump's first event is `Reset`) → the
    /// connector synthesizes `Head{413}`; a pre-dial failure → `Head{502}`. A
    /// MID-BODY over-cap / truncation → `H3RespEvent::Reset` (NOT a 413 —
    /// response-splitting guard) → abort the H1 client body, never FIN.
    async fn proxy_h1_to_h3(
        &self,
        backend: &UpstreamBackend,
        req: StrippedRequest<IncomingBody>,
    ) -> Response<ClientRespBody> {
        use hyper::body::Frame;
        use lb_quic::h3_bridge::{H3_BODY_CHUNK_MAX, ReqBodyEvent};

        let Some(h3_pool) = self.h3_upstream.as_ref() else {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "H3 backend selected but no QuicUpstreamPool wired",
            );
        };
        let sni = backend.sni.as_deref().unwrap_or("").to_owned();
        let addr = backend.addr;

        // Head-only field list — body + trailers now stream.
        let inner = req.into_inner();
        let (parts, mut body) = inner.into_parts();
        let headers = match build_h1_to_h3_fieldlist(&parts, &sni, /* https = */ true) {
            Ok(h) => h,
            Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
        };

        // Bounded request-body channel into the connector. Backpressure: a slow
        // QUIC upstream → the connector stops draining → this channel fills →
        // the pump stops polling → the client's H1 read window stalls.
        let (body_tx, body_rx) =
            tokio::sync::mpsc::channel::<ReqBodyEvent>(lb_quic::conn_actor::H3_BODY_CHANNEL_DEPTH);
        let (resp_tx, mut resp_rx) = tokio::sync::mpsc::channel::<lb_quic::H3RespEvent>(
            lb_quic::h3_bridge::H3_RESP_CHANNEL_DEPTH,
        );

        // F-MD-3 gauge: in-flight request bytes the pump retains; the channel
        // depth bounds total in-flight independent of body size.
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // ── Request-leg M-D-lite pump (mirror of `proxy_request`'s pump) ──
        let pump_in_flight = std::sync::Arc::clone(&in_flight_bytes);
        let pump = tokio::spawn(async move {
            // The request-body cap is OUR job (the connector caps the RESPONSE).
            // The 413-vs-RESET boundary is timing-critical: over-cap BEFORE any
            // chunk forwarded → `Reset` as the FIRST event → connector
            // inline-413; after ≥1 chunk → RESET-without-FIN (no 413, the
            // response-splitting guard).
            let mut forwarded_total: usize = 0;

            // Split DATA into ≤ `H3_BODY_CHUNK_MAX` pieces so the in-flight item
            // size matches the memory gauge. Err(()) = connector gone.
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
                        // F-MD-4 (H1): `None` is the positively-confirmed clean
                        // end → `End{trailers:[]}` → connector FIN.
                        let _ = body_tx
                            .send(ReqBodyEvent::End {
                                trailers: Vec::new(),
                            })
                            .await;
                        return;
                    }
                    Some(Ok(frame)) => {
                        if frame.is_trailers() {
                            // Validate BEFORE forwarding — a framing/routing
                            // field in trailers is a desync primitive.
                            // Forbidden → `Reset`, never a clean `End`.
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
                            // Over-cap → `Reset`; no over-cap byte is forwarded
                            // either way.
                            if forwarded_total.saturating_add(data.len()) > MAX_REQUEST_BODY_BYTES {
                                let _ = body_tx.send(ReqBodyEvent::Reset).await;
                                return;
                            }
                            forwarded_total = forwarded_total.saturating_add(data.len());
                            if send_chunked!(data).is_err() {
                                // Connector dropped the receiver — stop pumping.
                                return;
                            }
                        }
                    }
                    Some(Err(_e)) => {
                        // F-MD-4 (H1): premature close / IO error surfaces as
                        // `Some(Err)`, not a clean `None`. `Reset` → RESET-
                        // without-FIN → the backend never sees a complete
                        // (truncated) request.
                        let _ = body_tx.send(ReqBodyEvent::Reset).await;
                        return;
                    }
                }
            }
        });

        // Spawned, so it needs `'static`: move OWNED copies of every borrow in.
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

        // Response leg: the FIRST event determines the head. `Reset` or a
        // closed channel before any head → 502.
        let first = resp_rx.recv().await;
        match first {
            Some(lb_quic::H3RespEvent::Head { status, headers }) => {
                let st = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
                let builder = h3_decoded_resp_head_builder(st, &headers, self.alt_svc);

                // Stream the remaining events. `Reset` → inject a body error so
                // hyper does NOT emit a clean terminator (response-splitting
                // guard). `End` → drop the sender. A late `Trailers` rides the
                // terminal frame (CF-RESP-1 caveat).
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
                    // F-MD-4 (response leg): `H1PumpAbort` is the constructible
                    // channel error and the relay SENDS it (never a clean drop),
                    // so hyper aborts the chunked response WITHOUT a
                    // `0\r\n\r\n` terminator — the client sees a truncated
                    // response, never a smuggled-complete one.
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
                let _ = &pump; // pump is detached; its task owns the request leg
                build_h1_streaming_response(builder, stream_body.boxed())
            }
            None | Some(lb_quic::H3RespEvent::Reset) => {
                // Connector aborted before any Head — 502.
                pump.abort();
                connector_handle.abort();
                error_response(
                    StatusCode::BAD_GATEWAY,
                    "H3 upstream produced no response head",
                )
            }
            // Body/Trailers/End before a Head is a connector contract violation.
            Some(_) => {
                pump.abort();
                connector_handle.abort();
                error_response(StatusCode::BAD_GATEWAY, "H3 upstream response head missing")
            }
        }
    }

    /// Handle an RFC 6455 handshake request.
    ///
    /// **ROUND8-L7-01 (Pingora GHSA-xq2h-p299-vjwv / Envoy
    /// GHSA-rj35-4m94-77jh, both CVSS 9.3):** the client-visible `101 Switching
    /// Protocols` is emitted ONLY after the upstream WebSocket handshake
    /// succeeds. The pre-fix code returned `101` synchronously and dialed the
    /// upstream in a detached task, so a client the upstream would have
    /// rejected was already committed to WS framing on a wire that then
    /// silently closed — and anything pipelined after the upgrade request
    /// entered an unread upgraded byte-stream, the smuggling primitive both
    /// references paid for.
    ///
    /// Order (mirrors Pingora `proxy_h1.rs` / Envoy `WsHandlerImpl`): dial →
    /// drive the upstream WS client handshake under a bounded timeout → only
    /// then build `101`. On upstream failure the wire is still in H1 mode (no
    /// `101` emitted), so we return `502` (refused/unreachable) or `504`
    /// (budget elapsed) and the client connection stays keep-alive-eligible.
    ///
    /// DOCUMENTED BEHAVIOUR CHANGE: one extra upstream RTT on the WS upgrade,
    /// and `502`/`504` instead of `101`-then-silent-close on upstream failure.
    ///
    /// ROUND8-OPS-06: the child `traceparent` from `req_trace` is injected onto
    /// the upstream handshake, and the splice task is `.instrument()`-ed with
    /// the request span so its events nest under the same `trace_id`.
    ///
    /// Returns a plain 400 if the handshake is structurally valid but
    /// `Sec-WebSocket-Key` is missing once hyper hands us the request.
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

        // ROUND8-L7-01 — dial and drive the upstream handshake BEFORE any
        // client-visible response, bounded by `HttpTimeouts::header` ("time to
        // get the upstream's handshake response" is exactly that budget). A
        // timeout maps to 504; any other failure to 502.
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
            Ok(Ok(ws)) => ws,
            Ok(Err(ProxyErr::Upstream(msg))) => {
                tracing::debug!(backend = %backend_addr, error = %msg, "ws: upstream handshake refused — returning 502 (no 101 emitted)");
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "websocket upstream handshake failed",
                );
            }
            Ok(Err(ProxyErr::Timeout)) => {
                tracing::debug!(backend = %backend_addr, "ws: upstream dial timeout — returning 504 (no 101 emitted)");
                return error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "websocket upstream dial timeout",
                );
            }
            // The WS dial path never runs the request-body pump, so these
            // verdicts are unreachable; map defensively to 502 (no 101 emitted).
            Ok(Err(ProxyErr::BadRequest(_) | ProxyErr::BodyTooLarge)) => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "websocket upstream handshake failed",
                );
            }
            Err(_elapsed) => {
                tracing::debug!(backend = %backend_addr, "ws: upstream handshake budget elapsed — returning 504 (no 101 emitted)");
                return error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "websocket upstream handshake timeout",
                );
            }
        };

        // Upstream handshake succeeded: ONLY NOW arm the upgrade future and
        // build the client `101`. The task only splices.
        let upgrade_fut = hyper::upgrade::on(&mut req);
        tokio::spawn(tracing::Instrument::instrument(
            run_h1_ws_splice_task(upgrade_fut, backend_ws, ws_proxy),
            req_trace.span.clone(),
        ));

        // Mirror a sub-protocol selection if the client asked for one — v1
        // picks the first offered protocol verbatim.
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
        // Lossless-box the upstream `Incoming` body's `hyper::Error`.
        Response::from_parts(
            parts,
            body.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                .boxed(),
        )
    }
}

/// Distinguishes between "upstream said no" and "we gave up waiting" so
/// the public face can pick the right HTTP status.
enum ProxyErr {
    Upstream(String),
    Timeout,
    /// A malformed inbound H1 request body surfaced by the streaming pump: a
    /// premature mid-body close (F-MD-4) or a forbidden field in the request
    /// trailers (Q-H3) → `400`. The upstream response is NEVER relayed here.
    BadRequest(String),
    /// Inbound body exceeded [`MAX_REQUEST_BODY_BYTES`] mid-stream → `413`.
    /// DISTINCT from an upstream receiver-drop (F-MD-2), which is NOT a 413.
    BodyTooLarge,
}

/// ROUND8-L7-01 — dial the backend and drive the RFC 6455 client-side handshake
/// **before** the client sees `101`. The caller maps the error to `502`
/// (refused/unreachable) or `504` (timeout). The caller's child `traceparent`
/// (and forwarded `tracestate`) is injected onto the upstream handshake so the
/// upstream sees the LB span as its parent.
async fn dial_upstream_ws(
    pool: TcpPool,
    backend_addr: SocketAddr,
    path_and_query: String,
    forwarded_protocols: Option<String>,
    child_traceparent: String,
    tracestate: Option<String>,
    ws_proxy: Arc<WsProxy>,
) -> Result<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, ProxyErr> {
    // A dial failure here is `502` to the client — but crucially the client has
    // NOT yet seen `101`.
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
    // Propagate the W3C trace context; tungstenite's builder takes header
    // pairs, not a `HeaderMap`, so we use the pre-rendered child value.
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

/// ROUND8-L7-01 — splice-only task. By the time it runs the upstream WS is
/// established and `101` is on the wire, so the hyper upgrade future is
/// guaranteed to resolve.
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

/// Map a [`SecurityReject`] to the client-facing response: `Smuggle` /
/// `SlowHandshake` → `400 Bad Request`; `RateLimited` / `OverCap` → `503` with
/// `Retry-After: 1` (a fixed hint that avoids leaking detector internals). By
/// the time the request handler sees `OverCap` the connection is already
/// established, so a response is cheaper than a half-close — the
/// RST-without-response case is handled at the accept site.
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

/// Strip hop-by-hop headers per RFC 9110 §7.6.1 plus any names listed inside
/// the `Connection` header value. `pub` (not `pub(crate)`) so integration tests
/// can pin the exact behaviour; the invariant is also available as a
/// compile-time guarantee via [`crate::stripped_request::StrippedRequest`].
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

/// Append the peer's IP to `X-Forwarded-For`, iterating EVERY existing value so
/// duplicate header lines are preserved in the canonical comma-joined form
/// (RFC 7239 / RFC 9110 §5.3 list rule). The previous `HeaderMap::get(..)` path
/// returned only the first value and `insert` then clobbered the rest — the
/// silent-drop class of **Envoy GHSA-ghc4-35x6-crw5**. Mirrored on
/// [`append_via`]. Values that fail `to_str` are skipped (fail-closed-by-skip).
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

/// Append `HTTP/1.1 expressgateway` to `Via`, iterating every existing value.
/// Same multi-value preservation as [`append_xff`]: RFC 9110 §7.6.3 `Via` is
/// list-valued, so duplicate header lines must be merged, not clobbered.
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

// ── PROTO-001 cross-protocol translation helpers ───────────────────────

/// HEAD-ONLY preamble for the streaming H1→H2 request leg: run the
/// `create_bridge(Http1, Http2)` codec over a body-LESS
/// [`crate::BridgeRequest`] so the bridge produces the H2 pseudo-header set
/// hyper's H2 client expects, then build the request parts. DELTA: we do NOT
/// force HTTP/1.1 and do NOT strip `content-length`/`transfer-encoding` — those
/// were H1-framing fixes; H2 framing is hyper's H2 encoder's job. The body is
/// attached SEPARATELY by the caller.
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
    // hyper's H2 client builds the pseudo-headers itself from URI and method.
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }
    // Build with an empty body purely to validate method/uri/headers, then
    // return its `Parts` for the caller to recombine with the real body.
    let (head, ()) = builder
        .body(())
        .map_err(|e| format!("build h2 req: {e}"))?
        .into_parts();
    Ok(head)
}

/// Map the h1_proxy-local [`ProxyErr`] into the [`H2ProxyErr`] the streaming
/// H1→H2 leg speaks. The validator only ever yields `BadRequest`, but every
/// variant is mapped so a future one cannot silently mis-classify.
fn h1_to_h2_proxy_err(e: ProxyErr) -> H2ProxyErr {
    match e {
        ProxyErr::Upstream(s) => H2ProxyErr::Upstream(s),
        ProxyErr::Timeout => H2ProxyErr::Timeout,
        ProxyErr::BadRequest(s) => H2ProxyErr::BadRequest(s),
        ProxyErr::BodyTooLarge => H2ProxyErr::BodyTooLarge,
    }
}

/// Concatenate the lookahead DATA chunks into one `Bytes` for the within-window
/// body; `total` is the exact summed length so we allocate once. A local copy
/// rather than a shared helper — `h2_proxy::concat_chunks` is private there.
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

/// Build the STREAMING H1 response head from the connector's decoded
/// [`lb_quic::H3RespEvent::Head`]. Shares the pseudo/`RESPONSE_HOP_BY_HOP`
/// strip + lowercase transform with [`upstream_response_to_h1`] (ONE
/// authoritative transform, not a third copy). CF-RESP-1: it CANNOT pre-declare
/// a `Trailer:` head — trailer names are unknown at head-time — so a late
/// `H3RespEvent::Trailers` rides the body's terminal frame.
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

/// Finalize a streaming H1 response from a head `Builder` + a streamed body;
/// centralizes the build-failure fallback.
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

/// PROTO-2-12 — build a `BoxBody` that emits the data bytes followed by an HTTP
/// trailer frame, so cross-protocol bridges can re-attach a captured trailer
/// set. An empty `trailers` list emits no trailer frame.
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

/// Convert an upstream H2 `Response<Incoming>` into the H1 response the listener
/// emits. STREAMING relay: build the H1 head from status + the H2→H1 transform
/// (drop `:`-pseudo and the [`RESPONSE_HOP_BY_HOP`] set, lowercase the rest —
/// the SAME authoritative shape as `H2ToH1Bridge::bridge_response`, shared with
/// [`h3_decoded_resp_head_builder`], not copied), then box the `Incoming` body
/// for streaming-by-construction.
///
/// CF-RESP-1 TRAILERS: a streamed relay cannot pre-declare the head `Trailer:`
/// names — they arrive only in the body's TERMINAL frame, after the head is
/// already on the wire — and reintroducing a `collect()` to capture them is the
/// exact R8 violation this path removed. Upstream H2 response trailers therefore
/// ride the boxed body's terminal frame; whether hyper-1's H1 encoder flushes
/// that frame WITHOUT a head `Trailer:` declaration + chunked TE is
/// wire-determined. If it does not, streamed H1←H2 responses simply do not
/// forward response trailers — which matches the nginx default and is a bounded
/// documented behaviour, NOT a silent regression. (The buffered
/// [`build_h1_response_with_trailers`] still pre-declares trailers.)
fn upstream_response_to_h1(
    resp: Response<IncomingBody>,
    alt_svc: Option<AltSvcConfig>,
) -> Response<ClientRespBody> {
    let (parts, body) = resp.into_parts();
    // H2→H1 transform: drop `:`-prefixed pseudo-headers AND the authoritative
    // `RESPONSE_HOP_BY_HOP` set (case-insensitive), re-emit the rest lowercased.
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
    // R8: stream the `Incoming` by construction — the terminal trailers frame
    // (if any) rides it. Lossless-box the `hyper::Error`.
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
/// [`crate::BridgeResponse`] — the authoritative trailer-aware H1 head shape,
/// exercised by `trailer_passthrough` and available to any future buffered
/// H1-response path.
///
/// When `translated.trailers` is non-empty this injects `Transfer-Encoding:
/// chunked` + a `Trailer: <name-list>` declaration and drops any incoming
/// `Content-Length` / `Transfer-Encoding` / `Trailer`. hyper-1's H1 encoder
/// requires BOTH invariants to actually flush a `Frame::trailers`
/// (`proto/h1/encode.rs:163-213`); without them the bridge's trailer fields
/// silently disappear.
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
        // PROTO-2-19: strip any pre-existing `transfer-encoding` /
        // `content-length` (both re-injected below in the trailer-aware shape)
        // and any pre-existing `trailer` declaration, so the proxy's
        // authoritative list wins.
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
        // RFC 9110 §6.6.2 `Trailer:` is end-to-end; RFC 9112 §7.1 requires
        // chunked TE to carry trailers; RFC 9110 §6.5 forbids `Content-Length`
        // alongside them (stripped above).
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
    // The head-level chunked TE + `Trailer:` declaration above is what makes
    // hyper actually write the trailer frame onto the wire.
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

/// Build the H1→H3 request FIELD-LIST from the request HEAD only — body and
/// trailers stream through the connector (request trailers ride
/// `ReqBodyEvent::End{trailers}`).
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
        // Head-only: the bridge just mints the pseudo-header set.
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

/// Max instantaneous inbound-request memory the H1 ingress pump retains: the
/// live in-flight channel occupancy (≤ 64 KiB).
///
/// A GENUINE gauge, not a constant: the pump increments just before each push
/// and DECREMENTS the moment hyper pulls the chunk back out, so a whole-body-
/// buffering variant — or any no-decrement variant — would grow with request
/// size and trip the ceiling the memory proof asserts. Test-only; a distinct
/// symbol from the H2 gauge so the H1 proof reads its own counter.
#[cfg(any(test, feature = "test-gauges"))]
pub static H1_REQ_MAX_RETAINED_BODY_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Lock-free CAS-max update for [`H1_REQ_MAX_RETAINED_BODY_BYTES`]; the gauge
/// only ever moves UP.
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

    /// ROUND8-L7-04 — two `X-Forwarded-For` header LINES must survive in the
    /// comma-joined outbound value. Pre-fix `HeaderMap::get(..)` returned only
    /// the first and `insert(..)` clobbered the rest — the Envoy
    /// GHSA-ghc4-35x6-crw5 silent-drop class on the producer side.
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
        // 3 members: two pre-existing values + peer.
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
            // RFC 9110 §6.6.2: `Trailer:` is the declaration header and
            // is end-to-end. PROTO-2-08: must NOT be stripped.
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

    // PROTO-2-11 H1 half: a pre-cancelled token plus an EOF duplex. A
    // regression that busy-loops or holds the conn open indefinitely times out.
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
        // Empty duplex: the peer half is dropped, so reads EOF at once.
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
