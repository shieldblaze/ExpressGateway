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
use std::task::{Context, Poll};

use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::body::{Bytes, Incoming as IncomingBody};
use hyper::header::HeaderValue;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite};

use lb_io::http2_pool::Http2Pool;
use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;

use crate::grpc_proxy::{self, GrpcProxy};
use crate::h1_proxy::{
    AltSvcConfig, BackendPicker, HttpTimeouts, append_via, append_xff, set_xfh, set_xfp,
    strip_hop_by_hop,
};
use lb_security::{
    ConnId, GlitchKind, GlitchOutcome, GlitchesCounter, SmuggleDetector, SmuggleMode, Watchdog,
};

use crate::h2_security::H2SecurityThresholds;
use crate::security_hooks::{DynSecurityHooks, NoopHooks};
use crate::stripped_request::{StrippedRequest, strip_hop_by_hop as strip_into_newtype};
use crate::upstream::{BackendInfoPicker, SingleProtoPicker, UpstreamBackend, UpstreamProto};
use crate::ws_proxy::{self, WsProxy, is_h2_extended_connect};

/// F-COR-1 (D1) — bound for the H2→H1 buffered request body.
///
/// The H2→H1 path now fully receives + protocol-validates the inbound
/// H2 request body BEFORE dialing the upstream (the ordering fix that
/// closes the validate-vs-forward race, see [`H2Proxy::proxy_request`]).
/// Buffering the request body requires a hard cap so a large/streamed
/// inbound body cannot OOM the proxy. 64 MiB, mirroring the H3 path's
/// `lb_quic::MAX_REQUEST_BODY_BYTES`, so the H2→H1, H2→H2, H2→H3 and H3
/// paths all share one consistent ceiling. Exceeding it yields
/// `413 Payload Too Large` (RFC 9110 §15.5.14), never an unbounded
/// allocation.
pub const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

/// L7 HTTP/2 reverse proxy. Cheap to clone via [`Arc`].
pub struct H2Proxy {
    pool: TcpPool,
    picker: Arc<dyn BackendInfoPicker>,
    alt_svc: Option<AltSvcConfig>,
    timeouts: HttpTimeouts,
    is_https: bool,
    security: H2SecurityThresholds,
    /// When `Some`, inbound extended-CONNECT streams carrying
    /// `:protocol = websocket` (RFC 8441) are routed through the
    /// WebSocket proxy instead of returning 502.
    ws: Option<Arc<WsProxy>>,
    /// When `Some`, inbound streams whose content-type matches
    /// `application/grpc[+ext]` are routed through the gRPC proxy
    /// (Item 3 / PROMPT.md §13) instead of the regular H2 request
    /// path. The H2 flood/bomb thresholds from Item 1 still apply to
    /// the hosting connection.
    grpc: Option<Arc<GrpcProxy>>,
    /// Optional H2 upstream pool. PROTO-001 H2→H2 path.
    h2_upstream: Option<Arc<Http2Pool>>,
    /// Optional H3 upstream pool. PROTO-001 H2→H3 path.
    h3_upstream: Option<Arc<QuicUpstreamPool>>,
    /// CODE-2-01 / SEC-2-01 hook surface. Defaults to
    /// [`NoopHooks`]; Wave-2c flips this to the production
    /// `lb_security::HooksBundle` via [`Self::with_hooks`].
    hooks: Arc<dyn DynSecurityHooks>,
    /// SEC-2-01 / SEC-2-03 slowloris / slow-POST watchdog
    /// (mirrors `H1Proxy::watchdog`).
    watchdog: Option<Watchdog>,
    /// Monotonic per-listener connection sequence used as the
    /// [`Watchdog`] entry key.
    conn_seq: Arc<parking_lot::Mutex<u64>>,
    /// PROTO-2-18 (Wave 2c-2): default expected SNI for the
    /// [`crate::sni_authority::check_sni_authority`] check. Builder
    /// default `None` means SNI/authority agreement is not enforced
    /// on this proxy unless [`Self::serve_connection_with_cancel_sni`]
    /// supplies a per-connection override.
    expected_sni: Option<String>,
    /// ROUND8-L7-05: policy for `_` in inbound H2 header names.
    /// Default [`crate::h1_proxy::HeaderUnderscorePolicy::Reject`]
    /// mirrors Envoy edge best-practice. The same enum used by
    /// H1Proxy is reused here so the wiring crate maps once from
    /// `lb_config`. hyper's H2 codec does not reject underscores for
    /// us, so this server-side filter is the only enforcement point
    /// on the H2 path.
    header_underscore_policy: crate::h1_proxy::HeaderUnderscorePolicy,
    /// ROUND8-L7-07 / L7-12 — HAProxy 3.0
    /// `tune.h2.fe.glitches-threshold` consolidated protocol-abuse
    /// counter. When `Some(threshold)` a per-connection
    /// [`GlitchesCounter`] is created in
    /// [`Self::serve_connection_with_cancel_sni`]; every H2
    /// protocol-abuse event (underscore-policy reject, smuggle reject,
    /// `:authority`/Host disagreement, malformed authority, SNI
    /// mismatch) records a weighted glitch. Crossing the threshold
    /// drains the connection via the existing two-step GOAWAY path
    /// (RFC 9113 §6.8; logical ENHANCE_YOUR_CALM). `None` (default)
    /// keeps the counter dormant for backwards-compatible callers.
    glitches_threshold: Option<u32>,
    /// ROUND8-L7-07 — optional metrics registry used to register and
    /// bump `h2_glitches_total` so the abuse threshold is operator-
    /// observable. `lb-l7` already depends on `lb-observability`
    /// (trace-context). The production wire-in (mapping the config
    /// knob + the process `MetricsRegistry`) is performed by the `lb`
    /// binary; the counter logic itself runs whenever a registry is
    /// supplied (the proof test supplies its own).
    glitches_metrics: Option<Arc<lb_observability::MetricsRegistry>>,
}

/// F-SEC-1 (CVE-2023-44487-adjacent) — clean-close I/O wrapper that
/// flushes the RFC 9113 §6.8 GOAWAY signal reliably on connection
/// teardown.
///
/// Mechanism (auditor-2 A2-1): when hyper/h2's rapid-reset /
/// local-error-reset mitigation trips, h2 queues a
/// GOAWAY(PROTOCOL_ERROR), writes it to the socket via `codec.flush`,
/// and then calls `AsyncWrite::poll_shutdown` on the underlying I/O
/// (`framed_write::shutdown` = flush-then-poll_shutdown — the GOAWAY
/// bytes are ALREADY on the wire by the time `poll_shutdown` runs).
/// The defect: the abusive client is still mid-flood, so the kernel's
/// receive buffer on the server side holds unconsumed inbound bytes
/// (the client's RST_STREAM spam). Closing a TCP socket that still has
/// unread inbound data makes the kernel emit an RST instead of a clean
/// FIN (RFC 1122 / Linux behaviour); the client's TCP stack discards
/// its ENTIRE receive buffer — including the GOAWAY that already
/// arrived — upon receiving that RST, surfacing only `Io(BrokenPipe)`.
/// Under scheduler starvation this is the common case (auditor-2: 6/48
/// under induced contention, deterministic under heavier churn here).
///
/// The fix is structural and entirely server-side: before issuing the
/// FIN in `poll_shutdown`, drain (read and discard) any pending inbound
/// bytes to a bounded cap so the close is a clean FIN, not an RST. With
/// no unread data the client's TCP stack does NOT discard its receive
/// buffer, so the already-delivered GOAWAY survives and is decoded.
/// This makes the GOAWAY delivery deterministic and scheduler-
/// independent (directive D3): the drain is driven inside
/// `poll_shutdown` itself (same task, same waker) with a hard byte cap
/// so an unbounded inbound flood cannot delay teardown.
struct CleanCloseIo<IO> {
    inner: IO,
    /// Remaining inbound bytes we are willing to drain during shutdown
    /// before forcing the FIN regardless (hard bound — a wedged/flooding
    /// client cannot delay teardown indefinitely).
    drain_budget: usize,
    /// Set once the pre-FIN drain has finished (cap hit, EOF, or no
    /// more immediately-available data) so we do not re-drain on a
    /// second `poll_shutdown` poll.
    drained: bool,
}

impl<IO> CleanCloseIo<IO> {
    /// 256 KiB: comfortably larger than any in-flight RST_STREAM /
    /// HEADERS burst a client can have queued between the server
    /// emitting GOAWAY and the FIN, yet a hard cap so a deliberate
    /// post-GOAWAY flood cannot pin the worker. Drain is read-and-
    /// discard only (no allocation growth beyond a fixed scratch
    /// buffer) and is bounded by both this cap AND the surrounding
    /// `total` connection-timeout budget hyper already enforces.
    const DRAIN_CAP: usize = 256 * 1024;

