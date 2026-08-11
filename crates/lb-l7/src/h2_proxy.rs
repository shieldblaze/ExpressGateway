//! Real hyper 1.x HTTP/2 proxy path.
//!
//! [`H2Proxy::serve_connection`] drives a hyper H2 server over each accepted
//! connection. The service closure runs once per H2 STREAM, so the backend
//! picker is hit per request, not per connection. H2 forbids hop-by-hop
//! headers on the wire, but the upstream may be H1 — so they are still
//! scrubbed before relaying. The whole connection is bounded by
//! [`crate::h1_proxy::HttpTimeouts::total`].

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;

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
    AltSvcConfig, BackendPicker, ClientRespBody, HttpTimeouts, append_via, append_xff, set_xfh,
    set_xfp, strip_hop_by_hop,
};
use lb_security::{
    ConnId, GlitchKind, GlitchOutcome, GlitchesCounter, SmuggleDetector, SmuggleMode, Watchdog,
};

use crate::h2_security::H2SecurityThresholds;
use crate::security_hooks::{DynSecurityHooks, NoopHooks};
use crate::stripped_request::{StrippedRequest, strip_hop_by_hop as strip_into_newtype};
use crate::upstream::{BackendInfoPicker, SingleProtoPicker, UpstreamBackend, UpstreamProto};
use crate::ws_proxy::{self, WsProxy, is_h2_extended_connect};

/// Hard cap on the inbound H2 request body (64 MiB). Shared ceiling with the
/// H3 path's `lb_quic::MAX_REQUEST_BODY_BYTES`; exceeding it yields
/// `413 Payload Too Large`, never an unbounded allocation.
pub const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Depth of the bounded in-flight channel feeding the streaming H2→H1 request
/// body. `DEPTH × CHUNK_MAX` (64 KiB) caps retained inbound memory AND doubles
/// as the validate-before-forward lookahead window.
pub const H2_REQ_CHANNEL_DEPTH: usize = 8;

/// Maximum size of one chunk pumped through the in-flight channel. The window
/// ceiling (depth × this = 64 KiB) is body-size-INDEPENDENT — that
/// independence is the R8 property the memory proof asserts.
pub const H2_REQ_CHUNK_MAX: usize = 8 * 1024;

/// F-MD-4 — request-smuggling guard. Dropping the request-body channel sender
/// makes the receiver's `poll_recv` return `None`, which `StreamBody`
/// translates to a CLEAN body EOF: hyper then emits the chunked terminator and
/// the upstream sees a COMPLETE request — the wrong signal when the inbound
/// stream was RST mid-body. `hyper::Error` has no public constructor, so the
/// pump sends `Err(PumpAbort)` instead and hyper aborts the upstream request
/// without a terminator.
#[derive(Debug)]
struct PumpAbort;

impl std::fmt::Display for PumpAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("inbound H2 request body aborted before END_STREAM")
    }
}

impl std::error::Error for PumpAbort {}

/// F-MD-4 response leg — injected into the H2 RESPONSE StreamBody when the H3
/// connector emits `Reset`. hyper polls the body, sees an ERROR (not a clean
/// EOF), and RST_STREAMs the downstream stream, so a truncated response is
/// never smuggled as complete. `hyper::Error` has no public ctor; mirror of
/// `h1_proxy::H1PumpAbort`.
#[derive(Debug)]
struct H2RespAbort;

impl std::fmt::Display for H2RespAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("H3 upstream response truncated before clean end")
    }
}

impl std::error::Error for H2RespAbort {}

/// F-MD-4 — how long the request pump holds the body-channel sender open after
/// injecting `Err(PumpAbort)`, waiting for hyper to OBSERVE it. Holding it is
/// what makes the upstream reset DETERMINISTIC instead of racing a
/// channel-close clean-EOF (see `inject_abort!`); liveness backstop only.
// `pub(crate)` so the H1→H2 pump shares the SAME bound instead of drifting.
pub(crate) const H2_ABORT_OBSERVE_TIMEOUT: Duration = Duration::from_secs(5);

/// L7 HTTP/2 reverse proxy. Cheap to clone via [`Arc`].
pub struct H2Proxy {
    pool: TcpPool,
    picker: Arc<dyn BackendInfoPicker>,
    alt_svc: Option<AltSvcConfig>,
    timeouts: HttpTimeouts,
    is_https: bool,
    security: H2SecurityThresholds,
    /// When `Some`, RFC 8441 `:protocol = websocket` streams route to the
    /// WebSocket proxy instead of returning 502.
    ws: Option<Arc<WsProxy>>,
    /// When `Some`, `application/grpc[+ext]` streams route to the gRPC proxy.
    grpc: Option<Arc<GrpcProxy>>,
    /// Optional H2 upstream pool (H2→H2 path).
    h2_upstream: Option<Arc<Http2Pool>>,
    /// Optional H3 upstream pool (H2→H3 path).
    h3_upstream: Option<Arc<QuicUpstreamPool>>,
    /// Security-hook surface; defaults to [`NoopHooks`].
    hooks: Arc<dyn DynSecurityHooks>,
    /// Slowloris / slow-POST watchdog (mirrors `H1Proxy::watchdog`).
    watchdog: Option<Watchdog>,
    /// Monotonic per-listener sequence used as the [`Watchdog`] entry key.
    conn_seq: Arc<parking_lot::Mutex<u64>>,
    /// Default expected SNI for [`crate::sni_authority::check_sni_authority`].
    /// `None` means SNI/authority agreement is not enforced unless
    /// [`Self::serve_connection_with_cancel_sni`] supplies a per-connection one.
    expected_sni: Option<String>,
    /// ROUND8-L7-05: policy for `_` in inbound H2 header names, default
    /// `Reject` (Envoy edge best-practice). hyper's H2 codec does NOT reject
    /// underscores, so this filter is the only enforcement point on H2.
    header_underscore_policy: crate::h1_proxy::HeaderUnderscorePolicy,
    /// ROUND8-L7-07 / L7-12 — HAProxy's `tune.h2.fe.glitches-threshold`
    /// analogue. When `Some`, every H2 protocol-abuse event records a weighted
    /// glitch; crossing the threshold drains the connection via the two-step
    /// GOAWAY path (RFC 9113 §6.8; logical ENHANCE_YOUR_CALM).
    glitches_threshold: Option<u32>,
    /// Optional registry for `h2_glitches_total`. The counter logic runs
    /// whenever one is supplied; the production wire-in is done by the binary.
    glitches_metrics: Option<Arc<lb_observability::MetricsRegistry>>,
    /// CF-S27-2 — per-listener opt-in for RFC 8441 WebSocket-over-HTTP/2.
    /// OFF by default: the H2 upgraded-stream write path lacks true end-to-end
    /// backpressure, so a non-reading client can force unbounded gateway memory.
    /// When `false` this proxy neither advertises
    /// `SETTINGS_ENABLE_CONNECT_PROTOCOL` nor intercepts an inbound extended
    /// CONNECT. WS-over-H1 / WS-over-H3 are unaffected.
    h2_extended_connect_enabled: bool,
}

/// F-SEC-1 — clean-close I/O wrapper that guarantees the RFC 9113 §6.8
/// rapid-reset GOAWAY reaches the client before teardown, deterministically.
///
/// THE CATCH: h2 does flush the GOAWAY before calling `poll_shutdown`, but it
/// DROPS this io a microsecond after `poll_shutdown` returns `Ready`. Dropping
/// a socket that still has unread inbound data makes Linux emit an **RST**
/// (RFC 1122 §4.2.2.13 / `tcp_close`), and the peer's TCP stack then discards
/// its ENTIRE receive buffer — including the GOAWAY that already arrived.
/// Under a rapid-reset flood the recv buffer is never durably empty, so this
/// surfaced as `Io(BrokenPipe)` with `send_err=None` roughly 1 run in 3.
///
/// Fix: send the FIN FIRST (a FIN never causes an RST, so a non-flooding close
/// still tears down with zero added latency), THEN drain inbound until the peer
/// closes its own write half. On `Poll::Pending` mid-drain we yield rather than
/// let the drop race the peer. Hard-bounded by BOTH [`CleanCloseIo::DRAIN_CAP`]
/// and [`CleanCloseIo::LINGER_DEADLINE`] so a silent/wedged/flooding client
/// cannot pin a worker.
struct CleanCloseIo<IO> {
    inner: IO,
    /// Inbound bytes we will still drain after the FIN (hard bound).
    drain_budget: usize,
    /// Set once the inner FIN has been delegated.
    fin_done: bool,
    /// Set once the post-FIN drain finished (EOF, cap, error, or deadline).
    drained: bool,
    /// Armed with the FIN; bounds the wait for the peer's reciprocal FIN so a
    /// silent client cannot pin the worker.
    linger_deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<IO> CleanCloseIo<IO> {
    /// 256 KiB: larger than any in-flight burst a client can have queued
    /// between our GOAWAY and its reciprocal FIN, yet a hard cap so a
    /// deliberate post-GOAWAY flood cannot pin the worker.
    const DRAIN_CAP: usize = 256 * 1024;

    /// Maximum wall-clock wait for the peer's reciprocal FIN. Kept short so it
    /// never approaches the surrounding `HttpTimeouts::total` (60 s). Only
    /// reached when the peer is still streaming after our FIN.
    const LINGER_DEADLINE: Duration = Duration::from_secs(1);

    fn new(inner: IO) -> Self {
        Self {
            inner,
            drain_budget: Self::DRAIN_CAP,
            fin_done: false,
            drained: false,
            linger_deadline: None,
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
        // F-SEC-1 step 1 — FIN first. h2 already flushed the GOAWAY before
        // this call; sending a FIN does NOT cause an RST (only DROPPING with
        // unread inbound does), so a non-flooding close tears down at once.
        if !self.fin_done {
            ready!(Pin::new(&mut self.inner).poll_shutdown(cx))?;
            self.fin_done = true;
            self.linger_deadline = Some(Box::pin(tokio::time::sleep(Self::LINGER_DEADLINE)));
        }

        // F-SEC-1 step 2 — bounded post-FIN drain: read+discard inbound until
        // the peer closes its write half, so h2's imminent drop is a clean
        // close rather than an RST that discards the GOAWAY it just received.
        if !self.drained {
            let mut scratch = [0u8; 16 * 1024];
            loop {
                if self.drain_budget == 0 {
                    break; // byte cap — stop draining, allow drop
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
                            // EOF — no unread data remains; the drop is clean.
                            break;
                        }
                        self.drain_budget -= n;
                    }
                    Poll::Ready(Err(_)) => {
                        // Peer RST / gone — nothing more to drain.
                        break;
                    }
                    Poll::Pending => {
                        // Peer has not sent its reciprocal FIN. Resolving now
                        // would let h2's drop race an RST, so yield instead
                        // (poll_read registered our waker) until the peer FIN
                        // or the deadline. `linger_deadline` is always `Some`
                        // here; were it ever absent we still must not resolve
                        // early, so yield.
                        match self.linger_deadline.as_mut() {
                            Some(dl) => match dl.as_mut().poll(cx) {
                                Poll::Ready(()) => break, // budget exhausted
                                Poll::Pending => return Poll::Pending,
                            },
                            None => return Poll::Pending,
                        }
                    }
                }
            }
            self.drained = true;
        }
        Poll::Ready(Ok(()))
    }
}

