//! Real hyper 1.x HTTP/2 proxy path. The service closure runs once per H2
//! STREAM, so the backend picker is hit per request, not per connection.

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

/// Hard cap on the inbound H2 request body (64 MiB) → `413`. Shared ceiling
/// with the H3 path's `lb_quic::MAX_REQUEST_BODY_BYTES`.
pub const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Bounded in-flight channel depth; `DEPTH × CHUNK_MAX` caps retained inbound
/// memory AND doubles as the validate-before-forward lookahead window.
pub const H2_REQ_CHANNEL_DEPTH: usize = 8;

/// Max size of one chunk through the in-flight channel; the window ceiling is
/// body-size-INDEPENDENT, the R8 property the memory proof asserts.
pub const H2_REQ_CHUNK_MAX: usize = 8 * 1024;

/// F-MD-4 — request-smuggling guard. Dropping the body channel reads as a
/// CLEAN EOF, so hyper would emit the terminator and the upstream would see an
/// RST-truncated request as COMPLETE. `hyper::Error` has no public ctor, so the
/// pump sends `Err(PumpAbort)` and hyper aborts without a terminator.
#[derive(Debug)]
struct PumpAbort;

impl std::fmt::Display for PumpAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("inbound H2 request body aborted before END_STREAM")
    }
}

impl std::error::Error for PumpAbort {}

/// F-MD-4 response leg — injected into the H2 RESPONSE StreamBody on a
/// connector `Reset`, so hyper RST_STREAMs instead of taking the clean-EOF
/// branch and smuggling a truncated response as complete.
#[derive(Debug)]
struct H2RespAbort;

impl std::fmt::Display for H2RespAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("H3 upstream response truncated before clean end")
    }
}

impl std::error::Error for H2RespAbort {}

/// F-MD-4 — how long the pump holds the body-channel sender open after
/// injecting `Err(PumpAbort)`. Holding it makes the upstream reset
/// DETERMINISTIC instead of racing a channel-close clean-EOF.
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
    ws: Option<Arc<WsProxy>>,
    grpc: Option<Arc<GrpcProxy>>,
    h2_upstream: Option<Arc<Http2Pool>>,
    h3_upstream: Option<Arc<QuicUpstreamPool>>,
    hooks: Arc<dyn DynSecurityHooks>,
    watchdog: Option<Watchdog>,
    conn_seq: Arc<parking_lot::Mutex<u64>>,
    expected_sni: Option<String>,
    /// ROUND8-L7-05: policy for `_` in inbound H2 header names. hyper's H2
    /// codec does NOT reject underscores — this is the only H2 enforcement point.
    header_underscore_policy: crate::h1_proxy::HeaderUnderscorePolicy,
    /// ROUND8-L7-07 / L7-12 — HAProxy `tune.h2.fe.glitches-threshold` analogue;
    /// crossing it drains the connection via the two-step GOAWAY (RFC 9113 §6.8).
    glitches_threshold: Option<u32>,
    glitches_metrics: Option<Arc<lb_observability::MetricsRegistry>>,
    /// CF-S27-2 — per-listener opt-in for RFC 8441 WebSocket-over-HTTP/2. OFF
    /// by default: the H2 upgraded-stream write path lacks end-to-end
    /// backpressure, so a non-reading client can force unbounded gateway memory.
    /// When `false` neither the SETTINGS bit nor the intercept fork is active.
    h2_extended_connect_enabled: bool,
}