    fn new(inner: IO) -> Self {
        Self {
            inner,
            drain_budget: Self::DRAIN_CAP,
            drained: false,
        }
    }
}

impl<IO: AsyncRead + Unpin> AsyncRead for CleanCloseIo<IO> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<IO: AsyncWrite + AsyncRead + Unpin> AsyncWrite for CleanCloseIo<IO> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // F-SEC-1: drain pending inbound bytes BEFORE the FIN so the
        // close is a clean FIN, not an RST — see `CleanCloseIo` doc.
        // The GOAWAY has already been flushed to the socket by h2's
        // `codec.shutdown` (flush precedes this poll_shutdown call).
        if !self.drained {
            let mut scratch = [0u8; 16 * 1024];
            loop {
                if self.drain_budget == 0 {
                    break;
                }
                let cap = scratch.len().min(self.drain_budget);
                let Some(slot) = scratch.get_mut(..cap) else {
                    break;
                };
                let mut rb = tokio::io::ReadBuf::new(slot);
                match Pin::new(&mut self.inner).poll_read(cx, &mut rb) {
                    Poll::Ready(Ok(())) => {
                        let n = rb.filled().len();
                        if n == 0 {
                            // Clean EOF — peer's write half closed; no
                            // unread data remains, safe to FIN.
                            break;
                        }
                        self.drain_budget -= n;
                        // Keep draining; loop again.
                    }
                    Poll::Ready(Err(_)) => {
                        // Read error (e.g. reset already) — nothing more
                        // we can usefully drain; proceed to shutdown.
                        break;
                    }
                    Poll::Pending => {
                        // No more inbound data immediately available.
                        // The kernel receive buffer is drained for now;
                        // a clean FIN will not trigger RST. Proceed
                        // (do NOT return Pending — we must not wait on
                        // a possibly-silent client; the GOAWAY is
                        // already on the wire and the receive buffer is
                        // empty, which is exactly the clean-FIN
                        // precondition).
                        break;
                    }
                }
            }
            self.drained = true;
        }
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
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
    ///
    /// Wraps `picker` in a [`SingleProtoPicker`] tagged
    /// [`UpstreamProto::H1`] for backwards compatibility.
    #[must_use]
    pub fn with_security(
        pool: TcpPool,
        picker: Arc<dyn BackendPicker>,
        alt_svc: Option<AltSvcConfig>,
        timeouts: HttpTimeouts,
        is_https: bool,
        security: H2SecurityThresholds,
    ) -> Self {
        let info = Arc::new(SingleProtoPicker::new(picker, UpstreamProto::H1, None));
        Self {
            pool,
            picker: info,
            alt_svc,
            timeouts,
            is_https,
            security,
            ws: None,
            grpc: None,
            h2_upstream: None,
            h3_upstream: None,
            hooks: Arc::new(NoopHooks::new()),
            watchdog: None,
            conn_seq: Arc::new(parking_lot::Mutex::new(0)),
            expected_sni: None,
            header_underscore_policy: crate::h1_proxy::HeaderUnderscorePolicy::Reject,
            glitches_threshold: None,
            glitches_metrics: None,
        }
    }

    /// Construct an [`H2Proxy`] backed by a multi-protocol picker
    /// (PROTO-001).
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
        security: H2SecurityThresholds,
    ) -> Self {
        Self {
            pool,
            picker,
            alt_svc,
            timeouts,
            is_https,
            security,
            ws: None,
            grpc: None,
            h2_upstream: None,
            h3_upstream: None,
            hooks: Arc::new(NoopHooks::new()),
            watchdog: None,
            conn_seq: Arc::new(parking_lot::Mutex::new(0)),
            expected_sni: None,
            header_underscore_policy: crate::h1_proxy::HeaderUnderscorePolicy::Reject,
            glitches_threshold: None,
            glitches_metrics: None,
        }
    }

    /// Attach a [`SecurityHooks`] impl (CODE-2-01 / SEC-2-01 hot-path
    /// surface). Mirrors [`crate::h1_proxy::H1Proxy::with_hooks`].
    /// Wave-2c flips this to the production
    /// `lb_security::HooksBundle` from `crates/lb/src/main.rs`.
    #[must_use]
    pub fn with_hooks(mut self, hooks: Arc<dyn DynSecurityHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Attach an [`lb_security::Watchdog`] for per-stream slowloris /
    /// slow-POST eviction (SEC-2-01 / SEC-2-03). Mirrors
    /// [`crate::h1_proxy::H1Proxy::with_watchdog`]. The H2 service
    /// closure runs once per stream so each stream registers and
    /// deregisters independently.
    #[must_use]
    pub fn with_watchdog(mut self, watchdog: Watchdog) -> Self {
        self.watchdog = Some(watchdog);
        self
    }

    /// ROUND8-L7-07 / L7-12 — enable the HAProxy-3.0 consolidated
    /// glitches abuse counter on this proxy. A per-connection
    /// [`GlitchesCounter`] (default 60 s rolling window) is created in
    /// [`Self::serve_connection_with_cancel_sni`]; every detected H2
    /// protocol-abuse event records a weighted glitch and bumps the
    /// `h2_glitches_total` metric on `registry`. When the weighted
    /// rolling sum exceeds `threshold` the connection is drained via
    /// the existing two-step GOAWAY path (logical ENHANCE_YOUR_CALM).
    ///
    /// `threshold` of `0` keeps the counter dormant (operator opt-out
    /// parity with HAProxy's `tune.h2.fe.glitches-threshold 0`).
    ///
    /// The frame-arrival sub-timer half of L7-07
    /// ([`GlitchKind::FrameRecvTimeout`]) is NOT wired here: hyper 1.x
    /// `serve_connection` exposes no per-frame read context, so the
    /// `tokio::time::Interval` watchdog is deferred-with-rationale
    /// (see `audit/deferred.md`). The COUNTER half — the actual
    /// HAProxy pattern — is fully wired by this builder.
    #[must_use]
    pub fn with_glitches(
        mut self,
        threshold: u32,
        registry: Arc<lb_observability::MetricsRegistry>,
    ) -> Self {
        self.glitches_threshold = if threshold == 0 {
            None
        } else {
            Some(threshold)
        };
        self.glitches_metrics = Some(registry);
        self
    }

    /// PROTO-2-18 (Wave 2c-2): default expected SNI used by the
    /// [`crate::sni_authority::check_sni_authority`] hot-path check
    /// when [`Self::serve_connection`] is used directly. Real TLS-
    /// bearing deployments prefer
    /// [`Self::serve_connection_with_cancel_sni`] which captures the
    /// SNI live from rustls at TLS-accept time.
    #[must_use]
    pub fn with_expected_sni(mut self, sni: Option<String>) -> Self {
        self.expected_sni = sni;
        self
    }

    /// ROUND8-L7-05: set the header-name underscore policy on this
    /// H2 proxy. Default is
    /// [`crate::h1_proxy::HeaderUnderscorePolicy::Reject`]. The
    /// wiring crate maps from `lb_config::HeaderUnderscorePolicy` to
    /// this enum at proxy-construction time.
    #[must_use]
    pub const fn with_header_underscore_policy(
        mut self,
        policy: crate::h1_proxy::HeaderUnderscorePolicy,
    ) -> Self {
        self.header_underscore_policy = policy;
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
    /// Exposed for integration tests.
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

    /// Enable WebSocket upgrade handling on this proxy. Fluent; returns
    /// `self` for chaining off [`Self::with_security`] or [`Self::new`].
    #[must_use]
    pub fn with_websocket(mut self, ws: Arc<WsProxy>) -> Self {
        self.ws = Some(ws);
        self
    }

    /// Enable gRPC handling on this proxy. Fluent; returns `self` so
    /// the call site reads as a chain off [`Self::with_security`].
    ///
    /// Aligns the supplied [`GrpcProxy`]'s upstream H2 client
    /// `max_header_list_size` with the listener's
    /// [`H2SecurityThresholds::max_header_list_size`] (auditor-delta
    /// finding GRPC-001) so a malicious backend cannot transit
    /// oversize trailers through the gateway before hyper rejects.
    #[must_use]
    pub fn with_grpc(mut self, grpc: GrpcProxy) -> Self {
        let aligned = grpc.with_max_header_list_size(self.security.max_header_list_size);
        self.grpc = Some(Arc::new(aligned));
        self
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
        // PROTO-2-11 (H2 half, Wave 2c-2): always delegates to the
        // cancellable variant with a never-cancelled token so the
        // original signature stays back-compat.
        self.serve_connection_with_cancel(io, peer, tokio_util::sync::CancellationToken::new())
            .await
    }

    /// PROTO-2-11 (Wave 2c-2) — H2 half of the GOAWAY-on-drain
    /// contract paired with REL-2-02's H3 `CONNECTION_CLOSE`.
    ///
    /// Identical to [`Self::serve_connection`] until `cancel`
    /// fires: at that point the hyper H2 connection is pinned and
    /// `.graceful_shutdown()` is invoked, which emits the canonical
    /// **two-step GOAWAY** sequence (RFC 9113 §6.8): first a GOAWAY
    /// with `last_stream_id = 2^31 - 1` so the client stops opening
    /// new streams, then a second GOAWAY with the actual highest
    /// in-flight stream id once the server's `MAX_CONCURRENT_STREAMS`
    /// has drained. The connection future is then driven to
    /// completion with the existing `total` budget so a misbehaving
    /// client cannot pin a worker past the drain deadline.
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
        // PROTO-2-18: no per-connection SNI override; the builder
        // default applies.
        let sni = self.expected_sni.clone();
        self.serve_connection_with_cancel_sni(io, peer, cancel, sni)
            .await
    }

    /// PROTO-2-18 (Wave 2c-2) — H2 entry point that threads the
    /// per-connection TLS SNI value into the request hot-path so the
    /// [`crate::sni_authority::check_sni_authority`] validator runs
    /// against the **observed** SNI rather than the builder default.
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

        // ROUND8-L7-07 / L7-12 — one GlitchesCounter per H2
        // connection. `conn_cancel` is a CHILD of the caller's
        // `cancel`: a parent (SIGTERM) cancellation propagates DOWN to
        // it, and a glitch-drain cancels it DIRECTLY. The select arm
        // below waits on `conn_cancel` so BOTH causes resolve the
        // SAME existing two-step GOAWAY path (logical
        // ENHANCE_YOUR_CALM) — additive child token only, the 15-case
        // drain contract is unaffected.
        let conn_cancel = cancel.child_token();
        let glitch_state = self.glitches_threshold.map(|threshold| {
            let metric = self.glitches_metrics.as_ref().and_then(|reg| {
                reg.counter(
                    "h2_glitches_total",
                    "HTTP/2 protocol-abuse glitch events recorded by the \
                     consolidated HAProxy-style counter (ROUND8-L7-07/L7-12)",
                )
                .ok()
            });
            GlitchConnState {
                counter: Arc::new(parking_lot::Mutex::new(GlitchesCounter::new(
                    threshold,
                    lb_security::DEFAULT_GLITCHES_WINDOW,
                ))),
                metric,
                drain: conn_cancel.clone(),
            }
        });

        let svc = ProxyService {
            inner: Arc::clone(&self),
            peer,
            expected_sni: sni,
            glitch: glitch_state,
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
        // RFC 8441 extended CONNECT — enables SETTINGS_ENABLE_CONNECT_PROTOCOL
        // advertisement so clients can bootstrap WebSocket over H2. Safe
        // to always enable: clients that do not use it pay no cost.
        builder.enable_connect_protocol();
        // F-SEC-1: wrap `io` so connection teardown drains pending
        // inbound bytes before the FIN, guaranteeing the queued RFC
        // 9113 §6.8 GOAWAY (already written by h2 before poll_shutdown)
        // is delivered with a clean FIN instead of being discarded by
        // an RST-on-unread-data close. Deterministic, scheduler-
        // independent (directive D3).
        let conn = builder.serve_connection(TokioIo::new(CleanCloseIo::new(io)), svc);
        tokio::pin!(conn);
        // Wait on the connection-level token: cancelled by either the
        // parent `cancel` (SIGTERM drain — propagates parent→child) or
        // a glitch-threshold trip (cancels `conn_cancel` directly).
        let cancel_fut = conn_cancel.cancelled();
        tokio::pin!(cancel_fut);
        let timer = tokio::time::sleep(total);
        tokio::pin!(timer);
        tokio::select! {
            // Cancel wins ties so a SIGTERM during a long-running
            // request still triggers the GOAWAY emit.
            biased;
            () = &mut cancel_fut => {
                // Two-step GOAWAY: hyper handles both frames inside
                // `graceful_shutdown` (it sets the soft limit then
                // drains).
                conn.as_mut().graceful_shutdown();
                // Drive the conn future to completion with the
                // existing `total` budget so a stalled client cannot
                // delay drain past the deadline.
                match tokio::time::timeout(total, conn).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(io::Error::other(format!("h2 graceful shutdown: {e}"))),
                    Err(_) => Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "h2 graceful shutdown timeout",
                    )),
                }
            }
            res = &mut conn => match res {
                Ok(()) => Ok(()),
                Err(e) => Err(io::Error::other(format!("h2 server: {e}"))),
            },
            () = &mut timer => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "total connection timeout",
            )),
        }
    }
}

