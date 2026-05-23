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
// S9 / M-D-lite — reuse the documented cross-cell 64 MiB request-body cap
// (Q-H4 parity) by IMPORT; do NOT redefine it (it is `pub` in h2_proxy and
// names H1→H1 explicitly as the gap this cell closes).
use crate::h2_proxy::MAX_REQUEST_BODY_BYTES;
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
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
/// These are stripped from BOTH request and response in addition to any
/// header names listed inside the `Connection` header value. Built as
/// `HeaderName` constants so removal is panic-free at runtime (the
/// strings are checked at compile time via `HeaderName::from_static`).
///
/// RFC 9110 §7.6.1 enumerates exactly these eight header field names as
/// connection-level (hop-by-hop) controls: `Connection`, `Proxy-Connection`,
/// `Keep-Alive`, `Proxy-Authenticate`, `Proxy-Authorization`, `TE`,
/// `Transfer-Encoding`, `Upgrade`. PROTO-2-08 removed the entry
/// `"trailers"` which appeared here in error: `Trailers` is **not** a
/// real header field name — it is only a value-token recognised inside
/// `TE: trailers` and inside the `Trailer:` (singular) declaration
/// header. RFC 9110 §6.6.2 specifies the actual `Trailer:` header (which
/// is end-to-end, not hop-by-hop). Adding `keep-alive` (which was
/// missing) brings the set in line with RFC 9110 §7.6.1.
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

/// S9 / M-D-lite (Q-H1) — depth of the bounded in-flight channel feeding the
/// streaming H1→H1 request body. Mirrors the M-D `H2_REQ_CHANNEL_DEPTH = 8`.
/// The fixed in-flight window = `H1_REQ_CHANNEL_DEPTH × H1_REQ_CHUNK_MAX`
/// (= 64 KiB) is the ceiling on retained inbound-request memory,
/// body-size-INDEPENDENT and independent of [`MAX_REQUEST_BODY_BYTES`]
/// (64 MiB) — that body-independence is the R8 property the memory proof
/// asserts. (Defined H1-locally rather than reusing the H2-named const so the
/// H1 path reads cleanly; CF-DEDUP-1 unifies the two pumps later.)
const H1_REQ_CHANNEL_DEPTH: usize = 8;

/// S9 / M-D-lite (Q-H1) — maximum size of one chunk pumped through the
/// in-flight channel. Mirrors the M-D `H2_REQ_CHUNK_MAX = 8 KiB`. Window
/// ceiling `H1_REQ_CHANNEL_DEPTH × H1_REQ_CHUNK_MAX = 64 KiB`.
const H1_REQ_CHUNK_MAX: usize = 8 * 1024;

/// S9 / M-D-lite (F-MD-4, H1 mirror of the M-D `PumpAbort`) — request-
/// smuggling fix. The Branch-B-only streaming pump feeds the inbound H1
/// request body to the upstream via a bounded `mpsc` channel bridged into an
/// `http_body` `StreamBody`. A dropped channel sender makes the receiver's
/// `poll_recv` return `None`, which `StreamBody` translates to a CLEAN body
/// EOF — hyper then emits the chunked terminator (`0\r\n\r\n`) and the
/// upstream sees a COMPLETE request. That is the WRONG signal when the inbound
/// stream was truncated mid-body (premature client TCP half-close = smuggling
/// primitive): a truncated request must NEVER be relayed as complete.
///
/// `hyper::Error` has no public constructor, so we cannot inject one into the
/// channel directly. This tiny error is the channel's error type instead: on
/// every inbound-error / over-cap / trailer-reject arm the pump SENDS
/// `Err(H1PumpAbort)` into the channel BEFORE signaling its verdict, so hyper
/// polls the body, sees an ERROR (not a clean EOF), and aborts the upstream
/// request WITHOUT a terminator. It satisfies hyper's request-body bound
/// `Body::Error: Into<Box<dyn std::error::Error + Send + Sync>>`.
///
/// This is a LOCAL mirror of `h2_proxy::PumpAbort` (Q-H1 chose
/// mirror-over-share to keep the BUILT M-D cell byte-unchanged; CF-DEDUP-1
/// unifies them later).
#[derive(Debug)]
struct H1PumpAbort;

impl std::fmt::Display for H1PumpAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("inbound H1 request body aborted before clean end-of-body")
    }
}

impl std::error::Error for H1PumpAbort {}