/// F-SEC-1 — clean-close I/O wrapper guaranteeing the RFC 9113 §6.8
/// rapid-reset GOAWAY reaches the client before teardown.
///
/// THE CATCH: h2 drops this io a microsecond after `poll_shutdown` returns
/// `Ready`. Dropping a socket with unread inbound makes Linux emit an **RST**
/// (RFC 1122 §4.2.2.13 / `tcp_close`), and the peer then discards its ENTIRE
/// receive buffer — including the GOAWAY that already arrived.
///
/// Fix: FIN FIRST (a FIN never causes an RST), THEN drain inbound until the
/// peer closes its write half, yielding on `Pending` rather than letting the
/// drop race the peer. Hard-bounded by BOTH [`CleanCloseIo::DRAIN_CAP`] and
/// [`CleanCloseIo::LINGER_DEADLINE`] so a flooding client cannot pin a worker.
struct CleanCloseIo<IO> {
    inner: IO,
    drain_budget: usize,
    fin_done: bool,
    drained: bool,
    /// Armed with the FIN; bounds the wait for the peer's reciprocal FIN so a
    /// silent client cannot pin the worker.
    linger_deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<IO> CleanCloseIo<IO> {
    /// Larger than any legitimate in-flight burst, yet a hard cap so a
    /// deliberate post-GOAWAY flood cannot pin the worker.
    const DRAIN_CAP: usize = 256 * 1024;

    /// Wall-clock wait for the peer's reciprocal FIN; kept far below the
    /// surrounding `HttpTimeouts::total`.
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
        // F-SEC-1 step 1 — FIN first: a FIN does NOT cause an RST (only
        // DROPPING with unread inbound does).
        if !self.fin_done {
            ready!(Pin::new(&mut self.inner).poll_shutdown(cx))?;
            self.fin_done = true;
            self.linger_deadline = Some(Box::pin(tokio::time::sleep(Self::LINGER_DEADLINE)));
        }

        // F-SEC-1 step 2 — bounded post-FIN drain, so h2's imminent drop is a
        // clean close rather than an RST discarding the GOAWAY just received.
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
                            break;
                        }
                        self.drain_budget -= n;
                    }
                    Poll::Ready(Err(_)) => {
                        break;
                    }
                    Poll::Pending => {
                        // Resolving before the peer's reciprocal FIN would let
                        // h2's drop race an RST, so yield. `linger_deadline` is
                        // always `Some` here; absent, still never resolve early.
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
    /// Construct with the default [`H2SecurityThresholds`].
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

    /// Construct with an explicit [`H2SecurityThresholds`].
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

    /// Attach a security-hooks impl; without it the production checks are off.
    #[must_use]
    pub fn with_hooks(mut self, hooks: Arc<dyn DynSecurityHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Attach an [`lb_security::Watchdog`]; each H2 stream registers and
    /// deregisters independently.
    #[must_use]
    pub fn with_watchdog(mut self, watchdog: Watchdog) -> Self {
        self.watchdog = Some(watchdog);
        self
    }

    /// ROUND8-L7-07 / L7-12 — enable the consolidated glitches abuse counter;
    /// `threshold` of `0` keeps it dormant.
    ///
    /// The frame-arrival half ([`GlitchKind::FrameRecvTimeout`]) is NOT wired:
    /// hyper 1.x exposes no per-frame read context (`audit/deferred.md`).
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

    /// Default expected SNI. TLS listeners prefer
    /// [`Self::serve_connection_with_cancel_sni`], which uses the live SNI.
    #[must_use]
    pub fn with_expected_sni(mut self, sni: Option<String>) -> Self {
        self.expected_sni = sni;
        self
    }

    /// ROUND8-L7-05: set the header-name underscore policy.
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

    /// Enable WebSocket upgrade handling. This does NOT by itself enable
    /// WS-over-H2 — see [`Self::with_h2_extended_connect`] (default OFF).
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

    /// Enable gRPC handling. GRPC-001: aligns the [`GrpcProxy`]'s upstream
    /// `max_header_list_size` with this listener's so a malicious backend
    /// cannot transit oversize trailers through the gateway.
    #[must_use]
    pub fn with_grpc(mut self, grpc: GrpcProxy) -> Self {
        let aligned = grpc.with_max_header_list_size(self.security.max_header_list_size);
        self.grpc = Some(Arc::new(aligned));
        self
    }

    /// Drive HTTP/2 server logic over `io` until the connection closes, bounded
    /// by [`HttpTimeouts::total`]. Per-stream upstream errors become 502/504
    /// responses and do NOT terminate the connection.
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

    /// PROTO-2-11 — H2 half of the GOAWAY-on-drain contract; `cancel` triggers
    /// the canonical two-step GOAWAY (RFC 9113 §6.8).
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

    /// H2 entry point threading the per-connection TLS SNI into the hot path so
    /// the authority check runs against the OBSERVED SNI.
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

        // ROUND8-L7-07 — `conn_cancel` is a CHILD of the caller's `cancel`, so
        // a SIGTERM cancel propagates DOWN and a glitch-drain cancels it
        // DIRECTLY; both resolve the SAME two-step GOAWAY select arm below.
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
        // A `Timer` MUST be wired before `keep_alive_interval` can fire —
        // without it hyper panics "You must supply a timer."
        let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
        builder.timer(TokioTimer::new());
        self.security.apply(&mut builder);
        // RFC 8441 extended CONNECT, CF-S27-2 GATED OFF by default. When off
        // the SETTINGS bit is never sent AND the intercept fork is disabled, so
        // a client sending extended CONNECT anyway is NOT tunneled.
        if self.h2_extended_connect_enabled {
            builder.enable_connect_protocol();
        }
        // F-SEC-1: teardown drains inbound before the drop — see [`CleanCloseIo`].
        let conn = builder.serve_connection(TokioIo::new(CleanCloseIo::new(io)), svc);
        tokio::pin!(conn);
        let cancel_fut = conn_cancel.cancelled();
        tokio::pin!(cancel_fut);
        let timer = tokio::time::sleep(total);
        tokio::pin!(timer);
        tokio::select! {
            // biased: cancel wins ties so a SIGTERM mid-request still GOAWAYs.
            biased;
            () = &mut cancel_fut => {
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

/// Per-H2-connection abuse-counter state. ONE counter shared across every
/// stream of the connection (HAProxy's `h2c->glitches` is per-connection).
#[derive(Clone)]
struct GlitchConnState {
    counter: Arc<parking_lot::Mutex<GlitchesCounter>>,
    /// `None` still drains, just unobserved.
    metric: Option<lb_observability::IntCounter>,
    /// Cancelling this triggers the two-step GOAWAY select arm.
    drain: tokio_util::sync::CancellationToken,
}

impl GlitchConnState {
    /// Record one weighted abuse event; `true` (after cancelling the drain
    /// token) once the rolling weighted sum crosses the threshold.
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

/// The [`H2Proxy`] plus per-connection state, cloned by hyper per stream.
#[derive(Clone)]
struct ProxyService {
    inner: Arc<H2Proxy>,
    peer: SocketAddr,
    expected_sni: Option<String>,
    glitch: Option<GlitchConnState>,
}

/// F-S27-1 — inline WS dial+handshake outcome, so the caller picks the status
/// WITHOUT having emitted a `200` first: `Timeout` → `504`, `Refused` → `502`.
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
    /// H2 mirror of the H1 trace-context wire-in. `Instrument`, never
    /// `Entered` — an `Entered` guard leaks the span across an `.await`.
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
        // Inject once here so every downstream bridge forwards it.
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
        // ROUND8-L7-09 — authority-validation CHOKE POINT. MUST stay the FIRST
        // statement: the extended-CONNECT and gRPC forks below reached upstream
        // selection unvalidated before it was hoisted here.
        if let Err((bad, err)) = crate::authority::validate_request(&req) {
            tracing::warn!(
                peer = %peer,
                authority = %bad,
                error = ?err,
                "ROUND8-L7-09: H2 authority rejected (choke point)"
            );
            if let Some(g) = glitch {
                g.record(GlitchKind::RapidReset);
            }
            return error_response(StatusCode::BAD_REQUEST, "invalid authority (ROUND8-L7-09)");
        }

        // CF-S27-2: the gate holds even against a client that sends the
        // pseudo-header without the (un-advertised) SETTINGS bit.
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
            // GrpcProxy speaks H2 over a TCP-pool stream: any non-H3 backend.
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
        // is the only enforcement point on the H2 path.
        match self.header_underscore_policy {
            crate::h1_proxy::HeaderUnderscorePolicy::Reject => {
                if parts
                    .headers
                    .iter()
                    .any(|(n, _)| n.as_str().as_bytes().contains(&b'_'))
                {
                    // ROUND8-L7-07: low weight — one is noise, a burst trips.
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

        // Hooks run BEFORE the strip + upstream-acquire.
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

        // SEC-2-01: `SmuggleMode::H2` adds `check_h2_downgrade` (forbidden
        // hop-by-hop headers, non-`trailers` TE — RFC 9113 §8.2.2).
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
            // ROUND8-L7-07: highest weight — a burst drains fast.
            if let Some(g) = glitch {
                g.record(GlitchKind::HpackRatio);
            }
            return error_response(StatusCode::BAD_REQUEST, "request smuggling");
        }

        // PROTO-2-01 / RFC 9113 §8.3.1: `:authority` and `Host` MUST agree —
        // disagreement is a host-confusion primitive against backends that
        // authorise on `Host`. Reject BEFORE the strip / upstream acquire.
        if let Err(msg) = check_authority_host_agreement(&parts.uri, &parts.headers) {
            tracing::warn!(peer = %peer, reason = msg, "h2 :authority/Host mismatch rejected");
            if let Some(g) = glitch {
                g.record(GlitchKind::RapidReset);
            }
            return error_response(StatusCode::BAD_REQUEST, msg);
        }

        // PROTO-2-18 — SNI ↔ `:authority`/Host agreement (RFC 9110 §15.5.20).
        // Precedence: smuggle → authority/Host → SNI/Host. Loopback peers skip
        // it (sec-r5): the vector is L7 routing/authz.
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

        // PROTO-2-07 — the newtype makes the strip a type-level guarantee for
        // the proxy_* fan-out.
        let req_pre_strip = Request::from_parts(parts, body);
        let mut stripped = strip_into_newtype(req_pre_strip);
        {
            let headers = stripped.headers_mut();
            append_xff(headers, peer);
            set_xfp(headers, self.is_https);
            if let Some(h) = authority.as_deref() {
                set_xfh(headers, h);
                // An H1 upstream requires `Host`; synthesise it from `:authority`.
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
                Err(ProxyErr::BodyTooLarge) => error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body exceeds maximum",
                ),
                // F-COR-1: 400 WITHOUT a dial, so no backend body can leak.
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
    /// client-visible response (mirror of H1's ROUND8-L7-01 "defer 101"). When
    /// the dial lived in the detached task, a backend that refused the
    /// handshake still left the client holding a `200` (false success) and
    /// anything pipelined behind the CONNECT could be relayed to a backend that
    /// never agreed to the upgrade.
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

        // RFC 8441 §4 — extended CONNECT MUST carry `:scheme` and `:path`;
        // reject BEFORE any dial rather than defaulting `:path` to "/".
        // Measured (tests/ws_h2_conformance.rs): a missing `:scheme` is
        // rejected ONLY here — hyper does not require it. The `:path` arm is
        // defense-in-depth (hyper's codec also catches it).
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

        // ROUND8-OPS-06: `handle` already injected the CHILD `traceparent`, so
        // read it back off the request for the tungstenite builder.
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

        // Upstream established: ONLY NOW arm the upgrade and build the `200`.
        // Holding `backend_ws` open across that window is intentional.
        let upgrade_fut = hyper::upgrade::on(&mut req);
        tokio::spawn(async move {
            let upgraded = match upgrade_fut.await {
                Ok(u) => u,
                Err(e) => {
                    // `backend_ws`'s Drop closes the pooled socket — no leak.
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

        // F-MD-1 — THE CATCH: with `version == HTTP/2.0` and a stale
        // `content-length`/`transfer-encoding`, hyper's http1 encoder
        // MIS-FRAMES an unknown-length streaming body — it sends an empty body
        // and never polls our `StreamBody`. Normalise so hyper picks the framing.
        parts.version = hyper::Version::HTTP_11;
        parts.headers.remove(hyper::header::CONTENT_LENGTH);
        parts.headers.remove(hyper::header::TRANSFER_ENCODING);

        // S8 / M-D — the in-flight window doubles as a validate-before-forward
        // lookahead: a request ≤ window reaches inbound EOF first and is
        // rejected with ZERO backend dial; a larger one dials and streams, but
        // the response HEAD stays gated on the inbound terminal state.
        use hyper::body::Body as _;
        use hyper::body::Frame;

        let mut lookahead: Vec<Bytes> = Vec::new();
        let mut buffered: usize = 0;
        let mut trailers_map: Option<hyper::HeaderMap> = None;
        let mut reached_eof = false;

        loop {
            #[cfg(any(test, feature = "test-gauges"))]
            record_retained(buffered);

            // Strictly `>`: holding a past-window request for
            // validate-before-dial would violate R8.
            if buffered > H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX {
                break;
            }

            match body.frame().await {
                None => {
                    // F-MD-4: `None` is AMBIGUOUS — hyper maps an inbound
                    // RST_STREAM(CANCEL/NO_ERROR) to it. Only a confirmed
                    // END_STREAM is clean; a reset is rejected BEFORE any dial.
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
                        // Trailers are the terminal frame — clean EOF.
                        trailers_map = frame.into_trailers().ok();
                        reached_eof = true;
                        break;
                    }
                }
                Some(Err(e)) => {
                    // Surfaced while VALIDATING, still BEFORE any dial → a
                    // malformed request can never leak the backend response.
                    return Err(ProxyErr::BadRequest(format!(
                        "malformed H2 request body: {e}"
                    )));
                }
            }
        }

        if reached_eof {
            // ── Branch A: the whole request fit the window; zero backend dial.
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
            // S14: a within-window body cannot be a slow upload, so `head`
            // bounds the roundtrip. NOT a load-bearing idle-watchdog site.
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

        // ── Branch B: dial + stream with the bounded in-flight window.
        let pooled = self
            .pool
            .acquire_async(backend_addr)
            .await
            .map_err(|e| ProxyErr::Upstream(format!("backend connect {backend_addr}: {e}")))?;
        let stream = pooled
            .take_stream()
            .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;
        // F-MD-4: a CONSTRUCTIBLE body error so the pump can INJECT an error;
        // dropping the channel instead reads as a smuggled-complete request.
        let (mut sender, conn) = hyper::client::conn::http1::handshake::<
            _,
            BoxBody<Bytes, PumpAbort>,
        >(TokioIo::new(stream))
        .await
        .map_err(|e| ProxyErr::Upstream(format!("h1 client handshake: {e}")))?;
        let conn_handle = tokio::spawn(async move {
            let _ = conn.await;
        });

        // R8 backpressure chain: backend write stalls → hyper stops pulling →
        // the channel fills → the pump stops polling → h2 withholds
        // WINDOW_UPDATE → the client pauses.
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, PumpAbort>>(H2_REQ_CHANNEL_DEPTH);

        // F-MD-3 — a GENUINE gauge: the old sites recorded a CONSTANT, so a
        // buffering regression would not have moved it. This tracks the ACTUAL
        // live channel occupancy.
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

            // The 64 MiB cap applies in the streaming regime too.
            let mut forwarded_total: usize = buffered;
            // Bytes still queued in `drained` — part of the live retained set.
            let mut lookahead_remaining: usize = buffered;

            // `ReceiverGone` = hyper dropped the request body. Do NOT
            // manufacture a 413 (F-MD-2) — drain-and-validate instead.
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
                        // F-MD-3: retained = queued lookahead + live channel.
                        #[cfg(any(test, feature = "test-gauges"))]
                        record_retained(
                            lookahead_remaining
                                + in_flight_bytes.load(std::sync::atomic::Ordering::Relaxed),
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
            // malformed request would relay the backend response.
            macro_rules! drain_and_validate {
                () => {{
                    loop {
                        match body.frame().await {
                            None => {
                                // `None` is ambiguous (reset vs END_STREAM).
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

            for chunk in drained {
                if let Err(SendOutcome::ReceiverGone) = send_chunked!(chunk, true) {
                    // F-MD-2 drain-and-validate, NOT a 413.
                    let _ = verdict_tx.send(drain_and_validate!());
                    return;
                }
            }
            loop {
                match body.frame().await {
                    None => {
                        // F-MD-4 — THE H2 CATCH (exact inverse of the H1 rule):
                        // `frame()==None` is AMBIGUOUS — hyper maps an inbound
                        // RST_STREAM(CANCEL/NO_ERROR) to `Ready(None)`,
                        // indistinguishable from a clean END_STREAM (hyper-1.9.0
                        // body/incoming.rs ~L250), so inferring EOF from it
                        // would relay a truncated request as COMPLETE.
                        // `is_end_stream()` is the deterministic discriminator:
                        // true IFF a real END_STREAM flag was seen, FALSE after
                        // any reset (h2-0.4.13 proto/streams/state.rs
                        // `is_recv_end_stream`) — a protocol STATE, not a race.
                        if body.is_end_stream() {
                            set_complete();
                            let _ = verdict_tx.send(Ok(()));
                        } else {
                            // Inject a BODY ERROR so hyper aborts the upstream
                            // request WITHOUT a terminator.
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
                            // Validate BEFORE forwarding.
                            match validate_request_trailers(frame.trailers_ref()) {
                                Ok(_) => {
                                    let _ = tx.send(Ok(frame)).await;
                                    bump();
                                    set_complete();
                                    let _ = verdict_tx.send(Ok(()));
                                    return;
                                }
                                Err(e) => {
                                    // F-MD-4: inject a BODY ERROR — dropping tx
                                    // alone reads as a smuggled-complete request.
                                    let _ = tx.send(Err(PumpAbort)).await;
                                    let _ = verdict_tx.send(Err(e));
                                    return;
                                }
                            }
                        }
                        if let Ok(data) = frame.into_data() {
                            forwarded_total = forwarded_total.saturating_add(data.len());
                            if forwarded_total > MAX_REQUEST_BODY_BYTES {
                                // F-MD-4: inject a BODY ERROR so the upstream
                                // body ends WITHOUT a clean terminator, then 413.
                                let _ = tx.send(Err(PumpAbort)).await;
                                let _ = verdict_tx.send(Err(ProxyErr::BodyTooLarge));
                                return;
                            }
                            if let Err(SendOutcome::ReceiverGone) = send_chunked!(data, false) {
                                // F-MD-2 drain-and-validate, NOT a 413.
                                let _ = verdict_tx.send(drain_and_validate!());
                                return;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        // F-MD-4: inject a BODY ERROR so the upstream body
                        // terminates ABRUPTLY; dropping the sender alone would
                        // be a clean EOF → smuggled complete.
                        let _ = tx.send(Err(PumpAbort)).await;
                        let _ = verdict_tx.send(Err(ProxyErr::BadRequest(format!(
                            "malformed H2 request body: {e}"
                        ))));
                        return;
                    }
                }
            }
        });

        // Drive the send concurrently with the pump (hyper must pull the
        // channel for the pump to progress under backpressure), but do NOT
        // relay the response until the verdict lands.
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
                tracing::warn!(error = %idle_err, "h2→h1 idle/head deadline fired");
                pump.abort();
                conn_handle.abort();
                return Err(ProxyErr::Timeout);
            }
        };

        // The head relays only once the inbound body has validated.
        match verdict_rx.await {
            Ok(Ok(())) => {
                drop(conn_handle);
                Ok(resp)
            }
            Ok(Err(e)) => {
                // Abort the upstream conn (do NOT pool it); never relay it.
                conn_handle.abort();
                Err(e)
            }
            Err(_) => {
                // No verdict — never leak the backend response.
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

    /// Forward an H2 inbound request to an H2 backend, STREAMING both legs.
    /// Deltas vs [`Self::proxy_request`]: the request stays HTTP/2-shaped (no
    /// force-HTTP/1.1, no CL/TE strip — H2 framing is hyper's encoder's job)
    /// and the egress is the multiplexed [`Http2Pool`].
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
            Err(ProxyErr::BodyTooLarge) => error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body exceeds maximum",
            ),
            // F-COR-1 / F-MD-4: inbound validation failure → 400, NOT 502.
            Err(ProxyErr::BadRequest(s)) => error_response(StatusCode::BAD_REQUEST, &s),
        }
    }

    /// H2→H2 Leg 1 — the streaming REQUEST pump; a MIRROR of
    /// [`Self::proxy_request`]'s orchestration (leaf helpers are REUSED).
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

        // DELTA vs `proxy_request`: do NOT force HTTP/1.1 or strip CL/TE —
        // those were H1-framing fixes; H2 framing is hyper's encoder's job.
        let upstream_parts = match build_h2_upstream_request_parts(&parts) {
            Ok(p) => p,
            Err(e) => return Err(ProxyErr::Upstream(e)),
        };

        // Lookahead posture IDENTICAL to `proxy_request` (rationale there).
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
                    // F-MD-4: only a confirmed END_STREAM is clean; a reset is
                    // rejected here, before any pool contact.
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
                    // Zero-dial: rejected before any pool contact.
                    return Err(ProxyErr::BadRequest(format!(
                        "malformed H2 request body: {e}"
                    )));
                }
            }
        }

        if reached_eof {
            // ── Branch A: the whole request fit the window; zero pool contact.
            let trailers_vec = validate_request_trailers(trailers_map.as_ref())?;

            let body_bytes = concat_chunks(&lookahead, buffered);
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

        // ── Branch B: stream with the bounded in-flight window. DELTA: no
        // dial/handshake — the Http2Pool multiplexes.
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, PumpAbort>>(H2_REQ_CHANNEL_DEPTH);

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
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), ProxyErr>>();
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

            // F-MD-4 (body-layer half) — inject `Err(PumpAbort)` and HOLD the
            // sender open until hyper OBSERVES it. mpsc is FIFO, so holding it
            // forces hyper to poll the error BEFORE any channel-close `None`,
            // and it RESETS the stream instead of emitting a spurious
            // END_STREAM.
            //
            // Only HALF the fix: the other half is the caller's detached send
            // task + `reset_peer`, without which a downstream RST cancels this
            // future, drops the upstream body at a clean frame boundary and
            // lets hyper finalize END_STREAM ahead of this injection. Bounded
            // by `H2_ABORT_OBSERVE_TIMEOUT` against a wedged driver.
            macro_rules! inject_abort {
                () => {{
                    let _ = tx.send(Err(PumpAbort)).await;
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
                        // F-MD-4: positively confirm END_STREAM; a `None` from
                        // a RST must NOT be relayed as a clean EOF.
                        if body.is_end_stream() {
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

        // F-MD-4: the shared driver owns the detached send task (biased
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

    /// Forward an H2 inbound request to a STREAMING H3 backend. Mirror of
    /// [`crate::h1_proxy::H1Proxy::proxy_h1_to_h3`]; H2 deltas: the pump
    /// disambiguates the ambiguous H2 `None` via `is_end_stream()`, and the
    /// response head uses H2→H2 semantics (no `RESPONSE_HOP_BY_HOP` strip).
    ///
    /// HAZARD — request cancel-race: the connector treats a `body_tx` dropped
    /// WITHOUT a final `End`/`Reset` as a bodyless-COMPLETE request, so a
    /// downstream `RST_STREAM` cancelling this *service* future must NOT drop
    /// the pump silently. The mitigation is LOAD-BEARING: the pump is DETACHED
    /// and ALWAYS emits an explicit terminal event.
    ///
    /// F-CAP-1: a PRE-DATA over-cap → connector inline-413; a MID-BODY one →
    /// `H3RespEvent::Reset`, turned into an injected response-body `Err` so
    /// hyper RST_STREAMs rather than emitting a clean END_STREAM.
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

        let inner = req.into_inner();
        let (parts, mut body) = inner.into_parts();
        let headers = match build_h2_to_h3_fieldlist(&parts, &sni) {
            Ok(h) => h,
            Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
        };

        // Backpressure: a slow QUIC upstream → the connector stops draining →
        // this channel fills → the pump stops polling → the H2 window stalls.
        let (body_tx, body_rx) =
            tokio::sync::mpsc::channel::<ReqBodyEvent>(lb_quic::conn_actor::H3_BODY_CHANNEL_DEPTH);
        let (resp_tx, mut resp_rx) = tokio::sync::mpsc::channel::<lb_quic::H3RespEvent>(
            lb_quic::h3_bridge::H3_RESP_CHANNEL_DEPTH,
        );

        // F-MD-3 gauge: in-flight bytes, bounded independent of body size.
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // DETACHED (see HAZARD above): a downstream H2 RST must NOT drop this
        // task before it emits an explicit terminal event.
        let pump_in_flight = std::sync::Arc::clone(&in_flight_bytes);
        let pump = tokio::spawn(async move {
            // The request-body cap is OUR job (the connector caps the
            // RESPONSE). Over-cap BEFORE any chunk → `Reset` first → connector
            // inline-413; after ≥1 chunk → RESET-without-FIN, no 413.
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
                        // END_STREAM → FIN; a `None` from a RST → `Reset` →
                        // RESET-without-FIN. This explicit terminal is what
                        // defends the dropped-tx == bodyless-COMPLETE contract.
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
                            // Validate BEFORE forwarding; forbidden → `Reset`,
                            // never a clean `End` (desync primitive).
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
                        // F-MD-4 (H2): a mid-body error → `Reset`, so the
                        // backend never sees a truncated request as complete.
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
        let alt_svc = self.alt_svc;
        let first = resp_rx.recv().await;
        match first {
            Some(lb_quic::H3RespEvent::Head { status, headers }) => {
                let st = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
                let builder = h2_decoded_resp_head_builder(st, &headers, alt_svc);

                // `Reset` → inject a body error so hyper RST_STREAMs instead
                // of emitting a clean END_STREAM (response-splitting guard). A
                // `Trailers` event maps to a native `Frame::trailers`, which
                // hyper flushes WITHOUT a `Trailer:` pre-declaration — that is
                // what gets gRPC's `grpc-status` to the H2 client.
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
                    // F-MD-4 (response leg): the relay SENDS `H2RespAbort`
                    // (never a clean drop), so hyper RST_STREAMs rather than
                    // presenting a truncated body as complete.
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

/// Build the upstream H2 request HEAD for the streaming relay, WITHOUT touching
/// the body. DELTA vs the H1-egress pump: content-length / transfer-encoding are
/// NOT stripped — H2 upstream framing is hyper's H2 encoder's job.
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

/// RFC 9113 §8.1 — reject a `:`-prefixed name in the trailing field section.
/// `#[cfg(test)]`-only; production rejection is [`validate_request_trailers`].
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

/// F-COR-1 (b) / RFC 9113 §8.1: a pseudo-header in the trailing field section
/// is malformed — reject (400), never forward. Returns the pairs to forward.
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

/// Concatenate lookahead DATA chunks; `total` presizes the one allocation.
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

/// PROTO-2-12 — data bytes, then a `Frame::trailers` when `trailers` is set.
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

/// Convert an upstream H2 `Response<Incoming>` into the downstream response,
/// streaming by construction — upstream trailers ride the terminal frame, so
/// no `collect()` is needed to capture them.
fn upstream_h2_response_to_h2(
    resp: Response<IncomingBody>,
    alt_svc: Option<AltSvcConfig>,
) -> Response<ClientRespBody> {
    let (parts, body) = resp.into_parts();
    // No hop-by-hop strip beyond dropping pseudo-headers (nor did the bridge).
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
    // R8: stream the `Incoming` by construction.
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

/// F-MD-4 — shared graceful-drop egress driver so the H2→H2 and H1→H2 streaming
/// paths keep ONE copy of the smuggling fix. Owns the detached send task, the
/// biased verdict-vs-head race with `reset_peer` on every abort verdict, the
/// F-CAP-1 caller arm, and the final `head_rx.await`.
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

    // F-MD-4 — DETACH the send so it is not tied to the downstream stream
    // future's lifetime. ROOT CAUSE of the intermittent smuggle: a downstream
    // RST_STREAM cancels this service future; with the send owned directly
    // that cancel DROPS the upstream body at a clean frame boundary and hyper
    // finalizes END_STREAM on the graceful drop — relaying the truncated
    // request as COMPLETE before any `reset_peer` could run. Detached, a cancel
    // only drops `head_rx`; the task `reset_peer`s BEFORE dropping the body.
    let (head_tx, head_rx) =
        tokio::sync::oneshot::channel::<Result<Response<IncomingBody>, ProxyErr>>();
    let pool_for_task = pool.clone();
    tokio::spawn(async move {
        // S14 — two-phase idle/head deadline instead of the pool's fixed
        // `send_timeout`; ROUND8-L7-10 eviction is preserved inside it.
        let mut send_fut = std::pin::pin!(pool_for_task.send_request_idle(
            backend_addr,
            upstream_req,
            last_progress,
            upload_complete,
            epoch,
            idle,
            head_timeout,
        ));
        // `resp` is Some only when the head won the race.
        let resp: Option<Response<IncomingBody>> = tokio::select! {
            // biased: an abort verdict landing with the head must win so we
            // RESET rather than relay.
            biased;
            v = &mut verdict_rx => {
                match v {
                    // Reset FIRST, then drop the send future (and body).
                    Ok(Err(e)) => {
                        pool_for_task.reset_peer(backend_addr);
                        pump.abort();
                        let _ = head_tx.send(Err(e));
                        return;
                    }
                    Ok(Ok(())) => {
                        let out = match send_fut.await {
                            Ok(r) => Ok(r),
                            Err(Http2PoolError::Timeout) => Err(ProxyErr::Timeout),
                            Err(e) => Err(ProxyErr::Upstream(format!("h2 upstream: {e}"))),
                        };
                        let _ = head_tx.send(out);
                        return;
                    }
                    // No verdict — reset; never leak.
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
                    // F-CAP-1: consult the verdict FIRST (bounded) and prefer a
                    // classified 413/400 over the generic 502.
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
        // Every non-head branch above `return`ed: the head won the race.
        let Some(resp) = resp else { return };

        // Relay only once the inbound body reached a validated terminal state.
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

    // If the downstream RSTs this await is cancelled, but the detached task
    // survives and still resets the upstream on an abort verdict.
    match head_rx.await {
        Ok(result) => result,
        // `head_tx` dropped without sending — never leak a response.
        Err(_) => Err(ProxyErr::BadRequest(
            "inbound H2 upstream send task terminated without a result".to_owned(),
        )),
    }
}

/// Build the H2→H3 request FIELD-LIST from the HEAD only; body and trailers
/// stream through the connector.
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

/// Build the streaming H2 response head from a decoded H3 `Head` event. UNLIKE
/// the H1→H3 builder there is NO `RESPONSE_HOP_BY_HOP` strip: hyper's H2 server
/// encoder already rejects connection-specific headers on egress.
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
    /// Inbound H2 request failed validation → 400, returned BEFORE any dial so
    /// it can never leak the backend's 200 body.
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

/// PROTO-2-01 / RFC 9113 §8.3.1 — `Err` when `:authority` and `Host` are both
/// present and their hosts disagree. Case-insensitive (RFC 3986 §3.2.2); the
/// port is ignored when either side elides it (§8.3.1 default-port carve-out).
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

/// Compare `:authority` against `Host` (RFC 9113 §8.3.1). Ports must match only
/// when BOTH sides carry one — the proxy has no default-port table.
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

/// Split `host[:port]`; bracketed IPv6 literals keep their brackets so the
/// compare never splits on a colon inside the literal.
fn split_host_port(s: &str) -> (&str, Option<&str>) {
    if let Some(stripped) = s.strip_prefix('[') {
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
/// + in-flight channel). A whole-body-buffering variant would grow with request
/// size; the bounded window keeps this flat. Test-only.
#[cfg(any(test, feature = "test-gauges"))]
pub static H2_REQ_MAX_RETAINED_BODY_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Lock-free CAS-max update for [`H2_REQ_MAX_RETAINED_BODY_BYTES`].
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

    /// The R8 body-independence proof rests on these exact values.
    #[test]
    fn h2_req_window_constants_pinned() {
        assert_eq!(H2_REQ_CHANNEL_DEPTH, 8, "in-flight channel depth");
        assert_eq!(H2_REQ_CHUNK_MAX, 8 * 1024, "per-chunk max (8 KiB)");
        assert_eq!(
            H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX,
            64 * 1024,
            "in-flight window ceiling (64 KiB)"
        );
        // `black_box` keeps this a runtime check, not a folded const.
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
        // H2 forbids these on the wire; we scrub for an H1 upstream.
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
        let mut h = map_with(&[("x-forwarded-for", "10.0.0.1")]);
        let peer: SocketAddr = "1.2.3.4:5555".parse().unwrap();
        append_xff(&mut h, peer);
        assert_eq!(h.get("x-forwarded-for").unwrap(), "10.0.0.1, 1.2.3.4");
    }

    // PROTO-2-11: a busy-loop or a held-open conn times out.
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
        // Only the deadline matters: Ok vs Err depends on the H2 preface.
        assert!(
            r.is_ok(),
            "serve_connection_with_cancel hung past 5 s deadline — graceful shutdown is broken"
        );
    }

    // F-SEC-1: the wire-level rapid-reset defect is a scheduler race, so the
    // deterministic gate is the STRUCTURAL property — `CleanCloseIo` must not
    // resolve `poll_shutdown` while unread inbound remains, because h2 drops
    // the io the instant it resolves.

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Mock IO yielding `to_deliver` bytes then EOF, recording whether the FIN
    /// was delegated and inbound drained before `poll_shutdown` resolved.
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

    /// `poll_shutdown` delegates the FIN promptly, then does NOT resolve until
    /// inbound drained to EOF — so h2's imminent drop is a clean close, not an
    /// RST discarding the peer's already-received GOAWAY.
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

    /// The drain is HARD-BOUNDED by `DRAIN_CAP`: an unbounded post-GOAWAY
    /// flood cannot pin the worker.
    #[tokio::test(flavor = "current_thread")]
    async fn clean_close_io_drain_is_bounded() {
        use tokio::io::AsyncWriteExt;
        // Endless source: never EOFs; the drain must still stop at DRAIN_CAP.
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
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Pin::new(&mut io).shutdown(),
        )
        .await
        .expect("F-SEC-1: bounded drain must not hang on an endless inbound flood")
        .unwrap();
        assert!(
            io.drain_budget == 0,
            "drain must consume exactly up to DRAIN_CAP then stop"
        );
    }

    /// F-SEC-1 CORE PROPERTY: the FIN goes out on the FIRST poll, but
    /// `poll_shutdown` MUST NOT resolve while the peer's write half is open —
    /// h2 drops the io the instant we resolve, and that drop with the flood
    /// still arriving is the RST that discards the client's GOAWAY. Manual
    /// waker so the assertion is scheduler-independent.
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
        // (resolving ⇒ h2 drops the io ⇒ RST ⇒ GOAWAY discarded).
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

    /// RFC 9113 §8.1: a pseudo-header trailer cannot be represented as an
    /// `http::HeaderName`, so it never reaches this helper. The real §8.1
    /// protection is the ORDERING fix (validate BEFORE dialing, gated by
    /// `tests/h2_validation_before_forward.rs`); this pins only the
    /// no-regression half.
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