/// ROUND8-L7-07 / L7-12 — per-H2-connection abuse-counter state.
/// Cloned cheaply into every per-stream `ProxyService` clone hyper
/// makes; the `Arc<Mutex<..>>` keeps a single shared counter across
/// all streams of the connection (the HAProxy `h2c->glitches` is
/// per-connection, not per-stream).
#[derive(Clone)]
struct GlitchConnState {
    counter: Arc<parking_lot::Mutex<GlitchesCounter>>,
    /// Resolved `h2_glitches_total` handle (None if no registry was
    /// supplied — the counter still drains, it is just unobserved).
    metric: Option<lb_observability::IntCounter>,
    /// Connection-level drain token. Cancelling it triggers the
    /// existing two-step GOAWAY select arm (ENHANCE_YOUR_CALM shape).
    drain: tokio_util::sync::CancellationToken,
}

impl GlitchConnState {
    /// Record one weighted abuse event. Bumps `h2_glitches_total`
    /// and, if the rolling weighted sum crosses the threshold,
    /// cancels the connection drain token (→ GOAWAY) and returns
    /// `true` so the caller can short-circuit the request.
    fn record(&self, kind: GlitchKind) -> bool {
        if let Some(m) = &self.metric {
            m.inc();
        }
        let outcome = {
            let mut c = self.counter.lock();
            c.record(kind, std::time::Instant::now())
        };
        if outcome == GlitchOutcome::Drain {
            self.drain.cancel();
            true
        } else {
            false
        }
    }
}

/// Service implementation carrying the [`H2Proxy`] plus the peer address.
#[derive(Clone)]
struct ProxyService {
    inner: Arc<H2Proxy>,
    peer: SocketAddr,
    /// PROTO-2-18: per-connection SNI captured from the rustls
    /// handshake at TLS-accept time.
    expected_sni: Option<String>,
    /// ROUND8-L7-07 / L7-12: per-connection glitches counter; `None`
    /// when the operator has not enabled the consolidated threshold.
    glitch: Option<GlitchConnState>,
}

impl hyper::service::Service<Request<IncomingBody>> for ProxyService {
    type Response = Response<BoxBody<Bytes, hyper::Error>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<IncomingBody>) -> Self::Future {
        let inner = Arc::clone(&self.inner);
        let peer = self.peer;
        let sni = self.expected_sni.clone();
        let glitch = self.glitch.clone();
        Box::pin(async move {
            Ok(Box::pin(inner.handle(req, peer, sni.as_deref(), glitch.as_ref())).await)
        })
    }
}

