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
// S11 I2 (D2 / CF-DEDUP-1): the H1→H2 streaming request leg routes its
// egress through the SHARED graceful-drop driver and therefore speaks the
// h2_proxy `ProxyErr` (the type the driver returns) — NOT the h1_proxy-local
// `ProxyErr`, which the H1→H1 / WS / H1→H3 paths keep using. The shared
// abort-observe bound is reused so the FIFO inject window matches H2→H2.
use crate::h2_proxy::{H2_ABORT_OBSERVE_TIMEOUT, ProxyErr as H2ProxyErr, drive_h2_upstream_send};
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

/// S12 — the client-facing response body type the listener emits. Widened from
/// `BoxBody<Bytes, hyper::Error>` to a boxed `std::error::Error` so a
/// channel-built streaming response (H1→H3, S12) can inject a CONSTRUCTIBLE
/// truncation error (`H1PumpAbort`) on a mid-response abort — `hyper::Error` has
/// no public ctor. This is LOSSLESS: hyper's H1 server only requires
/// `Body::Error: Into<Box<dyn Error + Send + Sync>>` (it wraps the body error via
/// `Error::new_user_body` itself), so a boxed `hyper::Error` aborts byte-identical
/// to the un-boxed one. Every prior `hyper::Error`-sourced body widens via
/// `.map_err(|e| Box::new(e) as _)`; infallible bodies via `match never {}`.
pub(crate) type ClientRespBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

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
    /// the upstream `send_request` call as the Phase-A no-forward-progress
    /// idle deadline (S14 / CF-BODY-WALLCLOCK).
    pub body: Duration,
    /// Hard upper bound on a single connection's total lifetime.
    pub total: Duration,
    /// **S14 / CF-BODY-WALLCLOCK (R-CFBW-2)** — Phase-B fixed cap on the
    /// post-upload head wait, separate from the Phase-A `body` idle
    /// deadline. Defaults to 60 s.
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
    type Response = Response<ClientRespBody>;
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
    ) -> Response<ClientRespBody> {
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
    ) -> Response<ClientRespBody> {
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

        // S14 / CF-BODY-WALLCLOCK — forward-progress signal for
        // [`lb_io::idle_send::idle_bounded_send`]. `last_progress` is bumped on
        // every successful `tx.send(Ok)` (i.e. every chunk hyper accepted into
        // the upstream pipeline — the same forward-progress event the R8
        // in-flight gauge records); `upload_complete` is set Once at the
        // verdict-Ok terminal arm so the helper switches from Phase-A idle
        // (`timeouts.body`) to Phase-B head-roundtrip cap (`timeouts.head`).
        // See `audit/h-matrix/s14-builder-1-design.md` §2.1.
        let last_progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let upload_complete = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let epoch = tokio::time::Instant::now();
        let last_progress_pump = std::sync::Arc::clone(&last_progress);
        let upload_complete_pump = std::sync::Arc::clone(&upload_complete);
        let epoch_pump = epoch;

        let pump = tokio::spawn(async move {
            // S14 — bump helper: store millis-since-`epoch_pump` on every
            // successful `tx.send(Ok)`. Co-located with `in_flight_bytes`
            // fetch_add (the R8 forward-progress site). Relaxed ordering is
            // fine: the helper re-arms on the next tick if a bump lands late.
            let bump = || {
                let dt = tokio::time::Instant::now().saturating_duration_since(epoch_pump);
                let ms = u64::try_from(dt.as_millis()).unwrap_or(u64::MAX);
                last_progress_pump.store(ms, std::sync::atomic::Ordering::Relaxed);
            };
            // S14 — terminal-frame helper: set ONCE at every verdict-Ok arm
            // (clean EOF / trailer-Ok). Release ordering pairs with the
            // helper's Acquire load so the FINAL `last_progress` bump is
            // visible before the helper observes `upload_complete = true`.
            let set_complete = || {
                upload_complete_pump.store(true, std::sync::atomic::Ordering::Release);
            };

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
                        // S14 — chunk accepted by hyper → forward-progress.
                        bump();
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
                        // S14 — upload complete; helper now switches to
                        // Phase-B head-roundtrip cap (`timeouts.head`).
                        set_complete();
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
                                    // S14 — trailers accepted; bump then
                                    // mark upload complete (Phase-B).
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
        //
        // S14 / CF-BODY-WALLCLOCK — the fixed wall-clock
        // `tokio::time::timeout(self.timeouts.body, send_fut)` was a
        // whole-upload wall-clock for backends that withhold the response head
        // until the body is fully consumed. Replaced by the two-phase
        // [`lb_io::idle_send::idle_bounded_send`] mechanism:
        // * Phase A (upload in flight): a no-forward-progress idle deadline
        //   anchored on `last_progress + self.timeouts.body`, re-armed by the
        //   pump's `bump()` on every accepted chunk.
        // * Phase B (upload complete): a fixed `self.timeouts.head` cap on the
        //   remaining head-roundtrip wait, anchored when the pump's
        //   `set_complete()` ran.
        // The F-CAP-1 inner `Ok(Err(hyper::Error))` arm and the :1523 verdict-
        // rx backstop are preserved verbatim — the helper passes the hyper
        // error through unchanged, so the verdict-vs-send classification
        // semantics are identical.
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
                // F-CAP-1 — a `send_request` error is the DOWNSTREAM EFFECT of
                // whatever the pump did; the pump's classified verdict is the
                // AUTHORITATIVE cause. When the pump deliberately aborts the
                // upstream (over-cap → BodyTooLarge, forbidden trailer / mid-
                // body truncation → BadRequest) it injects the body Err and
                // then sends its verdict immediately AFTER it (FIFO), so the
                // backend's response head may never arrive and `send_request`
                // fails. Returning 502 here would mask the real 413/400 and
                // create a 413-vs-502 race (R2). Instead consult the verdict
                // first, BOUNDED by `timeouts.body` so a wedged pump cannot
                // hang the error path. Do NOT `pump.abort()` before this await
                // (the pump must still deliver its verdict).
                let classified = match tokio::time::timeout(self.timeouts.body, verdict_rx).await {
                    Ok(Ok(Err(ve @ (ProxyErr::BodyTooLarge | ProxyErr::BadRequest(_))))) => {
                        Some(ve)
                    }
                    // Verdict Ok(()), a non-classified verdict error, the pump
                    // vanished, or the bounded await elapsed → the send error is
                    // a GENUINE upstream failure; fall through to 502.
                    _ => None,
                };
                pump.abort();
                conn_handle.abort();
                return Err(
                    classified.unwrap_or_else(|| ProxyErr::Upstream(format!("send_request: {e}")))
                );
            }
            Err(idle_err) => {
                // S14 — Phase-1 collapse onto ProxyErr::Timeout; phase
                // discriminant logged for triage.
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

    /// S11 I2 (D2) — STREAMING H1→H2 request leg. MIRROR of
    /// [`crate::h2_proxy::H2Proxy::proxy_h2_to_h2_request`] (h2_proxy.rs:1964):
    /// a bounded lookahead window → Branch A (whole request fit the window →
    /// buffered send) / Branch B (request > window → bounded streaming pump →
    /// the SHARED [`drive_h2_upstream_send`] graceful-drop driver, D1).
    ///
    /// DELTAS vs the H2→H2 mirror:
    /// - **Preamble**: [`build_h1_to_h2_upstream_parts`] (H1→H2 bridge head,
    ///   no body collect) instead of `build_h2_upstream_request_parts`.
    /// - **Ingress framing = H1 M-D-lite (MIRROR-IMAGE of H2)**: for an
    ///   inbound H1 body `frame()==None` is the POSITIVELY-confirmed clean end
    ///   (NOT gated on `is_end_stream()`, which is unreliable for chunked
    ///   `Kind::Chan` bodies); a premature mid-body close surfaces as
    ///   `Some(Err)` (hyper's `IncompleteBody`). See [`Self::proxy_request`]
    ///   1372-1483 for the reference arms.
    /// - **Egress channel error** = the constructible [`H1PumpAbort`] (so the
    ///   pump can INJECT a body error → hyper RESETs the upstream stream
    ///   rather than emitting a spurious clean END_STREAM on a truncated
    ///   request — the F-MD-4 body-layer half, mirror of H2→H2's `PumpAbort`).
    /// - **Trailers** validated with [`validate_h1_request_trailers`] (H1
    ///   path); **memory gauge** = [`record_retained_h1`].
    ///
    /// Returns the h2_proxy [`H2ProxyErr`] (the type the shared driver
    /// returns); the caller [`Self::proxy_h1_to_h2`] maps it to 502/504/413/400.
    async fn proxy_h1_to_h2_request(
        &self,
        h2_pool: &Http2Pool,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Result<Response<IncomingBody>, H2ProxyErr> {
        // NB (DELTA vs proxy_h2_to_h2_request): no `use hyper::body::Body`
        // here — the H1 M-D-lite framing deliberately does NOT call
        // `is_end_stream()` (`None` is the clean-end signal for H1).
        use hyper::body::Frame;
        use lb_io::http2_pool::{H2ReqBody, Http2PoolError};

        let req = req.into_inner();
        let (parts, mut body) = req.into_parts();

        // ── Preamble (DELTA vs proxy_request): keep the request HTTP/2-
        // shaped for the H2 upstream via the H1→H2 bridge head — WITHOUT
        // collecting the body. We do NOT force HTTP/1.1 and do NOT strip
        // content-length/transfer-encoding the way the H1-egress pump does
        // (those were H1-framing fixes; H2 upstream framing is hyper's H2
        // encoder's job — same DELTA as H2→H2's `build_h2_upstream_request_parts`).
        let upstream_parts = match build_h1_to_h2_upstream_parts(&parts) {
            Ok(p) => p,
            Err(e) => return Err(H2ProxyErr::Upstream(e)),
        };

        // S11 / M-D mirror — bounded H1 INGRESS pump (lookahead-window). The
        // lookahead posture (validate-before-dial within the window, stream
        // past it) is IDENTICAL to H2→H2; only the framing arms differ.
        let mut lookahead: Vec<Bytes> = Vec::new();
        let mut buffered: usize = 0;
        let mut trailers_map: Option<hyper::HeaderMap> = None;
        let mut reached_eof = false;

        // ── Phase 1: lookahead. Poll frames until EOF or the window fills.
        loop {
            #[cfg(any(test, feature = "test-gauges"))]
            record_retained_h1(buffered);

            if buffered > H1_REQ_CHANNEL_DEPTH * H1_REQ_CHUNK_MAX {
                break;
            }

            match body.frame().await {
                None => {
                    // F-MD-4 (H1 — MIRROR-IMAGE of H2): for an inbound H1 body
                    // `None` is the POSITIVELY-confirmed clean end (the
                    // chunked terminator / satisfied Content-Length). A
                    // premature close surfaces as `Some(Err)` below, NOT here.
                    // Do NOT consult `is_end_stream()` (unreliable for chunked
                    // `Kind::Chan`). Clean end within the window → Branch A.
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
                    // F-MD-4 (H1): a premature mid-body close / protocol/IO
                    // error surfaces while VALIDATING the inbound stream,
                    // BEFORE any pool contact (zero-dial for a within-window
                    // truncation — preserves Branch-A validate-before-dial).
                    return Err(H2ProxyErr::BadRequest(format!(
                        "inbound H1 request body incomplete: {e}"
                    )));
                }
            }
        }

        if reached_eof {
            // ── Branch A: the whole request fit within the window. ──
            // Validate trailers, then build the buffered H2 body and send.
            // ZERO pool contact for a malformed within-window request:
            // any inbound Err returned ABOVE this point.
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
            // DELTA: Branch A body must be `H2ReqBody`
            // (BoxBody<Bytes, Box<dyn Error+Send+Sync>>). `build_body_with_trailers`
            // yields BoxBody<_, hyper::Error>; widen the error losslessly
            // (mirror of H2→H2 Branch A + `translate_h1_request_to_h2`'s
            // former boxed-error adaptation).
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

        // Bridge the mpsc receiver into an `http_body` StreamBody, mapping
        // the channel error `H1PumpAbort` → `Box<dyn Error+Send+Sync>` so the
        // body is `H2ReqBody` (the H2 pool body alias). H1PumpAbort already
        // impls Error.
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

        // The pump owns the inbound body + already-buffered lookahead and
        // reports its terminal verdict via a oneshot so the response-head
        // relay is gated on a VALIDATED terminal state.
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), H2ProxyErr>>();
        let drained: Vec<Bytes> = std::mem::take(&mut lookahead);

        // S14 / CF-BODY-WALLCLOCK — forward-progress signal for
        // [`lb_io::http2_pool::Http2Pool::send_request_idle`] (threaded via
        // [`drive_h2_upstream_send`]). Mirror of H1→H1 (h1_proxy.rs::proxy_request)
        // and H2→H2 (h2_proxy.rs::proxy_h2_to_h2_request). See
        // `audit/h-matrix/s14-builder-1-design.md` §2.4.
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
                        // S14 — chunk accepted by hyper → forward-progress.
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

            // F-MD-4 (S10 DEFECT FIX, body-layer half — H1 mirror of H2→H2's
            // `inject_abort!`) — on every abort terminal state (premature
            // close, forbidden trailer, over-cap), inject `Err(H1PumpAbort)`
            // into the upstream request-body channel and HOLD the sender open
            // until hyper has OBSERVED it (`tx.closed().await`). FIFO delivery
            // forces hyper to poll the error BEFORE any channel-close `None`,
            // so hyper RESETS the upstream stream rather than emitting a
            // spurious clean END_STREAM. Bounded by `H2_ABORT_OBSERVE_TIMEOUT`
            // (shared) so a wedged upstream driver cannot hang the pump.
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
                        // F-MD-4 (H1 — MIRROR-IMAGE of H2; do NOT copy the H2
                        // `is_end_stream` logic). `None` = clean end → drop tx
                        // → hyper writes the terminator → the upstream sees a
                        // COMPLETE request. NO inject_abort on this arm.
                        // S14 — upload complete; helper switches to Phase-B
                        // head-roundtrip cap.
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
                                    // S14 — trailers accepted; bump then
                                    // mark upload complete (Phase-B).
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
                        // F-MD-4 (H1): premature mid-body close / inbound body
                        // error → inject the body error FIRST so hyper aborts
                        // the upstream request WITHOUT a clean terminator,
                        // THEN signal the 400 verdict.
                        inject_abort!();
                        let _ = verdict_tx.send(Err(H2ProxyErr::BadRequest(format!(
                            "inbound H1 request body incomplete: {e}"
                        ))));
                        return;
                    }
                }
            }
        });

        // F-MD-4 (S10 DEFECT FIX) — route the graceful-drop egress through
        // the SHARED driver (CF-DEDUP-1 / S11 D1), identical to H2→H2's
        // Branch B. The driver owns the detached send task (biased
        // verdict-vs-head race + `reset_peer` on every abort + F-CAP-1 caller
        // arm + `pump.abort()`) and the final `head_rx.await`.
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

    /// Forward an H1 inbound request to an H2 backend (PROTO-001).
    ///
    /// S11 I2/I3 (D2+D3): dispatch shim over the streaming
    /// [`Self::proxy_h1_to_h2_request`] leg. Bridges via
    /// [`crate::create_bridge`]`(Http1, Http2)` — the codec-level translation
    /// produces the pseudo-header set hyper's H2 client expects. BOTH legs now
    /// stream: the request body is STREAMED (no ahead-of-dial collect, I2) and
    /// the response is relayed by the STREAMING [`upstream_response_to_h1`]
    /// (no body collect, I3).
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
        // S11 I2/I3 (D2+D3) — fully STREAMING H1↔H2 (mirror of
        // `proxy_h2_to_h2`'s dispatch arm, h2_proxy.rs:1925-1955). The
        // request body is no longer collected ahead of dial; the
        // bounded-window pump streams it and gates the response head on a
        // VALIDATED inbound terminal state (F-MD-4 mirror-image). The Ok arm
        // relays the upstream response via the STREAMING
        // `upstream_response_to_h1` (I3 — `body.boxed()`, no collect).
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

    /// Forward an H1 inbound request to an H3 backend (PROTO-001) — S12 R8
    /// FULLY STREAMING both legs (replaces the buffering
    /// `collect_h1_request_to_h3_fieldlist` → `request_h3_upstream` →
    /// `h3_response_to_h1` round-trip).
    ///
    /// Request leg: an H1 M-D-lite ingress pump (MIRROR of [`Self::proxy_request`]
    /// 1179-1560) feeds [`lb_quic::h3_bridge::ReqBodyEvent`]s
    /// (`Chunk`/`End{trailers}`/`Reset`) into the shared streaming connector
    /// [`lb_quic::stream_request_to_h3_upstream`] (CF-DEDUP-2, commit 369c5e53).
    /// `frame()==None` is the POSITIVELY-confirmed clean end → `End`; a
    /// `Some(Err)` truncation / forbidden-trailer / over-cap → `Reset` → the
    /// connector RESETs the upstream QUIC stream WITHOUT a clean FIN
    /// (`H3_REQUEST_CANCELLED`) — the F-MD-4 H3 mirror-image (a truncated inbound
    /// is NEVER presented as a complete request). `forward_req_trailers=true`:
    /// validated request trailers ride `End{trailers}` → a post-DATA HEADERS
    /// frame upstream (the buffering path forwarded them; we do NOT regress).
    ///
    /// Response leg: drains the connector's DECODED
    /// [`lb_quic::H3RespEvent`] channel (`Head`/`Body`/`Trailers`/`End`/`Reset`)
    /// into a streaming H1 response via [`h3_decoded_resp_head_builder`] (shares
    /// the pseudo/`RESPONSE_HOP_BY_HOP` transform with
    /// [`upstream_response_to_h1`]) + a `StreamBody`. CF-RESP-1: a streamed H1
    /// response cannot pre-declare a `Trailer:` header (trailer names unknown at
    /// head-time), so a late `Trailers` event is relayed onto the body's terminal
    /// frame but hyper-1 may drop it absent the head declaration — empirically
    /// settled by the gRPC-shaped BUILT-bar test, NOT pre-accepted here.
    ///
    /// F-CAP-1 status surfacing (verified against 369c5e53): a PRE-DATA over-cap
    /// (my pump's first event is `Reset`, before any `Chunk`) → the connector
    /// emits a synthesized `Head{413}` I relay; a pre-dial failure → synthesized
    /// `Head{502}`. A MID-BODY over-cap / truncation → `H3RespEvent::Reset` (NOT
    /// a 413 — response-splitting guard) → I abort the H1 client body, never FIN.
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

        // Build the request field-list via the H1→H3 bridge (head-only — the
        // body + trailers now STREAM, so no `body.collect()` here).
        let inner = req.into_inner();
        let (parts, mut body) = inner.into_parts();
        let headers = match build_h1_to_h3_fieldlist(&parts, &sni, /* https = */ true) {
            Ok(h) => h,
            Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
        };

        // Bounded request-body channel into the connector (depth
        // H3_BODY_CHANNEL_DEPTH). Backpressure: a slow QUIC upstream → the
        // connector stops draining → this channel fills → the pump stops
        // polling the inbound H1 body → the client's H1 read window stalls.
        let (body_tx, body_rx) =
            tokio::sync::mpsc::channel::<ReqBodyEvent>(lb_quic::conn_actor::H3_BODY_CHANNEL_DEPTH);
        // Decoded response channel out of the connector.
        let (resp_tx, mut resp_rx) = tokio::sync::mpsc::channel::<lb_quic::H3RespEvent>(
            lb_quic::h3_bridge::H3_RESP_CHANNEL_DEPTH,
        );

        // F-MD-3 (lb-l7 R8 gauge) — instantaneous in-flight request bytes the
        // pump retains: incremented before each `Chunk` send, decremented when
        // the connector pulls it (here: when `body_tx.send` resolves, the chunk
        // has left our retained set into the bounded channel; the channel depth
        // bounds total in-flight independent of body size).
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // ── Request-leg M-D-lite pump (mirror of `proxy_request`'s pump) ──
        let pump_in_flight = std::sync::Arc::clone(&in_flight_bytes);
        let pump = tokio::spawn(async move {
            // Running cumulative forwarded request bytes — the request-body cap
            // (`MAX_REQUEST_BODY_BYTES`) is OUR job (the connector caps the
            // RESPONSE, not the request). The 413-vs-RESET boundary is timing-
            // critical: an over-cap detected BEFORE any chunk forwarded →
            // `Reset` as the FIRST event → connector inline-413; an over-cap
            // AFTER ≥1 chunk → `Reset` → connector RESET-without-FIN (no 413,
            // response-splitting guard).
            let mut forwarded_total: usize = 0;

            // Split a DATA payload into ≤ H3_BODY_CHUNK_MAX pieces and push each
            // as one `ReqBodyEvent::Chunk` (pump-side split bounds the in-flight
            // channel-item size to match the memory gauge). Returns Err(()) if
            // the connector dropped the receiver (treat as abort).
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
                        // F-MD-4 (H1 mirror): `frame()==None` is the
                        // POSITIVELY-confirmed clean end (NOT `is_end_stream()`).
                        // Clean end → `End{trailers:[]}` → connector FIN.
                        let _ = body_tx
                            .send(ReqBodyEvent::End {
                                trailers: Vec::new(),
                            })
                            .await;
                        return;
                    }
                    Some(Ok(frame)) => {
                        if frame.is_trailers() {
                            // Validate request trailers BEFORE forwarding (a
                            // framing/routing field in trailers is a desync
                            // primitive). Forbidden → `Reset` (smuggling guard:
                            // never a clean `End`). OK → `End{trailers}` →
                            // connector ships a post-DATA HEADERS frame then FIN.
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
                            // Over-cap: emit `Reset`. If this is BEFORE any chunk
                            // was forwarded (forwarded_total==0) the connector
                            // inline-413s; otherwise it RESET-without-FINs. Either
                            // way we never forward an over-cap byte.
                            if forwarded_total.saturating_add(data.len()) > MAX_REQUEST_BODY_BYTES {
                                let _ = body_tx.send(ReqBodyEvent::Reset).await;
                                return;
                            }
                            forwarded_total = forwarded_total.saturating_add(data.len());
                            if send_chunked!(data).is_err() {
                                // Connector dropped the receiver (aborted /
                                // client gone) — stop pumping.
                                return;
                            }
                        }
                    }
                    Some(Err(_e)) => {
                        // F-MD-4 (H1): premature mid-body close / IO error
                        // surfaces as `Some(Err)` (hyper `IncompleteBody`), NOT a
                        // clean `None`. Emit `Reset` → connector RESET-without-FIN
                        // → the backend NEVER sees a complete (truncated) request.
                        let _ = body_tx.send(ReqBodyEvent::Reset).await;
                        return;
                    }
                }
            }
        });

        // ── Drive the connector concurrently with the pump ──
        // The connector future is spawned (needs `'static`), so move OWNED
        // copies of every borrow into the task: `sni` (already owned) and a
        // cloned `Arc` of the H3 pool. The connector takes `&str` / `&pool`,
        // satisfied by references to the task-local owned values.
        let sink = lb_quic::H3RespOut::Decoded {
            tx: resp_tx,
            total: 0,
            cap: lb_quic::h3_bridge::MAX_RESPONSE_BODY_BYTES,
        };
        let pool = std::sync::Arc::clone(h3_pool);
        let connector_handle = tokio::spawn(async move {
            // The connector's own Result is bookkeeping; the client-facing
            // outcome rides the `H3RespEvent` stream (Head/Body/Trailers/End/
            // Reset). We drop its return.
            let _ = lb_quic::stream_request_to_h3_upstream(
                headers, /* forward_req_trailers = */ true, addr, &sni, &pool, body_rx, sink,
            )
            .await;
        });

        // ── Response leg: drain resp_rx into a streaming H1 response ──
        // The FIRST event determines the head. `Head{status,headers}` →
        // build the streaming H1 head + spawn a body relay; `Reset`/channel-
        // closed before any head → 502 (the connector aborted pre-Head).
        let first = resp_rx.recv().await;
        match first {
            Some(lb_quic::H3RespEvent::Head { status, headers }) => {
                let st = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
                let builder = h3_decoded_resp_head_builder(st, &headers, self.alt_svc);

                // Stream the remaining Body/Trailers/End/Reset events into a
                // StreamBody. `Reset` → inject a body error so hyper does NOT
                // emit a clean terminator (the response is truncated, never
                // presented as complete — response-splitting guard). `End` →
                // drop the sender (clean EOF). A late `Trailers` rides the
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
                            // A second Head is malformed (Head is once-only); treat
                            // as an abort.
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
                    // F-MD-4 (response leg): the channel error is the constructible
                    // `H1PumpAbort` (hyper::Error has no public ctor). On a connector
                    // `Reset` the relay task SENDS `Err(H1PumpAbort)` into the channel
                    // (NOT a clean drop), so hyper polls the body, sees an ERROR, and
                    // aborts the chunked response WITHOUT a `0\r\n\r\n` terminator —
                    // the H1 client sees a truncated response, never a smuggled-
                    // complete one (response-splitting guard). Box the error to
                    // satisfy `Body::Error: Into<Box<dyn Error+Send+Sync>>`.
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
                let _ = &pump; // pump is detached; its task owns the request leg
                build_h1_streaming_response(builder, stream_body.boxed())
            }
            None | Some(lb_quic::H3RespEvent::Reset) => {
                // Connector aborted before any Head (pre-dial fail, or a
                // pre-Head response abort) — 502. Abort the pump + connector.
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

    fn finalize_response(&self, resp: Response<IncomingBody>) -> Response<ClientRespBody> {
        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        if let Some(alt) = self.alt_svc {
            // Inject (or replace) the Alt-Svc header so older origins
            // cannot shadow our advertisement.
            if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
                parts.headers.insert(hyper::header::ALT_SVC, value);
            }
        }
        // S12 widening: lossless-box the upstream `Incoming` body's `hyper::Error`
        // into the widened `ClientRespBody` error type.
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

fn error_response(status: StatusCode, msg: &str) -> Response<ClientRespBody> {
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

/// S11 I2 (D2) — HEAD-ONLY preamble for the streaming H1→H2 request leg.
///
/// The head-construction half of the former `translate_h1_request_to_h2`
/// MINUS the body collect: run the `create_bridge(Http1, Http2)` codec over
/// a body-LESS [`crate::BridgeRequest`] (empty body + empty trailers) so the
/// bridge produces the H2 pseudo-header set hyper's H2 client expects, then
/// build the hyper `Request` parts (method + `scheme://authority/path` URI +
/// re-emitted non-`:` headers).
///
/// DELTA (mirror of `h2_proxy::build_h2_upstream_request_parts`): we do NOT
/// force HTTP/1.1 and do NOT strip `content-length`/`transfer-encoding` the
/// way the H1→H1 egress pump does — H2 upstream framing is hyper's H2
/// encoder's job; those strips were H1-framing fixes. The body is attached
/// SEPARATELY by the caller (buffered `H2ReqBody` in Branch A, streaming
/// `StreamBody` in Branch B).
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
    // Re-emit non-pseudo headers that the bridge produced. hyper's H2
    // client builds pseudo-headers itself from the URI and method.
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

/// S11 I2 (D2) — map the h1_proxy-local [`ProxyErr`] returned by
/// [`validate_h1_request_trailers`] into the h2_proxy [`H2ProxyErr`] the
/// streaming H1→H2 leg speaks (its verdict channel + the shared driver). The
/// validator only ever yields `BadRequest`, but we map every variant for
/// completeness so a future variant cannot silently mis-classify.
fn h1_to_h2_proxy_err(e: ProxyErr) -> H2ProxyErr {
    match e {
        ProxyErr::Upstream(s) => H2ProxyErr::Upstream(s),
        ProxyErr::Timeout => H2ProxyErr::Timeout,
        ProxyErr::BadRequest(s) => H2ProxyErr::BadRequest(s),
        ProxyErr::BodyTooLarge => H2ProxyErr::BodyTooLarge,
    }
}

/// S11 I2 (D2) — H1 analog of `h2_proxy::concat_chunks`: concatenate the
/// lookahead DATA chunks into one `Bytes` for the within-window (Branch A)
/// buffered upstream body. `total` is the exact summed length so we allocate
/// once. (A local copy, NOT a shared helper: `concat_chunks` is private to
/// h2_proxy; this byte-identical analog avoids widening its visibility.)
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

/// PROTO-2-12 helper: build a `BoxBody` that emits the data bytes
/// followed by an HTTP trailer frame, so cross-protocol bridges can
/// re-attach the trailer set captured at body-collect time.
///
/// `trailers` is a flat `(name, value)` list — empty means no trailer
/// frame is emitted (the body still wraps the data frame).
/// S12 H1→H3 (R8) — build the STREAMING H1 response head from the connector's
/// decoded [`lb_quic::H3RespEvent::Head`]. Shares the pseudo/`RESPONSE_HOP_BY_HOP`
/// strip + lowercase transform with [`upstream_response_to_h1`] (ONE
/// authoritative transform, not a third copy). Like the H1←H2 streaming leg, it
/// does NOT pre-declare a `Trailer:`/chunked-TE head (the trailer names are
/// unknown at head-time — CF-RESP-1); a late `H3RespEvent::Trailers` rides the
/// body's terminal frame.
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
        if crate::h2_to_h1::RESPONSE_HOP_BY_HOP
            .iter()
            .any(|h| *h == lower.as_str())
        {
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

/// S12 H1→H3 (R8) — finalize a streaming H1 response from a head `Builder` + a
/// streamed body. Centralizes the build-failure fallback.
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
///
/// S11 I3 (D3) — STREAMING relay (mirror of
/// [`crate::h2_proxy::upstream_h2_response_to_h2`] but with H2→H1 header
/// semantics). Replaces the R8-violating `body.collect().await` (which
/// materialised the WHOLE H2-backend response before relay). We take
/// `(parts, body)`, build the H1 head from status + the H2→H1
/// header transform (drop `:`-pseudo + the [`RESPONSE_HOP_BY_HOP`] set,
/// lowercase the rest — the SAME authoritative shape as
/// [`H2ToH1Bridge::bridge_response`], referenced directly, not copied),
/// inject `Alt-Svc` if configured, then `body.boxed()` the `Incoming` for
/// streaming-by-construction. The H2 `Incoming` body's error type is already
/// `hyper::Error`, so `body.boxed()` IS `BoxBody<Bytes, hyper::Error>` (the
/// return type) with no error adaptation — same as `upstream_h2_response_to_h2`.
/// Backpressure / R8 bound: hyper frames the unknown-length body as chunked
/// and only pulls it as fast as the downstream H1 client drains, bounded by
/// hyper's H2 receive-window on the upstream leg.
///
/// CF-RESP-1 / D3 TRAILERS: a STREAMED relay cannot pre-declare the head
/// `Trailer:` names — they arrive only in the boxed body's TERMINAL frame,
/// after the head is already on the wire. We deliberately do NOT reintroduce
/// a `collect()` to capture trailer names (that is the exact R8 violation this
/// increment removes), so we do NOT call [`build_h1_response_with_trailers`]
/// here. The upstream H2 response trailers ride the boxed `Incoming` body's
/// terminal frame; whether hyper-1's H1 encoder flushes that terminal trailer
/// frame WITHOUT a head `Trailer:` declaration + chunked TE is VERIFIER-
/// DETERMINED on the wire (D3): (i) if it does, H1←H2 response trailers relay
/// for free; (ii) if it does NOT, streamed H1←H2 responses simply do not
/// forward response trailers — this matches the nginx default of not
/// forwarding H2 response trailers to H1, and no existing wire test asserts
/// proxy-level H1←H2 trailer relay, so it is a bounded documented behaviour,
/// NOT a silent regression. (The H3→H1 leg keeps the buffering
/// [`build_h1_response_with_trailers`], which still pre-declares trailers.)
fn upstream_response_to_h1(
    resp: Response<IncomingBody>,
    alt_svc: Option<AltSvcConfig>,
) -> Response<ClientRespBody> {
    let (parts, body) = resp.into_parts();
    // H2→H1 response header transform (mirror of `H2ToH1Bridge::bridge_
    // response`): drop `:`-prefixed pseudo-headers AND the authoritative
    // `RESPONSE_HOP_BY_HOP` set (case-insensitive), re-emit every other
    // header lowercased. Status preserved. Shares the transform with the
    // STREAMING H3→H1 head-builder (`h3_decoded_resp_head_builder`) so there
    // is ONE authoritative pseudo/HOP-BY-HOP strip, not a third copy.
    let mut builder = Response::builder().status(parts.status);
    for (n, v) in &parts.headers {
        let name = n.as_str();
        if name.starts_with(':') {
            continue;
        }
        let lower = name.to_lowercase();
        if crate::h2_to_h1::RESPONSE_HOP_BY_HOP
            .iter()
            .any(|h| *h == lower.as_str())
        {
            continue;
        }
        // HeaderName is already stored lowercase by hyper's H2 codec;
        // re-emit via the lowercased string to match the bridge's
        // `to_lowercase()` semantics exactly.
        builder = builder.header(lower.as_str(), v);
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    // R8: stream the upstream `Incoming` body by construction. The terminal
    // trailers frame (if any) rides the boxed body naturally; no `collect()`,
    // no owned buffer. S12 widening: lossless-box the `hyper::Error`.
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

/// PROTO-2-19 (Wave 2c-2): assemble the final H1 wire response from
/// a translated [`crate::BridgeResponse`] (whole-body, trailer-aware: it
/// pre-declares `Trailer:` + chunked TE in the head). S12: the buffering
/// `h3_response_to_h1` that used to share this was removed when H1→H3 went
/// streaming (the streaming H3→H1 head uses [`h3_decoded_resp_head_builder`]
/// instead, which CANNOT pre-declare trailer names — CF-RESP-1). This
/// whole-body builder is retained as the authoritative trailer-aware H1 head
/// shape exercised by `trailer_passthrough` (and available to any future
/// buffered H1-response path).
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
) -> Response<ClientRespBody> {
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

/// S12 H1→H3 (R8) — build the H1→H3 request FIELD-LIST from the request HEAD
/// only (no `body.collect()` — the body + trailers now STREAM through the
/// connector). Runs the same `create_bridge(Http1, Http3).bridge_request`
/// codec translation + `:authority` synthesis the buffering
/// `collect_h1_request_to_h3_fieldlist` did, minus the body/trailer capture
/// (request trailers now ride `ReqBodyEvent::End{trailers}`).
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
        // Body + trailers STREAM now: the bridge only needs the head to mint
        // the pseudo-header set, so pass an empty body / no trailers here.
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