/// S9 / Q-H3 (H1 request-trailer validation) — reject framing/routing fields
/// in inbound H1 request trailers.
///
/// **Decoder fact (hyper 1.9.0, `proto/h1/decode.rs::decode_trailers`):** the
/// inbound H1 chunked-trailer decoder only validates trailer name/value
/// *syntax* and inserts EVERY field into the trailers `HeaderMap` — it does
/// NOT strip or reject framing/routing fields. So forwarding inbound trailers
/// verbatim (the R3 requirement that keeps `trailer_passthrough.rs` green)
/// would also forward a `Transfer-Encoding`/`Content-Length`/etc. carried in
/// the trailers, which RFC 7230 §4.1.2 / RFC 9110 §6.5.1 forbid because they
/// are a request-smuggling/desync primitive when relayed to the next hop.
///
/// The H1 analogue of M-D's pseudo-header-in-trailers reject (H1 has no
/// pseudo-headers): trailers MUST NOT carry message-framing or routing fields.
/// We reject `Transfer-Encoding`, `Content-Length`, `Host`, `Trailer`, `TE`,
/// `Connection`, plus any [`HOP_BY_HOP`] name. A legitimate trailer (e.g.
/// `x-checksum`, `grpc-status`) passes through untouched and is forwarded
/// byte-faithfully by the pump.
fn validate_h1_request_trailers(trailers: &hyper::HeaderMap) -> Result<(), ProxyErr> {
    use hyper::header::{CONNECTION, CONTENT_LENGTH, HOST, TE, TRAILER, TRANSFER_ENCODING};
    for name in trailers.keys() {
        // Framing/routing fields explicitly forbidden in trailers
        // (RFC 7230 §4.1.2): they alter message framing/routing if relayed.
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

/// ROUND8-L7-05 — runtime-side policy for `_` in inbound HTTP header
/// names. Mirrors `lb_config::HeaderUnderscorePolicy`; lives in lb-l7
/// to avoid a wide dep edge from the proxy onto the config crate. The
/// wiring crate (`lb` binary) is responsible for mapping the schema
/// enum to this enum at proxy-construction time.
///
/// Default: [`HeaderUnderscorePolicy::Reject`] — matches Envoy edge
/// best-practice (`headers_with_underscores_action = REJECT_REQUEST`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeaderUnderscorePolicy {
    /// Reject the request with `400 Bad Request` if any inbound
    /// header name contains `_`. Default.
    #[default]
    Reject,
    /// Silently strip underscore-bearing headers before forwarding;
    /// matches nginx default.
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
    /// CODE-2-01 / SEC-2-01 hook surface. Defaults to
    /// [`NoopHooks`]; Wave-2c flips this to the production
    /// `lb_security::HooksBundle` via [`Self::with_hooks`].
    hooks: Arc<dyn DynSecurityHooks>,
    /// SEC-2-01 / SEC-2-03 slowloris / slow-POST watchdog.
    ///
    /// `None` (default) leaves the request handler running with no
    /// per-stream deadline tracking; the proxy still relies on
    /// [`HttpTimeouts::header`] / [`HttpTimeouts::body`] /
    /// [`HttpTimeouts::total`]. Wave-2c sets a production
    /// [`Watchdog`] via [`Self::with_watchdog`].
    watchdog: Option<Watchdog>,
    /// Monotonic per-listener connection sequence used as the
    /// [`Watchdog`] entry key (combined with the peer IP so two
    /// concurrent NAT-egress connections are distinguishable).
    conn_seq: Arc<parking_lot::Mutex<u64>>,
    /// SEC-2-01 strict-TE policy flag. When `true` the per-request
    /// [`SmuggleDetector::check_all_mode`] call in `handle` runs in
    /// [`SmuggleMode::H1Strict`] mode (rejects any
    /// `Transfer-Encoding` codec other than `chunked`). Default
    /// `false` keeps the lenient RFC 9112 baseline.
    smuggle_strict: bool,
    /// ROUND8-L7-05: policy for `_` in inbound HTTP header names.
    /// Default [`HeaderUnderscorePolicy::Reject`] mirrors Envoy edge
    /// best-practice + nginx default behaviour. Set via
    /// [`Self::with_header_underscore_policy`]. The lb-config
    /// schema's `[runtime].header_underscore_policy` carries the
    /// operator-facing knob; mapping into this field is the
    /// responsibility of the wiring crate.
    header_underscore_policy: HeaderUnderscorePolicy,
    /// PROTO-2-18 (Wave 2c-2): default expected SNI for the
    /// [`crate::sni_authority::check_sni_authority`] check. Builder
    /// default `None` means SNI/authority agreement is not enforced
    /// on this proxy unless [`Self::serve_connection_with_cancel_sni`]
    /// supplies a per-connection override. Real deployments call
    /// [`Self::serve_connection_with_cancel_sni`] at the TLS-accept
    /// site to pass `rustls::ServerConnection::server_name()`.
    expected_sni: Option<String>,
    /// ROUND8-L7-06: hard cap on requests served per keep-alive
    /// connection (nginx `keepalive_requests 100` / Pingora 0.8.0
    /// `keepalive_requests`). `0` disables. On the `cap`-th response
    /// the per-connection wrapper sets `Connection: close` and signals
    /// the connection driver to `graceful_shutdown` after the body
    /// flushes. Default `100`; set via
    /// [`Self::with_max_keepalive_requests`].
    max_keepalive_requests: u32,
    /// ROUND8-L7-06: process-wide counter incremented once per
    /// cap-triggered keep-alive close. The wiring crate lifts this
    /// into `lb_keepalive_terminated_by_count_cap_total{listener,
    /// protocol}` via the existing metrics sampler (lb-l7 has no
    /// metrics-registry dep edge; an `AtomicU64` keeps the surface
    /// minimal).
    keepalive_cap_terminations: Arc<std::sync::atomic::AtomicU64>,
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
            hooks: Arc::new(NoopHooks::new()),
            watchdog: None,
            conn_seq: Arc::new(parking_lot::Mutex::new(0)),
            smuggle_strict: false,
            header_underscore_policy: HeaderUnderscorePolicy::Reject,
            expected_sni: None,
            // ROUND8-L7-06: nginx-parity default. The wiring crate
            // overrides from `[runtime].max_keepalive_requests` via
            // `with_max_keepalive_requests`.
            max_keepalive_requests: 100,
            keepalive_cap_terminations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Construct an [`H1Proxy`] backed by a multi-protocol picker.
    ///
    /// The picker may return `H1`, `H2`, or `H3` backends per call;
    /// the proxy branches on `UpstreamProto` and dials via the matching
    /// pool. Call sites must populate `h2_upstream` / `h3_upstream`
    /// when their picker can return that protocol — a pick whose
    /// matching pool is `None` falls back to a 502 response.
    ///
    /// Defaults the CODE-2-01 `hooks` field to
    /// [`NoopHooks`]; Wave-2c overrides via [`Self::with_hooks`]. The
    /// constructor is no longer `const fn` because the default
    /// [`NoopHooks`] allocates an [`Arc`].
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
            // ROUND8-L7-06: nginx-parity default. The wiring crate
            // overrides from `[runtime].max_keepalive_requests` via
            // `with_max_keepalive_requests`.
            max_keepalive_requests: 100,
            keepalive_cap_terminations: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Attach a [`SecurityHooks`] impl (CODE-2-01 / SEC-2-01 hot-path
    /// surface). The Wave-2c wiring in `crates/lb/src/main.rs` calls
    /// this with `Arc::new(lb_security::HooksBundle::new(...))` so the
    /// production smuggle / cap / watchdog checks run on every
    /// inbound request. Without this call, the proxy falls back to
    /// [`NoopHooks`] (CODE-2-01 lb-l7 shim default).
    #[must_use]
    pub fn with_hooks(mut self, hooks: Arc<dyn DynSecurityHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Attach an [`lb_security::Watchdog`] for per-stream slowloris /
    /// slow-POST eviction (SEC-2-01 / SEC-2-03). The proxy registers
    /// each inbound request with a deadline derived from the
    /// configured [`HttpTimeouts::header`] and records a single
    /// `progress` measurement with the request-header byte estimate.
    /// Per-chunk body progress is a follow-up (requires IO wrapping);
    /// the request-side register covers the slowloris header phase
    /// gap. A periodic sweeper task driven by `lb_core::Shutdown`
    /// (Wave-2c) closes the full slow-POST window.
    #[must_use]
    pub fn with_watchdog(mut self, watchdog: Watchdog) -> Self {
        self.watchdog = Some(watchdog);
        self
    }

    /// Enable strict-TE policy on the per-request
    /// [`SmuggleDetector::check_all_mode`] call (rejects any
    /// `Transfer-Encoding` codec other than `chunked` —
    /// [`SmuggleMode::H1Strict`]). Default is lenient.
    #[must_use]
    pub const fn with_smuggle_strict(mut self, strict: bool) -> Self {
        self.smuggle_strict = strict;
        self
    }

    /// ROUND8-L7-05: set the header-name underscore policy on this
    /// proxy. Default is [`HeaderUnderscorePolicy::Reject`] (Envoy
    /// edge best-practice). The wiring crate maps from
    /// `lb_config::HeaderUnderscorePolicy` to this enum at proxy
    /// construction time.
    #[must_use]
    pub const fn with_header_underscore_policy(mut self, policy: HeaderUnderscorePolicy) -> Self {
        self.header_underscore_policy = policy;
        self
    }

    /// PROTO-2-18 (Wave 2c-2): default expected SNI used by the
    /// [`crate::sni_authority::check_sni_authority`] hot-path check
    /// when [`Self::serve_connection`] is used directly (no per-
    /// connection SNI threaded in). Real TLS-bearing deployments
    /// prefer [`Self::serve_connection_with_cancel_sni`] which
    /// captures the SNI live from rustls at TLS-accept time.
    #[must_use]
    pub fn with_expected_sni(mut self, sni: Option<String>) -> Self {
        self.expected_sni = sni;
        self
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

    /// Whether an H2 upstream pool has been wired for this proxy.
    /// Exposed for integration tests asserting the binary-wiring path
    /// constructs a multi-protocol proxy correctly.
    #[must_use]
    pub const fn has_h2_upstream(&self) -> bool {
        self.h2_upstream.is_some()
    }

    /// Whether an H3 upstream pool has been wired for this proxy.
    /// Exposed for integration tests.
    #[must_use]
    pub const fn has_h3_upstream(&self) -> bool {
        self.h3_upstream.is_some()
    }

    /// ROUND8-L7-06: set the per-keep-alive-connection request cap.
    /// `0` disables (transparent-pass — only the wall-clock / idle
    /// timeouts apply). The wiring crate maps
    /// `[runtime].max_keepalive_requests` here.
    #[must_use]
    pub fn with_max_keepalive_requests(mut self, cap: u32) -> Self {
        self.max_keepalive_requests = cap;
        self
    }

    /// ROUND8-L7-06: shared handle to the cap-triggered-close counter
    /// so the wiring crate can lift it into
    /// `lb_keepalive_terminated_by_count_cap_total` without an
    /// lb-l7 → metrics-registry dep edge.
    #[must_use]
    pub fn keepalive_cap_termination_counter(&self) -> Arc<std::sync::atomic::AtomicU64> {
        Arc::clone(&self.keepalive_cap_terminations)
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
        // PROTO-2-11 (H1 half, Wave 2c-2): always delegates to the
        // cancellable variant with a never-cancelled token so the
        // original signature stays back-compat.
        self.serve_connection_with_cancel(io, peer, tokio_util::sync::CancellationToken::new())
            .await
    }

    /// PROTO-2-11 (Wave 2c-2) — H1 half of the drain-on-cancel contract
    /// paired with the H2 GOAWAY + H3 CONNECTION_CLOSE emit. Identical
    /// to [`Self::serve_connection`] until `cancel` fires, at which
    /// point the hyper H1 connection is pinned and
    /// `.graceful_shutdown()` is invoked.
    ///
    /// PROTO-2-16: hyper-1's `http1::graceful_shutdown` calls
    /// `disable_keep_alive()`; the `Connection: close` header is
    /// injected only on a not-yet-flushed response head (the H1
    /// encoder serialises `Connection: close` when the keep-alive
    /// state is `Disabled` and the response head has not yet been
    /// written). If the cancel fires *after* the current response
    /// head has already been flushed, the only close signal the
    /// client receives is the FIN at body completion — the header
    /// is not retroactively added. RFC 9110 §7.6.1 permits this
    /// (the FIN is the truth-signal; the header is advisory). The
    /// connection future is then driven to completion with the
    /// existing `total` budget so a misbehaving client cannot pin
    /// a worker past the drain deadline.
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
        // PROTO-2-18: no per-connection SNI override; the proxy's
        // builder-level `expected_sni` (commonly `None`) is the
        // effective value. Real TLS deployments use
        // `serve_connection_with_cancel_sni`.
        let sni = self.expected_sni.clone();
        self.serve_connection_with_cancel_sni(io, peer, cancel, sni)
            .await
    }

    /// PROTO-2-18 (Wave 2c-2) — H1 entry point that threads the
    /// per-connection TLS SNI value into the request hot-path so the
    /// [`crate::sni_authority::check_sni_authority`] validator runs
    /// against the **observed** SNI rather than the builder default.
    ///
    /// `sni` is the value captured from the rustls handshake (commonly
    /// `tls_stream.get_ref().1.server_name().map(str::to_owned)`).
    /// Plain-TCP listeners and SNI-omitting clients pass `None`, which
    /// disables the SNI/authority agreement check (the validator returns
    /// `Ok(())` on `None`).
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
        // ROUND8-L7-06: per-connection request counter + close-notify,
        // constructed once here and shared across hyper's per-request
        // service clones.
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
        let conn = hyper::server::conn::http1::Builder::new()
            .keep_alive(true)
            .serve_connection(TokioIo::new(io), svc)
            .with_upgrades();
        tokio::pin!(conn);
        let cancel_fut = cancel.cancelled();
        tokio::pin!(cancel_fut);
        let timer = tokio::time::sleep(total);
        tokio::pin!(timer);
        // ROUND8-L7-06: cap-driven close. Additive arm — does NOT
        // touch div-ops's SIGTERM-cancel / total-timeout arms (the
        // drain-coordinator phase logic). When the per-connection cap
        // is hit the service has already set `Connection: close` on
        // the cap-th response head; this arm then drives the same
        // `graceful_shutdown` hyper uses for SIGTERM so the socket is
        // torn down after that response flushes (RFC 9110 §7.6.1).
        let cap_close = close_signal.notified();
        tokio::pin!(cap_close);
        tokio::select! {
            // Cancel wins ties so a SIGTERM during a long-running
            // request still triggers the graceful_shutdown emit.
            biased;
            () = &mut cancel_fut => {
                // PROTO-2-16: hyper-1's `http1::graceful_shutdown`
                // internally calls `disable_keep_alive()` — the
                // `Connection: close` header is then injected only
                // on a not-yet-flushed response head (the encoder
                // serialises it when keep-alive is `Disabled` and
                // the head is still pending). For responses already
                // on the wire, only the FIN at body completion
                // signals close (RFC 9110 §7.6.1 permits this). The
                // conn future is driven to completion bounded by
                // `total`.
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
            // ROUND8-L7-06: per-connection request cap reached. The
            // cap-th response already carries `Connection: close`;
            // drive the same graceful_shutdown the SIGTERM arm uses so
            // the socket FIN follows the flushed response. Bounded by
            // `total` so a wedged client cannot pin the conn open
            // after the cap. A clean completion here is `Ok(())` (the
            // cap close is the *intended* terminal state, not an
            // error — unlike the total-timeout arm).
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

/// Service implementation carrying the [`H1Proxy`] plus the peer address.
///
/// ROUND8-L7-06: the service is cloned by hyper once per request but
/// the *connection*-scoped state (`served`, `cap`, `close_signal`)
/// lives behind `Arc`s constructed once per connection in
/// [`H1Proxy::serve_connection_with_cancel_sni`], so every per-request
/// clone shares one counter and one close-notify.
#[derive(Clone)]
struct ProxyService {
    inner: Arc<H1Proxy>,
    peer: SocketAddr,
    /// PROTO-2-18: SNI captured from the TLS handshake at accept time
    /// and threaded through `serve_connection_with_cancel_sni`. `None`
    /// on plain-TCP listeners and SNI-omitting clients.
    expected_sni: Option<String>,
    /// ROUND8-L7-06: per-connection request counter (shared across
    /// the per-request `Clone`s).
    served: Arc<std::sync::atomic::AtomicU32>,
    /// ROUND8-L7-06: per-connection request cap (`0` disables).
    cap: u32,
    /// ROUND8-L7-06: notified once when the cap is reached so the
    /// connection driver issues `graceful_shutdown` after the
    /// cap-th response flushes.
    close_signal: Arc<tokio::sync::Notify>,
}

impl hyper::service::Service<Request<IncomingBody>> for ProxyService {
    type Response = Response<BoxBody<Bytes, hyper::Error>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<IncomingBody>) -> Self::Future {
        let inner = Arc::clone(&self.inner);
        let peer = self.peer;
        let sni = self.expected_sni.clone();
        // ROUND8-L7-06: count this request against the per-connection
        // cap BEFORE handling it. `fetch_add` returns the prior value
        // so `count` is 1-based for this request.
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
                // RFC 9110 §7.6.1: advertise the close on the response
                // head. hyper still serialises the body; the driver's
                // `graceful_shutdown` (signalled below) tears the
                // socket down after the flush. `count == cap` fires
                // exactly once (later requests on a
                // disable-keep-alive'd conn do not reach here).
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
    /// Span-opening wrapper. Extracts the inbound W3C trace context,
    /// opens the request span, and runs the request body
    /// `.instrument()`-ed so the span is current for every `.await`
    /// in the handler *and* for any task the handler spawns with
    /// `.instrument(req_trace.span.clone())` — without holding a
    /// `tracing::span::Entered` guard across an `.await` (that
    /// anti-pattern leaks the span onto whatever the executor polls
    /// next on the same thread; it bites only under concurrent load,
    /// which is exactly the "lesson-not-yet-paid-for" class).
    ///
    /// ROUND8-OPS-06 / REL-2-07: this is the FIRST L7 callsite of
    /// `lb_observability::tracing_propagation`. The codec shipped in
    /// 1d462c7 but the proxy wire-in was deferred every round since,
    /// leaving REL-2-07 stuck at `Verified-Fixed-Partial`.
    async fn handle(
        &self,
        req: Request<IncomingBody>,
        peer: SocketAddr,
        expected_sni: Option<&str>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        use tracing::Instrument;
        // H1Proxy carries no per-bind label and the constructor
        // boundary into main.rs is div-ops's house, so the span's
        // `listener` field is the protocol family (h1/h1s).
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
        // Record the response status on the request span (OTLP
        // `http.status_code`) before it closes.
        span.record("http.status_code", resp.status().as_u16());
        resp
    }

    async fn handle_inner(
        &self,
        req: Request<IncomingBody>,
        peer: SocketAddr,
        expected_sni: Option<&str>,
        req_trace: crate::trace_ctx::RequestTrace,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        // ROUND8-L7-09 — uniform authority validation CHOKE POINT.
        // HAProxy `BUG/MAJOR: http: forbid comma character in
        // authority value` + `BUG/MEDIUM: h1: Enforce the authority
        // validation during H1 request parsing` (the H1 parser was
        // missing the check the H2/H3 path had). This MUST run on
        // EVERY parser path BEFORE the request forks into the
        // WebSocket-upgrade handler / picker — a comma / whitespace /
        // control byte in `Host` (or an absolute-form target) is a
        // routing/ACL-desync primitive. Placed as the FIRST statement
        // so the WS-upgrade fork below cannot bypass it (the prior
        // verify pass found `handle_ws_upgrade` reached `pick_info()`
        // unvalidated — `audit/round-8/verify/fixback.md`).
        if let Err((bad, err)) = crate::authority::validate_request(&req) {
            tracing::warn!(
                peer = %peer,
                authority = %bad,
                error = ?err,
                "ROUND8-L7-09: H1 authority rejected (choke point)"
            );
            return error_response(StatusCode::BAD_REQUEST, "invalid authority (ROUND8-L7-09)");
        }

        // gRPC requires HTTP/2 (RFC: gRPC over HTTP/2 §3.4 — gRPC PROTOCOL
        // section). An H1 listener cannot serve gRPC: framing relies on H2
        // streams, trailers, and HEADERS continuation. Reject early with
        // 415 so a misconfigured client gets a clear actionable signal
        // rather than 502 from a downstream H1 backend that wouldn't know
        // what to do with `application/grpc` either.
        //
        // Match exactly `application/grpc` or `application/grpc+<sub>`
        // (e.g. `application/grpc+proto`, `application/grpc+json`) on a
        // case-insensitive media-type token (RFC 7231 §3.1.1.1). Strip any
        // `;`-prefixed parameters (`charset=utf-8`, etc.) before matching.
        // The trailing `+` keeps `application/grpc-web` (hyphen) — which
        // is plain HTTP and forwards transparently — outside the reject.
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

        // WebSocket upgrade intercept (RFC 6455 §4). Only fires when the
        // listener is configured with a `WsProxy`; all other listener
        // traffic continues through the regular H1 request path.
        if self
            .ws
            .as_ref()
            .is_some_and(|w| w.config().enabled && is_h1_upgrade_request(&req))
        {
            return self.handle_ws_upgrade(req, req_trace).await;
        }

        let (mut parts, body) = req.into_parts();

        // ROUND8-L7-05: enforce header-name underscore policy before
        // any other inspection. Envoy edge best-practice + nginx
        // default; default mode is `Reject`. See
        // `audit/round-8/findings/ROUND8-L7-05.md` and
        // `docs/edge-defaults.md`.
        //
        // SEC-2-01 defence-in-depth: when smuggle mode is strict
        // (`smuggle_strict = true`), the policy is forced to `Reject`
        // regardless of operator configuration. Operators who
        // deliberately opt out of underscore rejection must also opt
        // out of strict-TE mode.
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

        // CODE-2-01 / SEC-2-01: run the security hooks before hop-by-hop
        // strip + upstream-acquire so a rejected request never spends a
        // pool slot. The reconstructed `Request<()>` is a header-only
        // borrow surface — the trait reads `headers()` + `version()`,
        // it does not consume the body.
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

        // ROUND8-L7-09 authority validation now runs at the
        // `handle_inner` choke point (above the WS-upgrade fork) via
        // `crate::authority::validate_request`, so it covers the
        // upgrade path too. No second call needed here.

        // PROTO-2-18 (Wave 2c-2) — SNI ↔ Host agreement (RFC 9110
        // §15.5.20). H1 carries the authority in the `Host` header
        // (RFC 9112 §3.2). The validator returns `Ok(())` when SNI
        // is absent (plain TCP / SNI-omitting client) or when the
        // `Host` header is missing/empty — PROTO-2-01 covers those
        // gates upstream. Per the sec-r5 caveat, loopback peers
        // (typically health-checkers and same-host curl users
        // hitting `127.0.0.1`) skip enforcement: SNI-vs-Host
        // confusion is a Layer-7 routing/authz attack vector that
        // doesn't apply to the loopback path, and the operator's
        // probe scripts often use IP-literal Host headers that
        // don't match the cert's SNI.
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

        // SEC-2-01 — defense-in-depth explicit `SmuggleDetector` call.
        // The hook surface above already invokes the detector when
        // wired with `HooksBundle`; this call site fires regardless of
        // which `DynSecurityHooks` impl is in use (`NoopHooks`
        // included) so the detector is no longer dead code on the
        // proxy hot path even before Wave-2c flips the production
        // bundle in. The `(name, value)` pair shape matches the
        // detector's existing API.
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

        // SEC-2-01 / SEC-2-03 — register the request with the slowloris
        // watchdog. The deadline is `now + HttpTimeouts::header`; if
        // the per-request future overruns it, the watchdog evicts via
        // `progress` (or the sweeper) and the next progress call
        // returns `WatchdogError::Deadline`.
        let watch_id = self.watchdog.as_ref().map(|wd| {
            let seq = {
                let mut g = self.conn_seq.lock();
                *g = g.wrapping_add(1);
                *g
            };
            let id = ConnId::new(peer.ip(), seq);
            let deadline = std::time::Instant::now() + self.timeouts.header;
            wd.register(id, deadline);
            // Account the header bytes (approximate) as the initial
            // progress checkpoint. The detector treats progress as
            // cumulative bytes-read; a zero-byte baseline is fine if
            // the estimate is unavailable.
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

        // PROTO-2-07 — run the strip via the `StrippedRequest` newtype
        // factory so the downstream proxy_* methods consume a type that
        // statically guarantees hop-by-hop has been removed. Other
        // request-mutating helpers (`append_xff`, `set_xfp`, `set_xfh`,
        // `append_via`) operate on the underlying header map via the
        // newtype's borrow surface.
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
                // S9 / M-D-lite — a malformed inbound H1 request body
                // (F-MD-4 truncation or Q-H3 forbidden trailer) is the
                // CLIENT's fault → 400, never the backend's response.
                Err(ProxyErr::BadRequest(s)) => error_response(StatusCode::BAD_REQUEST, &s),
                // S9 / Q-H4 — inbound body over the 64 MiB cap → 413.
                Err(ProxyErr::BodyTooLarge) => error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body exceeds maximum allowed size",
                ),
            },
            UpstreamProto::H2 => Box::pin(self.proxy_h1_to_h2(backend.addr, stripped)).await,
            UpstreamProto::H3 => Box::pin(self.proxy_h1_to_h3(&backend, stripped)).await,
        };
        // SEC-2-01 / SEC-2-03 — deregister the request from the
        // watchdog on the normal completion path. The sweeper covers
        // the abandoned-future case.
        if let (Some(wd), Some(id)) = (self.watchdog.as_ref(), watch_id) {
            wd.deregister(id);
        }
        resp
    }

    /// Forward an H1 inbound request to an H1 upstream backend over a
    /// single-use TCP stream.
    ///
    /// **ROUND8-L7-10 — take-and-discard upstream stream pattern.**
    /// The line below calls `pooled.take_stream()` immediately after
    /// the async `acquire_async` resolves. `take_stream` consumes the
    /// `PooledTcp` wrapper without invoking its `Drop` return-to-pool
    /// path, so the H1 upstream connection is effectively **single-use**:
    /// even though the pool has a `set_reusable` API
    /// ([`lb_io::pool::PooledTcp::set_reusable`]), this code path
    /// never reuses an H1 upstream socket.
    ///
    /// This is *correct by accident*. Pingora paid for this bug twice
    /// (0.6.0 "Discard extra upstream body and disable keepalive" plus
    /// 0.8.0 "Ensure http1 downstream session is not reused on more
    /// body bytes than expected"): an upstream that sends fewer or
    /// more body bytes than its declared Content-Length corrupts the
    /// next pipelined request on a reused connection — an upstream
    /// request-smuggling primitive. Single-use stops that class cold.
    ///
    /// **Refactor warning.** Any future change that pools H1 upstream
    /// connections (drops `take_stream()` in favour of letting
    /// `PooledTcp` drop normally) MUST implement the Pingora-class
    /// over-read / under-read guard: read the response body, compare
    /// to `Content-Length`, and call
    /// [`lb_io::pool::PooledTcp::set_reusable(false)`](lb_io::pool::PooledTcp::set_reusable)
    /// on any mismatch before letting the wrapper drop. Without that
    /// guard the bug Pingora paid for twice will return. See also the
    /// H2 cousin in `Http2Pool::send_request`, which evicts on every
    /// Send-class error for the same reason.
    async fn proxy_request(
        &self,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Result<Response<IncomingBody>, ProxyErr> {
        use hyper::body::Frame;

        let req = req.into_inner();
        let (mut parts, mut body) = req.into_parts();

        // F-MD-1 (carried forward from M-D) — let hyper's HTTP/1.1 encoder
        // choose the framing for the unknown-length streaming body we hand it.
        //  • Force the request version to HTTP/1.1 (the upstream protocol).
        //  • STRIP `content-length` + `transfer-encoding`: an inbound H1
        //    request CAN carry a `content-length`, and a stale CL alongside an
        //    unknown-length `StreamBody` mis-frames identically to F-MD-1 (the
        //    encoder either truncates to the stale CL or emits an empty body
        //    and never polls our pump). With both stripped, hyper frames the
        //    streaming body as `Transfer-Encoding: chunked` itself.
        // (Header-level CL/TE smuggling was already rejected pre-pump by
        // `SmuggleDetector::check_all_mode` in `handle`; this strip is the
        // framing-correctness step, not a security check.)
        parts.version = hyper::Version::HTTP_11;
        parts.headers.remove(hyper::header::CONTENT_LENGTH);
        parts.headers.remove(hyper::header::TRANSFER_ENCODING);

        // CODE-2-09 follow-on: async dial via `TcpPool::acquire_async`.
        // The pool's `PoolConfig::connect_timeout` (5 s default,
        // sourced from `runtime.connect_timeout_ms`) bounds the syscall.
        //
        // S9 / M-D-lite — Branch-B-only (no lookahead): H1 ingress has no
        // HPACK / no H2 framing / no validate-before-forward ordering
        // requirement, so we forward-as-it-arrives. We dial first (as the
        // pre-S9 code did) and stream the inbound body through a bounded
        // in-flight window — NO whole-body buffering, NO `collect()`.
        let pooled = self
            .pool
            .acquire_async(backend_addr)
            .await
            .map_err(|e| ProxyErr::Upstream(format!("backend connect {backend_addr}: {e}")))?;

        // ROUND8-L7-10: see the doc-comment block on this function.
        // `take_stream` defeats the pool's return-to-pool Drop path,
        // making this H1 upstream connection single-use. Do not
        // remove without first implementing the body-length guard
        // documented above.
        let stream = pooled
            .take_stream()
            .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;

        // F-MD-4: the upstream request body is a `StreamBody` whose error type
        // is the constructible `H1PumpAbort` (not `hyper::Error`, which has no
        // public constructor) so the pump can INJECT a body error on the
        // truncation/abort path instead of silently dropping the channel (a
        // drop = clean EOF = smuggled-complete request).
        let (mut sender, conn) = hyper::client::conn::http1::handshake::<
            _,
            BoxBody<Bytes, H1PumpAbort>,
        >(TokioIo::new(stream))
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

        // Bounded in-flight channel (depth = H1_REQ_CHANNEL_DEPTH). When the
        // backend write stalls, hyper stops pulling → the channel fills → the
        // pump stops polling the inbound body → the frontend H1 read window
        // stalls → the client send is paused (the R8 backpressure chain).
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, H1PumpAbort>>(H1_REQ_CHANNEL_DEPTH);

        // F-MD-3 (gauge wiring lands in I2) — track ACTUAL instantaneous
        // in-flight channel occupancy: incremented by the pump just before it
        // pushes a chunk and DECREMENTED in the body's poll the moment hyper
        // pulls that chunk back out. (The `record_retained_h1` call sites are
        // added in I2; the counter itself is load-bearing for them.)
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight_body = std::sync::Arc::clone(&in_flight_bytes);

        // Bridge the mpsc receiver into an `http_body` stream body (futures-util
        // is already a dependency). As hyper pulls each frame we decrement the
        // live in-flight counter (the chunk has left our retained set and is
        // now owned by hyper's write buffer).
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

        // The pump owns the inbound body. It reports its terminal verdict via a
        // oneshot so the response-head relay is gated on a VALIDATED terminal
        // state (clean end-of-body, or a surfaced error mapped to 400/413).
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), ProxyErr>>();
        let pump = tokio::spawn(async move {
            // Running cumulative total of forwarded request bytes — the Q-H4
            // total-body cap (`MAX_REQUEST_BODY_BYTES`, 64 MiB) applies in the
            // streaming regime exactly as it does on the H2→H1 path.
            let mut forwarded_total: usize = 0;

            // `ReceiverGone` = hyper dropped the request body (the backend
            // early-responded WITHOUT reading the body). On `ReceiverGone` we
            // MUST NOT manufacture a 413 (F-MD-2); we switch to
            // drain-and-validate so the backend's real response is relayed once
            // the inbound body validates.
            enum SendOutcome {
                ReceiverGone,
            }

            // Split a DATA payload into ≤ H1_REQ_CHUNK_MAX pieces and push each
            // through the bounded channel (the backpressure point). Increments
            // the live in-flight gauge before each push. Returns
            // Err(ReceiverGone) if the receiver (hyper body) dropped.
            macro_rules! send_chunked {
                ($bytes:expr) => {{
                    let mut data: Bytes = $bytes;
                    let mut outcome: Result<(), SendOutcome> = Ok(());
                    while !data.is_empty() {
                        let take = data.len().min(H1_REQ_CHUNK_MAX);
                        let chunk = data.split_to(take);
                        let clen = chunk.len();
                        // This chunk is about to enter the channel: it joins the
                        // live in-flight set.
                        in_flight_bytes.fetch_add(clen, std::sync::atomic::Ordering::Relaxed);
                        // F-MD-3: record the ACTUAL retained inbound set = the
                        // live in-flight channel occupancy (the decrement
                        // happens in the StreamBody poll when hyper pulls).
                        #[cfg(any(test, feature = "test-gauges"))]
                        record_retained_h1(
                            in_flight_bytes.load(std::sync::atomic::Ordering::Relaxed),
                        );
                        if tx.send(Ok(Frame::data(chunk))).await.is_err() {
                            // hyper dropped the receiver before accepting this
                            // chunk → it never entered hyper's buffer; back the
                            // counter out so the gauge stays honest.
                            in_flight_bytes.fetch_sub(clen, std::sync::atomic::Ordering::Relaxed);
                            outcome = Err(SendOutcome::ReceiverGone);
                            break;
                        }
                    }
                    outcome
                }};
            }

            // drain-and-validate (F-MD-2): the backend stopped reading the
            // request body (early/short response). We can no longer forward,
            // but we MUST still drive the inbound body to a validated terminal
            // state so a malformed/truncated request never relays the backend's
            // response. Bytes are DISCARDED (memory stays bounded — one frame
            // at a time), but the 64 MiB cap and the trailer guard still apply.
            macro_rules! drain_and_validate {
                () => {{
                    loop {
                        match body.frame().await {
                            // F-MD-4 (H1): clean end-of-body. For an H1 inbound
                            // body `frame()==None` is the POSITIVELY-confirmed
                            // clean end (see the streaming-loop comment); a
                            // truncation surfaces as `Some(Err)` instead.
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
                        // F-MD-4 (H1 — MIRROR-IMAGE of the M-D / H2 case;
                        // do NOT copy the H2 `is_end_stream` logic here).
                        //
                        // The inbound H1 server body is hyper's `Kind::Chan`
                        // (a channel fed by the H1 connection driver), NOT
                        // `Kind::H2`. For H1, `frame()==None` is the
                        // POSITIVELY-confirmed clean end-of-body: the chunked
                        // decoder reached the real `0\r\n\r\n` terminator (or a
                        // Content-Length body was fully satisfied) and the
                        // driver dropped the body sender → the channel yields
                        // `None`. A PREMATURE mid-body TCP half-close does NOT
                        // reach here as `None`: hyper-1.9.0's decoder emits
                        // `IncompleteBody` (UnexpectedEof) on early EOF for BOTH
                        // chunked (`decode.rs` ~L162) and Content-Length
                        // (~L504) framings, which the driver pushes into the
                        // body channel as `Some(Err(..))` — handled in the
                        // `Some(Err)` arm below. (Request bodies are never
                        // close-delimited: a request with neither CL nor TE has
                        // no body, so there is no "EOF == clean end" framing on
                        // the request path. The verifier proves this on the wire
                        // for BOTH CL and chunked premature closes → complete=0.)
                        //
                        // We deliberately do NOT consult `Body::is_end_stream()`
                        // for H1: for `Kind::Chan` it returns
                        // `content_length == ZERO`, which is unreliable for
                        // chunked bodies (CHUNKED is never decremented to ZERO).
                        // `None`-after-no-error is the correct, sufficient
                        // clean-end signal here.
                        //
                        // Clean end → drop `tx` → the StreamBody yields `None`
                        // → hyper writes the chunked terminator → the upstream
                        // sees a COMPLETE request.
                        let _ = verdict_tx.send(Ok(()));
                        return;
                    }
                    Some(Ok(frame)) => {
                        if frame.is_trailers() {
                            // Q-H3: validate trailers BEFORE forwarding them. A
                            // framing/routing field in trailers is a desync
                            // primitive (hyper's inbound decoder does NOT reject
                            // it — see `validate_h1_request_trailers`).
                            let verdict = frame
                                .trailers_ref()
                                .map_or(Ok(()), validate_h1_request_trailers);
                            match verdict {
                                Ok(()) => {
                                    // Forward the legitimate trailers frame
                                    // byte-faithfully (R3: keeps
                                    // `trailer_passthrough` green), then a clean
                                    // verdict.
                                    let _ = tx.send(Ok(frame)).await;
                                    let _ = verdict_tx.send(Ok(()));
                                    return;
                                }
                                Err(e) => {
                                    // FIFO Err-before-close: inject the body
                                    // error FIRST so hyper aborts the upstream
                                    // request WITHOUT a clean terminator
                                    // (dropping tx alone = clean EOF =
                                    // smuggled-complete request), THEN signal
                                    // the verdict.
                                    let _ = tx.send(Err(H1PumpAbort)).await;
                                    let _ = verdict_tx.send(Err(e));
                                    return;
                                }
                            }
                        }
                        if let Ok(data) = frame.into_data() {
                            forwarded_total = forwarded_total.saturating_add(data.len());
                            if forwarded_total > MAX_REQUEST_BODY_BYTES {
                                // Q-H4 total-body cap exceeded mid-stream. FIFO
                                // Err-before-close: inject the body error FIRST
                                // (the upstream body terminates abruptly WITHOUT
                                // a clean terminator; the caller aborts the conn
                                // and never relays its response), THEN the 413
                                // verdict.
                                let _ = tx.send(Err(H1PumpAbort)).await;
                                let _ = verdict_tx.send(Err(ProxyErr::BodyTooLarge));
                                return;
                            }
                            if let Err(SendOutcome::ReceiverGone) = send_chunked!(data) {
                                // Backend stopped reading mid-stream (early/
                                // short response) — F-MD-2 drain-and-validate,
                                // NOT a 413.
                                let _ = verdict_tx.send(drain_and_validate!());
                                return;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        // F-MD-4 (H1): a premature mid-body close (or any
                        // inbound body protocol/IO error) surfaces HERE as
                        // `Some(Err)` (hyper's `IncompleteBody` on early EOF),
                        // NOT as a clean `None`. FIFO Err-before-close: inject
                        // the body error FIRST so hyper sees the upstream
                        // request body terminate ABRUPTLY (no clean `0\r\n\r\n`
                        // terminator) and aborts the upstream request — the
                        // backend NEVER observes a COMPLETE (truncated) request.
                        // Then signal the 400 verdict. The caller also aborts
                        // the connection (defense in depth) and never relays the
                        // response; single-use `take_stream` ensures the aborted
                        // upstream conn is dropped, not pooled.
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
        // the channel for the pump to make progress under backpressure), but do
        // NOT relay the response until the pump's terminal verdict lands.
        let send_fut = sender.send_request(req);
        let resp = match tokio::time::timeout(self.timeouts.body, send_fut).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                pump.abort();
                conn_handle.abort();
                return Err(ProxyErr::Upstream(format!("send_request: {e}")));
            }
            Err(_) => {
                pump.abort();
                conn_handle.abort();
                return Err(ProxyErr::Timeout);
            }
        };

        // Validate-before-RESPONSE-relay gate: the response head only relays
        // once the inbound body has reached a validated terminal state.
        match verdict_rx.await {
            Ok(Ok(())) => {
                // We deliberately do NOT await `conn_handle` — the response
                // body streaming still needs the driver task running. Detach it.
                drop(conn_handle);
                Ok(resp)
            }
            Ok(Err(e)) => {
                // Malformed/truncated inbound (F-MD-4) or over-cap (Q-H4):
                // abort the upstream connection (do NOT pool it) and the pump,
                // and NEVER relay the upstream response.
                pump.abort();
                conn_handle.abort();
                Err(e)
            }
            Err(_) => {
                // Pump task vanished without a verdict (panic/abort) — treat as
                // an inbound failure; never leak the backend response.
                conn_handle.abort();
                Err(ProxyErr::BadRequest(
                    "inbound H1 request pump terminated without a verdict".to_owned(),
                ))
            }
        }
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
        req: StrippedRequest<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let Some(h2_pool) = self.h2_upstream.as_ref() else {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "H2 backend selected but no Http2Pool wired",
            );
        };
        let translated = match translate_h1_request_to_h2(req.into_inner()).await {
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
        req: StrippedRequest<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let Some(h3_pool) = self.h3_upstream.as_ref() else {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "H3 backend selected but no QuicUpstreamPool wired",
            );
        };
        let sni = backend.sni.as_deref().unwrap_or("");
        let inner = req.into_inner();
        let (headers, body, trailers) =
            match collect_h1_request_to_h3_fieldlist(inner, sni, /* https = */ true).await {
                Ok(p) => p,
                Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
            };
        let h3_resp = Box::pin(lb_quic::request_h3_upstream(
            headers,
            body,
            trailers,
            backend.addr,
            sni,
            h3_pool,
        ))
        .await;
        h3_response_to_h1(h3_resp, self.alt_svc)
    }

    /// Handle an RFC 6455 handshake request.
    ///
    /// **ROUND8-L7-01 (Pingora GHSA-xq2h-p299-vjwv / Envoy
    /// GHSA-rj35-4m94-77jh, both CVSS 9.3):** the client-visible
    /// `101 Switching Protocols` is emitted **only after** the
    /// upstream WebSocket handshake has completed successfully. The
    /// pre-fix code returned `101` synchronously and dialed the
    /// upstream in a detached task — so a client the upstream would
    /// have rejected was already committed to WS framing on a wire
    /// that then silently closed, and any bytes pipelined after the
    /// upgrade request entered an unread upgraded byte-stream (the
    /// smuggling primitive both references paid for).
    ///
    /// New order (mirrors Pingora `proxy_h1.rs` and Envoy
    /// `WsHandlerImpl`): dial upstream → drive the upstream WS client
    /// handshake under a bounded timeout → only then build `101` from
    /// the upstream-accepted handshake. On upstream failure the wire
    /// is still in H1 mode (no `101` emitted) so we return
    /// `502 Bad Gateway` (handshake rejected / unreachable) or
    /// `504 Gateway Timeout` (dial/handshake budget elapsed) and the
    /// client connection stays keep-alive-eligible.
    ///
    /// Behaviour change (documented in CHANGELOG): one upstream-RTT of
    /// added latency on the WS upgrade, and `502/504` instead of
    /// `101`-then-silent-close on upstream failure.
    ///
    /// ROUND8-OPS-06: `req_trace` carries the request span + the child
    /// W3C context; the child `traceparent` is injected onto the
    /// upstream WS handshake request so an on-call engineer can pivot
    /// from an upgrade failure to the exact upstream dial. The
    /// detached splice task is `.instrument()`-ed with the request
    /// span so its events nest under the same `trace_id`.
    ///
    /// Returns a plain 400 if the handshake is structurally valid but
    /// `Sec-WebSocket-Key` is missing once hyper hands us the request
    /// (race: the detector accepted it).
    async fn handle_ws_upgrade(
        &self,
        mut req: Request<IncomingBody>,
        req_trace: crate::trace_ctx::RequestTrace,
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

        let path_and_query = req
            .uri()
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string);
        let forwarded_protocols = req
            .headers()
            .get(&WS_PROTOCOL)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        // ROUND8-L7-01 — dial the upstream and drive its WS handshake
        // BEFORE any client-visible response. Bounded by the H1
        // header-receipt budget (`HttpTimeouts::header`): "time to get
        // the upstream's handshake response" is exactly that budget's
        // semantics. A timeout maps to 504; any other failure to 502.
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
            // The WS dial path (`dial_upstream_ws`) never runs the request-body
            // pump, so it cannot produce these M-D-lite body verdicts; map them
            // defensively to 502 to keep the match exhaustive (no 101 emitted).
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

        // Upstream handshake succeeded. ONLY NOW arm the hyper upgrade
        // future and build the client `101`. The detached task no
        // longer dials — it just splices the already-established
        // upstream WS to the post-upgrade client stream.
        let upgrade_fut = hyper::upgrade::on(&mut req);
        tokio::spawn(tracing::Instrument::instrument(
            run_h1_ws_splice_task(upgrade_fut, backend_ws, ws_proxy),
            req_trace.span.clone(),
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
    /// S9 / M-D-lite — a malformed inbound H1 request body surfaced by the
    /// streaming pump: a premature mid-body close (F-MD-4 truncation) or a
    /// forbidden field in the request trailers (Q-H3). Mapped to `400 Bad
    /// Request`. The upstream response (if any) is NEVER relayed on this arm.
    BadRequest(String),
    /// S9 / Q-H4 — the inbound request body exceeded the cross-cell
    /// [`MAX_REQUEST_BODY_BYTES`] (64 MiB) cap mid-stream. Mapped to `413
    /// Payload Too Large` (RFC 9110 §15.5.14). DISTINCT from an upstream
    /// receiver-drop (F-MD-2), which is NOT a 413.
    BodyTooLarge,
}

/// ROUND8-L7-01 — dial the backend and drive the RFC 6455 client-side
/// handshake **before** the client sees `101`. On success returns the
/// established upstream [`WebSocketStream`]; on failure a [`ProxyErr`]
/// the caller maps to `502` (refused/unreachable) or `504` (timeout).
///
/// ROUND8-OPS-06: the caller's child `traceparent` (and forwarded
/// `tracestate`) is injected onto the upstream handshake request so
/// the upstream sees the LB span as its parent.
async fn dial_upstream_ws(
    pool: TcpPool,
    backend_addr: SocketAddr,
    path_and_query: String,
    forwarded_protocols: Option<String>,
    child_traceparent: String,
    tracestate: Option<String>,
    ws_proxy: Arc<WsProxy>,
) -> Result<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, ProxyErr> {
    // CODE-2-09 follow-on: async dial (no blocking-pool thread for the
    // TCP RTT). A dial failure here is `502` to the client — but
    // crucially the client has NOT yet seen `101`.
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
    // ROUND8-OPS-06: propagate the W3C trace context onto the upstream
    // WS handshake request (tungstenite's builder takes header pairs,
    // not a HeaderMap, so we use the pre-rendered child header value).
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

/// ROUND8-L7-01 — splice-only task. By the time this runs the upstream
/// WS is already established and `101` has been written to the client;
/// we only await the hyper upgrade future (now guaranteed to resolve
/// because the wire flipped) and hand both halves to
/// [`WsProxy::proxy_frames`].
async fn run_h1_ws_splice_task(
    upgrade_fut: hyper::upgrade::OnUpgrade,
    backend_ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    ws_proxy: Arc<WsProxy>,
) {
    let upgraded = match upgrade_fut.await {
        Ok(u) => u,
        Err(e) => {
            // The upstream WS is dropped here — `backend_ws`'s Drop
            // closes the pooled TCP socket so we do not leak it.
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

fn error_response(status: StatusCode, msg: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = Full::new(Bytes::from(msg.to_owned()))
        .map_err(|never| match never {})
        .boxed();
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    resp
}

/// CODE-2-01 / SEC-2-01: map a [`SecurityReject`] to the HTTP response
/// the proxy returns to the client.
///
/// * `Smuggle` / `SlowHandshake` → `400 Bad Request`. Smuggling is a
///   structural request defect; slow-handshake aligns with RFC 9110
///   §15.5.1 "request line / headers malformed".
/// * `RateLimited` → `503 Service Unavailable` with
///   `Retry-After: 1`. The detector path treats rate-limiting as a
///   transient per-peer condition; a fixed 1-second hint is the
///   minimum useful value and avoids leaking detector internals.
/// * `OverCap` → `503 Service Unavailable` with `Retry-After: 1`. The
///   per-IP / per-listener counter saturated; RST-without-response is
///   handled at the accept site (Wave-2c), not here — by the time the
///   request handler sees `OverCap`, the connection is already
///   established and a response is cheaper than a half-close.
pub(crate) fn reject_to_response(rej: &SecurityReject) -> Response<BoxBody<Bytes, hyper::Error>> {
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

/// Strip hop-by-hop headers per RFC 9110 §7.6.1 plus any names listed
/// inside the `Connection` header value.
///
/// Exposed `pub` (rather than `pub(crate)`) so PROTO-2-08 / PROTO-2-07
/// integration tests can pin the exact strip behaviour. The "stripped"
/// invariant is also exposed as a compile-time guarantee via
/// [`crate::stripped_request::StrippedRequest`] (PROTO-2-07).
pub fn strip_hop_by_hop(headers: &mut hyper::HeaderMap) {
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

/// Append the peer's IP to `X-Forwarded-For`, iterating every
/// existing value so multi-line / duplicate `X-Forwarded-For`
/// headers are preserved in the canonical list form. The previous
/// `HeaderMap::get(...)` path returned only the first value, then
/// `insert` clobbered the rest — a silent-drop bug equivalent to
/// **Envoy GHSA-ghc4-35x6-crw5** (RBAC comma-joined-string bypass
/// when duplicate headers existed). Mirrored on `append_via`.
///
/// RFC 7239 / RFC 9110 §5.3 list-rule: comma-separate every prior
/// value, append the peer. Non-ASCII / non-printable existing values
/// fail `to_str` and are skipped (fail-closed-by-skip); a future
/// `xff_policy` knob can promote that to a hard reject.
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

/// Append `HTTP/1.1 expressgateway` to `Via`, iterating every existing
/// value. Same multi-value preservation pattern as [`append_xff`] —
/// RFC 9110 §7.6.3 `Via` is a list-valued header; duplicate header
/// lines must be preserved as comma-separated members of one outbound
/// canonical value, not clobbered.
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

/// Lift an inbound H1 [`Request<IncomingBody>`] into the shape hyper's
/// H2 client expects. Body is collected into `Bytes`; the URI is
/// rebuilt to include the authority hyper extracts into `:authority`.
///
/// Uses the `lb_l7::create_bridge(Http1, Http2)` codec for header
/// transformation. Hop-by-hop headers + Host are stripped by the
/// bridge; hyper synthesises pseudo-headers from the rewritten URI.
async fn translate_h1_request_to_h2(
    req: Request<IncomingBody>,
) -> Result<Request<lb_io::http2_pool::H2ReqBody>, String> {
    let (parts, body) = req.into_parts();
    // PROTO-2-12: collect body + trailers in a single round-trip so
    // the bridge sees both.
    let collected = body
        .collect()
        .await
        .map_err(|e| format!("body collect: {e}"))?;
    let trailers_map = collected.trailers().cloned();
    let body_bytes = collected.to_bytes();
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
        trailers: trailers_vec,
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
    // PROTO-2-12: emit body + trailers as a StreamBody so the H2
    // client sends a separate trailers frame after the data frame.
    // I0.5: the H2 pool body type is widened to a boxed error.
    // `build_body_with_trailers` is shared with the H1-response path
    // (still `hyper::Error`); re-box its error into the pool alias
    // here. `hyper::Error: Into<Box<dyn Error+Send+Sync>>`, so this is
    // a lossless type adaptation (no behavioural change).
    let body: lb_io::http2_pool::H2ReqBody =
        build_body_with_trailers(body_bytes, &translated.trailers)
            .map_err(Into::into)
            .boxed();
    builder.body(body).map_err(|e| format!("build h2 req: {e}"))
}

/// PROTO-2-12 helper: build a `BoxBody` that emits the data bytes
/// followed by an HTTP trailer frame, so cross-protocol bridges can
/// re-attach the trailer set captured at body-collect time.
///
/// `trailers` is a flat `(name, value)` list — empty means no trailer
/// frame is emitted (the body still wraps the data frame).
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

/// Convert an upstream `Response<Incoming>` (H2) back into the H1
/// response shape the listener emits to the client.
async fn upstream_response_to_h1(
    resp: Response<IncomingBody>,
    alt_svc: Option<AltSvcConfig>,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let (parts, body) = resp.into_parts();
    // PROTO-2-12: capture trailers alongside body.
    let collected = match body.collect().await {
        Ok(c) => c,
        Err(e) => {
            return error_response(StatusCode::BAD_GATEWAY, &format!("upstream body read: {e}"));
        }
    };
    let trailers_map = collected.trailers().cloned();
    let body_bytes = collected.to_bytes();
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
        trailers: trailers_vec,
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
    build_h1_response_with_trailers(translated, alt_svc)
}

/// PROTO-2-19 (Wave 2c-2): assemble the final H1 wire response from
/// a translated [`crate::BridgeResponse`]. Shared between
/// [`upstream_response_to_h1`] (H2→H1) and [`h3_response_to_h1`]
/// (H3→H1) so the trailer-aware head shape is identical on both
/// bridges.
///
/// When `translated.trailers` is non-empty this function injects
/// `Transfer-Encoding: chunked` + a `Trailer: <name-list>` declaration
/// on the response head and drops any incoming `Content-Length` /
/// `Transfer-Encoding` / `Trailer` (the proxy's authoritative shape
/// wins). hyper-1's H1 encoder requires both invariants to actually
/// flush a `Frame::trailers` onto the wire
/// (`proto/h1/encode.rs:163-213`); without them the bridge fields
/// would silently disappear. See `audit/protocol/round-6-delta-
/// findings.md` Vector 6 for the upstream-library citation.
pub fn build_h1_response_with_trailers(
    translated: crate::BridgeResponse,
    alt_svc: Option<AltSvcConfig>,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let status = StatusCode::from_u16(translated.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    let has_trailers = !translated.trailers.is_empty();
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        // PROTO-2-19: when re-emitting trailers on the H1 wire, the
        // response head MUST advertise chunked TE and a `Trailer:`
        // declaration listing the trailer names. We strip any
        // pre-existing `transfer-encoding` and `content-length` from
        // the upstream-translated headers — both are re-injected
        // below in the trailer-aware shape — and any pre-existing
        // `trailer` declaration so the proxy's authoritative list
        // wins.
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
        // RFC 9110 §6.6.2: `Trailer:` is end-to-end. RFC 9112 §7.1:
        // chunked TE is required to carry trailers. RFC 9110 §6.5:
        // `Content-Length` MUST NOT accompany trailers on a
        // chunked body — handled by the strip above.
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
    // PROTO-2-12/-19: emit body + trailers via StreamBody. With
    // trailers present, the head-level `Transfer-Encoding: chunked`
    // + `Trailer:` declaration above ensures hyper actually writes
    // the trailer frame onto the wire.
    let body = build_body_with_trailers(translated.body, &translated.trailers);
    builder.body(body).unwrap_or_else(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "build h1 response failed",
        )
    })
}

/// Collect an inbound H1 request, run the H1→H3 codec bridge, and
/// return a `(field_list, body, trailers)` triple for
/// `lb_quic::request_h3_upstream`.
///
/// PROTO-2-12: inbound H1 request trailers are captured via
/// `Collected::trailers()` at body-collect time, bridged through
/// `bridge_request`, and returned so the caller can ship a post-DATA
/// `Frame::trailers` HEADERS frame on the upstream QUIC stream.
async fn collect_h1_request_to_h3_fieldlist(
    req: Request<IncomingBody>,
    sni: &str,
    is_https: bool,
) -> Result<(Vec<(String, String)>, Bytes, Vec<(String, String)>), String> {
    let (parts, body) = req.into_parts();
    // PROTO-2-12: capture request trailers alongside the body.
    let collected = body
        .collect()
        .await
        .map_err(|e| format!("body collect: {e}"))?;
    let trailers_map = collected.trailers().cloned();
    let body_bytes = collected.to_bytes();
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
        // PROTO-2-12: forward inbound H1 request trailers through the
        // H1→H3 bridge; the caller ships them as a post-DATA HEADERS
        // frame on the upstream QUIC stream.
        trailers: trailers_vec,
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
    Ok((field_list, body_bytes, translated.trailers))
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
        // PROTO-2-12: forward the H3 upstream's trailing field
        // section (parsed from the post-DATA HEADERS frame) down the
        // H3→H1 bridge; `build_h1_response_with_trailers` re-emits it
        // as a chunked-trailer block on the H1 wire.
        trailers: resp.trailers,
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
    // PROTO-2-19 (Wave 2c-2): share the trailer-aware H1 head shape
    // with the H2→H1 path via `build_h1_response_with_trailers`.
    // PROTO-2-12 (H3 leg landed): `translated.trailers` now carries
    // the H3 upstream's trailing field section, so the trailer-
    // injection branch emits a chunked-trailer block on the H1 wire.
    build_h1_response_with_trailers(translated, alt_svc)
}

/// S9 / M-D-lite (F-MD-3 — lb-l7 R8 gauge) — the maximum, observed at any
/// instant, of the inbound H1→H1 REQUEST memory the bounded ingress pump
/// retains while a request is in flight: the instantaneous in-flight channel
/// occupancy (≤ `H1_REQ_CHANNEL_DEPTH × H1_REQ_CHUNK_MAX` = 64 KiB).
///
/// This is a GENUINE retained-memory gauge, NOT a constant: the pump
/// increments a live `in_flight_bytes` counter just before it pushes each
/// chunk into the bounded channel and DECREMENTS it the moment hyper pulls the
/// chunk back out (in the `StreamBody`'s poll). It records the live occupancy
/// at each push. A whole-body buffering implementation (the raw-`IncomingBody`
/// hand-off this cell replaces would have held the whole body in hyper's write
/// buffer) — or any no-decrement variant — would make this grow with request
/// size and trip the ≤256 KiB ceiling the memory proof asserts. The bounded
/// window keeps it ≤ 64 KiB, independent of total request size and of
/// [`MAX_REQUEST_BODY_BYTES`].
///
/// Test-only (off by default so production never compiles the gauge); mirrors
/// `h2_proxy::H2_REQ_MAX_RETAINED_BODY_BYTES` and lb-quic
/// `h3_bridge::MAX_RETAINED_BODY_BYTES`. Distinct symbol name (NOT the H2
/// gauge) so the H1 memory proof reads its own counter.
#[cfg(any(test, feature = "test-gauges"))]
pub static H1_REQ_MAX_RETAINED_BODY_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// S9 / M-D-lite (test-gauge) — max-update for
/// [`H1_REQ_MAX_RETAINED_BODY_BYTES`]. Identical lock-free CAS-max to
/// `h2_proxy::record_retained` / lb-quic `h3_bridge::record_retained`: the
/// gauge only ever moves UP, recording the largest instantaneous retained
/// inbound-request memory the pump observes.
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

    /// ROUND8-L7-04 — two `X-Forwarded-For` header LINES must be
    /// preserved in the comma-joined outbound value. Pre-fix the
    /// helper called `HeaderMap::get(...)` which returned only the
    /// first value; `insert(...)` then clobbered the rest. This is
    /// the Envoy GHSA-ghc4-35x6-crw5 silent-drop class on the
    /// producer side.
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

    /// ROUND8-L7-04 — same shape for `Via`: two header lines in,
    /// one canonical comma-joined header line out.
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

    // PROTO-2-11 H1 half (Wave 2c-2): smoke test for the cancel-aware
    // variant. Mirrors the H2 test
    // `h2_proxy::tests::test_sigterm_emits_two_step_goaway`. Build a
    // minimal H1Proxy, hand it a duplex pair with the peer side
    // dropped, plus a pre-cancelled token. The expected outcome is
    // that `serve_connection_with_cancel` returns promptly via its
    // graceful_shutdown branch — a regression that re-introduces a
    // busy-loop or holds the conn open indefinitely would time out
    // here.
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
        // Empty duplex — peer half dropped, so any read returns EOF
        // and hyper's H1 conn resolves without ever parsing a request
        // line.
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