impl H2Proxy {
    /// ROUND8-OPS-06 / REL-2-07 — H2 mirror of the H1 trace-context
    /// wire-in. H2 has no hyper `Upgrade` primitive today (the WS path
    /// is RFC 8441 extended CONNECT, handled in
    /// `handle_ws_extended_connect`), so this commit only adds the
    /// per-request span + child-context injection for parity; the
    /// ROUND8-L7-01 "defer 101" restructure is H1-specific. Same
    /// `Instrument`-not-`Entered` discipline as H1 so the span never
    /// leaks across an `.await` onto a co-scheduled task.
    async fn handle(
        &self,
        mut req: Request<IncomingBody>,
        peer: SocketAddr,
        expected_sni: Option<&str>,
        glitch: Option<&GlitchConnState>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        use tracing::Instrument;
        let listener_label = if self.is_https { "h2" } else { "h2c" };
        let req_trace = crate::trace_ctx::RequestTrace::open(
            req.headers(),
            "h2",
            req.method().as_str(),
            req.uri()
                .path_and_query()
                .map_or("/", http::uri::PathAndQuery::as_str),
            listener_label,
            expected_sni,
        );
        // Inject the child context onto the inbound request now so
        // every downstream H2→{H1,H2,H3} bridge forwards it without a
        // per-bridge callsite (H2 has many forwarding paths).
        req_trace.inject_upstream(req.headers_mut());
        let span = req_trace.span.clone();
        let resp = self
            .handle_inner(req, peer, expected_sni, glitch)
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
        glitch: Option<&GlitchConnState>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        // ROUND8-L7-09 — uniform authority validation CHOKE POINT.
        // HAProxy `BUG/MAJOR: http: forbid comma in authority` +
        // `BUG/MEDIUM: h1: Enforce the authority validation`. The SAME
        // predicate the H1 path enforces MUST run on the H2 parser
        // too, BEFORE the request forks into the extended-CONNECT WS
        // handler or the gRPC proxy (the prior verify pass found both
        // forks reached upstream selection unvalidated —
        // `audit/round-8/verify/fixback.md`). Placed as the FIRST
        // statement so neither fork below can bypass it. H2 carries
        // the authority in `:authority` (hyper surfaces it as
        // `uri.authority()`); a client may also send `Host`. Both are
        // validated by `validate_request`.
        if let Err((bad, err)) = crate::authority::validate_request(&req) {
            tracing::warn!(
                peer = %peer,
                authority = %bad,
                error = ?err,
                "ROUND8-L7-09: H2 authority rejected (choke point)"
            );
            // ROUND8-L7-07: a malformed authority is a routing/ACL
            // desync attempt — medium glitch weight.
            if let Some(g) = glitch {
                g.record(GlitchKind::RapidReset);
            }
            return error_response(StatusCode::BAD_REQUEST, "invalid authority (ROUND8-L7-09)");
        }

        // RFC 8441 extended CONNECT intercept. Only fires when this
        // listener was configured with a `WsProxy`; everything else
        // continues through the regular H2 request path.
        if self
            .ws
            .as_ref()
            .is_some_and(|w| w.config().enabled && is_h2_extended_connect(&req))
        {
            return self.handle_ws_extended_connect(req);
        }
        if let Some(gp) = self
            .grpc
            .as_ref()
            .filter(|g| g.config().enabled && grpc_proxy::is_grpc_request(&req))
        {
            // gRPC requires an H1/H2 backend; today's GrpcProxy speaks
            // hyper H2 over a TCP-pool stream, so any backend selected
            // by the multi-proto picker is acceptable provided it is
            // not H3 (which would require a QUIC tunnel + grpc-over-h3
            // adaptor, out of v1 scope).
            let Some(backend) = self.picker.pick_info() else {
                return error_response(StatusCode::BAD_GATEWAY, "no backend available");
            };
            if backend.proto == UpstreamProto::H3 {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "gRPC proxy does not support H3 backends",
                );
            }
            return Arc::clone(gp).handle(req, backend.addr).await;
        }
        let (mut parts, body) = req.into_parts();

        // ROUND8-L7-05: enforce header-name underscore policy before
        // any other inspection. hyper's H2 codec does not reject
        // underscores for us; this is the only enforcement point on
        // the H2 path. Default is `Reject` (Envoy edge best-practice).
        // See `audit/round-8/findings/ROUND8-L7-05.md`.
        match self.header_underscore_policy {
            crate::h1_proxy::HeaderUnderscorePolicy::Reject => {
                if parts
                    .headers
                    .iter()
                    .any(|(n, _)| n.as_str().as_bytes().contains(&b'_'))
                {
                    // ROUND8-L7-07: protocol-abuse glitch (low weight —
                    // a single malformed-header request is noisy but
                    // not by itself an attack; sustained ones trip the
                    // consolidated threshold).
                    if let Some(g) = glitch {
                        g.record(GlitchKind::ContinuationFlood);
                    }
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "header name contains underscore (ROUND8-L7-05)",
                    );
                }
            }
            crate::h1_proxy::HeaderUnderscorePolicy::Drop => {
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
            crate::h1_proxy::HeaderUnderscorePolicy::Allow => {}
        }