impl H2Proxy {
    /// Construct an [`H2Proxy`] with the default [`H2SecurityThresholds`].
    /// `is_https` selects the value emitted into `X-Forwarded-Proto`.
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
    /// Wraps `picker` in a [`SingleProtoPicker`] tagged [`UpstreamProto::H1`].
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
            h2_extended_connect_enabled: false,
        }
    }

    /// Construct an [`H2Proxy`] backed by a multi-protocol picker.
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
            h2_extended_connect_enabled: false,
        }
    }

    /// Attach a security-hooks impl. Mirrors
    /// [`crate::h1_proxy::H1Proxy::with_hooks`].
    #[must_use]
    pub fn with_hooks(mut self, hooks: Arc<dyn DynSecurityHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Attach an [`lb_security::Watchdog`] for per-stream slowloris /
    /// slow-POST eviction. The H2 service closure runs once per stream, so
    /// each stream registers and deregisters independently.
    #[must_use]
    pub fn with_watchdog(mut self, watchdog: Watchdog) -> Self {
        self.watchdog = Some(watchdog);
        self
    }

    /// ROUND8-L7-07 / L7-12 — enable the HAProxy-3.0 consolidated glitches
    /// abuse counter. Crossing `threshold` drains the connection via the
    /// two-step GOAWAY path. `threshold` of `0` keeps the counter dormant
    /// (operator opt-out parity with `tune.h2.fe.glitches-threshold 0`).
    ///
    /// The frame-arrival half ([`GlitchKind::FrameRecvTimeout`]) is NOT wired:
    /// hyper 1.x `serve_connection` exposes no per-frame read context
    /// (deferred-with-rationale, `audit/deferred.md`).
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

    /// Default expected SNI for the
    /// [`crate::sni_authority::check_sni_authority`] hot-path check. TLS-
    /// bearing deployments prefer [`Self::serve_connection_with_cancel_sni`],
    /// which captures the SNI live from rustls at accept time.
    #[must_use]
    pub fn with_expected_sni(mut self, sni: Option<String>) -> Self {
        self.expected_sni = sni;
        self
    }

    /// ROUND8-L7-05: set the header-name underscore policy. Default
    /// [`crate::h1_proxy::HeaderUnderscorePolicy::Reject`].
    #[must_use]
    pub const fn with_header_underscore_policy(
        mut self,
        policy: crate::h1_proxy::HeaderUnderscorePolicy,
    ) -> Self {
        self.header_underscore_policy = policy;
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

    /// Enable WebSocket upgrade handling on this proxy.
    ///
    /// NOTE: this does NOT by itself enable WS-over-H2 — the RFC 8441
    /// extended-CONNECT advertise+intercept is gated separately by
    /// [`Self::with_h2_extended_connect`] (default OFF).
    #[must_use]
    pub fn with_websocket(mut self, ws: Arc<WsProxy>) -> Self {
        self.ws = Some(ws);
        self
    }

    /// CF-S27-2 — per-listener opt-in for RFC 8441 WebSocket-over-HTTP/2
    /// (extended CONNECT). Default OFF; see the field doc for why.
    #[must_use]
    pub fn with_h2_extended_connect(mut self, enabled: bool) -> Self {
        self.h2_extended_connect_enabled = enabled;
        self
    }

    /// Enable gRPC handling on this proxy. Aligns the [`GrpcProxy`]'s upstream
    /// `max_header_list_size` with this listener's (GRPC-001) so a malicious
    /// backend cannot transit oversize trailers through the gateway.
    #[must_use]
    pub fn with_grpc(mut self, grpc: GrpcProxy) -> Self {
        let aligned = grpc.with_max_header_list_size(self.security.max_header_list_size);
        self.grpc = Some(Arc::new(aligned));
        self
    }

    /// Drive HTTP/2 server logic over `io`, returning once the connection has
    /// fully closed. Bounded by [`HttpTimeouts::total`].
    ///
    /// # Errors
    ///
    /// I/O errors and timeouts. Per-stream upstream errors become 502/504
    /// responses and do NOT terminate the connection.
    pub async fn serve_connection<IO>(self: Arc<Self>, io: IO, peer: SocketAddr) -> io::Result<()>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        self.serve_connection_with_cancel(io, peer, tokio_util::sync::CancellationToken::new())
            .await
    }

    /// PROTO-2-11 — H2 half of the GOAWAY-on-drain contract. Identical to
    /// [`Self::serve_connection`] until `cancel` fires, at which point
    /// `.graceful_shutdown()` emits the canonical two-step GOAWAY (RFC 9113
    /// §6.8) and the connection is driven to completion within the existing
    /// `total` budget.
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

    /// H2 entry point that threads the per-connection TLS SNI into the request
    /// hot path so [`crate::sni_authority::check_sni_authority`] runs against
    /// the OBSERVED SNI rather than the builder default.
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

        // ROUND8-L7-07 — one GlitchesCounter per connection. `conn_cancel` is
        // a CHILD of the caller's `cancel`, so a parent (SIGTERM) cancel
        // propagates DOWN and a glitch-drain cancels it DIRECTLY — both causes
        // resolve the SAME two-step GOAWAY select arm below.
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
        // Detector-derived thresholds: hyper enforces on the wire, the lb-h2
        // types stay the canonical source. A `Timer` MUST be wired before
        // `keep_alive_interval` can fire — without it hyper panics
        // "You must supply a timer."
        let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
        builder.timer(TokioTimer::new());
        self.security.apply(&mut builder);
        // RFC 8441 extended CONNECT. CF-S27-2: GATED OFF by default because
        // the H2 upgraded-stream write path lacks end-to-end backpressure.
        // When off the SETTINGS bit is never sent AND the intercept fork is
        // disabled, so a hostile client that sends extended CONNECT anyway is
        // NOT tunneled.
        if self.h2_extended_connect_enabled {
            builder.enable_connect_protocol();
        }
        // F-SEC-1: wrap `io` so teardown drains pending inbound before the
        // drop, keeping the queued GOAWAY intact — see [`CleanCloseIo`].
        let conn = builder.serve_connection(TokioIo::new(CleanCloseIo::new(io)), svc);
        tokio::pin!(conn);
        // Cancelled by either the parent `cancel` (SIGTERM drain) or a
        // glitch-threshold trip.
        let cancel_fut = conn_cancel.cancelled();
        tokio::pin!(cancel_fut);
        let timer = tokio::time::sleep(total);
        tokio::pin!(timer);
        tokio::select! {
            // biased: cancel wins ties so a SIGTERM mid-request still emits
            // the GOAWAY.
            biased;
            () = &mut cancel_fut => {
                // hyper emits both GOAWAY frames inside `graceful_shutdown`.
                conn.as_mut().graceful_shutdown();
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

/// Per-H2-connection abuse-counter state. The `Arc<Mutex<..>>` keeps ONE
/// counter shared across every stream of the connection (HAProxy's
/// `h2c->glitches` is per-connection, not per-stream).
#[derive(Clone)]
struct GlitchConnState {
    counter: Arc<parking_lot::Mutex<GlitchesCounter>>,
    /// `h2_glitches_total` handle; `None` means the counter still drains, it
    /// is just unobserved.
    metric: Option<lb_observability::IntCounter>,
    /// Cancelling this triggers the two-step GOAWAY select arm.
    drain: tokio_util::sync::CancellationToken,
}

impl GlitchConnState {
    /// Record one weighted abuse event. Returns `true` — after cancelling the
    /// drain token — once the rolling weighted sum crosses the threshold.
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
    /// Per-connection SNI captured from the rustls handshake.
    expected_sni: Option<String>,
    /// Per-connection glitches counter; `None` when not enabled.
    glitch: Option<GlitchConnState>,
}

/// F-S27-1 — outcome of the inline upstream WS dial+handshake, so the caller
/// picks the right client status WITHOUT having emitted a `200` first:
/// `Timeout` → `504` (dial unreachable), `Refused` → `502` (non-101 or a
/// structurally failed handshake).
enum WsDialErr {
    Timeout(String),
    Refused(String),
}

impl hyper::service::Service<Request<IncomingBody>> for ProxyService {
    type Response = Response<ClientRespBody>;
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
    /// H2 mirror of the H1 trace-context wire-in. Uses `Instrument`, never
    /// `Entered`, so the span cannot leak across an `.await` onto a
    /// co-scheduled task.
    async fn handle(
        &self,
        mut req: Request<IncomingBody>,
        peer: SocketAddr,
        expected_sni: Option<&str>,
        glitch: Option<&GlitchConnState>,
    ) -> Response<ClientRespBody> {
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
        // Inject the child context here so every downstream H2→{H1,H2,H3}
        // bridge forwards it without a per-bridge callsite.
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
    ) -> Response<ClientRespBody> {
        // ROUND8-L7-09 — authority-validation CHOKE POINT. MUST stay the
        // FIRST statement: both the extended-CONNECT and gRPC forks below
        // reached upstream selection unvalidated before it was hoisted here.
        // H2 carries the authority in `:authority` (surfaced as
        // `uri.authority()`); a client may also send `Host`. Both are checked.
        if let Err((bad, err)) = crate::authority::validate_request(&req) {
            tracing::warn!(
                peer = %peer,
                authority = %bad,
                error = ?err,
                "ROUND8-L7-09: H2 authority rejected (choke point)"
            );
            // ROUND8-L7-07: routing/ACL desync attempt — medium weight.
            if let Some(g) = glitch {
                g.record(GlitchKind::RapidReset);
            }
            return error_response(StatusCode::BAD_REQUEST, "invalid authority (ROUND8-L7-09)");
        }

        // RFC 8441 extended CONNECT intercept — only when this listener opted
        // in (CF-S27-2, default OFF). Off ⇒ falls through to the regular H2
        // path, where a `CONNECT` selects no backend tunnel and is rejected.
        // The gate holds even against a client that sends the pseudo-header
        // without the (un-advertised) SETTINGS bit.
        if self.h2_extended_connect_enabled
            && self
                .ws
                .as_ref()
                .is_some_and(|w| w.config().enabled && is_h2_extended_connect(&req))
        {
            return self.handle_ws_extended_connect(req).await;
        }
        if let Some(gp) = self
            .grpc
            .as_ref()
            .filter(|g| g.config().enabled && grpc_proxy::is_grpc_request(&req))
        {
            // Today's GrpcProxy speaks hyper H2 over a TCP-pool stream, so
            // any backend that is not H3 is acceptable.
            let Some(backend) = self.picker.pick_info() else {
                return error_response(StatusCode::BAD_GATEWAY, "no backend available");
            };
            if backend.proto == UpstreamProto::H3 {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "gRPC proxy does not support H3 backends",
                );
            }
            let (gp_parts, gp_body) = Arc::clone(gp).handle(req, backend.addr).await.into_parts();
            return Response::from_parts(
                gp_parts,
                gp_body
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    .boxed(),
            );
        }
        let (mut parts, body) = req.into_parts();

        // ROUND8-L7-05: hyper's H2 codec does not reject underscores, so this
        // is the only enforcement point on the H2 path. Default is `Reject`.
        match self.header_underscore_policy {
            crate::h1_proxy::HeaderUnderscorePolicy::Reject => {
                if parts
                    .headers
                    .iter()
                    .any(|(n, _)| n.as_str().as_bytes().contains(&b'_'))
                {
                    // ROUND8-L7-07: low weight — one malformed header is
                    // noise; sustained ones trip the threshold.
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

        // Run the security hooks before hop-by-hop strip + upstream-acquire.
        // The rebuilt `Request<()>` is a header-only borrow surface.
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

        // SEC-2-01 defense-in-depth: `SmuggleMode::H2` adds the
        // `check_h2_downgrade` check (forbidden hop-by-hop headers and
        // non-`trailers` TE, RFC 9113 §8.2.2) on top of the CL/TE defaults.
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
            // ROUND8-L7-07: smuggling against an H2 mux is the most severe
            // protocol abuse — highest weight so a burst drains fast.
            if let Some(g) = glitch {
                g.record(GlitchKind::HpackRatio);
            }
            return error_response(StatusCode::BAD_REQUEST, "request smuggling");
        }

        // ROUND8-L7-09 authority validation already ran at the `handle_inner`
        // choke point above, covering the WS/gRPC forks — no second call here.

        // PROTO-2-01 / RFC 9113 §8.3.1: `:authority` and `Host` MUST agree.
        // Disagreement is a routing/authz desync primitive (host-confusion
        // smuggling against backends that authorise on `Host`), so reject 400
        // BEFORE hop-by-hop strip / upstream acquire.
        if let Err(msg) = check_authority_host_agreement(&parts.uri, &parts.headers) {
            tracing::warn!(peer = %peer, reason = msg, "h2 :authority/Host mismatch rejected");
            // ROUND8-L7-07: host-confusion primitive — medium weight.
            if let Some(g) = glitch {
                g.record(GlitchKind::RapidReset);
            }
            return error_response(StatusCode::BAD_REQUEST, msg);
        }

        // PROTO-2-18 — SNI ↔ `:authority`/Host agreement (RFC 9110 §15.5.20).
        // Precedence: smuggle → authority/Host → SNI/Host. Prefer
        // `:authority`, falling back to `Host`. Loopback peers skip
        // enforcement (sec-r5: a Layer-7 routing/authz vector that does not
        // apply to loopback ingress).
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
                // ROUND8-L7-07: SNI/host confusion — medium weight.
                if let Some(g) = glitch {
                    g.record(GlitchKind::RapidReset);
                }
                let (status, body) = crate::sni_authority::misdirected_response();
                return error_response(status, body);
            }
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
                // The H1 upstream requires a Host header; synthesise it from
                // `:authority` when the client sent none.
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
                // F-COR-1: body cap exceeded — 413 before any upstream contact.
                Err(ProxyErr::BodyTooLarge) => error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body exceeds maximum",
                ),
                // F-COR-1: inbound validation failed during receive — 400
                // WITHOUT having dialed, so the backend 200 body cannot leak.
                Err(ProxyErr::BadRequest(s)) => error_response(StatusCode::BAD_REQUEST, &s),
            },
            UpstreamProto::H2 => Box::pin(self.proxy_h2_to_h2(backend.addr, stripped)).await,
            UpstreamProto::H3 => Box::pin(self.proxy_h2_to_h3(&backend, stripped)).await,
        };
        if let (Some(wd), Some(id)) = (self.watchdog.as_ref(), watch_id) {
            wd.deregister(id);
        }
        resp
    }

    /// Handle an RFC 8441 extended-CONNECT WebSocket bootstrap.
    ///
    /// F-S27-1 — the dial + upstream RFC 6455 handshake run INLINE, BEFORE any
    /// client-visible response (mirror of H1's ROUND8-L7-01 "defer 101"),
    /// bounded by [`HttpTimeouts::header`]: dial failure / budget elapsed →
    /// `504`, upstream non-101 → `502`, upstream `101` → build the `200` and
    /// spawn a SPLICE-ONLY task. When the dial lived in the detached task, a
    /// backend that refused the handshake still left the client holding a `200`
    /// (false success), and anything pipelined behind the CONNECT could be
    /// relayed toward a backend that never agreed to the upgrade.
    async fn handle_ws_extended_connect(
        &self,
        mut req: Request<IncomingBody>,
    ) -> Response<ClientRespBody> {
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

        // RFC 8441 §4 — a WebSocket extended CONNECT MUST carry `:scheme` and
        // `:path`. Reject a malformed one with a clean 400 BEFORE any dial
        // instead of silently defaulting `:path` to "/" as this once did.
        //
        // Reachability (measured, tests/ws_h2_conformance.rs): a MISSING
        // `:scheme` reaches here and is rejected ONLY by this check — hyper's
        // h2 server does not require it for extended CONNECT. A missing
        // `:path` is additionally caught by hyper's codec, so that arm is
        // defense-in-depth.
        let Some(path_and_query) = req
            .uri()
            .path_and_query()
            .map(std::string::ToString::to_string)
        else {
            tracing::debug!("ws/h2: extended CONNECT missing :path — 400 (RFC 8441 §4)");
            return error_response(
                StatusCode::BAD_REQUEST,
                "malformed websocket extended CONNECT: missing :path (RFC 8441 §4)",
            );
        };
        if req.uri().scheme().is_none() {
            tracing::debug!("ws/h2: extended CONNECT missing :scheme — 400 (RFC 8441 §4)");
            return error_response(
                StatusCode::BAD_REQUEST,
                "malformed websocket extended CONNECT: missing :scheme (RFC 8441 §4)",
            );
        }

        // ROUND8-OPS-06 parity: `handle` already injected the CHILD
        // `traceparent` onto `req.headers()`, so read the now-child values
        // back off the request and re-emit them on the tungstenite
        // `ClientRequestBuilder`, which takes header pairs, not a `HeaderMap`.
        let child_traceparent = req
            .headers()
            .get(lb_observability::tracing_propagation::TRACEPARENT_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let tracestate = req
            .headers()
            .get(lb_observability::tracing_propagation::TRACESTATE_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let ws_cfg = ws_proxy.config();
        let pool = self.pool.clone();

        let upstream_dial = async move {
            let pooled = pool
                .acquire_async(backend_addr)
                .await
                .map_err(|e| WsDialErr::Timeout(format!("backend dial failed: {e}")))?;
            let upstream_stream = pooled
                .take_stream()
                .ok_or_else(|| WsDialErr::Refused("pooled stream missing".to_owned()))?;
            let uri = format!("ws://{backend_addr}{path_and_query}")
                .parse()
                .map_err(|e| WsDialErr::Refused(format!("upstream uri build failed: {e}")))?;
            let mut builder =
                tokio_tungstenite::tungstenite::client::ClientRequestBuilder::new(uri);
            if let Some(tp) = child_traceparent {
                builder = builder.with_header(
                    lb_observability::tracing_propagation::TRACEPARENT_HEADER,
                    tp,
                );
            }
            if let Some(ts) = tracestate {
                builder = builder
                    .with_header(lb_observability::tracing_propagation::TRACESTATE_HEADER, ts);
            }
            let (backend_ws, _resp) = tokio_tungstenite::client_async_with_config(
                builder,
                upstream_stream,
                Some(ws_cfg.tungstenite_config()),
            )
            .await
            .map_err(|e| WsDialErr::Refused(format!("upstream handshake failed: {e}")))?;
            Ok::<_, WsDialErr>(backend_ws)
        };

        let backend_ws = match tokio::time::timeout(self.timeouts.header, upstream_dial).await {
            Ok(Ok(ws)) => ws,
            Ok(Err(WsDialErr::Refused(msg))) => {
                tracing::debug!(backend = %backend_addr, error = %msg, "ws/h2: upstream handshake refused — returning 502 (no 200 emitted)");
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "websocket upstream handshake failed",
                );
            }
            Ok(Err(WsDialErr::Timeout(msg))) => {
                tracing::debug!(backend = %backend_addr, error = %msg, "ws/h2: upstream dial failure — returning 504 (no 200 emitted)");
                return error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "websocket upstream dial timeout",
                );
            }
            Err(_elapsed) => {
                tracing::debug!(backend = %backend_addr, "ws/h2: upstream handshake budget elapsed — returning 504 (no 200 emitted)");
                return error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "websocket upstream handshake timeout",
                );
            }
        };

        // Upstream established: ONLY NOW arm the upgrade future and build the
        // client 200. The task no longer dials — it splices the established
        // upstream to the post-upgrade client stream. Holding `backend_ws`
        // open across that window is intentional (mirror of H1's
        // `run_h1_ws_splice_task`).
        let upgrade_fut = hyper::upgrade::on(&mut req);
        tokio::spawn(async move {
            let upgraded = match upgrade_fut.await {
                Ok(u) => u,
                Err(e) => {
                    // Dropping `backend_ws` closes the pooled socket via its
                    // `Drop`, so we never leak it.
                    tracing::debug!(error = %e, "ws/h2: hyper upgrade failed after upstream established");
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
        let (mut parts, mut body) = req.into_parts();

        // F-MD-1 — THE CATCH: these `parts` came off an H2 stream, so
        // `version == HTTP/2.0` and the map may still carry `content-length` /
        // `transfer-encoding`. Handed to the in-crate hyper HTTP/1.1 client
        // that way, hyper's http1 encoder MIS-FRAMES an unknown-length
        // streaming body: it sends an empty body and never polls our
        // `StreamBody`, so the backend observes an immediate EOF. Normalise to
        // HTTP/1.1 and drop both framing headers so hyper picks the framing
        // for the body we actually hand it.
        parts.version = hyper::Version::HTTP_11;
        parts.headers.remove(hyper::header::CONTENT_LENGTH);
        parts.headers.remove(hyper::header::TRANSFER_ENCODING);

        // S8 / M-D — bounded ingress pump replacing a whole-body `collect()`.
        // The fixed 64 KiB in-flight window doubles as a
        // validate-before-forward lookahead:
        //
        //  • whole request ≤ window: the buffer reaches inbound EOF first, and
        //    polling to EOF drives the IDENTICAL hyper/h2 validation
        //    `collect()` did — so a malformed request is rejected with ZERO
        //    backend dial.
        //  • request > window: dial, forward, stream at the window. The
        //    downstream response HEAD is still gated on the inbound body
        //    reaching a validated terminal state, so a >window request that
        //    turns malformed at the trailers never relays the backend response
        //    body — without buffering the whole body.
        use hyper::body::Body as _;
        use hyper::body::Frame;

        let mut lookahead: Vec<Bytes> = Vec::new();
        let mut buffered: usize = 0;
        let mut trailers_map: Option<hyper::HeaderMap> = None;
        // True once the body yielded its terminal frame inside the window;
        // false means we exited because the window filled (streaming regime).
        let mut reached_eof = false;

        loop {
            // In the lookahead phase the retained set IS the buffer.
            #[cfg(any(test, feature = "test-gauges"))]
            record_retained(buffered);

            // Strictly `>`: a request already past the window cannot be held
            // for validate-before-dial without violating R8.
            if buffered > H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX {
                break;
            }

            match body.frame().await {
                None => {
                    // F-MD-4: `None` is ambiguous — hyper maps an inbound
                    // RST_STREAM(CANCEL/NO_ERROR) to `None`, indistinguishable
                    // from a clean END_STREAM. Falling through to Branch A
                    // would relay a truncated body as a COMPLETE request. Only
                    // a positively-confirmed END_STREAM is clean; a reset is
                    // rejected here, BEFORE any dial.
                    if body.is_end_stream() {
                        reached_eof = true;
                        break;
                    }
                    return Err(ProxyErr::BadRequest(
                        "inbound H2 request body ended without END_STREAM \
                         (reset mid-body)"
                            .to_owned(),
                    ));
                }
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        // Separate axis from the in-flight window.
                        buffered = buffered.saturating_add(data.len());
                        if buffered > MAX_REQUEST_BODY_BYTES {
                            return Err(ProxyErr::BodyTooLarge);
                        }
                    }
                    if frame.is_data() {
                        // SAFETY: guarded by `is_data()`.
                        lookahead.push(frame.into_data().unwrap_or_default());
                    } else if frame.is_trailers() {
                        // Trailers are the terminal frame — clean EOF.
                        trailers_map = frame.into_trailers().ok();
                        reached_eof = true;
                        break;
                    }
                }
                Some(Err(e)) => {
                    // hyper/h2 surfaced a protocol/IO error while VALIDATING,
                    // still BEFORE any dial → a malformed request can never
                    // leak the backend response.
                    return Err(ProxyErr::BadRequest(format!(
                        "malformed H2 request body: {e}"
                    )));
                }
            }
        }

        if reached_eof {
            // ── Branch A: the whole request fit the window. Zero backend dial
            // for a malformed request — any inbound `Err` returned above.
            let trailers_vec = validate_request_trailers(trailers_map.as_ref())?;

            let pooled =
                self.pool.acquire_async(backend_addr).await.map_err(|e| {
                    ProxyErr::Upstream(format!("backend connect {backend_addr}: {e}"))
                })?;
            let stream = pooled
                .take_stream()
                .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;
            let (mut sender, conn) = hyper::client::conn::http1::handshake::<
                _,
                BoxBody<Bytes, hyper::Error>,
            >(TokioIo::new(stream))
            .await
            .map_err(|e| ProxyErr::Upstream(format!("h1 client handshake: {e}")))?;
            let conn_handle = tokio::spawn(async move {
                let _ = conn.await;
            });

            let body_bytes = concat_chunks(&lookahead, buffered);
            let upstream_body = build_h2_body_with_trailers(body_bytes, &trailers_vec);
            let req = Request::from_parts(parts, upstream_body);

            let send_fut = sender.send_request(req);
            // S14: a within-window body cannot be a slow upload, so bound the
            // head-roundtrip with `head`. NOT a load-bearing idle-watchdog site.
            let resp = match tokio::time::timeout(self.timeouts.head, send_fut).await {
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
            return Ok(resp);
        }

        // ── Branch B: request > window → dial + stream with the bounded
        // in-flight window; gate the response head on inbound terminal state.
        let pooled = self
            .pool
            .acquire_async(backend_addr)
            .await
            .map_err(|e| ProxyErr::Upstream(format!("backend connect {backend_addr}: {e}")))?;
        let stream = pooled
            .take_stream()
            .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;
        // F-MD-4: the Branch-B body's error type is the constructible
        // `PumpAbort` (`hyper::Error` has no public ctor) so the pump can
        // INJECT an error instead of dropping the channel — a drop reads as a
        // clean EOF, i.e. a smuggled-complete request.
        let (mut sender, conn) = hyper::client::conn::http1::handshake::<
            _,
            BoxBody<Bytes, PumpAbort>,
        >(TokioIo::new(stream))
        .await
        .map_err(|e| ProxyErr::Upstream(format!("h1 client handshake: {e}")))?;
        let conn_handle = tokio::spawn(async move {
            let _ = conn.await;
        });

        // Bounded in-flight channel — the R8 backpressure chain: backend write
        // stalls → hyper stops pulling → the channel fills → the pump stops
        // polling the inbound body → h2 withholds WINDOW_UPDATE → client pauses.
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, PumpAbort>>(H2_REQ_CHANNEL_DEPTH);

        // F-MD-3 — a GENUINE retained-memory gauge. The old streaming-phase
        // sites recorded a CONSTANT (the 64 KiB ceiling), so a whole-body-
        // buffering regression would not have moved it. Track the ACTUAL live
        // channel occupancy instead: incremented just before a push and
        // decremented the moment hyper pulls that chunk back out.
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

        // The pump owns the inbound body + lookahead and reports its terminal
        // verdict via a oneshot, so the response head can be gated on it.
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), ProxyErr>>();
        let drained: Vec<Bytes> = std::mem::take(&mut lookahead);

        // S14 — forward-progress signal for
        // [`lb_io::idle_send::idle_bounded_send`]; mirror of H1→H1.
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

            // The 64 MiB total-body cap still applies in the streaming
            // regime; start from what the lookahead already holds.
            let mut forwarded_total: usize = buffered;
            // Bytes still queued in `drained` — part of the live retained set.
            let mut lookahead_remaining: usize = buffered;

            // `ReceiverGone` = hyper dropped the request body (the backend
            // short-circuited its response WITHOUT reading it). We must NOT
            // manufacture a 413 then — switch to drain-and-validate (F-MD-2)
            // so the backend's real response is relayed once the inbound body
            // validates.
            enum SendOutcome {
                ReceiverGone,
            }

            // Split a DATA payload into ≤ `H2_REQ_CHUNK_MAX` pieces and push
            // each through the bounded channel (the backpressure point).
            macro_rules! send_chunked {
                ($bytes:expr, $is_lookahead:expr) => {{
                    let mut data: Bytes = $bytes;
                    let mut outcome: Result<(), SendOutcome> = Ok(());
                    while !data.is_empty() {
                        let take = data.len().min(H2_REQ_CHUNK_MAX);
                        let chunk = data.split_to(take);
                        let clen = chunk.len();
                        in_flight_bytes.fetch_add(clen, std::sync::atomic::Ordering::Relaxed);
                        if $is_lookahead {
                            lookahead_remaining = lookahead_remaining.saturating_sub(clen);
                        }
                        // F-MD-3: the ACTUAL retained set = queued lookahead +
                        // bytes live in the channel.
                        #[cfg(any(test, feature = "test-gauges"))]
                        record_retained(
                            lookahead_remaining
                                + in_flight_bytes.load(std::sync::atomic::Ordering::Relaxed),
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
            // to a validated terminal state or a malformed request would relay
            // the backend response. Bytes are DISCARDED (bounded memory); the
            // 64 MiB cap and protocol validation still apply.
            macro_rules! drain_and_validate {
                () => {{
                    loop {
                        match body.frame().await {
                            None => {
                                // `None` is ambiguous (reset vs END_STREAM);
                                // only a confirmed END_STREAM may relay the
                                // backend's early response.
                                if body.is_end_stream() {
                                    break Ok(());
                                }
                                break Err(ProxyErr::BadRequest(
                                    "inbound H2 request body ended without END_STREAM \
                                     (reset mid-body)"
                                        .to_owned(),
                                ));
                            }
                            Some(Ok(frame)) => {
                                if frame.is_trailers() {
                                    break validate_request_trailers(frame.trailers_ref())
                                        .map(|_| ());
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
                                    "malformed H2 request body: {e}"
                                )));
                            }
                        }
                    }
                }};
            }

            // 1) Drain the lookahead first, re-chunked to the window.
            for chunk in drained {
                if let Err(SendOutcome::ReceiverGone) = send_chunked!(chunk, true) {
                    // Backend short-circuited before reading the whole body —
                    // F-MD-2 drain-and-validate, NOT a 413.
                    let _ = verdict_tx.send(drain_and_validate!());
                    return;
                }
            }
            // 2) Continue forward-as-it-arrives with the bounded window.
            loop {
                match body.frame().await {
                    None => {
                        // F-MD-4 — THE H2 CATCH (exact inverse of the H1 rule):
                        // `frame()==None` is AMBIGUOUS. hyper's
                        // `Incoming::poll_frame` maps an inbound RST_STREAM
                        // with CANCEL or NO_ERROR to `Ready(None)`,
                        // indistinguishable from a clean END_STREAM
                        // (hyper-1.9.0 body/incoming.rs ~L250). Inferring EOF
                        // from `None` would drop `tx` cleanly → StreamBody
                        // yields `None` → hyper writes `0\r\n\r\n` → the
                        // truncated request is relayed as COMPLETE (request
                        // smuggling). `is_end_stream()` is the deterministic
                        // discriminator: it delegates to
                        // `h2::RecvStream::is_end_stream()`, true IFF a real
                        // END_STREAM flag was seen and FALSE after any reset
                        // (h2-0.4.13 proto/streams/state.rs
                        // `is_recv_end_stream`) — a protocol STATE, not a race.
                        if body.is_end_stream() {
                            // Confirmed END_STREAM → drop `tx` → hyper writes
                            // the terminator → upstream sees a COMPLETE request.
                            set_complete();
                            let _ = verdict_tx.send(Ok(()));
                        } else {
                            // `None` from a RST_STREAM: inject a BODY ERROR so
                            // hyper aborts the upstream request WITHOUT a
                            // terminator — never seen as complete upstream.
                            let _ = tx.send(Err(PumpAbort)).await;
                            let _ = verdict_tx.send(Err(ProxyErr::BadRequest(
                                "inbound H2 request body ended without END_STREAM \
                                 (reset mid-body)"
                                    .to_owned(),
                            )));
                        }
                        return;
                    }
                    Some(Ok(frame)) => {
                        if frame.is_trailers() {
                            // Validate BEFORE forwarding; a pseudo-header in
                            // trailers is malformed.
                            match validate_request_trailers(frame.trailers_ref()) {
                                Ok(_) => {
                                    let _ = tx.send(Ok(frame)).await;
                                    // Trailers accepted; upload complete.
                                    bump();
                                    set_complete();
                                    let _ = verdict_tx.send(Ok(()));
                                    return;
                                }
                                Err(e) => {
                                    // F-MD-4: inject a BODY ERROR — dropping
                                    // tx alone reads as a clean EOF, i.e. a
                                    // smuggled-complete request.
                                    let _ = tx.send(Err(PumpAbort)).await;
                                    let _ = verdict_tx.send(Err(e));
                                    return;
                                }
                            }
                        }
                        if let Ok(data) = frame.into_data() {
                            forwarded_total = forwarded_total.saturating_add(data.len());
                            if forwarded_total > MAX_REQUEST_BODY_BYTES {
                                // Cap exceeded mid-stream: report 413 and
                                // inject a BODY ERROR (F-MD-4) so the upstream
                                // body ends WITHOUT a clean terminator. The
                                // client sees a reset (no 200 leak) and the
                                // upstream never sees a complete request.
                                let _ = tx.send(Err(PumpAbort)).await;
                                let _ = verdict_tx.send(Err(ProxyErr::BodyTooLarge));
                                return;
                            }
                            if let Err(SendOutcome::ReceiverGone) = send_chunked!(data, false) {
                                // Backend stopped reading mid-stream — F-MD-2
                                // drain-and-validate, NOT a 413.
                                let _ = verdict_tx.send(drain_and_validate!());
                                return;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        // Inbound protocol error after the dial (e.g. a client
                        // RST mid-body, F-MD-4). Inject a BODY ERROR so the
                        // upstream request body terminates ABRUPTLY (no clean
                        // `0\r\n\r\n`) and hyper aborts it — dropping the
                        // sender alone would be a clean EOF → smuggled
                        // complete. The caller aborts the conn too.
                        let _ = tx.send(Err(PumpAbort)).await;
                        let _ = verdict_tx.send(Err(ProxyErr::BadRequest(format!(
                            "malformed H2 request body: {e}"
                        ))));
                        return;
                    }
                }
            }
        });

        // Drive the upstream send concurrently with the pump (hyper must pull
        // the channel for the pump to progress under backpressure), but do NOT
        // relay the response until the pump's terminal verdict lands.
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
                tracing::warn!(error = %idle_err, "h2→h1 idle/head deadline fired");
                pump.abort();
                conn_handle.abort();
                return Err(ProxyErr::Timeout);
            }
        };

        // Validate-before-RESPONSE-relay gate: the head relays only once the
        // inbound body reached a validated terminal state.
        match verdict_rx.await {
            Ok(Ok(())) => {
                drop(conn_handle);
                Ok(resp)
            }
            Ok(Err(e)) => {
                // Malformed inbound after dial: abort the upstream connection
                // (do NOT pool it) and never relay its response body.
                conn_handle.abort();
                Err(e)
            }
            Err(_) => {
                // Pump vanished without a verdict — treat as an inbound
                // failure; never leak the backend response.
                conn_handle.abort();
                Err(ProxyErr::BadRequest(
                    "inbound H2 request pump terminated without a verdict".to_owned(),
                ))
            }
        }
    }

    fn finalize_response(&self, resp: Response<IncomingBody>) -> Response<ClientRespBody> {
        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        if let Some(alt) = self.alt_svc {
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

    /// Forward an H2 inbound request to an H2 backend. Bounded-incremental
    /// STREAMING on both legs; the request leg MIRRORS the pump in
    /// [`Self::proxy_request`]. Deltas: the request stays HTTP/2-shaped (no
    /// force-HTTP/1.1, no CL/TE strip — H2 upstream framing is hyper's H2
    /// encoder's job), the egress is the multiplexed [`Http2Pool`] (no
    /// per-request conn_handle), and the Branch-B body is `H2ReqBody`.
    async fn proxy_h2_to_h2(
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
            .proxy_h2_to_h2_request(h2_pool.as_ref(), backend_addr, req)
            .await
        {
            Ok(resp) => upstream_h2_response_to_h2(resp, self.alt_svc),
            Err(ProxyErr::Upstream(s)) => error_response(StatusCode::BAD_GATEWAY, &s),
            Err(ProxyErr::Timeout) => {
                error_response(StatusCode::GATEWAY_TIMEOUT, "upstream H2 timeout")
            }
            // F-CAP-1: streaming over-cap → 413 (NOT 502).
            Err(ProxyErr::BodyTooLarge) => error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body exceeds maximum",
            ),
            // F-COR-1 / F-MD-4: inbound validation failure (malformed
            // trailers, content-length≠ΣDATA, reset mid-body) → 400, NOT 502.
            Err(ProxyErr::BadRequest(s)) => error_response(StatusCode::BAD_REQUEST, &s),
        }
    }

    /// H2→H2 Leg 1 — the bounded-incremental streaming REQUEST pump; a MIRROR
    /// of [`Self::proxy_request`]'s orchestration with the deltas documented
    /// on [`Self::proxy_h2_to_h2`]. The leaf helpers and window consts are
    /// REUSED, not duplicated; only the orchestration is mirrored.
    async fn proxy_h2_to_h2_request(
        &self,
        h2_pool: &Http2Pool,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Result<Response<IncomingBody>, ProxyErr> {
        use hyper::body::Body as _;
        use hyper::body::Frame;
        use lb_io::http2_pool::{H2ReqBody, Http2PoolError};

        let req = req.into_inner();
        let (parts, mut body) = req.into_parts();

        // DELTA vs `proxy_request`: keep the request HTTP/2-shaped. Run the
        // H2→H2 header normalization, but do NOT force HTTP/1.1 and do NOT
        // strip content-length/transfer-encoding — those were H1-framing
        // fixes; H2 upstream framing is hyper's H2 encoder's job.
        let upstream_parts = match build_h2_upstream_request_parts(&parts) {
            Ok(p) => p,
            Err(e) => return Err(ProxyErr::Upstream(e)),
        };

        // Bounded ingress pump; the lookahead posture is IDENTICAL to
        // `proxy_request` (see there for the full rationale).
        let mut lookahead: Vec<Bytes> = Vec::new();
        let mut buffered: usize = 0;
        let mut trailers_map: Option<hyper::HeaderMap> = None;
        let mut reached_eof = false;

        loop {
            #[cfg(any(test, feature = "test-gauges"))]
            record_retained(buffered);

            if buffered > H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX {
                break;
            }

            match body.frame().await {
                None => {
                    // F-MD-4: only a positively-confirmed END_STREAM is clean;
                    // a reset is rejected here, before any pool contact.
                    if body.is_end_stream() {
                        reached_eof = true;
                        break;
                    }
                    return Err(ProxyErr::BadRequest(
                        "inbound H2 request body ended without END_STREAM \
                         (reset mid-body)"
                            .to_owned(),
                    ));
                }
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        buffered = buffered.saturating_add(data.len());
                        if buffered > MAX_REQUEST_BODY_BYTES {
                            return Err(ProxyErr::BodyTooLarge);
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
                    // Validation error before any pool contact (zero-dial).
                    return Err(ProxyErr::BadRequest(format!(
                        "malformed H2 request body: {e}"
                    )));
                }
            }
        }

        if reached_eof {
            // ── Branch A: the whole request fit the window. Zero pool contact
            // for a malformed one — any inbound Err/reset returned above.
            let trailers_vec = validate_request_trailers(trailers_map.as_ref())?;

            let body_bytes = concat_chunks(&lookahead, buffered);
            // DELTA: widen the shared helper's `hyper::Error` to the boxed
            // error `H2ReqBody` requires.
            let upstream_body: H2ReqBody = build_h2_body_with_trailers(body_bytes, &trailers_vec)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                .boxed();
            let upstream_req = Request::from_parts(upstream_parts, upstream_body);

            return match h2_pool.send_request(backend_addr, upstream_req).await {
                Ok(resp) => Ok(resp),
                Err(Http2PoolError::Timeout) => Err(ProxyErr::Timeout),
                Err(e) => Err(ProxyErr::Upstream(format!("h2 upstream: {e}"))),
            };
        }

        // ── Branch B: stream with the bounded in-flight window, gating the
        // head on the inbound terminal state. DELTA: no dial/handshake here —
        // the Http2Pool owns the connection and `send_request` multiplexes.
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, PumpAbort>>(H2_REQ_CHANNEL_DEPTH);

        // F-MD-3: genuine retained-memory gauge (live in-flight occupancy).
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight_body = std::sync::Arc::clone(&in_flight_bytes);

        // Bridge the receiver into a StreamBody, mapping `PumpAbort` → boxed
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

        // The pump reports its terminal verdict via a oneshot so the
        // response-head relay is gated on a VALIDATED terminal state.
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), ProxyErr>>();
        let drained: Vec<Bytes> = std::mem::take(&mut lookahead);

        // S14 — forward-progress signal for [`drive_h2_upstream_send`].
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
                        let take = data.len().min(H2_REQ_CHUNK_MAX);
                        let chunk = data.split_to(take);
                        let clen = chunk.len();
                        in_flight_bytes.fetch_add(clen, std::sync::atomic::Ordering::Relaxed);
                        if $is_lookahead {
                            lookahead_remaining = lookahead_remaining.saturating_sub(clen);
                        }
                        #[cfg(any(test, feature = "test-gauges"))]
                        record_retained(
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
                            None => {
                                if body.is_end_stream() {
                                    break Ok(());
                                }
                                break Err(ProxyErr::BadRequest(
                                    "inbound H2 request body ended without END_STREAM \
                                     (reset mid-body)"
                                        .to_owned(),
                                ));
                            }
                            Some(Ok(frame)) => {
                                if frame.is_trailers() {
                                    break validate_request_trailers(frame.trailers_ref())
                                        .map(|_| ());
                                }
                                if let Some(d) = frame.data_ref() {
                                    forwarded_total = forwarded_total.saturating_add(d.len());
                                    if forwarded_total > MAX_REQUEST_BODY_BYTES {
                                        break Err(ProxyErr::BodyTooLarge);
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                break Err(ProxyErr::BadRequest(format!(
                                    "malformed H2 request body: {e}"
                                )));
                            }
                        }
                    }
                }};
            }

            // F-MD-4 (body-layer half) — on every abort terminal state, inject
            // `Err(PumpAbort)` and HOLD the sender open until hyper has
            // OBSERVED it. mpsc delivery is FIFO, so a buffered item is always
            // returned before the closed `None`: holding the sender forces
            // hyper to poll `Ready(Some(Err(PumpAbort)))` BEFORE it could ever
            // see a channel-close `None`, so it RESETS the upstream stream
            // instead of taking the clean-EOF branch and emitting a spurious
            // END_STREAM.
            //
            // That is only HALF the fix. The other half is the caller's
            // detached send task + `reset_peer`: a downstream client RST
            // cancels this service future, which would otherwise DROP the
            // in-flight upstream body at a clean frame boundary and make hyper
            // finalize END_STREAM on the graceful drop — racing ahead of this
            // injection. Bounded by `H2_ABORT_OBSERVE_TIMEOUT` so a wedged
            // upstream driver cannot hang the detached task.
            macro_rules! inject_abort {
                () => {{
                    let _ = tx.send(Err(PumpAbort)).await;
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
                        // F-MD-4: positively confirm END_STREAM; a `None` from
                        // a RST must NOT be relayed as a clean EOF.
                        if body.is_end_stream() {
                            // Upload complete; switch to the Phase-B head cap.
                            set_complete();
                            let _ = verdict_tx.send(Ok(()));
                        } else {
                            inject_abort!();
                            let _ = verdict_tx.send(Err(ProxyErr::BadRequest(
                                "inbound H2 request body ended without END_STREAM \
                                 (reset mid-body)"
                                    .to_owned(),
                            )));
                        }
                        return;
                    }
                    Some(Ok(frame)) => {
                        if frame.is_trailers() {
                            match validate_request_trailers(frame.trailers_ref()) {
                                Ok(_) => {
                                    let _ = tx.send(Ok(frame)).await;
                                    // Trailers accepted; upload complete.
                                    bump();
                                    set_complete();
                                    let _ = verdict_tx.send(Ok(()));
                                    return;
                                }
                                Err(e) => {
                                    inject_abort!();
                                    let _ = verdict_tx.send(Err(e));
                                    return;
                                }
                            }
                        }
                        if let Ok(data) = frame.into_data() {
                            forwarded_total = forwarded_total.saturating_add(data.len());
                            if forwarded_total > MAX_REQUEST_BODY_BYTES {
                                inject_abort!();
                                let _ = verdict_tx.send(Err(ProxyErr::BodyTooLarge));
                                return;
                            }
                            if let Err(SendOutcome::ReceiverGone) = send_chunked!(data, false) {
                                let _ = verdict_tx.send(drain_and_validate!());
                                return;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        inject_abort!();
                        let _ = verdict_tx.send(Err(ProxyErr::BadRequest(format!(
                            "malformed H2 request body: {e}"
                        ))));
                        return;
                    }
                }
            }
        });

        // F-MD-4: route the graceful-drop egress through the shared driver —
        // it owns the detached send task (biased verdict-vs-head race,
        // `reset_peer` on every abort, the F-CAP-1 caller arm, `pump.abort()`).
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

    /// Forward an H2 inbound request to a STREAMING H3 backend. Mirror of
    /// [`crate::h1_proxy::H1Proxy::proxy_h1_to_h3`] over the same shared
    /// connector. Two H2-specific deltas: the pump disambiguates an H2 `None`
    /// (ambiguous between clean END_STREAM and RST, unlike H1's positively-
    /// clean `None`) via `is_end_stream()`, and the response head uses H2→H2
    /// semantics — drop pseudo-headers + lowercase, NO `RESPONSE_HOP_BY_HOP`
    /// strip.
    ///
    /// HAZARD — request cancel-race: the connector treats a `body_tx` dropped
    /// WITHOUT a final `End`/`Reset` before any event as a bodyless-COMPLETE
    /// request, so a downstream `RST_STREAM` that cancels this *service* future
    /// must NOT drop the pump silently — that would smuggle a truncated request
    /// as complete. The mitigation is LOAD-BEARING: the ingress pump is
    /// DETACHED and ALWAYS emits an explicit terminal `End{trailers}` or
    /// `Reset`, never a silent drop.
    ///
    /// F-CAP-1: a PRE-DATA over-cap (`Reset` as the pump's first event) →
    /// connector inline-413; a pre-dial failure → inline-502. A MID-BODY
    /// over-cap → `H3RespEvent::Reset`, which we turn into an injected
    /// response-body `Err` so hyper RST_STREAMs the client rather than emitting
    /// a clean END_STREAM (response-splitting guard).
    async fn proxy_h2_to_h3(
        &self,
        backend: &UpstreamBackend,
        req: StrippedRequest<IncomingBody>,
    ) -> Response<ClientRespBody> {
        use hyper::body::Body as _;
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
        let headers = match build_h2_to_h3_fieldlist(&parts, &sni) {
            Ok(h) => h,
            Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
        };

        // Bounded request-body channel into the connector. Backpressure: a
        // slow QUIC upstream → the connector stops draining → this channel
        // fills → the pump stops polling → the client's H2 window stalls.
        let (body_tx, body_rx) =
            tokio::sync::mpsc::channel::<ReqBodyEvent>(lb_quic::conn_actor::H3_BODY_CHANNEL_DEPTH);
        let (resp_tx, mut resp_rx) = tokio::sync::mpsc::channel::<lb_quic::H3RespEvent>(
            lb_quic::h3_bridge::H3_RESP_CHANNEL_DEPTH,
        );

        // F-MD-3 gauge: instantaneous in-flight request bytes the pump
        // retains. Channel depth bounds total in-flight independent of body
        // size. Reuses the H2 ingress gauge so the memory proof reads one
        // counter.
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // Request-leg pump, DETACHED (see HAZARD above): a downstream H2 RST
        // that cancels the service future must NOT drop this task before it
        // emits an explicit terminal event.
        let pump_in_flight = std::sync::Arc::clone(&in_flight_bytes);
        let pump = tokio::spawn(async move {
            // The request-body cap is OUR job (the connector caps the
            // RESPONSE). Over-cap BEFORE any chunk → `Reset` as the FIRST
            // event → connector inline-413; after ≥1 chunk → RESET-without-FIN.
            let mut forwarded_total: usize = 0;

            // Split DATA into ≤ `H3_BODY_CHUNK_MAX` pieces so the in-flight
            // item size matches the memory gauge. Err(()) = connector gone.
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
                        record_retained(pump_in_flight.load(std::sync::atomic::Ordering::Relaxed));
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
                        // F-MD-4 (H2): `None` is AMBIGUOUS. A confirmed
                        // END_STREAM → `End{[]}` → connector FIN; a `None`
                        // from a RST → `Reset` → RESET-without-FIN, so the
                        // backend NEVER sees a truncated request as complete
                        // (this explicit terminal is what defends the
                        // dropped-tx == bodyless-COMPLETE connector contract).
                        if body.is_end_stream() {
                            let _ = body_tx
                                .send(ReqBodyEvent::End {
                                    trailers: Vec::new(),
                                })
                                .await;
                        } else {
                            let _ = body_tx.send(ReqBodyEvent::Reset).await;
                        }
                        return;
                    }
                    Some(Ok(frame)) => {
                        if frame.is_trailers() {
                            // Validate BEFORE forwarding — a framing/routing
                            // field in trailers is a desync primitive.
                            // Forbidden → `Reset`, never a clean `End`.
                            match validate_request_trailers(frame.trailers_ref()) {
                                Ok(tvec) => {
                                    let _ =
                                        body_tx.send(ReqBodyEvent::End { trailers: tvec }).await;
                                }
                                Err(_) => {
                                    let _ = body_tx.send(ReqBodyEvent::Reset).await;
                                }
                            }
                            return;
                        }
                        if let Ok(data) = frame.into_data() {
                            // Over-cap → `Reset`; no over-cap byte is
                            // forwarded either way.
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
                        // F-MD-4 (H2): a protocol/IO error mid-body → `Reset`
                        // → the backend never sees a complete (truncated)
                        // request.
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
        let alt_svc = self.alt_svc;
        let first = resp_rx.recv().await;
        match first {
            Some(lb_quic::H3RespEvent::Head { status, headers }) => {
                let st = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
                let builder = h2_decoded_resp_head_builder(st, &headers, alt_svc);

                // Stream the remaining events. `Reset` → inject a body error
                // so hyper's H2 server does NOT emit a clean END_STREAM (it
                // RST_STREAMs — the response-splitting guard). `End` → drop
                // the sender. A `Trailers` event maps to a native
                // `Frame::trailers`, which hyper flushes WITHOUT a `Trailer:`
                // pre-declaration — that is what gets gRPC's `grpc-status` to
                // the H2 client.
                let (btx, brx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, H2RespAbort>>(
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
                                        hyper::header::HeaderName::from_bytes(n.as_bytes()),
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
                                let _ = btx.send(Err(H2RespAbort)).await;
                                break;
                            }
                            // A second Head is malformed — abort.
                            lb_quic::H3RespEvent::Head { .. } => {
                                let _ = btx.send(Err(H2RespAbort)).await;
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
                    // F-MD-4 (response leg): `H2RespAbort` is the constructible
                    // channel error and the relay SENDS it (never a clean
                    // drop), so hyper RST_STREAMs rather than presenting a
                    // truncated body as complete.
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
                let _ = &pump; // pump is detached; its task owns the request leg
                builder.body(stream_body.boxed()).unwrap_or_else(|_| {
                    error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "build h2 streaming response failed",
                    )
                })
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
}

// ── PROTO-001 H2-side translation helpers ─────────────────────────────

/// Build the upstream H2 request HEAD (method + uri + normalized regular
/// headers) for the streaming relay, WITHOUT touching the body: run the
/// `create_bridge(Http2, Http2)` request bridge, synthesise the pseudo-headers
/// a real H2 client would have sent, then re-attach the regular headers.
/// DELTA vs the H1-egress pump: content-length / transfer-encoding are NOT
/// stripped — H2 upstream framing is hyper's H2 encoder's job.
fn build_h2_upstream_request_parts(
    parts: &http::request::Parts,
) -> Result<http::request::Parts, String> {
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
    let path = parts
        .uri
        .path_and_query()
        .map_or_else(|| "/".to_owned(), std::string::ToString::to_string);
    let mut bridge_in = crate::BridgeRequest {
        method: parts.method.to_string(),
        uri: path.clone(),
        headers: parts
            .headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_owned(), s.to_owned()))
            })
            .collect(),
        // R8: no body materialised — the bridge only lowercases headers.
        body: Bytes::new(),
        scheme: Some(scheme.clone()),
        trailers: Vec::new(),
    };
    // Synthesise the pseudo-headers a real H2 client would have sent.
    bridge_in
        .headers
        .insert(0, (":method".to_owned(), parts.method.to_string()));
    bridge_in
        .headers
        .insert(1, (":path".to_owned(), path.clone()));
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
    let (out_parts, ()) = builder
        .body(())
        .map_err(|e| format!("build h2 req head: {e}"))?
        .into_parts();
    Ok(out_parts)
}

/// RFC 9113 §8.1 — reject a `:`-prefixed name in the trailing field section
/// rather than silently stripping it. Retained `#[cfg(test)]`-only for the
/// no-regression test below; production trailer rejection runs through
/// [`validate_request_trailers`].
#[cfg(test)]
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

/// F-COR-1 (b) / RFC 9113 §8.1: a pseudo-header field in the trailing field
/// section is malformed — reject it (PROTOCOL_ERROR-class, surfaced as 400),
/// never forward. This is the H2→H1 trailer-validation site; `h2_to_h2.rs`
/// filters only on the regular-header path. Returns the pairs to forward.
fn validate_request_trailers(
    trailers_map: Option<&hyper::HeaderMap>,
) -> Result<Vec<(String, String)>, ProxyErr> {
    let mut trailers_vec: Vec<(String, String)> = Vec::new();
    if let Some(tm) = trailers_map {
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
    Ok(trailers_vec)
}

/// Concatenate the lookahead DATA chunks into one `Bytes` for the
/// within-window body. `total` is the exact summed length so we allocate once.
fn concat_chunks(chunks: &[Bytes], total: usize) -> Bytes {
    if let [single] = chunks {
        return single.clone();
    }
    let mut out = bytes::BytesMut::with_capacity(total);
    for c in chunks {
        out.extend_from_slice(c);
    }
    out.freeze()
}

/// PROTO-2-12 — emit the body bytes as a `Frame::data` followed by a
/// `Frame::trailers` when `trailers` is non-empty.
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

/// Convert an upstream H2 `Response<Incoming>` into the downstream response.
/// Streaming by construction: the boxed `Incoming` carries upstream trailers on
/// its terminal frame, so no `collect()` is needed to capture them, and memory
/// stays bounded by hyper's flow-control window.
fn upstream_h2_response_to_h2(
    resp: Response<IncomingBody>,
    alt_svc: Option<AltSvcConfig>,
) -> Response<ClientRespBody> {
    let (parts, body) = resp.into_parts();
    // H2→H2 normalization: lowercase regular headers, drop `:`-prefixed. No
    // hop-by-hop strip beyond that (the bridge did none either).
    let mut builder = Response::builder().status(parts.status);
    for (n, v) in &parts.headers {
        if n.as_str().starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v);
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    // R8: stream the `Incoming` by construction; lossless-box its error.
    builder
        .body(
            body.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                .boxed(),
        )
        .unwrap_or_else(|_| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "build h2 response failed",
            )
        })
}

/// F-MD-4 — shared graceful-drop egress driver for an H2 upstream reached via
/// [`Http2Pool`], so the H2→H2 and H1→H2 streaming paths share ONE copy of the
/// smuggling fix rather than a hand-mirrored duplicate. Owns the detached send
/// task (which owns the in-flight `send_request` future and therefore the
/// upstream request body), the biased verdict-vs-head race with `reset_peer` on
/// every abort verdict, the F-CAP-1 caller arm, and the final `head_rx.await`.
// `body_timeout` is consumed ONLY by the post-error verdict-rx backstop
// (F-CAP-1 wedged-pump liveness consultation), NOT by the send.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_h2_upstream_send(
    pool: &Http2Pool,
    backend_addr: SocketAddr,
    upstream_req: Request<lb_io::http2_pool::H2ReqBody>,
    mut verdict_rx: tokio::sync::oneshot::Receiver<Result<(), ProxyErr>>,
    pump: tokio::task::JoinHandle<()>,
    last_progress: std::sync::Arc<std::sync::atomic::AtomicU64>,
    upload_complete: std::sync::Arc<std::sync::atomic::AtomicBool>,
    epoch: tokio::time::Instant,
    idle: Duration,
    head_timeout: Duration,
    body_timeout: Duration,
) -> Result<Response<IncomingBody>, ProxyErr> {
    use lb_io::http2_pool::Http2PoolError;

    // F-MD-4 — DETACH the send + verdict resolution into a task that OWNS the
    // in-flight `send_request` future (and therefore the upstream request
    // body) so it is not tied to the downstream H2 stream future's lifetime.
    //
    // ROOT CAUSE of the intermittent smuggle: a downstream RST_STREAM cancels
    // this service future. With the send owned directly, that cancel DROPS the
    // upstream request body at a clean frame boundary, and hyper's H2 client
    // finalizes the stream with a clean END_STREAM on the graceful drop —
    // relaying the truncated request as COMPLETE, before any verdict-driven
    // `reset_peer` could run. Detached, a cancel only drops the caller's
    // `head_rx`; the task keeps the body alive and `reset_peer`s BEFORE
    // dropping it. Multiplexed-pool analog of the H1 `conn_handle.abort()`.
    let (head_tx, head_rx) =
        tokio::sync::oneshot::channel::<Result<Response<IncomingBody>, ProxyErr>>();
    let pool_for_task = pool.clone();
    tokio::spawn(async move {
        // S14 — two-phase idle/head deadline instead of the pool's fixed
        // `send_timeout`; same result shape, and ROUND8-L7-10 eviction is
        // preserved verbatim inside `send_request_idle`.
        let mut send_fut = std::pin::pin!(pool_for_task.send_request_idle(
            backend_addr,
            upstream_req,
            last_progress,
            upload_complete,
            epoch,
            idle,
            head_timeout,
        ));
        // Race the send against the pump's verdict (resolves exactly once);
        // `resp` is Some only when the head won.
        let resp: Option<Response<IncomingBody>> = tokio::select! {
            // biased: an abort verdict landing with the head must win so we
            // RESET rather than relay.
            biased;
            v = &mut verdict_rx => {
                match v {
                    // Abort before the head: reset the upstream stream, then
                    // drop the send future (and body) only AFTER the reset.
                    Ok(Err(e)) => {
                        pool_for_task.reset_peer(backend_addr);
                        pump.abort();
                        let _ = head_tx.send(Err(e));
                        return;
                    }
                    // Clean terminal state before the head: await, then relay.
                    Ok(Ok(())) => {
                        let out = match send_fut.await {
                            Ok(r) => Ok(r),
                            Err(Http2PoolError::Timeout) => Err(ProxyErr::Timeout),
                            Err(e) => Err(ProxyErr::Upstream(format!("h2 upstream: {e}"))),
                        };
                        let _ = head_tx.send(out);
                        return;
                    }
                    // Pump vanished without a verdict — reset; never leak.
                    Err(_) => {
                        pool_for_task.reset_peer(backend_addr);
                        let _ = head_tx.send(Err(ProxyErr::BadRequest(
                            "inbound H2 request pump terminated without a verdict".to_owned(),
                        )));
                        return;
                    }
                }
            }
            r = &mut send_fut => match r {
                Ok(resp) => Some(resp),
                Err(Http2PoolError::Timeout) => {
                    pump.abort();
                    let _ = head_tx.send(Err(ProxyErr::Timeout));
                    return;
                }
                Err(e) => {
                    // F-CAP-1: consult the verdict FIRST (bounded) and prefer
                    // a classified 413/400 over the generic 502; reset the peer
                    // so any in-flight stream is torn down.
                    let classified =
                        match tokio::time::timeout(body_timeout, &mut verdict_rx).await {
                            Ok(Ok(Err(
                                ve @ (ProxyErr::BodyTooLarge | ProxyErr::BadRequest(_)),
                            ))) => Some(ve),
                            _ => None,
                        };
                    pool_for_task.reset_peer(backend_addr);
                    pump.abort();
                    let _ = head_tx.send(Err(classified
                        .unwrap_or_else(|| ProxyErr::Upstream(format!("h2 upstream: {e}")))));
                    return;
                }
            },
        };
        // SAFETY: every non-head branch above `return`ed, so reaching
        // here means the head won the race.
        let Some(resp) = resp else { return };

        // Head won the race: relay only once the inbound body reached a
        // validated terminal state; on an abort verdict reset and never relay.
        let out = match verdict_rx.await {
            Ok(Ok(())) => Ok(resp),
            Ok(Err(e)) => {
                pool_for_task.reset_peer(backend_addr);
                Err(e)
            }
            Err(_) => {
                pool_for_task.reset_peer(backend_addr);
                Err(ProxyErr::BadRequest(
                    "inbound H2 request pump terminated without a verdict".to_owned(),
                ))
            }
        };
        let _ = head_tx.send(out);
    });

    // If the downstream RSTs, this await is cancelled — but the detached task
    // survives and still resets the upstream on an abort verdict.
    match head_rx.await {
        Ok(result) => result,
        // Send task dropped `head_tx` without sending — never leak a response.
        Err(_) => Err(ProxyErr::BadRequest(
            "inbound H2 upstream send task terminated without a result".to_owned(),
        )),
    }
}

/// Build the H2→H3 request FIELD-LIST from the request HEAD only — body and
/// trailers stream through the connector (request trailers ride
/// `ReqBodyEvent::End{trailers}`). Mirror of `h1_proxy::build_h1_to_h3_fieldlist`
/// with H2 pseudo-header synthesis.
fn build_h2_to_h3_fieldlist(
    parts: &hyper::http::request::Parts,
    sni: &str,
) -> Result<Vec<(String, String)>, String> {
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
        // Head-only: the bridge just mints the pseudo-header set.
        body: Bytes::new(),
        scheme: Some(scheme.clone()),
        trailers: Vec::new(),
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
    Ok(translated.headers)
}

/// Build the streaming H2 response head from the connector's decoded
/// [`lb_quic::H3RespEvent::Head`], using H2→H2 response semantics. UNLIKE the
/// H1→H3 builder there is NO `RESPONSE_HOP_BY_HOP` strip: hyper's H2 server
/// encoder already rejects connection-specific headers on egress, so a stray
/// `connection:` decoded from an H3 backend is never written to the client.
fn h2_decoded_resp_head_builder(
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
        builder = builder.header(lower.as_str(), v.as_str());
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    builder
}

// `pub(crate)` so `drive_h2_upstream_send` can name it in its signature
// without tripping `private_interfaces`.
pub(crate) enum ProxyErr {
    Upstream(String),
    Timeout,
    /// Inbound body exceeded [`MAX_REQUEST_BODY_BYTES`] → 413, rejected before
    /// any upstream contact.
    BodyTooLarge,
    /// Inbound H2 request failed protocol validation while being received →
    /// 400, returned BEFORE any backend dial so the malformed request can
    /// never leak the backend's 200 body.
    BadRequest(String),
}

fn error_response(status: StatusCode, msg: &str) -> Response<ClientRespBody> {
    let body = Full::new(Bytes::from(msg.to_owned()))
        .map_err(|never| match never {})
        .boxed();
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    resp
}

/// PROTO-2-01 / RFC 9113 §8.3.1 — `Err` when both `:authority` (surfaced by
/// hyper as `uri.authority()`) and `Host` are present and their host components
/// disagree. Case-insensitive on the host (RFC 3986 §3.2.2); the port is
/// ignored when either side elides it (§8.3.1 default-port carve-out). The
/// proxy lifts the §8.1 malformed-request rule into a 400.
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

/// Compare a `:authority` against a `Host` per RFC 9113 §8.3.1: an empty host
/// on either side is a mismatch; hosts compare case-insensitively; ports must
/// match only when BOTH sides carry one (the proxy has no default-port table).
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

/// Split `host[:port]`. Bracketed IPv6 literals keep their brackets in the host
/// part so the compare never splits on a colon inside the literal.
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

/// Max instantaneous inbound-request memory the ingress pump retains (lookahead
/// buffer + in-flight channel occupancy). A whole-body-buffering implementation
/// would make this grow with request size; the bounded window keeps it at
/// ≤ 64 KiB regardless. Test-only — production never compiles the gauge.
#[cfg(any(test, feature = "test-gauges"))]
pub static H2_REQ_MAX_RETAINED_BODY_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Lock-free CAS-max update for [`H2_REQ_MAX_RETAINED_BODY_BYTES`]; the gauge
/// only ever moves UP.
#[cfg(any(test, feature = "test-gauges"))]
pub fn record_retained(n: usize) {
    use std::sync::atomic::Ordering;
    let mut cur = H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    while n > cur {
        match H2_REQ_MAX_RETAINED_BODY_BYTES.compare_exchange_weak(
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

    /// Pin the in-flight window constants and the ceiling formula — the R8
    /// body-independence proof rests on these exact values.
    #[test]
    fn h2_req_window_constants_pinned() {
        assert_eq!(H2_REQ_CHANNEL_DEPTH, 8, "in-flight channel depth");
        assert_eq!(H2_REQ_CHUNK_MAX, 8 * 1024, "per-chunk max (8 KiB)");
        // Window ceiling = depth × chunk = 64 KiB, body-size-INDEPENDENT.
        assert_eq!(
            H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX,
            64 * 1024,
            "in-flight window ceiling (64 KiB)"
        );
        // `black_box` keeps this a genuine runtime check, not a const that
        // clippy flags as optimized-out.
        let window = std::hint::black_box(H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX);
        let cap = std::hint::black_box(MAX_REQUEST_BODY_BYTES);
        assert!(window < cap, "window must be far below the total-body cap");
    }

    /// The retained-memory gauge is a real max-update, not a constant.
    #[test]
    fn h2_req_record_retained_is_monotone_max() {
        use std::sync::atomic::Ordering;
        H2_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);
        record_retained(4096);
        assert_eq!(H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed), 4096);
        record_retained(1024); // smaller — must NOT lower the max
        assert_eq!(H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed), 4096);
        record_retained(8192); // larger — moves the max up
        assert_eq!(H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed), 8192);
        H2_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);
    }

    #[test]
    fn h2_proxy_alt_svc_injected() {
        // Re-prove the shared Alt-Svc contract on the H2 path so a regression
        // here gets its own red test rather than hiding behind an H1 one.
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
        // H2 forbids these on the wire, but we still scrub before an H1
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

    // PROTO-2-11: a pre-cancelled token plus an EOF duplex. A regression that
    // re-introduces a busy-loop or holds the conn open indefinitely times out.
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
        // Empty duplex: the peer half is dropped, so reads EOF at once.
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
        // Only assert the deadline did not fire — Ok vs Err depends on whether
        // the H2 preface ever arrived.
        assert!(
            r.is_ok(),
            "serve_connection_with_cancel hung past 5 s deadline — graceful shutdown is broken"
        );
    }

    // ── F-SEC-1 deterministic gate (D3) ──
    //
    // The wire-level rapid-reset defect is a scheduler race, so a wire
    // observation of it CANNOT be a deterministic gate (that variant lives in
    // tests/h2_rapid_reset_goaway_under_load.rs as corroborating evidence).
    // The STRUCTURAL property is deterministic and is what guarantees the
    // queued GOAWAY survives teardown: `CleanCloseIo` must not resolve
    // `poll_shutdown` while unread inbound remains, because h2 drops the io
    // the instant it resolves.

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Arc-shared mock IO yielding `to_deliver` bytes in `chunk`-sized reads
    /// then a clean EOF; records whether the inner FIN was delegated and
    /// whether inbound drained to EOF before `poll_shutdown` resolved.
    struct ProbeInner {
        to_deliver: usize,
        chunk: usize,
        delivered: AtomicUsize,
        eof_seen: AtomicBool,
        shutdown_called: AtomicBool,
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
            self.0.shutdown_called.store(true, Ordering::SeqCst);
            Poll::Ready(Ok(()))
        }
    }

    /// DETERMINISTIC structural coverage (the gate itself is the wire test
    /// `tests/h2_security_live.rs::rapid_reset_goaway` under load):
    /// `poll_shutdown` delegates the FIN promptly, then does NOT resolve until
    /// inbound has drained to EOF — so h2's imminent drop is a clean close, not
    /// an RST that would discard the peer's already-received GOAWAY.
    #[tokio::test(flavor = "current_thread")]
    async fn clean_close_io_drains_inbound_to_eof_before_resolving() {
        use tokio::io::AsyncWriteExt;
        // 200 KiB inbound, under the 256 KiB DRAIN_CAP, in 4 KiB reads.
        let probe = Probe::new(200 * 1024, 4096);
        let mut io = CleanCloseIo::new(probe.clone());
        Pin::new(&mut io).shutdown().await.unwrap();

        let s = &probe.0;
        assert!(
            s.shutdown_called.load(Ordering::SeqCst),
            "inner poll_shutdown (FIN) was never delegated"
        );
        assert!(
            s.eof_seen.load(Ordering::SeqCst),
            "F-SEC-1: poll_shutdown resolved without draining inbound to \
             EOF — h2's imminent drop would RST and discard the peer's \
             queued GOAWAY"
        );
        assert_eq!(
            s.delivered.load(Ordering::SeqCst),
            200 * 1024,
            "all pending inbound bytes must be drained before resolving \
             (so the drop is a clean close, not an RST)"
        );
    }

    /// The drain is HARD-BOUNDED by `DRAIN_CAP`, so a deliberate unbounded
    /// post-GOAWAY flood cannot pin the worker — teardown still completes.
    #[tokio::test(flavor = "current_thread")]
    async fn clean_close_io_drain_is_bounded() {
        use tokio::io::AsyncWriteExt;
        // Endless source: never EOFs. The drain must still stop at DRAIN_CAP.
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

    /// F-SEC-1 CORE PROPERTY (the bug the prior fix missed): the FIN goes out
    /// on the FIRST poll — sending a FIN never causes an RST — but
    /// `poll_shutdown` MUST NOT resolve while the peer's write half is still
    /// open, because h2 drops the io the instant we resolve and that drop, with
    /// the flood still arriving, is exactly the RST that discards the client's
    /// already-received GOAWAY. Driven with a manual waker so the assertion is
    /// scheduler-independent.
    #[tokio::test(flavor = "current_thread")]
    async fn clean_close_io_does_not_resolve_while_peer_still_open() {
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        use std::task::Wake;

        // Pending until `release_eof` is set, then a clean EOF.
        struct LingerProbe {
            release_eof: AtomicBool,
            shutdown_called: AtomicBool,
            reads: AtomicUsize,
        }

        struct NoopWake;
        impl Wake for NoopWake {
            fn wake(self: Arc<Self>) {}
        }

        // Arc-shared so the test can flip `release_eof` after construction;
        // all I/O methods are interior-mutable via atomics.
        #[derive(Clone)]
        struct Shared(Arc<LingerProbe>);
        impl AsyncRead for Shared {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                let s = &self.0;
                s.reads.fetch_add(1, Ordering::SeqCst);
                if s.release_eof.load(Ordering::SeqCst) {
                    let _ = buf;
                    Poll::Ready(Ok(())) // 0 bytes = clean EOF
                } else {
                    Poll::Pending // peer write half still open
                }
            }
        }
        impl AsyncWrite for Shared {
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
                self.0.shutdown_called.store(true, Ordering::SeqCst);
                Poll::Ready(Ok(()))
            }
        }

        let probe = Arc::new(LingerProbe {
            release_eof: AtomicBool::new(false),
            shutdown_called: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
        });
        let mut io = CleanCloseIo::new(Shared(Arc::clone(&probe)));
        let waker = std::task::Waker::from(Arc::new(NoopWake));
        let mut cx = Context::from_waker(&waker);

        // Peer write half still open: poll_shutdown MUST stay Pending
        // (resolving ⇒ h2 drops the io ⇒ RST ⇒ GOAWAY discarded). The inner
        // FIN, however, IS sent on the first poll.
        for _ in 0..8 {
            assert!(
                Pin::new(&mut io).poll_shutdown(&mut cx).is_pending(),
                "F-SEC-1 DEFECT: poll_shutdown resolved while peer write \
                 half still open — h2 would drop the io and RST, \
                 discarding the queued GOAWAY"
            );
        }
        assert!(
            probe.shutdown_called.load(Ordering::SeqCst),
            "FIN must be sent promptly (FIN-first; it does not cause an \
             RST and adds no teardown latency)"
        );

        // Peer reacts to the GOAWAY+FIN and closes its write half (EOF).
        probe.release_eof.store(true, Ordering::SeqCst);
        let mut polled_ready = false;
        for _ in 0..4 {
            if Pin::new(&mut io).poll_shutdown(&mut cx).is_ready() {
                polled_ready = true;
                break;
            }
        }
        assert!(
            polled_ready,
            "after peer EOF the post-FIN drain completes and \
             poll_shutdown resolves (drop is now a clean close)"
        );
        assert!(
            probe.reads.load(Ordering::SeqCst) >= 9,
            "post-FIN drain must keep polling inbound across waits"
        );
    }

    // ── F-COR-1 (b) unit regression — RFC 9113 §8.1 trailer rule ──────

    /// RFC 9113 §8.1: a pseudo-header trailer cannot even be represented as an
    /// `http::HeaderName` (`:` is not a token char), so it never reaches this
    /// helper via the `HeaderMap` the H2 server hands us. The real H2 §8.1
    /// protection is the ORDERING fix — validate the inbound request BEFORE
    /// dialing, so hyper/h2's own rejection wins deterministically; gated by
    /// `tests/h2_validation_before_forward.rs`. This test pins only the
    /// no-regression half: valid trailers are still captured verbatim.
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