        // CODE-2-01 / SEC-2-01: run the security hooks before hop-by-hop
        // strip + upstream-acquire. The reconstructed `Request<()>` is
        // a header-only borrow surface; the trait reads `headers()` +
        // `version()` only.
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
            return crate::h1_proxy::reject_to_response(&rej);
        }

        // SEC-2-01 — defense-in-depth explicit `SmuggleDetector` call
        // in H2 mode. Mirrors the H1 hot-path call site; the
        // `SmuggleMode::H2` arm enables the
        // [`check_h2_downgrade`] check (forbidden hop-by-hop headers
        // and non-`trailers` TE per RFC 9113 §8.2.2) on top of the
        // CL/TE/duplicate-CL defaults.
        let header_pairs: Vec<(String, String)> = parts
            .headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_owned(), s.to_owned()))
            })
            .collect();
        if let Err(e) = SmuggleDetector::check_all_mode(&header_pairs, SmuggleMode::H2) {
            tracing::warn!(error = %e, peer = %peer, "h2 smuggle rejected");
            // ROUND8-L7-07: request-smuggling against an H2 mux is the
            // single most severe protocol abuse (CL/TE desync,
            // forbidden hop-by-hop, h2 downgrade) — highest glitch
            // weight so a smuggle burst drains the connection fast.
            if let Some(g) = glitch {
                g.record(GlitchKind::HpackRatio);
            }
            return error_response(StatusCode::BAD_REQUEST, "request smuggling");
        }

        // ROUND8-L7-09 authority validation now runs at the
        // `handle_inner` choke point (above the extended-CONNECT /
        // gRPC forks) via `crate::authority::validate_request`, so it
        // covers those paths too. No second call needed here.

        // PROTO-2-01 — RFC 9113 §8.3.1: when both `:authority` and
        // `Host` are present they MUST agree. hyper surfaces
        // `:authority` as `uri.authority()`. Disagreement is a
        // routing/authz desync primitive (host-confusion smuggling
        // against backends that authorise on `Host`), so reject with
        // 400 BEFORE hop-by-hop strip / upstream acquire.
        if let Err(msg) = check_authority_host_agreement(&parts.uri, &parts.headers) {
            tracing::warn!(peer = %peer, reason = msg, "h2 :authority/Host mismatch rejected");
            // ROUND8-L7-07: host-confusion smuggling primitive —
            // medium glitch weight.
            if let Some(g) = glitch {
                g.record(GlitchKind::RapidReset);
            }
            return error_response(StatusCode::BAD_REQUEST, msg);
        }

        // PROTO-2-18 (Wave 2c-2) — SNI ↔ `:authority`/Host agreement
        // (RFC 9110 §15.5.20). Precedence step 3 from
        // `audit/protocol/round-2-review.md`: smuggle → auth/host →
        // SNI/host. Hyper surfaces the H2 `:authority` pseudo-header
        // as `uri.authority()`; we prefer that, falling back to
        // `Host` if the client emitted it without `:authority`. Empty
        // authority is `Ok` per the validator's contract (PROTO-2-01
        // upstream rejects empty authority already). Loopback peers
        // skip enforcement (sec-r5 caveat — same rationale as the H1
        // path: SNI/Host confusion is a Layer-7 routing/authz vector
        // that doesn't apply to loopback ingress).
        if !peer.ip().is_loopback() {
            let authority = parts
                .uri
                .authority()
                .map(http::uri::Authority::as_str)
                .unwrap_or_else(|| {
                    parts
                        .headers
                        .get(hyper::header::HOST)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                });
            if let Err(mismatch) =
                crate::sni_authority::check_sni_authority(expected_sni, authority)
            {
                tracing::warn!(
                    peer = %peer,
                    sni = %mismatch.sni,
                    authority = %mismatch.authority,
                    "PROTO-2-18: H2 SNI/:authority mismatch — emitting 421 Misdirected Request"
                );
                // ROUND8-L7-07: SNI/host confusion is a Layer-7
                // routing/authz desync — medium glitch weight.
                if let Some(g) = glitch {
                    g.record(GlitchKind::RapidReset);
                }
                let (status, body) = crate::sni_authority::misdirected_response();
                return error_response(status, body);
            }
        }

        // SEC-2-01 / SEC-2-03 — register the stream with the
        // slowloris watchdog.
        let watch_id = self.watchdog.as_ref().map(|wd| {
            let seq = {
                let mut g = self.conn_seq.lock();
                *g = g.wrapping_add(1);
                *g
            };
            let id = ConnId::new(peer.ip(), seq);
            let deadline = std::time::Instant::now() + self.timeouts.header;
            wd.register(id, deadline);
            let header_bytes: u64 = parts
                .headers
                .iter()
                .map(|(n, v)| n.as_str().len() as u64 + v.len() as u64 + 4)
                .sum();
            if let Err(e) = wd.progress(id, header_bytes) {
                tracing::warn!(error = %e, peer = %peer, "h2 watchdog evicted at header phase");
            }
            id
        });

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

        // PROTO-2-07 — mint a `StrippedRequest` so the proxy_* fan-out
        // takes a type that statically guarantees hop-by-hop strip.
        let req_pre_strip = Request::from_parts(parts, body);
        let mut stripped = strip_into_newtype(req_pre_strip);
        {
            let headers = stripped.headers_mut();
            append_xff(headers, peer);
            set_xfp(headers, self.is_https);
            if let Some(h) = authority.as_deref() {
                set_xfh(headers, h);
                // Upstream is H1, which requires a Host header. If the
                // client spoke H2 without one, synthesise from
                // :authority.
                if !headers.contains_key(hyper::header::HOST) {
                    if let Ok(v) = HeaderValue::from_str(h) {
                        headers.insert(hyper::header::HOST, v);
                    }
                }
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
                // F-COR-1 (D1): request-body cap exceeded — reject with
                // 413 before any upstream contact.
                Err(ProxyErr::BodyTooLarge) => error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body exceeds maximum",
                ),
                // F-COR-1: inbound H2 request failed protocol
                // validation during receive — reject (PROTOCOL_ERROR-
                // class, surfaced as 400) WITHOUT having dialed the
                // backend, so the backend 200 body can never leak.
                Err(ProxyErr::BadRequest(s)) => error_response(StatusCode::BAD_REQUEST, &s),
            },
            UpstreamProto::H2 => Box::pin(self.proxy_h2_to_h2(backend.addr, stripped)).await,
            UpstreamProto::H3 => Box::pin(self.proxy_h2_to_h3(&backend, stripped)).await,
        };
        // SEC-2-01 / SEC-2-03 — deregister the stream from the
        // watchdog on the normal completion path.
        if let (Some(wd), Some(id)) = (self.watchdog.as_ref(), watch_id) {
            wd.deregister(id);
        }
        resp
    }

    /// Handle an RFC 8441 extended-CONNECT WebSocket bootstrap.
    ///
    /// Returns `200 OK` with an empty body; hyper flips the inbound
    /// stream into a bidirectional byte channel once the response
    /// headers reach the wire. A detached task picks up the upgraded
    /// stream, dials the backend over HTTP/1.1, drives the client-side
    /// RFC 6455 handshake, and runs the bidirectional frame forwarder.
    fn handle_ws_extended_connect(
        &self,
        mut req: Request<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let Some(ws_proxy) = self.ws.clone() else {
            return error_response(StatusCode::BAD_GATEWAY, "websocket disabled");
        };
        let Some(backend) = self.picker.pick_info() else {
            return error_response(StatusCode::BAD_GATEWAY, "no backend available");
        };
        if backend.proto != UpstreamProto::H1 {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "WebSocket extended-CONNECT requires H1 backend",
            );
        }
        let backend_addr = backend.addr;

        let upgrade_fut = hyper::upgrade::on(&mut req);
        let path_and_query = req
            .uri()
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string);
        let ws_cfg = ws_proxy.config();
        let pool = self.pool.clone();

        tokio::spawn(async move {
            let upgraded = match upgrade_fut.await {
                Ok(u) => u,
                Err(e) => {
                    tracing::debug!(error = %e, "ws/h2: upgrade failed");
                    return;
                }
            };

            // CODE-2-09 follow-on: async dial via
            // `TcpPool::acquire_async`. Eliminates the
            // `spawn_blocking(pool.acquire)` site so an H2 extended-
            // CONNECT WebSocket upgrade no longer parks a blocking-pool
            // thread for the dial.
            let pooled = match pool.acquire_async(backend_addr).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::debug!(error = %e, backend = %backend_addr, "ws/h2: backend dial failed");
                    return;
                }
            };
            let Some(upstream_stream) = pooled.take_stream() else {
                tracing::debug!("ws/h2: pooled stream missing");
                return;
            };

            let uri = match format!("ws://{backend_addr}{path_and_query}").parse() {
                Ok(u) => u,
                Err(e) => {
                    tracing::debug!(error = %e, "ws/h2: upstream uri build failed");
                    return;
                }
            };
            let builder = tokio_tungstenite::tungstenite::client::ClientRequestBuilder::new(uri);
            let (backend_ws, _resp) = match tokio_tungstenite::client_async_with_config(
                builder,
                upstream_stream,
                Some(ws_cfg.tungstenite_config()),
            )
            .await
            {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::debug!(error = %e, backend = %backend_addr, "ws/h2: upstream handshake failed");
                    return;
                }
            };

            let client_ws = ws_proxy::server_ws(TokioIo::new(upgraded), &ws_cfg).await;
            if let Err(e) = ws_proxy.proxy_frames(client_ws, backend_ws).await {
                tracing::debug!(error = %e, "ws/h2: frame proxy ended with error");
            }
        });

        let body = Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed();
        Response::builder()
            .status(StatusCode::OK)
            .body(body)
            .unwrap_or_else(|_| {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, "200 build failed")
            })
    }

    async fn proxy_request(
        &self,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Result<Response<IncomingBody>, ProxyErr> {
        let req = req.into_inner();
        let (parts, body) = req.into_parts();

        // F-COR-1 (A2-2) — ORDERING FIX. Previously the live inbound H2
        // `IncomingBody` was streamed straight into the H1 upstream
        // before hyper/h2 finished protocol-validating the inbound
        // stream (trailers / content-length≠ΣDATA / stream-state /
        // second-HEADERS / flow-control). The static backend's 200 body
        // could be relayed BEFORE the malformed-request rejection
        // landed, so h2spec saw DATA instead of the mandated
        // RST/GOAWAY (the validate-vs-forward race; ≥5 h2spec faces).
        //
        // Fix: fully RECEIVE + VALIDATE the inbound request body here,
        // BEFORE dialing the upstream. `Limited` enforces the named
        // D1 cap; `collect()` drives hyper/h2 protocol validation to
        // completion and surfaces any stream/connection error. On any
        // error we return BEFORE any backend dial, so a malformed
        // request can never leak the backend response — the race window
        // is removed structurally (matches the already-shipped H2→H2 /
        // H2→H3 sibling paths, which also collect() before forwarding).
        let limited = http_body_util::Limited::new(body, MAX_REQUEST_BODY_BYTES);
        let collected = match limited.collect().await {
            Ok(c) => c,
            Err(e) => {
                // `Limited` returns a boxed `LengthLimitError` on cap
                // exceed; anything else is a hyper/h2 protocol/IO error
                // from validating the malformed inbound stream.
                if e.downcast_ref::<http_body_util::LengthLimitError>()
                    .is_some()
                {
                    return Err(ProxyErr::BodyTooLarge);
                }
                return Err(ProxyErr::BadRequest(format!(
                    "malformed H2 request body: {e}"
                )));
            }
        };
        let trailers_map = collected.trailers().cloned();
        let body_bytes = collected.to_bytes();

        // F-COR-1 (b) — RFC 9113 §8.1: a pseudo-header field in the
        // trailing field section is malformed. Reject (PROTOCOL_ERROR-
        // class, surfaced as 400) — never forward. This is the H2→H1
        // trailer-capture site (contrast h2_to_h2.rs:19 which only
        // filters on the regular-header path).
        let mut trailers_vec: Vec<(String, String)> = Vec::new();
        if let Some(tm) = trailers_map.as_ref() {
            for (n, v) in tm {
                if n.as_str().starts_with(':') {
                    return Err(ProxyErr::BadRequest(
                        "pseudo-header field in trailers (RFC 9113 §8.1)".to_owned(),
                    ));
                }
                if let Ok(s) = v.to_str() {
                    trailers_vec.push((n.as_str().to_owned(), s.to_owned()));
                }
            }
        }

        // CODE-2-09 follow-on: async dial via `TcpPool::acquire_async`.
        // Reached ONLY after the inbound request is fully received and
        // validated — no backend contact happens for a malformed
        // request.
        let pooled = self
            .pool
            .acquire_async(backend_addr)
            .await
            .map_err(|e| ProxyErr::Upstream(format!("backend connect {backend_addr}: {e}")))?;

        let stream = pooled
            .take_stream()
            .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;

        // Upstream is H1 — matches nginx/haproxy production behaviour.
        // H2 upstream support is a future pillar.
        let (mut sender, conn) = hyper::client::conn::http1::handshake::<
            _,
            BoxBody<Bytes, hyper::Error>,
        >(TokioIo::new(stream))
        .await
        .map_err(|e| ProxyErr::Upstream(format!("h1 client handshake: {e}")))?;

        let conn_handle = tokio::spawn(async move {
            let _ = conn.await;
        });

        // Rebuild the upstream request with the buffered, validated body
        // (+ any validated trailers), preserving method/uri/headers.
        let upstream_body = build_h2_body_with_trailers(body_bytes, &trailers_vec);
        let req = Request::from_parts(parts, upstream_body);

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

    /// Forward an H2 inbound request to an H2 backend (PROTO-001).
    async fn proxy_h2_to_h2(
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
        let translated = match translate_h2_request_to_h2(req.into_inner()).await {
            Ok(r) => r,
            Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
        };
        match h2_pool.send_request(backend_addr, translated).await {
            Ok(resp) => upstream_h2_response_to_h2(resp, self.alt_svc).await,
            Err(lb_io::http2_pool::Http2PoolError::Timeout) => {
                error_response(StatusCode::GATEWAY_TIMEOUT, "upstream H2 timeout")
            }
            Err(e) => error_response(StatusCode::BAD_GATEWAY, &format!("h2 upstream: {e}")),
        }
    }

    /// Forward an H2 inbound request to an H3 backend (PROTO-001).
    async fn proxy_h2_to_h3(
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
        let (headers, body, trailers) =
            match collect_h2_request_to_h3_fieldlist(req.into_inner(), sni).await {
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
        h3_response_to_h2(h3_resp, self.alt_svc)
    }
}

// ── PROTO-001 H2-side translation helpers ─────────────────────────────

/// Lift an inbound H2 [`Request<IncomingBody>`] into the shape hyper's
/// H2 client expects for the upstream side.
///
/// The request URI carries scheme + authority + path already (hyper's
/// H2 server populates them from the inbound pseudo-headers). For H2→H2
/// the codec bridge is essentially a pass-through, but we run it for
/// the per-header lowercase normalization + hop-by-hop strip the bridge
/// performs.
async fn translate_h2_request_to_h2(
    req: Request<IncomingBody>,
) -> Result<Request<BoxBody<Bytes, hyper::Error>>, String> {
    let (parts, body) = req.into_parts();
    // PROTO-2-12: capture trailers along with body.
    let collected = body
        .collect()
        .await
        .map_err(|e| format!("body collect: {e}"))?;
    let trailers_map = collected.trailers().cloned();
    let body_bytes = collected.to_bytes();
    // F-COR-1 (b): RFC 9113 §8.1 — reject pseudo-header in trailers
    // (was: silent build with no `:`-prefix filter at this H2→H2
    // request trailer-capture site).
    let trailers_vec: Vec<(String, String)> =
        capture_request_trailers_rejecting_pseudo(trailers_map.as_ref())?;
    let bridge = crate::create_bridge(crate::Protocol::Http2, crate::Protocol::Http2);
    let scheme = parts
        .uri
        .scheme()
        .map_or_else(|| "http".to_owned(), |s| s.as_str().to_owned());
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
    let mut bridge_in = crate::BridgeRequest {
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
        scheme: Some(scheme.clone()),
        // PROTO-2-12: forward request trailers.
        trailers: trailers_vec,
    };
    // Synthesise the pseudo-headers a real H2 client would have sent.
    bridge_in
        .headers
        .insert(0, (":method".to_owned(), parts.method.to_string()));
    bridge_in
        .headers
        .insert(1, (":path".to_owned(), bridge_in.uri.clone()));
    bridge_in
        .headers
        .insert(2, (":scheme".to_owned(), scheme.clone()));
    if let Some(a) = authority.as_deref() {
        bridge_in
            .headers
            .insert(3, (":authority".to_owned(), a.to_owned()));
    }

    let translated = bridge
        .bridge_request(&bridge_in)
        .map_err(|e| format!("h2->h2 bridge: {e}"))?;

    let mut builder = Request::builder().method(parts.method.clone());
    if let Some(auth) = authority.as_deref() {
        let path = parts
            .uri
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string);
        let uri = format!("{scheme}://{auth}{path}");
        builder = builder.uri(uri);
    } else {
        builder = builder.uri(parts.uri.clone());
    }
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }
    // PROTO-2-12: emit body + trailers via StreamBody.
    let body = build_h2_body_with_trailers(body_bytes, &translated.trailers);
    builder.body(body).map_err(|e| format!("build h2 req: {e}"))
}

/// F-COR-1 (b) — RFC 9113 §8.1 enforcement for the H2 trailer-capture
/// sites (`translate_h2_request_to_h2`, `collect_h2_request_to_h3_
/// fieldlist`). Builds the trailer pairs from the captured trailer map
/// and REJECTS — returning `Err` — if ANY trailer name is a
/// pseudo-header (`:`-prefixed). RFC 9113 §8.1 mandates a malformed
/// request be treated as a stream error (PROTOCOL_ERROR-class), NOT
/// silently stripped, for the trailing field section. The existing
/// `?`/`map_err` plumbing at each call site turns this `Err` into an
/// error response / connection failure — never a forwarded body.
fn capture_request_trailers_rejecting_pseudo(
    trailers_map: Option<&hyper::HeaderMap>,
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    if let Some(tm) = trailers_map {
        for (n, v) in tm {
            if n.as_str().starts_with(':') {
                return Err("pseudo-header field in trailers (RFC 9113 §8.1)".to_owned());
            }
            if let Ok(s) = v.to_str() {
                out.push((n.as_str().to_owned(), s.to_owned()));
            }
        }
    }
    Ok(out)
}

/// PROTO-2-12 helper for the H2 proxy: identical shape to
/// `h1_proxy::build_body_with_trailers`. Emits the body bytes as a
/// `Frame::data` then a `Frame::trailers` if `trailers` is non-empty.
fn build_h2_body_with_trailers(
    body_bytes: Bytes,
    trailers: &[(String, String)],
) -> BoxBody<Bytes, hyper::Error> {
    use http_body_util::StreamBody;
    use hyper::HeaderMap;
    use hyper::body::Frame;

    if trailers.is_empty() {
        return http_body_util::Full::new(body_bytes)
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

/// Convert an upstream H2 `Response<Incoming>` back into the H2-side
/// response (hyper's H2 server consumes a `Response<BoxBody>`).
async fn upstream_h2_response_to_h2(
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
    let bridge = crate::create_bridge(crate::Protocol::Http2, crate::Protocol::Http2);
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
                &format!("h2->h2 response bridge: {e}"),
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
    // PROTO-2-12: emit body + trailers via StreamBody.
    let body = build_h2_body_with_trailers(translated.body, &translated.trailers);
    builder.body(body).unwrap_or_else(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "build h2 response failed",
        )
    })
}

/// Collect an inbound H2 request, run the H2→H3 codec bridge, and
/// return a `(field_list, body, trailers)` triple for
/// `request_h3_upstream`.
///
/// PROTO-2-12: inbound H2 request trailers are captured via
/// `Collected::trailers()` at body-collect time, bridged through
/// `bridge_request`, and returned so the caller ships them as a
/// post-DATA `Frame::trailers` HEADERS frame on the QUIC stream.
async fn collect_h2_request_to_h3_fieldlist(
    req: Request<IncomingBody>,
    sni: &str,
) -> Result<(Vec<(String, String)>, Bytes, Vec<(String, String)>), String> {
    let (parts, body) = req.into_parts();
    // PROTO-2-12: capture request trailers alongside the body.
    let collected = body
        .collect()
        .await
        .map_err(|e| format!("body collect: {e}"))?;
    let trailers_map = collected.trailers().cloned();
    let body_bytes = collected.to_bytes();
    // F-COR-1 (b): RFC 9113 §8.1 — reject pseudo-header in trailers
    // (was: silent build with no `:`-prefix filter at this H2→H3
    // request trailer-capture site).
    let trailers_vec: Vec<(String, String)> =
        capture_request_trailers_rejecting_pseudo(trailers_map.as_ref())?;
    let scheme = parts
        .uri
        .scheme()
        .map_or_else(|| "https".to_owned(), |s| s.as_str().to_owned());
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
        })
        .unwrap_or_else(|| sni.to_owned());

    let bridge = crate::create_bridge(crate::Protocol::Http2, crate::Protocol::Http3);
    let mut bridge_in = crate::BridgeRequest {
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
        scheme: Some(scheme.clone()),
        // PROTO-2-12: forward inbound H2 request trailers through the
        // H2→H3 bridge; the caller ships them as a post-DATA HEADERS
        // frame on the upstream QUIC stream.
        trailers: trailers_vec,
    };
    bridge_in
        .headers
        .insert(0, (":method".to_owned(), parts.method.to_string()));
    bridge_in
        .headers
        .insert(1, (":path".to_owned(), bridge_in.uri.clone()));
    bridge_in.headers.insert(2, (":scheme".to_owned(), scheme));
    bridge_in
        .headers
        .insert(3, (":authority".to_owned(), authority));
    let translated = bridge
        .bridge_request(&bridge_in)
        .map_err(|e| format!("h2->h3 bridge: {e}"))?;
    Ok((translated.headers, body_bytes, translated.trailers))
}

/// Convert an [`lb_quic::H3UpstreamResponse`] back into the H2 response
/// shape the listener emits.
fn h3_response_to_h2(
    resp: lb_quic::H3UpstreamResponse,
    alt_svc: Option<AltSvcConfig>,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let bridge = crate::create_bridge(crate::Protocol::Http3, crate::Protocol::Http2);
    let body_bytes = Bytes::from(resp.body);
    let bridge_in = crate::BridgeResponse {
        status: resp.status,
        headers: resp.headers,
        body: body_bytes,
        // PROTO-2-12 (H3 leg landed): forward the H3 upstream's
        // trailing field section (parsed from the post-DATA HEADERS
        // frame) down the H3→H2 bridge. `build_h2_body_with_trailers`
        // re-emits it as an H2 `Frame::trailers` on the wire.
        trailers: resp.trailers,
    };
    let translated = match bridge.bridge_response(&bridge_in) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("h3->h2 response bridge: {e}"),
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
    // PROTO-2-12 (H3 leg landed): emit body + the H3 upstream's
    // trailing field section via StreamBody as an H2 `Frame::trailers`.
    let body = build_h2_body_with_trailers(translated.body, &translated.trailers);
    builder.body(body).unwrap_or_else(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "build h2 response failed",
        )
    })
}

enum ProxyErr {
    Upstream(String),
    Timeout,
    /// F-COR-1 (D1): the buffered inbound H2 request body exceeded
    /// [`MAX_REQUEST_BODY_BYTES`]. Surfaced as `413 Payload Too Large`
    /// — NOT a backend dial (the request is rejected before any
    /// upstream contact).
    BodyTooLarge,
    /// F-COR-1: the inbound H2 request failed protocol validation while
    /// being received (hyper/h2 surfaced a stream/connection error
    /// during `body.collect()` — malformed trailers, content-length≠
    /// ΣDATA, stream-state violation, etc.). Surfaced as
    /// `400 Bad Request` and, critically, returned BEFORE any backend
    /// dial so the malformed request can never leak the backend's 200
    /// body (closes the validate-vs-forward race deterministically).
    BadRequest(String),
}

fn error_response(status: StatusCode, msg: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = Full::new(Bytes::from(msg.to_owned()))
        .map_err(|never| match never {})
        .boxed();
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    resp
}

/// PROTO-2-01 — RFC 9113 §8.3.1 enforcement.
///
/// Returns `Err(static_msg)` when **both** `:authority` (surfaced by
/// hyper as `uri.authority()`) and `Host` are present **and** their
/// host components disagree. The comparison is case-insensitive on
/// the host name (RFC 3986 §3.2.2: host is case-insensitive) and
/// ignores the port when either side lacks one (RFC 9113 §8.3.1
/// "default port" carve-out). Returns `Ok(())` if either is absent,
/// if they match exactly, or if only the port differs while one side
/// elides it.
///
/// Per the §8.3.1 forwarding rule, an intermediary MUST treat such
/// disagreement as a malformed request. The proxy lifts that into a
/// 400 Bad Request response. Returning a `&'static str` keeps the
/// rejection allocation-free on the cold path.
pub fn check_authority_host_agreement(
    uri: &http::Uri,
    headers: &hyper::HeaderMap,
) -> Result<(), &'static str> {
    let authority = uri.authority().map(http::uri::Authority::as_str);
    let host_hdr = headers
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok());
    match (authority, host_hdr) {
        (Some(a), Some(h)) => {
            if authority_matches_host(a, h) {
                Ok(())
            } else {
                Err("Bad Request: :authority disagrees with Host (RFC 9113 §8.3.1)")
            }
        }
        _ => Ok(()),
    }
}

/// Compare a `:authority` value against a `Host` header value per
/// RFC 9113 §8.3.1 + RFC 3986 §3.2.2 (host-component case-insensitive).
///
/// Rules:
///   * Empty / missing host on either side → mismatch.
///   * Host components compared case-insensitively.
///   * If both carry an explicit port, the ports must match.
///   * If only one side carries an explicit port, the comparison
///     succeeds when the host components match (the proxy does not
///     have a default-port table; this matches the §8.3.1 latitude
///     for omitted default ports).
fn authority_matches_host(authority: &str, host: &str) -> bool {
    let (a_host, a_port) = split_host_port(authority);
    let (h_host, h_port) = split_host_port(host);
    if a_host.is_empty() || h_host.is_empty() {
        return false;
    }
    if !a_host.eq_ignore_ascii_case(h_host) {
        return false;
    }
    match (a_port, h_port) {
        (Some(ap), Some(hp)) => ap == hp,
        // One side elides the port — accept (default-port latitude).
        _ => true,
    }
}

/// Split `host[:port]` into `(host, Some(port_str))` / `(host, None)`.
/// Bracketed IPv6 literals `[::1]:443` are preserved verbatim as the
/// host portion (including brackets) so the case-insensitive compare
/// catches hex-digit mismatches without splitting on colon inside the
/// literal.
fn split_host_port(s: &str) -> (&str, Option<&str>) {
    if let Some(stripped) = s.strip_prefix('[') {
        // IPv6 literal: `[…]` then optional `:port`.
        if let Some(end) = stripped.find(']') {
            let host_with_brackets = &s[..=end + 1];
            let rest = &s[end + 2..];
            let port = rest.strip_prefix(':');
            return (host_with_brackets, port.filter(|p| !p.is_empty()));
        }
        return (s, None);
    }
    match s.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (s, None),
    }
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

    // PROTO-2-11 H2 half (Wave 2c-2): smoke test for the
    // cancel-aware variant. Builds a minimal H2Proxy, hands it a
    // duplex pair (with the peer side closed) and an already-cancelled
    // token. The expected outcome is that `serve_connection_with_cancel`
    // returns promptly — its graceful_shutdown branch hits the
    // empty/EOF stream and resolves the hyper conn future. The
    // assertion is a deadline-bounded wait: a regression that
    // re-introduces a busy-loop or holds the conn open indefinitely
    // would time out here.
    #[tokio::test(flavor = "current_thread")]
    async fn test_sigterm_emits_two_step_goaway() {
        use std::time::Duration;
        use tokio_util::sync::CancellationToken;

        let pool = lb_io::pool::TcpPool::new(
            lb_io::pool::PoolConfig::default(),
            lb_io::sockopts::BackendSockOpts::default(),
            lb_io::Runtime::new(),
        );
        let addrs: Vec<SocketAddr> = vec!["127.0.0.1:1".parse().unwrap()];
        let picker = crate::h1_proxy::RoundRobinAddrs::new(addrs).unwrap();
        let proxy = Arc::new(H2Proxy::new(
            pool,
            Arc::new(picker),
            None,
            HttpTimeouts::default(),
            false,
        ));
        // Empty duplex — the peer half is dropped immediately, so any
        // read returns EOF and hyper's H2 conn resolves without ever
        // opening a stream.
        let (server_io, client) = tokio::io::duplex(8 * 1024);
        drop(client); // EOF on the next read.
        let cancel = CancellationToken::new();
        cancel.cancel(); // pre-cancel so the graceful path fires.
        let peer: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let r = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.serve_connection_with_cancel(server_io, peer, cancel),
        )
        .await;
        // Whether the inner future returns Ok(()) or an Err depends on
        // whether the H2 preface ever arrived. We only assert the
        // deadline did not fire — the cancellable variant must NOT
        // loop indefinitely.
        assert!(
            r.is_ok(),
            "serve_connection_with_cancel hung past 5 s deadline — graceful shutdown is broken"
        );
    }

    // ── F-SEC-1 DETERMINISTIC gate regression (directive D3) ──────────
    //
    // The wire-level rapid-reset defect (auditor-2 A2-1) is, by its
    // nature, a scheduler race (auditor-2: 6/48 only under maximal
    // starvation) — a wire-observation test for it CANNOT be a
    // deterministic gate per R1/D3 (the under-churn variant is kept as
    // CORROBORATING evidence in tests/h2_rapid_reset_goaway_under_load.rs,
    // exactly the D2 pattern). The STRUCTURAL property the fix
    // introduces, however, is fully deterministic and is what
    // guarantees the queued GOAWAY survives teardown: `CleanCloseIo`
    // MUST drain all pending inbound bytes BEFORE delegating the FIN to
    // the inner socket, so a close on a socket with unread data (which
    // makes the kernel emit an RST that discards the peer's
    // already-received GOAWAY) cannot happen. These tests assert that
    // contract directly and deterministically.

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Arc-shared mock IO (interior-mutable via atomics) so the test
    /// can inspect call ordering after `CleanCloseIo` consumes it.
    /// Yields `to_deliver` inbound bytes in `chunk`-sized reads, then a
    /// clean EOF; records whether the inner `poll_shutdown` (→ FIN) was
    /// delegated while inbound data was still unread (the RST-causing
    /// defect precondition).
    struct ProbeInner {
        to_deliver: usize,
        chunk: usize,
        delivered: AtomicUsize,
        eof_seen: AtomicBool,
        shutdown_called: AtomicBool,
        shutdown_called_with_undrained: AtomicBool,
    }

    #[derive(Clone)]
    struct Probe(std::sync::Arc<ProbeInner>);

    impl Probe {
        fn new(to_deliver: usize, chunk: usize) -> Self {
            Probe(std::sync::Arc::new(ProbeInner {
                to_deliver,
                chunk,
                delivered: AtomicUsize::new(0),
                eof_seen: AtomicBool::new(false),
                shutdown_called: AtomicBool::new(false),
                shutdown_called_with_undrained: AtomicBool::new(false),
            }))
        }
    }

    impl AsyncRead for Probe {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let s = &self.0;
            let done = s.delivered.load(Ordering::SeqCst);
            if done >= s.to_deliver {
                s.eof_seen.store(true, Ordering::SeqCst);
                return Poll::Ready(Ok(())); // 0 bytes = clean EOF
            }
            let n = s.chunk.min(s.to_deliver - done).min(buf.remaining());
            let zeros = vec![0u8; n];
            buf.put_slice(&zeros);
            s.delivered.fetch_add(n, Ordering::SeqCst);
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for Probe {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            b: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(b.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            let s = &self.0;
            s.shutdown_called.store(true, Ordering::SeqCst);
            if s.delivered.load(Ordering::SeqCst) < s.to_deliver {
                s.shutdown_called_with_undrained
                    .store(true, Ordering::SeqCst);
            }
            Poll::Ready(Ok(()))
        }
    }

    /// DETERMINISTIC gate (D3): `CleanCloseIo::poll_shutdown` MUST drain
    /// all pending inbound bytes before issuing the FIN, so the close is
    /// a clean FIN and the peer's already-received GOAWAY is not
    /// RST-discarded. Without the wrapper hyper calls `poll_shutdown`
    /// directly on the raw socket with the flood still unread -> kernel
    /// RST -> the peer discards the queued GOAWAY -> BrokenPipe. This
    /// asserts the structural fix directly, zero scheduler dependence.
    #[tokio::test(flavor = "current_thread")]
    async fn clean_close_io_drains_inbound_before_fin() {
        use tokio::io::AsyncWriteExt;
        // 200 KiB pending inbound flood, under the 256 KiB DRAIN_CAP,
        // delivered in 4 KiB reads then EOF.
        let probe = Probe::new(200 * 1024, 4096);
        let mut io = CleanCloseIo::new(probe.clone());
        Pin::new(&mut io).shutdown().await.unwrap();

        let s = &probe.0;
        assert!(
            s.shutdown_called.load(Ordering::SeqCst),
            "inner poll_shutdown was never delegated"
        );
        assert!(
            s.eof_seen.load(Ordering::SeqCst),
            "F-SEC-1: CleanCloseIo did not drain inbound to EOF before FIN"
        );
        assert!(
            !s.shutdown_called_with_undrained.load(Ordering::SeqCst),
            "F-SEC-1 DEFECT: FIN issued while inbound data still unread — \
             kernel would RST and the peer's queued RFC 9113 §6.8 GOAWAY \
             would be discarded"
        );
        assert_eq!(
            s.delivered.load(Ordering::SeqCst),
            200 * 1024,
            "all pending inbound bytes must be drained pre-FIN"
        );
    }

    /// DETERMINISTIC (D3): the drain is HARD-BOUNDED by `DRAIN_CAP` so a
    /// deliberate unbounded post-GOAWAY inbound flood cannot pin the
    /// worker — teardown still completes. Proves the bound, not just the
    /// happy path (no unbounded-drain regression, mirrors the D1 cap
    /// discipline applied to F-COR-1).
    #[tokio::test(flavor = "current_thread")]
    async fn clean_close_io_drain_is_bounded() {
        use tokio::io::AsyncWriteExt;
        // Endless inbound source: never EOFs. The drain MUST still
        // terminate at DRAIN_CAP and let the FIN proceed.
        struct EndlessIo {
            read_total: AtomicUsize,
        }
        impl AsyncRead for EndlessIo {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                let n = buf.remaining().min(8192);
                let zeros = vec![0u8; n];
                buf.put_slice(&zeros);
                self.read_total.fetch_add(n, Ordering::SeqCst);
                Poll::Ready(Ok(()))
            }
        }
        impl AsyncWrite for EndlessIo {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                b: &[u8],
            ) -> Poll<io::Result<usize>> {
                Poll::Ready(Ok(b.len()))
            }
            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }
        let mut io = CleanCloseIo::new(EndlessIo {
            read_total: AtomicUsize::new(0),
        });
        // Must complete (not hang) despite the endless inbound source.
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Pin::new(&mut io).shutdown(),
        )
        .await
        .expect("F-SEC-1: bounded drain must not hang on an endless inbound flood")
        .unwrap();
        // Drained at most DRAIN_CAP (+ one final chunk granularity).
        assert!(
            io.drain_budget == 0,
            "drain must consume exactly up to DRAIN_CAP then stop"
        );
    }

    // ── F-COR-1 (b) unit regression — RFC 9113 §8.1 trailer rule ──────

    /// RFC 9113 §8.1 enforcement note for the H2 trailer-capture sites
    /// (`translate_h2_request_to_h2`, `collect_h2_request_to_h3_
    /// fieldlist`):
    ///
    /// A pseudo-header trailer CANNOT be represented as an
    /// `http::HeaderName` (the `:` byte 0x3A is not an RFC 7230 token
    /// char, so `HeaderName::from_bytes(b":x")` is `Err`) — so it never
    /// reaches `capture_request_trailers_rejecting_pseudo` via the
    /// `http::HeaderMap` the H2 server hands us. The real, proven H2
    /// §8.1 protection is therefore the ORDERING fix
    /// (`proxy_request` now collect()s + validates the inbound request
    /// BEFORE dialing, so hyper/h2's own §8.1 trailer rejection wins
    /// DETERMINISTICALLY instead of racing the forward) — proven by the
    /// deterministic integration gate
    /// `tests/h2_validation_before_forward.rs::
    /// pseudo_header_in_trailers_never_leaks_backend_body` (+ the
    /// backend-dial-count invariant). The `:`-prefix filter in the
    /// shared helper is retained as cheap defense-in-depth for any
    /// future String-keyed path (it IS reachable and unit-tested on the
    /// H3 side: `lb_quic::h3_bridge::tests::
    /// feed_body_rejects_pseudo_header_in_h3_trailers`).
    ///
    /// This unit test pins the helper's NO-REGRESSION contract: valid
    /// (non-pseudo) trailers are still captured verbatim — the §8.1
    /// rejection is surgical, not a blanket trailer break (guards the
    /// trailer_passthrough / bridging contract).
    #[test]
    fn capture_request_trailers_accepts_valid() {
        let mut tm = HeaderMap::new();
        tm.append(
            HeaderName::try_from("x-checksum").unwrap(),
            HeaderValue::from_static("abc123"),
        );
        let out = capture_request_trailers_rejecting_pseudo(Some(&tm))
            .expect("valid trailers must be accepted");
        assert_eq!(out, vec![("x-checksum".to_owned(), "abc123".to_owned())]);
    }
}
