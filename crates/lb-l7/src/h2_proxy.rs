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

/// S8 / M-D (Q-D4) — depth of the bounded in-flight channel feeding the
/// streaming H2→H1 request body. Mirrors the H3 cells'
/// `H3_BODY_CHANNEL_DEPTH = 8`. The fixed in-flight window =
/// `H2_REQ_CHANNEL_DEPTH × H2_REQ_CHUNK_MAX` (= 64 KiB) is the ceiling on
/// retained inbound-request memory and DOUBLES as the validate-before-forward
/// lookahead: a whole request that fits inside the window is polled to EOF
/// (driving the identical hyper/h2 validation `collect()` did) BEFORE the
/// upstream is dialed.
pub const H2_REQ_CHANNEL_DEPTH: usize = 8;

/// S8 / M-D (Q-D4) — maximum size of one chunk pumped through the in-flight
/// channel. Mirrors the H3 cells' `H3_BODY_CHUNK_MAX = 8 KiB`. The window
/// ceiling `H2_REQ_CHANNEL_DEPTH × H2_REQ_CHUNK_MAX = 64 KiB` is
/// body-size-INDEPENDENT and independent of [`MAX_REQUEST_BODY_BYTES`]
/// (64 MiB) — that body-independence is the R8 property the memory proof
/// asserts.
pub const H2_REQ_CHUNK_MAX: usize = 8 * 1024;

/// F-MD-4 (S8 remediation) — request-smuggling fix. The Branch-B streaming
/// pump feeds the inbound request body to the upstream via a bounded `mpsc`
/// channel bridged into an `http_body` `StreamBody`. A dropped channel sender
/// makes the receiver's `poll_recv` return `None`, which `StreamBody`
/// translates to a CLEAN body EOF — hyper then emits the chunked terminator
/// (`0\r\n\r\n`) and the upstream sees a COMPLETE request. That is the wrong
/// signal when the inbound stream was RST mid-body (smuggling): a truncated
/// request must NEVER be relayed as complete.
///
/// `hyper::Error` has no public constructor, so we cannot inject one into the
/// channel directly. This tiny error is the channel's error type instead: on
/// every inbound-error / abort path the pump SENDS `Err(PumpAbort)` into the
/// channel BEFORE returning, so hyper polls the body, sees an ERROR (not a
/// clean EOF), and aborts the upstream request WITHOUT a terminator. It
/// satisfies hyper's request-body bound
/// `Body::Error: Into<Box<dyn std::error::Error + Send + Sync>>`.
#[derive(Debug)]
struct PumpAbort;

impl std::fmt::Display for PumpAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("inbound H2 request body aborted before END_STREAM")
    }
}

impl std::error::Error for PumpAbort {}

/// S13 H2→H3 (F-MD-4 response leg) — the constructible truncation error
/// injected into the H2 RESPONSE body StreamBody when the H3 connector emits
/// `H3RespEvent::Reset` (a partial / premature-FIN / upstream-reset response).
/// `hyper::Error` has no public ctor, so this tiny error is the StreamBody's
/// error type: hyper polls the body, sees an ERROR (not a clean EOF), and
/// RST_STREAMs the downstream H2 stream WITHOUT a clean END_STREAM — the H2
/// client sees an aborted body, never a smuggled-complete response (response-
/// splitting guard). Mirror of `h1_proxy::H1PumpAbort` for the H2 front; boxed
/// at the StreamBody boundary to satisfy
/// `Body::Error: Into<Box<dyn std::error::Error + Send + Sync>>`.
#[derive(Debug)]
struct H2RespAbort;

impl std::fmt::Display for H2RespAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("H3 upstream response truncated before clean end")
    }
}

impl std::error::Error for H2RespAbort {}

/// F-MD-4 (S10 H2→H2 DEFECT FIX) — upper bound on how long the Branch-B
/// request pump waits, after injecting `Err(PumpAbort)` into the upstream
/// request-body channel, for hyper to OBSERVE that error and drop the
/// channel receiver (i.e. reset the upstream H2 stream). Holding the
/// sender open across this window is what makes the upstream reset
/// DETERMINISTIC instead of racing a channel-close clean-EOF — see the
/// `inject_abort!` macro in [`H2Proxy::proxy_h2_to_h2_request`]. hyper
/// drops the receiver essentially immediately once it polls the body, so
/// this bound is only a liveness backstop against a wedged upstream
/// driver; it must not hang the detached pump task forever. 5 s is
/// generous relative to the sub-millisecond observed reset latency while
/// remaining well under typical request/body timeouts.
// CF-DEDUP-1 / S11 I2: `pub(crate)` so the H1→H2 streaming request pump
// (`h1_proxy::proxy_h1_to_h2_request`) shares the SAME abort-observe bound
// rather than re-declaring a drifting copy (mechanical, no behaviour change).
pub(crate) const H2_ABORT_OBSERVE_TIMEOUT: Duration = Duration::from_secs(5);

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
/// guarantees the RFC 9113 §6.8 rapid-reset GOAWAY actually reaches the
/// client before connection teardown, deterministically, under load.
///
/// ## Proven mechanism (hyper-1.9.0 + h2-0.4.13 source + phase3-final R1)
///
/// Rapid-reset enforcement is delegated to hyper/h2
/// (`max_pending_accept_reset_streams` / `max_local_error_reset_streams`
/// via [`H2SecurityThresholds::apply`]). On the trip:
/// `h2::server::Connection::poll_accept` → `poll_closed` →
/// `connection.poll()`; `poll2`'s `recv_frame` trips the reset counter →
/// `Err(GoAway(ENHANCE_YOUR_CALM, Library))`; `handle_go_away` queues the
/// GOAWAY frame and sets `State::Closing`; the next loop iteration runs
/// `ready!(self.codec.shutdown(cx))?` =
/// `framed_write::shutdown` = `flush()` (which `poll_write_buf`s the
/// GOAWAY bytes into this io then `poll_flush`es) **then**
/// `inner.poll_shutdown(cx)` (the FIN). h2 then transitions
/// `State::Closed` and `connection.poll()` resolves
/// `Poll::Ready(Err(library_go_away))`; hyper
/// (`proto/h2/server.rs` `Some(Err(e)) => return Poll::Ready(Err(..))`)
/// surfaces it, the gateway's `res = &mut conn` select arm returns
/// `Err`, and `conn` (owning this io) is dropped.
///
/// So h2 *does* push the GOAWAY into the kernel TCP send buffer before
/// the FIN. The real defect was the **abortive RST close**: the prior
/// drain implementation read pending inbound only until the first
/// `Poll::Pending`, then *broke and FINed anyway*. During a rapid-reset
/// flood the abusive client is *continuously* streaming RST_STREAM
/// frames, so the server kernel recv buffer is essentially never
/// durably empty and the client still has bytes in flight even when it
/// momentarily is. Closing/dropping a TCP socket while the peer is
/// still actively sending makes Linux emit an **RST** instead of a
/// clean FIN (RFC 1122 §4.2.2.13 / Linux `tcp_close`); the client's TCP
/// stack discards its *entire* receive buffer — including the GOAWAY
/// that already arrived — on the RST, surfacing only `Io(BrokenPipe)`
/// with `send_err=None` (the exact phase3-final signature, ~1/3 under
/// 8-core full-workspace contention).
///
/// ## Fix — FIN-first then bounded post-FIN drain (lingering close)
///
/// The RST that discards the client's GOAWAY is caused by **dropping /
/// `close()`ing a socket that still has unread data in its receive
/// buffer** (RFC 1122 §4.2.2.13 / Linux `tcp_close`). Sending the
/// TCP FIN (write-half shutdown) does NOT cause an RST. h2 drops the
/// io *immediately* after our `poll_shutdown` returns `Ready` (verified
/// in source: `State::Closing` → `codec.shutdown` → `State::Closed`
/// → conn future resolves `Err` → hyper returns → conn dropped). So:
///
/// `poll_shutdown` (1) first delegates the inner `poll_shutdown` to
/// send the FIN/`close_notify` **promptly** (no teardown latency added
/// — a keep-alive-timeout / abrupt close still closes right away), then
/// (2) performs a bounded post-FIN drain: read+discard any remaining
/// inbound until the peer closes its own write half (**EOF** — the
/// normal reaction to receiving the GOAWAY+FIN), so that when h2 drops
/// the socket a microsecond later there is no unread data and the close
/// is clean (no RST). On `Poll::Pending` during the post-FIN drain we
/// **return `Poll::Pending`** (yield) rather than letting the drop race
/// the peer. The drain is hard-bounded by BOTH a byte cap
/// (`DRAIN_CAP`) AND a short wall-clock deadline (`LINGER_DEADLINE`):
/// a silent/wedged/flooding client cannot pin the worker — once either
/// bound is hit we return `Ready` and let the drop proceed (DoS
/// mitigation unchanged: the connection still dies, bounded). Because
/// the FIN is sent FIRST, a client that never sends anything more (e.g.
/// the keep-alive-stall probe) observes the close immediately; the
/// post-FIN drain only matters for a client that is still streaming
/// (the rapid-reset flood) — exactly the case that needs it.
///
/// Net effect: the rapid-reset client receives `…GOAWAY…FIN` in order
/// on a socket that is then cleanly closed (no RST), decodes the
/// GOAWAY, and its h2 conn future resolves `Err(GoAway(_, _, Remote))`
/// (`is_go_away()`/`is_remote()`), the CVE-2023-44487 / RFC 9113 §6.8
/// signalling contract. No protocol behaviour change for conformant
/// peers; no teardown-latency regression for non-flooding closes.
struct CleanCloseIo<IO> {
    inner: IO,
    /// Remaining inbound bytes we are willing to drain after the FIN
    /// before letting the drop proceed regardless (hard bound — a
    /// flooding client cannot delay teardown indefinitely).
    drain_budget: usize,
    /// Set once the inner FIN/`close_notify` has been delegated so we
    /// do not re-issue it on subsequent polls.
    fin_done: bool,
    /// Set once the post-FIN drain has finished (EOF, byte cap, read
    /// error, or deadline) so `poll_shutdown` returns `Ready`.
    drained: bool,
    /// Lazily-armed wall-clock deadline for the post-FIN drain. `None`
    /// until the FIN is sent; `Some` thereafter. Bounds the time we
    /// wait for the peer's reciprocal FIN so a silent/wedged client
    /// cannot pin the worker.
    linger_deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<IO> CleanCloseIo<IO> {
    /// 256 KiB: comfortably larger than any in-flight RST_STREAM /
    /// HEADERS burst a client can have queued between the server
    /// emitting GOAWAY and its own reciprocal FIN, yet a hard cap so a
    /// deliberate post-GOAWAY flood cannot pin the worker. Drain is
    /// read-and-discard only (fixed scratch buffer, no allocation
    /// growth).
    const DRAIN_CAP: usize = 256 * 1024;

    /// Maximum wall-clock time the post-FIN drain will wait for the
    /// peer's reciprocal FIN. A conformant client closes within an RTT
    /// of decoding the GOAWAY+FIN — far inside this budget. Kept short
    /// (1 s) so it never approaches the surrounding
    /// `HttpTimeouts::total` (60 s) and a wedged client is reaped
    /// promptly. Only reached at all when the peer is still streaming
    /// after our FIN (the rapid-reset flood); a non-flooding close
    /// hits EOF first and returns immediately.
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
        // F-SEC-1: FIN-first, then bounded post-FIN drain — see the
        // `CleanCloseIo` doc for the full mechanism + source proof.
        //
        // Step 1: send the FIN / TLS close_notify PROMPTLY. The GOAWAY
        // was already flushed by h2's `codec.shutdown` flush BEFORE this
        // poll_shutdown (verified in h2-0.4.13 source). Sending the FIN
        // does NOT cause an RST; only DROPPING the socket with unread
        // inbound data does. Doing this first means a non-flooding
        // client (e.g. a keep-alive-stall close) sees the connection
        // close immediately — zero added teardown latency.
        if !self.fin_done {
            ready!(Pin::new(&mut self.inner).poll_shutdown(cx))?;
            self.fin_done = true;
            // Arm the bounded post-FIN drain deadline.
            self.linger_deadline = Some(Box::pin(tokio::time::sleep(Self::LINGER_DEADLINE)));
        }

        // Step 2: bounded post-FIN drain. h2 drops this io a microsecond
        // after we return Ready; if the peer is still streaming (the
        // rapid-reset flood) that drop would RST and discard the
        // client's already-received GOAWAY. So read+discard inbound
        // until the peer closes its own write half (EOF) — its normal
        // reaction to the GOAWAY+FIN — before we let the drop proceed.
        // Hard-bounded by a byte cap AND a short wall-clock deadline so
        // a silent/wedged/flooding client cannot pin the worker.
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
                            // EOF — peer closed its write half. No unread
                            // data remains; the imminent drop is a clean
                            // close (no RST). Done.
                            break;
                        }
                        self.drain_budget -= n;
                    }
                    Poll::Ready(Err(_)) => {
                        // Peer RST / gone — nothing more to drain.
                        break;
                    }
                    Poll::Pending => {
                        // No inbound right now and the peer has not yet
                        // sent its reciprocal FIN. Do NOT let the drop
                        // proceed yet (it would race an RST). Yield and
                        // wait for the peer FIN (poll_read registered our
                        // waker) or the bounded deadline. `linger_deadline`
                        // is always `Some` here (armed together with
                        // `fin_done`); if it were ever absent we still
                        // must not resolve early, so yield.
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
    ) -> Response<ClientRespBody> {
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
            // S12 widening: the gRPC proxy returns a `hyper::Error`-bodied
            // response; lossless-box it into the widened `ClientRespBody`.
            let (gp_parts, gp_body) = Arc::clone(gp).handle(req, backend.addr).await.into_parts();
            return Response::from_parts(
                gp_parts,
                gp_body
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    .boxed(),
            );
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
        let (mut parts, mut body) = req.into_parts();

        // F-MD-1 (S8 remediation) — the inbound request `parts` were minted
        // from an HTTP/2 stream, so `parts.version == HTTP/2.0` and the header
        // map may carry the inbound framing headers (`content-length`,
        // `transfer-encoding`). This request is about to be handed to the
        // in-crate hyper HTTP/1.1 client. hyper's http1 encoder, when it sees
        // an HTTP/2-versioned request OR a stale `content-length` alongside an
        // unknown-length streaming body, MIS-FRAMES the body: it sends an
        // empty/zero-length body and never polls our `StreamBody`, so the
        // backend observes an immediate EOF (0 bytes forwarded). We MUST let
        // hyper choose the http1 framing itself.
        //
        //  • Force the request version to HTTP/1.1 (the upstream protocol).
        //  • Strip `content-length` and `transfer-encoding` so hyper sets the
        //    framing for the body we actually hand it (chunked for the
        //    streaming Branch B; content-length for the Full body in Branch A).
        //
        // (Branch A's `Full` body happened to work even at HTTP/2.0 because it
        // has an exact size hint and hyper emitted content-length anyway; the
        // streaming Branch B body has no exact size, which is where the
        // mis-framing struck. Normalising here fixes Branch B and keeps Branch
        // A correct and explicit.)
        parts.version = hyper::Version::HTTP_11;
        parts.headers.remove(hyper::header::CONTENT_LENGTH);
        parts.headers.remove(hyper::header::TRANSFER_ENCODING);

        // S8 / M-D — bounded H2 INGRESS pump (lookahead-window), replaces
        // the R8-violating `Limited::collect()` (whole-body buffer).
        //
        // The fixed in-flight window (`H2_REQ_CHANNEL_DEPTH × H2_REQ_CHUNK_MAX`
        // = 64 KiB, body-size-INDEPENDENT) DOUBLES as a validate-before-forward
        // lookahead. We poll the inbound `IncomingBody` frame-by-frame into a
        // bounded lookahead buffer:
        //
        //  • Whole request ≤ window (the common case; ALL malformed-probe
        //    gate tests use 2-byte bodies): the buffer reaches inbound EOF
        //    (incl. trailers) BEFORE the window fills. Polling to EOF drives
        //    the IDENTICAL hyper/h2 validation `collect()` did — collect is
        //    just poll-to-EOF — so content-length≠ΣDATA surfaces as the
        //    terminal `Err` and a trailer pseudo-header is checked on the
        //    trailers frame. Validation completes BEFORE the dial → ZERO
        //    backend dial for a malformed request (the F-COR-1 A2-2 ordering
        //    fix is preserved structurally; the two zero-dial gate tests pass
        //    UNCHANGED).
        //  • Request > window: when the buffer hits the high-watermark before
        //    EOF, we dial, forward headers, drain the buffer and enter
        //    streaming mode (forward-as-it-arrives, memory pinned at the
        //    window). The downstream RESPONSE head is gated on the inbound
        //    body reaching a validated terminal state (clean EOF, or a
        //    surfaced protocol `Err` mapped to RST/GOAWAY), so even a >window
        //    request that turns malformed at the trailers NEVER relays the
        //    backend response body downstream (the h2spec invariant), without
        //    buffering the whole body.
        use hyper::body::Body as _;
        use hyper::body::Frame;

        let mut lookahead: Vec<Bytes> = Vec::new();
        let mut buffered: usize = 0;
        let mut trailers_map: Option<hyper::HeaderMap> = None;
        // True once the inbound body has yielded its terminal frame within
        // the window (clean EOF). When this stays false we exited the
        // lookahead because the window filled → streaming regime.
        let mut reached_eof = false;

        // ── Phase 1: lookahead. Poll frames until EOF or the window fills. ──
        loop {
            // Record the max instantaneous retained inbound memory (Q-D3
            // gauge). In the lookahead phase the retained set IS the buffer.
            #[cfg(any(test, feature = "test-gauges"))]
            record_retained(buffered);

            // `> window` (strictly) is the streaming trigger: a request whose
            // bytes-so-far already exceed the in-flight window cannot be held
            // for validate-before-dial without violating R8.
            if buffered > H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX {
                break;
            }

            match body.frame().await {
                None => {
                    // F-MD-4 (S8 PROTO smuggling fix) — `None` is ambiguous:
                    // an inbound RST_STREAM(CANCEL/NO_ERROR) is mapped to
                    // `None` by hyper, indistinguishable from clean END_STREAM.
                    // Within the window this would otherwise fall to Branch A
                    // and relay a fully-buffered (truncated) body as a COMPLETE
                    // request — the within-window smuggling variant. Only a
                    // POSITIVELY-confirmed END_STREAM is a clean terminal state.
                    // A reset is rejected here, BEFORE any dial (zero-dial).
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
                        // Cap accounting at the named total-body cap exactly
                        // as `Limited` did (413 on exceed) — independent of
                        // the in-flight window axis.
                        buffered = buffered.saturating_add(data.len());
                        if buffered > MAX_REQUEST_BODY_BYTES {
                            return Err(ProxyErr::BodyTooLarge);
                        }
                    }
                    if frame.is_data() {
                        // SAFETY: guarded by `is_data()`.
                        lookahead.push(frame.into_data().unwrap_or_default());
                    } else if frame.is_trailers() {
                        // The trailers frame is the terminal frame; capture it
                        // and treat the body as ended (clean EOF).
                        trailers_map = frame.into_trailers().ok();
                        reached_eof = true;
                        break;
                    }
                }
                Some(Err(e)) => {
                    // hyper/h2 surfaced a protocol/IO error while VALIDATING
                    // the inbound stream (content-length≠ΣDATA, stream-state,
                    // flow-control, …). In the lookahead phase this is BEFORE
                    // any dial → the malformed request can never leak the
                    // backend response (zero-dial; gate tests pass unchanged).
                    return Err(ProxyErr::BadRequest(format!(
                        "malformed H2 request body: {e}"
                    )));
                }
            }
        }

        if reached_eof {
            // ── Branch A: the whole request fit within the window. ──
            // Identical posture to the old buffered path: validate trailers,
            // dial, send the buffered body. Zero backend dial for malformed
            // requests is preserved because any inbound `Err` returned above
            // BEFORE this point.
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
            // S14 / R-CFBW-3: Branch A buffered body cannot be a slow-
            // progressing upload (within the lookahead window). Bound the
            // head-roundtrip with `head_timeout` for consistency with the
            // Class A streaming sites' Phase-B cap; this is a
            // rename-only / semantic-only change, NOT a load-bearing
            // idle-watchdog site.
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
        // F-MD-4: the Branch-B upstream request body is a `StreamBody` whose
        // error type is the constructible `PumpAbort` (not `hyper::Error`,
        // which has no public constructor) so the pump can INJECT a body
        // error on the inbound-abort path instead of silently dropping the
        // channel (a drop = clean EOF = smuggled-complete request).
        let (mut sender, conn) = hyper::client::conn::http1::handshake::<
            _,
            BoxBody<Bytes, PumpAbort>,
        >(TokioIo::new(stream))
        .await
        .map_err(|e| ProxyErr::Upstream(format!("h1 client handshake: {e}")))?;
        let conn_handle = tokio::spawn(async move {
            let _ = conn.await;
        });

        // Bounded in-flight channel (depth = H2_REQ_CHANNEL_DEPTH). When the
        // backend write stalls, hyper stops pulling → the channel fills → the
        // pump stops polling the inbound body → hyper/h2 withholds
        // WINDOW_UPDATE → the H2 client is paused (the R8 backpressure chain).
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, PumpAbort>>(H2_REQ_CHANNEL_DEPTH);

        // F-MD-3 (S8 remediation) — a GENUINE retained-memory gauge. The two
        // streaming-phase record sites previously stored a CONSTANT (the
        // 64 KiB window ceiling), so a whole-body-buffering regression would
        // not move the gauge. Instead we track the ACTUAL instantaneous
        // in-flight channel occupancy: `in_flight_bytes` is incremented by the
        // pump just before it pushes a chunk into the channel and DECREMENTED
        // in the body's poll the moment hyper pulls that chunk back out. The
        // pump then records `lookahead_remaining + live_in_flight` at each push
        // — the real retained inbound set. A buffering regression that held the
        // whole body in `in_flight_bytes` (or a lookahead that never drained)
        // would push the gauge above the window ceiling and trip the bound.
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight_body = std::sync::Arc::clone(&in_flight_bytes);

        // Bridge the mpsc receiver into an `http_body` stream body without a
        // new dep (futures-util is already a dependency). As hyper pulls each
        // frame we decrement the live in-flight counter (the chunk has left
        // our retained set and is now owned by hyper's write buffer).
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

        // The pump owns the inbound body + the already-buffered lookahead
        // chunks. It reports its terminal verdict via a oneshot so the
        // response-head relay can be gated on a VALIDATED terminal state.
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), ProxyErr>>();
        let drained: Vec<Bytes> = std::mem::take(&mut lookahead);

        // S14 / CF-BODY-WALLCLOCK — forward-progress signal for
        // [`lb_io::idle_send::idle_bounded_send`]. Mirror of H1→H1
        // (h1_proxy.rs::proxy_request). See `audit/h-matrix/s14-builder-1-design.md` §2.2.
        let last_progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let upload_complete = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let epoch = tokio::time::Instant::now();
        let last_progress_pump = std::sync::Arc::clone(&last_progress);
        let upload_complete_pump = std::sync::Arc::clone(&upload_complete);
        let epoch_pump = epoch;

        let pump = tokio::spawn(async move {
            // S14 — `bump` on every `tx.send(Ok)` success (co-located with
            // `in_flight_bytes` fetch_add). `set_complete` once at the
            // verdict-Ok terminal arm (clean END_STREAM / trailer-Ok).
            let bump = || {
                let dt = tokio::time::Instant::now().saturating_duration_since(epoch_pump);
                let ms = u64::try_from(dt.as_millis()).unwrap_or(u64::MAX);
                last_progress_pump.store(ms, std::sync::atomic::Ordering::Relaxed);
            };
            let set_complete = || {
                upload_complete_pump.store(true, std::sync::atomic::Ordering::Release);
            };

            // Running cumulative total of forwarded request bytes — the D1
            // total-body cap (`MAX_REQUEST_BODY_BYTES`, 64 MiB) still applies
            // in the streaming regime, exactly as it did under the buffered
            // path. Starts at the bytes already in the lookahead buffer.
            let mut forwarded_total: usize = buffered;
            // Bytes still sitting in the lookahead `drained` queue, not yet
            // pushed into the channel. Part of the live retained set (F-MD-3).
            let mut lookahead_remaining: usize = buffered;

            // Outcome of the forwarding phase. `Forwarded` = the channel
            // accepted the whole body (clean EOF) → verdict Ok. `ReceiverGone`
            // = hyper dropped the request body (the backend short-circuited its
            // response WITHOUT reading the body — an early/short response). On
            // `ReceiverGone` we MUST NOT manufacture a 413; we switch to
            // drain-and-validate (F-MD-2) so the backend's real response is
            // relayed once the inbound body validates.
            enum SendOutcome {
                ReceiverGone,
            }

            // Helper: split a DATA payload into ≤ H2_REQ_CHUNK_MAX pieces and
            // push each through the bounded channel (the backpressure point).
            // Increments the live in-flight gauge before each push and records
            // the real retained set. Returns Err(ReceiverGone) if the receiver
            // (hyper body) dropped (backend stopped reading).
            macro_rules! send_chunked {
                ($bytes:expr, $is_lookahead:expr) => {{
                    let mut data: Bytes = $bytes;
                    let mut outcome: Result<(), SendOutcome> = Ok(());
                    while !data.is_empty() {
                        let take = data.len().min(H2_REQ_CHUNK_MAX);
                        let chunk = data.split_to(take);
                        let clen = chunk.len();
                        // This chunk is about to enter the channel: it joins
                        // the live in-flight set and leaves the lookahead set
                        // (if it came from there).
                        in_flight_bytes.fetch_add(clen, std::sync::atomic::Ordering::Relaxed);
                        if $is_lookahead {
                            lookahead_remaining = lookahead_remaining.saturating_sub(clen);
                        }
                        // F-MD-3: record the ACTUAL retained inbound set =
                        // lookahead still queued + bytes live in the channel.
                        #[cfg(any(test, feature = "test-gauges"))]
                        record_retained(
                            lookahead_remaining
                                + in_flight_bytes.load(std::sync::atomic::Ordering::Relaxed),
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
            // state so a malformed request never relays the backend response.
            // Bytes are DISCARDED (memory stays bounded — we hold at most one
            // frame at a time), but the 64 MiB cap and protocol validation
            // still apply. Returns the terminal verdict.
            macro_rules! drain_and_validate {
                () => {{
                    loop {
                        match body.frame().await {
                            None => {
                                // `None` is ambiguous (reset vs END_STREAM); see
                                // the streaming-loop comment. Only a positively
                                // confirmed END_STREAM is a clean terminal state
                                // that may relay the backend's (early) response.
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

            // 1) Drain the lookahead buffer first (oldest chunks first),
            // re-chunked to the window granularity.
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
                        // F-MD-4 (S8 PROTO smuggling fix) — `frame()==None` is
                        // AMBIGUOUS: hyper's `Incoming::poll_frame` maps an
                        // inbound H2 RST_STREAM with reason CANCEL or NO_ERROR
                        // to `Poll::Ready(None)` — INDISTINGUISHABLE from a
                        // clean END_STREAM (hyper-1.9.0 body/incoming.rs ~L250).
                        // A client that streams a body then drops it without
                        // END_STREAM (RST_STREAM/CANCEL) therefore surfaces as
                        // `None` here, NOT as `Some(Err)`. Inferring clean EOF
                        // from `None` alone would drop `tx` cleanly → the
                        // StreamBody yields `None` → hyper writes the chunked
                        // terminator `0\r\n\r\n` → the truncated request is
                        // relayed to the upstream as COMPLETE (request
                        // smuggling). We MUST positively confirm END_STREAM.
                        //
                        // `Body::is_end_stream()` for the H2 kind delegates to
                        // `h2::RecvStream::is_end_stream()`, true IFF the stream
                        // reached `Closed(Cause::EndStream)`/`HalfClosedRemote`
                        // (a real END_STREAM flag) and FALSE after any reset
                        // (`Closed(Cause::Error(Reset))` /
                        // `ScheduledLibraryReset`) — h2-0.4.13
                        // proto/streams/state.rs `is_recv_end_stream`. This is
                        // deterministic under arbitrary scheduling: it reflects
                        // the protocol terminal STATE, not a timing race.
                        if body.is_end_stream() {
                            // Positively-confirmed clean END_STREAM → drop `tx`
                            // → StreamBody yields `None` → hyper writes the
                            // terminator → upstream sees a COMPLETE request.
                            // S14 — upload complete; helper switches to
                            // Phase-B head-roundtrip cap.
                            set_complete();
                            let _ = verdict_tx.send(Ok(()));
                        } else {
                            // `None` from a RST_STREAM (no END_STREAM): inject a
                            // BODY ERROR so hyper aborts the upstream request
                            // WITHOUT a terminator; the caller aborts the conn
                            // and never relays its response. The truncated
                            // request is NEVER seen as complete upstream.
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
                            // Validate trailers BEFORE forwarding them; a
                            // pseudo-header in trailers is malformed.
                            match validate_request_trailers(frame.trailers_ref()) {
                                Ok(_) => {
                                    let _ = tx.send(Ok(frame)).await;
                                    // S14 — trailers accepted; bump then
                                    // mark upload complete (Phase-B).
                                    bump();
                                    set_complete();
                                    let _ = verdict_tx.send(Ok(()));
                                    return;
                                }
                                Err(e) => {
                                    // F-MD-4: inject a BODY ERROR so hyper
                                    // aborts the upstream request WITHOUT a
                                    // clean terminator (dropping tx alone =
                                    // clean EOF = smuggled-complete request).
                                    let _ = tx.send(Err(PumpAbort)).await;
                                    let _ = verdict_tx.send(Err(e));
                                    return;
                                }
                            }
                        }
                        if let Ok(data) = frame.into_data() {
                            forwarded_total = forwarded_total.saturating_add(data.len());
                            if forwarded_total > MAX_REQUEST_BODY_BYTES {
                                // D1 total-body cap exceeded mid-stream. Report
                                // 413 and inject a BODY ERROR (F-MD-4) → the
                                // upstream body terminates abruptly WITHOUT a
                                // clean terminator; the caller aborts the
                                // connection and never relays its response. The
                                // client sees a stream reset (no 200 leak), and
                                // the upstream never sees a complete request.
                                let _ = tx.send(Err(PumpAbort)).await;
                                let _ = verdict_tx.send(Err(ProxyErr::BodyTooLarge));
                                return;
                            }
                            if let Err(SendOutcome::ReceiverGone) = send_chunked!(data, false) {
                                // Backend stopped reading mid-stream (early/
                                // short response) — F-MD-2 drain-and-validate,
                                // NOT a 413.
                                let _ = verdict_tx.send(drain_and_validate!());
                                return;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        // Inbound protocol error AFTER the dial (streaming
                        // regime) — e.g. the client RST_STREAMs mid-body
                        // (smuggling, F-MD-4). Inject a BODY ERROR into the
                        // channel so hyper sees the upstream request body
                        // terminate ABRUPTLY (no clean `0\r\n\r\n` terminator)
                        // and aborts the upstream request: the backend never
                        // observes a COMPLETE (truncated) request. Dropping the
                        // sender alone would be a clean EOF → smuggled complete.
                        // The caller also aborts the connection (defense in
                        // depth) and never relays the response.
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
        // the channel for the pump to make progress under backpressure), but
        // do NOT relay the response until the pump's terminal verdict lands.
        //
        // S14 / CF-BODY-WALLCLOCK — see the H1→H1 mirror at
        // `h1_proxy.rs::proxy_request` for the two-phase rationale. The
        // helper preserves the F-CAP-1 inner `Ok(Err(hyper::Error))` arm
        // verbatim and the :1881 verdict-rx backstop is untouched.
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
                // body reset → BadRequest) it injects the body Err and then
                // sends its verdict immediately AFTER it (FIFO), so the
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
                tracing::warn!(error = %idle_err, "h2→h1 idle/head deadline fired");
                pump.abort();
                conn_handle.abort();
                return Err(ProxyErr::Timeout);
            }
        };

        // Validate-before-RESPONSE-relay gate: the response head only relays
        // once the inbound body has reached a validated terminal state.
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
                // Pump task vanished without a verdict (panic/abort) — treat
                // as an inbound failure; never leak the backend response.
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
        // S12 widening: lossless-box the upstream `Incoming` body's `hyper::Error`.
        Response::from_parts(
            parts,
            body.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                .boxed(),
        )
    }

    /// Forward an H2 inbound request to an H2 backend (PROTO-001).
    ///
    /// S10 / H2→H2 R8: this is now a bounded-incremental STREAMING relay
    /// on both legs (no `collect()` on either side). The request leg
    /// MIRRORS the M-D pump in [`Self::proxy_request`] (the BUILT/promoted
    /// H2→H1 cell, which is NOT edited) — lookahead validate-before-dial,
    /// Branch A (≤window buffered) / Branch B (streaming with a bounded
    /// in-flight window), F-MD-4 reset-mid-body smuggling guard, and the
    /// F-CAP-1 caller arm that prefers a classified 413/400 over a generic
    /// 502. The ONLY deltas vs `proxy_request` are: the request stays
    /// HTTP/2-shaped (no force-HTTP/1.1, no CL/TE strip — H2 upstream
    /// framing is hyper's H2 encoder's job); the egress is the
    /// `Http2Pool::send_request` multiplexed pool (no per-request
    /// conn_handle to spawn/abort); and the Branch-B body is `H2ReqBody`
    /// (channel error `PumpAbort` mapped to `Box<dyn Error+Send+Sync>`).
    /// The response leg streams the upstream `Incoming` by construction.
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
            // F-COR-1 / F-MD-4: inbound H2 request failed protocol
            // validation while being received (malformed trailers,
            // content-length≠ΣDATA, reset mid-body) → 400 (NOT 502).
            Err(ProxyErr::BadRequest(s)) => error_response(StatusCode::BAD_REQUEST, &s),
        }
    }

    /// S10 / H2→H2 Leg 1 — the bounded-incremental streaming REQUEST
    /// pump. A MIRROR of [`Self::proxy_request`]'s M-D orchestration with
    /// the H2-upstream deltas documented on [`Self::proxy_h2_to_h2`]. The
    /// leaf helpers (`PumpAbort`, `validate_request_trailers`,
    /// `concat_chunks`, `build_h2_body_with_trailers`, `record_retained`)
    /// and the window consts are REUSED, not duplicated; only the
    /// orchestration is mirrored (Q-HH-1 = mirror, defer extraction).
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

        // ── Preamble (DELTA vs proxy_request): keep the request HTTP/2-
        // shaped for the H2 upstream. Run the H2→H2 header normalization
        // (lowercase regular headers, keep pseudo-headers, MAX_HEADERS
        // check) exactly as the old buffering `translate_h2_request_to_h2`
        // did — but WITHOUT collecting the body. We do NOT force HTTP/1.1
        // and do NOT strip content-length/transfer-encoding the way the
        // H1-egress pump does (those were H1-framing fixes; H2 upstream
        // framing is hyper's H2 encoder's job). Method/uri/authority/
        // scheme are preserved for the upstream H2 request.
        let upstream_parts = match build_h2_upstream_request_parts(&parts) {
            Ok(p) => p,
            Err(e) => return Err(ProxyErr::Upstream(e)),
        };

        // S10 / M-D mirror — bounded H2 INGRESS pump (lookahead-window),
        // replacing the R8-violating `body.collect().await`. See
        // `proxy_request` for the full design rationale; the lookahead
        // posture (validate-before-dial within the window, stream past
        // it) is IDENTICAL.
        let mut lookahead: Vec<Bytes> = Vec::new();
        let mut buffered: usize = 0;
        let mut trailers_map: Option<hyper::HeaderMap> = None;
        let mut reached_eof = false;

        // ── Phase 1: lookahead. Poll frames until EOF or the window fills.
        loop {
            #[cfg(any(test, feature = "test-gauges"))]
            record_retained(buffered);

            if buffered > H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX {
                break;
            }

            match body.frame().await {
                None => {
                    // F-MD-4: `None` is ambiguous (reset vs END_STREAM).
                    // Only a positively-confirmed END_STREAM is a clean
                    // terminal state. A reset is rejected here, BEFORE any
                    // pool contact (zero-dial for a within-window reset).
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
                    // Protocol/IO error surfaced while VALIDATING the
                    // inbound stream, BEFORE any pool contact (zero-dial).
                    return Err(ProxyErr::BadRequest(format!(
                        "malformed H2 request body: {e}"
                    )));
                }
            }
        }

        if reached_eof {
            // ── Branch A: the whole request fit within the window. ──
            // Validate trailers, then build the buffered H2 body and send.
            // ZERO pool contact for a malformed within-window request:
            // any inbound Err/reset returned ABOVE this point (F-COR-1).
            let trailers_vec = validate_request_trailers(trailers_map.as_ref())?;

            let body_bytes = concat_chunks(&lookahead, buffered);
            // DELTA: Branch A body must be `H2ReqBody`
            // (BoxBody<Bytes, Box<dyn Error+Send+Sync>>). The shared helper
            // yields BoxBody<_, hyper::Error>; widen the error losslessly.
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

        // ── Branch B: request > window → stream with the bounded in-flight
        // window; gate the response head on the inbound terminal state.
        // DELTA: no dial/handshake/conn_handle here — the Http2Pool owns
        // the connection + driver; `send_request` dials/multiplexes.
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, PumpAbort>>(H2_REQ_CHANNEL_DEPTH);

        // F-MD-3: genuine retained-memory gauge (live in-flight occupancy).
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight_body = std::sync::Arc::clone(&in_flight_bytes);

        // Bridge the mpsc receiver into an `http_body` StreamBody, mapping
        // the channel error `PumpAbort` → `Box<dyn Error+Send+Sync>` so the
        // body is `H2ReqBody` (DELTA: H2 pool body alias, not the H1
        // `BoxBody<_, PumpAbort>`). PumpAbort already impls Error.
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
        let (verdict_tx, verdict_rx) = tokio::sync::oneshot::channel::<Result<(), ProxyErr>>();
        let drained: Vec<Bytes> = std::mem::take(&mut lookahead);

        // S14 / CF-BODY-WALLCLOCK — forward-progress signal for
        // [`lb_io::http2_pool::Http2Pool::send_request_idle`] (threaded via
        // [`drive_h2_upstream_send`]). See `audit/h-matrix/s14-builder-1-design.md` §2.4.
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

            // F-MD-4 (S10 DEFECT FIX, body-layer half) — on every abort
            // terminal state (client RST mid-body, forbidden trailer,
            // over-cap), inject `Err(PumpAbort)` into the upstream
            // request-body channel and HOLD the sender open until hyper has
            // OBSERVED it (`tx.closed().await`, which resolves once hyper
            // drops the receiver). Because mpsc delivery is FIFO (a buffered
            // item is always returned before the closed `None`), holding the
            // sender forces hyper to poll `Ready(Some(Err(PumpAbort)))`
            // BEFORE it can ever see a channel-close `None` — so hyper
            // RESETS the upstream stream rather than taking the clean-EOF
            // (`Ready(None)`) branch and emitting a spurious END_STREAM.
            //
            // This is necessary but is only HALF the fix: it guarantees
            // hyper does not infer a clean EOF *from the channel*. The other
            // half lives in the caller (`proxy_h2_to_h2_request`'s detached
            // send task + `reset_peer`): a downstream client RST cancels
            // this gateway's service future, which would otherwise DROP the
            // in-flight upstream request body at a clean frame boundary and
            // make hyper finalize END_STREAM on the graceful drop — racing
            // ahead of this injection. Owning the send in a detached task
            // keeps the body alive across the downstream cancel, and the
            // verdict-driven `reset_peer` (connection teardown, the
            // multiplexed-pool analog of the H1 pump's `conn_handle.abort()`
            // backstop) deterministically resets the upstream stream. The
            // `tx.closed()` wait is bounded by `H2_ABORT_OBSERVE_TIMEOUT` so
            // a wedged upstream driver cannot hang the detached task.
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
                        // F-MD-4: positively confirm END_STREAM; a `None`
                        // from a RST_STREAM must NOT be relayed as a clean
                        // EOF (request smuggling).
                        if body.is_end_stream() {
                            // S14 — upload complete; helper switches to
                            // Phase-B head-roundtrip cap.
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
                                    // S14 — trailers accepted; bump then
                                    // mark upload complete (Phase-B).
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

        // F-MD-4 (S10 DEFECT FIX) — route the graceful-drop egress through
        // the shared driver (CF-DEDUP-1 / S11 D1). NET BEHAVIOUR UNCHANGED:
        // `drive_h2_upstream_send` owns the detached send task (biased
        // verdict-vs-head race + `reset_peer` on every abort + F-CAP-1
        // caller arm + `pump.abort()`) and the final `head_rx.await`, byte-
        // for-byte identical to the prior inlined block.
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

    /// Forward an H2 inbound request to a STREAMING H3 backend (PROTO-001 /
    /// S13 H2→H3, R8). EXACT mirror of [`crate::h1_proxy::H1Proxy::proxy_h1_to_h3`]:
    /// the buffering `collect_h2_request_to_h3_fieldlist` → `request_h3_upstream`
    /// → `h3_response_to_h2` triple (whole-body `Bytes` + buffering builder, the
    /// R8 violation) is replaced by a both-legs streaming relay on the SAME shared
    /// connector ([`lb_quic::stream_request_to_h3_upstream`] + `H3RespOut::Decoded`)
    /// that H1→H3 and H3→H3 use VERBATIM.
    ///
    /// Two H2-specific deltas vs the H1→H3 mirror:
    ///  - the request-leg pump sources frames the H2 way (`body.frame()` +
    ///    `is_end_stream()` disambiguation — an H2 `None` is ambiguous between a
    ///    clean END_STREAM and a RST, unlike H1's positively-clean `None`); and
    ///  - the response-head transform is H2→H2 semantics (drop `:`-pseudo +
    ///    lowercase, NO `RESPONSE_HOP_BY_HOP` strip — [`h2_decoded_resp_head_builder`]),
    ///    not the H1 pseudo+hop-by-hop transform.
    ///
    /// HAZARD (a) — request cancel-race (brief §3 / s13 plan §2(a)): the connector
    /// treats a `body_tx` dropped WITHOUT a final `End`/`Reset` BEFORE any event as
    /// a bodyless-COMPLETE request (`h3_bridge.rs:3309-3313`), so a downstream H2
    /// `RST_STREAM` that cancels this *service* future must NOT be allowed to drop
    /// the pump silently (it would smuggle a truncated request as complete). The
    /// mitigation is LOAD-BEARING: the ingress pump is DETACHED (`tokio::spawn`),
    /// so a service-future cancel only drops the caller's `resp_rx` receive — the
    /// pump that owns the inbound body survives — and the pump ALWAYS emits an
    /// explicit terminal `End{trailers}` or `Reset` (never a silent drop), mapping
    /// the H2 ingress terminal states exactly as `proxy_h2_to_h2_request` does.
    ///
    /// F-CAP-1 status surfacing (mirror of H1→H3): a PRE-DATA over-cap (the pump's
    /// first event is `Reset`, before any `Chunk`) → connector inline-413; a
    /// pre-dial failure → connector inline-502 (both as a synthesized `Head` we
    /// relay). A MID-BODY over-cap / truncation → `H3RespEvent::Reset` (NOT a 413,
    /// response-splitting guard) → we inject `Err` into the H2 response body so
    /// hyper's H2 server RST_STREAMs the client, never a clean END_STREAM.
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

        // Build the request field-list via the H2→H3 bridge (head-only — the
        // body + trailers now STREAM, so no `body.collect()` here).
        let inner = req.into_inner();
        let (parts, mut body) = inner.into_parts();
        let headers = match build_h2_to_h3_fieldlist(&parts, &sni) {
            Ok(h) => h,
            Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
        };

        // Bounded request-body channel into the connector (depth
        // H3_BODY_CHANNEL_DEPTH). Backpressure: a slow QUIC upstream → the
        // connector stops draining → this channel fills → the pump stops
        // polling the inbound H2 body → the client's H2 flow-control window
        // stalls.
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
        // bounds total in-flight independent of body size). Reuses the H2 ingress
        // gauge `H2_REQ_MAX_RETAINED_BODY_BYTES` (same crate, same bounded-window
        // semantics) so the memory proof reads the H2 cell's own counter.
        let in_flight_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // ── Request-leg M-D pump (mirror of `proxy_h1_to_h3`'s pump, sourced
        // the H2 way). DETACHED (Hazard (a)): a downstream H2 RST that cancels
        // the service future must NOT drop this task before it emits an explicit
        // terminal event. ──
        let pump_in_flight = std::sync::Arc::clone(&in_flight_bytes);
        let pump = tokio::spawn(async move {
            // Running cumulative forwarded request bytes — the request-body cap
            // (`MAX_REQUEST_BODY_BYTES`) is OUR job (the connector caps the
            // RESPONSE, not the request). 413-vs-RESET boundary: over-cap BEFORE
            // any chunk forwarded → `Reset` as the FIRST event → connector
            // inline-413; over-cap AFTER ≥1 chunk → `Reset` → RESET-without-FIN.
            let mut forwarded_total: usize = 0;

            // Split a DATA payload into ≤ H3_BODY_CHUNK_MAX pieces and push each
            // as one `ReqBodyEvent::Chunk` (pump-side split bounds the in-flight
            // channel-item size to match the memory gauge). Returns Err(()) if the
            // connector dropped the receiver (treat as abort).
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
                        // F-MD-4 (H2 mirror): `None` is AMBIGUOUS (clean
                        // END_STREAM vs RST). Only a positively-confirmed
                        // END_STREAM is a clean terminal state → `End{[]}` →
                        // connector FIN. A `None` from a RST_STREAM (e.g. a
                        // downstream cancel mid-body) → `Reset` → connector
                        // RESET-without-FIN; the backend NEVER sees a truncated
                        // request as complete (Hazard (a): the explicit terminal
                        // is what defends the dropped-tx == bodyless-COMPLETE
                        // connector contract).
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
                            // Validate request trailers BEFORE forwarding (a
                            // framing/routing field in trailers is a desync
                            // primitive). Forbidden → `Reset` (smuggling guard:
                            // never a clean `End`). OK → `End{trailers}` →
                            // connector ships a post-DATA HEADERS frame then FIN.
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
                            // Over-cap → `Reset`. BEFORE any chunk
                            // (forwarded_total==0) → connector inline-413;
                            // otherwise → RESET-without-FIN. Either way no
                            // over-cap byte is forwarded.
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
                        // F-MD-4 (H2): a protocol/IO error mid-body surfaces as
                        // `Some(Err)` — emit `Reset` → connector RESET-without-FIN
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
        // cloned `Arc` of the H3 pool.
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

        // ── Response leg: drain resp_rx into a streaming H2 response ──
        // The FIRST event determines the head. `Head{status,headers}` →
        // build the streaming H2 head + spawn a body relay; `Reset`/channel-
        // closed before any head → 502 (the connector aborted pre-Head).
        let alt_svc = self.alt_svc;
        let first = resp_rx.recv().await;
        match first {
            Some(lb_quic::H3RespEvent::Head { status, headers }) => {
                let st = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
                let builder = h2_decoded_resp_head_builder(st, &headers, alt_svc);

                // Stream the remaining Body/Trailers/End/Reset events into a
                // StreamBody. `Reset` → inject a body error so hyper's H2 server
                // does NOT emit a clean END_STREAM (it RST_STREAMs the client —
                // the response is truncated, never presented as complete: the
                // response-splitting guard). `End` → drop the sender (clean EOF).
                // A `Trailers` event maps to a native H2 `Frame::trailers` that
                // hyper's H2 server encoder flushes WITHOUT a `Trailer:`
                // pre-declaration (the trailer-mandate WIN — gRPC `grpc-status`
                // reaches the H2 client).
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
                            // A second Head is malformed (Head is once-only);
                            // treat as an abort.
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
                    // F-MD-4 (response leg): the channel error is the
                    // constructible `H2RespAbort`. On a connector `Reset` the
                    // relay task SENDS `Err(H2RespAbort)` (NOT a clean drop), so
                    // hyper polls the body, sees an ERROR, and RST_STREAMs the H2
                    // response stream — the client sees an errored/aborted body,
                    // never a smuggled-complete clean END_STREAM (response-
                    // splitting guard). Box the error to satisfy
                    // `Body::Error: Into<Box<dyn Error+Send+Sync>>`.
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
}

// ── PROTO-001 H2-side translation helpers ─────────────────────────────

/// S10 / H2→H2 R8 — build the upstream H2 request HEAD (method + uri +
/// normalized regular headers) for the streaming relay, WITHOUT touching
/// the body.
///
/// Replaces the head-construction half of the old buffering
/// `translate_h2_request_to_h2`; the body is now pumped incrementally by
/// [`H2Proxy::proxy_h2_to_h2_request`] (the R8 streaming path), so this
/// helper takes only the `&Parts`. The header treatment is IDENTICAL to
/// the old path: run the `create_bridge(Http2, Http2)` request bridge
/// (lowercase regular headers, keep pseudo-headers, `MAX_HEADERS` check)
/// over a body-less `BridgeRequest`, synthesise the pseudo-headers a real
/// H2 client would have sent (`:method`/`:path`/`:scheme`/`:authority`),
/// then re-attach the regular (non-`:`-prefixed) headers to the upstream
/// builder while preserving method/scheme://authority/path. Content-length
/// / transfer-encoding are NOT stripped (DELTA vs the H1-egress pump): H2
/// upstream framing is hyper's H2 encoder's job.
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
        // R8: NO body materialised here. The bridge only lowercases
        // regular headers; the body field is irrelevant to the head.
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

/// F-COR-1 (b) — RFC 9113 §8.1 enforcement for the H2 trailer-capture
/// sites (`translate_h2_request_to_h2`, `collect_h2_request_to_h3_
/// fieldlist`). Builds the trailer pairs from the captured trailer map
/// and REJECTS — returning `Err` — if ANY trailer name is a
/// pseudo-header (`:`-prefixed). RFC 9113 §8.1 mandates a malformed
/// request be treated as a stream error (PROTOCOL_ERROR-class), NOT
/// silently stripped, for the trailing field section. The existing
/// `?`/`map_err` plumbing at each call site turns this `Err` into an
/// error response / connection failure — never a forwarded body.
///
/// S13 H2→H3 (R8): the former production caller (the buffering
/// `collect_h2_request_to_h3_fieldlist`) was replaced by the streaming
/// `proxy_h2_to_h3`, whose pump runs the IDENTICAL `:`-prefix rejection via
/// [`validate_request_trailers`]. This helper is now retained only for its
/// NO-REGRESSION unit test below, so it is `#[cfg(test)]`-gated (production
/// trailer rejection is fully covered by `validate_request_trailers`).
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

/// PROTO-2-12 helper for the H2 proxy: identical shape to
/// `h1_proxy::build_body_with_trailers`. Emits the body bytes as a
/// `Frame::data` then a `Frame::trailers` if `trailers` is non-empty.
/// S8 / M-D — F-COR-1 (b) / RFC 9113 §8.1: a pseudo-header field in the
/// trailing field section is malformed. Reject (PROTOCOL_ERROR-class,
/// surfaced as 400) — never forward. This is the H2→H1 trailer-validation
/// site (contrast `h2_to_h2.rs` which filters only on the regular-header
/// path). Returns the validated `(name, value)` pairs to forward upstream.
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

/// S8 / M-D — concatenate the lookahead DATA chunks into a single `Bytes` for
/// the within-window (Branch A) buffered upstream body. `total` is the exact
/// summed length so we allocate once.
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
///
/// S10 / H2→H2 Leg 2 — STREAMING relay (mirror of
/// [`H2Proxy::finalize_response`] + H2→H2 header normalization). Replaces
/// the R8-violating `body.collect().await` (whole upstream response
/// materialised before relay). We take `(parts, body)` from the upstream
/// `Response<Incoming>`, build the downstream response from status +
/// lowercased regular headers (dropping `:`-prefixed) + optional alt-svc,
/// then `body.boxed()` the `Incoming` body for streaming-by-construction.
/// Upstream trailers flow through the boxed Incoming body's terminal
/// frame naturally — no collect needed to capture them. Backpressure:
/// downstream H2 flow control → hyper stops pulling the upstream Incoming
/// → upstream H2 flow control. No owned intermediate body buffer; memory
/// bounded by hyper's window by construction.
fn upstream_h2_response_to_h2(
    resp: Response<IncomingBody>,
    alt_svc: Option<AltSvcConfig>,
) -> Response<ClientRespBody> {
    let (parts, body) = resp.into_parts();
    // H2→H2 response header normalization (mirror of
    // `H2ToH2Bridge::bridge_response`): lowercase regular headers, drop
    // `:`-prefixed pseudo-headers. No hop-by-hop strip beyond that for
    // H2→H2 (the bridge did none either).
    let mut builder = Response::builder().status(parts.status);
    for (n, v) in &parts.headers {
        if n.as_str().starts_with(':') {
            continue;
        }
        // Re-emit the regular header lowercased (HeaderName is already
        // stored lowercase by hyper's H2 codec, but normalize explicitly
        // to match the bridge's `to_lowercase()` semantics).
        builder = builder.header(n.as_str(), v);
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    // R8: stream the upstream `Incoming` body by construction. Trailers
    // ride its terminal frame; no `collect()`, no owned buffer. S12 widening:
    // lossless-box the `hyper::Error`.
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

/// F-MD-4 (S10 DEFECT FIX) — shared graceful-drop egress driver for an H2
/// upstream reached via [`Http2Pool`]. Extracted VERBATIM from
/// `proxy_h2_to_h2_request`'s Branch B (CF-DEDUP-1 / S11 D1) so the H2→H2
/// and H1→H2 streaming paths share ONE copy of the smuggling fix rather
/// than a hand-mirrored duplicate.
///
/// Owns the detached `tokio::spawn` send task (which OWNS the in-flight
/// `send_request` future and therefore the upstream request body), the
/// biased verdict-vs-head race (`reset_peer` on every abort verdict, the
/// F-CAP-1 caller arm classifying BodyTooLarge/BadRequest over 502,
/// `pump.abort()`), and the final `head_rx.await`. Logic is byte-for-byte
/// identical to the prior inlined Branch B block.
// S14 / CF-BODY-WALLCLOCK — signature widens to thread the two-phase
// idle/head deadline through to `Http2Pool::send_request_idle`. The pre-
// existing `body_timeout` parameter is RETAINED but now consumed ONLY by
// the post-error verdict-rx backstop at :3003 (F-CAP-1 wedged-pump
// liveness consultation), NOT the send. New params: `last_progress` /
// `upload_complete` / `epoch` (owned + bumped by the caller's pump);
// `idle` (Phase-A no-progress deadline) + `head_timeout` (Phase-B fixed
// cap). See `audit/h-matrix/s14-builder-1-design.md` §2.4.
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

    // F-MD-4 (S10 DEFECT FIX) — DETACH the upstream send + verdict
    // resolution into a task that OWNS the in-flight `send_request`
    // future (and therefore the upstream request body) and is NOT tied
    // to the downstream H2 server stream future's lifetime.
    //
    // Root cause of the verifier's intermittent smuggle: when the
    // downstream client RST_STREAMs mid-body, hyper's H2 server cancels
    // this service future. If the in-flight `send_request` future (and
    // its request body) were owned DIRECTLY by this future, that cancel
    // would DROP the upstream request body at a clean frame boundary,
    // and hyper's H2 client finalizes the upstream stream with a clean
    // END_STREAM on that graceful drop — relaying the truncated request
    // as COMPLETE, BEFORE any verdict-driven `reset_peer` could run.
    //
    // By moving the send into a detached task, a downstream cancel only
    // drops the caller's `recv` of `head_rx`; the detached task keeps
    // the request body alive and resolves the pump's verdict. On an
    // abort verdict it forcibly `reset_peer`s (connection teardown →
    // upstream stream RESET) BEFORE dropping the body, so the backend
    // can never observe a clean END_STREAM for a truncated request.
    // On a clean verdict it relays the response head back via `head_rx`.
    // This is the multiplexed-pool analog of the H1 pump's
    // `conn_handle.abort()` backstop.
    let (head_tx, head_rx) =
        tokio::sync::oneshot::channel::<Result<Response<IncomingBody>, ProxyErr>>();
    let pool_for_task = pool.clone();
    tokio::spawn(async move {
        // S14 / CF-BODY-WALLCLOCK — replace the fixed-wall-clock
        // `Http2Pool::send_request` (bounded by the pool's `send_timeout`)
        // with the two-phase idle/head deadline `send_request_idle`. The
        // biased select against `verdict_rx` (below) is unchanged — both
        // pool methods return the same `Result<Response<Incoming>,
        // Http2PoolError>` shape. ROUND8-L7-10 eviction policy is
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
        // Race the upstream send against the pump's verdict (resolves
        // exactly once). `resp` is Some only when the response head won
        // the race; every other branch reports its result + returns.
        let resp: Option<Response<IncomingBody>> = tokio::select! {
            // Bias toward the verdict: an abort verdict landing at the
            // same time as the head must win so we RESET rather than
            // relay.
            biased;
            v = &mut verdict_rx => {
                match v {
                    // Abort terminal state reached BEFORE the head:
                    // deterministically reset the upstream stream
                    // (connection teardown) and report the classified
                    // error. The send future (and body) is dropped only
                    // AFTER the reset.
                    Ok(Err(e)) => {
                        pool_for_task.reset_peer(backend_addr);
                        pump.abort();
                        let _ = head_tx.send(Err(e));
                        return;
                    }
                    // Clean terminal state before the head (small/fast
                    // body): await the head then relay.
                    Ok(Ok(())) => {
                        let out = match send_fut.await {
                            Ok(r) => Ok(r),
                            Err(Http2PoolError::Timeout) => Err(ProxyErr::Timeout),
                            Err(e) => Err(ProxyErr::Upstream(format!("h2 upstream: {e}"))),
                        };
                        let _ = head_tx.send(out);
                        return;
                    }
                    // Pump vanished without a verdict (panic/abort):
                    // reset and never leak the backend response.
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
                    // F-CAP-1 (mirror of proxy_request:1822–1849):
                    // consult the verdict FIRST (BOUNDED) and prefer a
                    // classified 413/400 over the generic 502; also reset
                    // the peer so any in-flight stream is torn down.
                    // (`send_request` failing IS the downstream effect of
                    // the pump's abort.)
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

        // The response head won the race (arrived before any verdict).
        // Validate-before-RESPONSE-relay gate (mirror of 1857–1878):
        // relay only once the inbound body reached a validated terminal
        // state; on an abort verdict reset and never relay.
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

    // Await the detached task's verdict-gated result. If the downstream
    // RSTs, this await is cancelled — but the detached task survives and
    // still resets the upstream on an abort verdict (the smuggling fix).
    match head_rx.await {
        Ok(result) => result,
        // The send task dropped `head_tx` without sending (it was
        // aborted, or panicked) — never leak a backend response.
        Err(_) => Err(ProxyErr::BadRequest(
            "inbound H2 upstream send task terminated without a result".to_owned(),
        )),
    }
}

/// S13 H2→H3 (R8) — build the H2→H3 request FIELD-LIST from the request HEAD
/// only (no `body.collect()` — the body + trailers now STREAM through the
/// connector). Head-only refactor of the former buffering
/// `collect_h2_request_to_h3_fieldlist`: KEEP the `create_bridge(Http2, Http3)`
/// call plus the `:method`/`:path`/`:scheme`/`:authority` synthesis byte-for-
/// byte, DROP the `body.collect()` and trailer capture (request trailers now
/// ride `ReqBodyEvent::End{trailers}` through the connector). Direct mirror of
/// `h1_proxy::build_h1_to_h3_fieldlist` but with H2 pseudo-header synthesis.
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
        // Body + trailers STREAM now: the bridge only needs the head to mint
        // the pseudo-header set, so pass an empty body / no trailers here.
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

/// S13 H2→H3 (R8) — build the STREAMING H2 response head from the connector's
/// decoded [`lb_quic::H3RespEvent::Head`]. Uses H2→H2 response semantics
/// (mirror of [`upstream_h2_response_to_h2`]): drop `:`-prefixed pseudo-headers,
/// lowercase regular headers, add optional alt-svc. UNLIKE the H1→H3 head
/// builder (`h3_decoded_resp_head_builder`) there is NO `RESPONSE_HOP_BY_HOP`
/// strip — H2 upstream/downstream framing is hyper's H2 encoder's job, and the
/// H2→H2 bridge stripped nothing beyond pseudo-headers either. (Verified: hyper's
/// H2 server encoder rejects connection-specific headers on the egress, so a
/// stray `connection:`/`keep-alive` decoded from the H3 backend is never written
/// to the H2 client — no targeted strip needed here.)
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

// CF-DEDUP-1 / S11 D1: widened to `pub(crate)` (mechanical, no behaviour
// change) so the shared `pub(crate) fn drive_h2_upstream_send` can name it
// in its signature without tripping `private_interfaces`.
pub(crate) enum ProxyErr {
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

fn error_response(status: StatusCode, msg: &str) -> Response<ClientRespBody> {
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

/// S8 / M-D (Q-D3 — lb-l7 R8 gauge) — the maximum, observed at any instant,
/// of the inbound H2→H1 REQUEST memory the bounded ingress pump retains while
/// a request is in flight: the lookahead/streaming buffer length PLUS the
/// in-flight channel occupancy (≤ `H2_REQ_CHANNEL_DEPTH × H2_REQ_CHUNK_MAX`).
/// A whole-body buffering implementation (the `collect()` this cell replaces)
/// would make this grow with request size; the bounded window keeps it
/// ≤ `H2_REQ_CHANNEL_DEPTH × H2_REQ_CHUNK_MAX = 64 KiB`, independent of total
/// request size and of [`MAX_REQUEST_BODY_BYTES`]. Test-only (off by default
/// so production never compiles the gauge); mirrors lb-quic
/// `h3_bridge::MAX_RETAINED_BODY_BYTES`.
#[cfg(any(test, feature = "test-gauges"))]
pub static H2_REQ_MAX_RETAINED_BODY_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// S8 / M-D (test-gauge) — max-update for [`H2_REQ_MAX_RETAINED_BODY_BYTES`].
/// Identical lock-free CAS-max to lb-quic `h3_bridge::record_retained`: the
/// gauge only ever moves UP, recording the largest instantaneous retained
/// inbound-request memory the pump observes.
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

    /// S8 / M-D (Q-D4) — pin the in-flight window constants and the ceiling
    /// formula. The bounded ingress pump's body-independence proof (the R8
    /// memory bar) rests on these exact values; this guards them against a
    /// silent drift. `MAX_REQUEST_BODY_BYTES` is the total-body cap (a
    /// SEPARATE axis from the in-flight window) and is pinned at its def site
    /// — not duplicated here.
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
        // The window is independent of the total-body cap: the ceiling must
        // be << MAX_REQUEST_BODY_BYTES so retained memory cannot scale to it.
        // `black_box` keeps this a genuine runtime check (not a const that
        // clippy would flag as optimized-out via assertions_on_constants).
        let window = std::hint::black_box(H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX);
        let cap = std::hint::black_box(MAX_REQUEST_BODY_BYTES);
        assert!(window < cap, "window must be far below the total-body cap");
    }

    /// S8 / M-D (Q-D3) — the retained-memory gauge is a real max-update, not a
    /// constant: it only moves up, and a smaller value never lowers it.
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
    /// delegated and whether inbound was fully drained to EOF before
    /// `CleanCloseIo::poll_shutdown` resolved (the no-RST precondition).
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

    /// DETERMINISTIC structural coverage (NOT the F-SEC-1 gate — the
    /// gate is the real wire test
    /// `tests/h2_security_live.rs::rapid_reset_goaway` under load).
    ///
    /// F-SEC-1 v2 invariant: `CleanCloseIo::poll_shutdown` (1) delegates
    /// the inner FIN, and (2) does NOT return `Ready` until all pending
    /// inbound has been drained to EOF — so that when h2 drops the io
    /// immediately afterwards there is no unread data and the close is
    /// clean (no RST that would discard the peer's already-received RFC
    /// 9113 §6.8 GOAWAY). The FIN is sent first (prompt teardown, no
    /// added latency) but the future does not resolve — i.e. the drop
    /// does not happen — until the peer is fully drained.
    #[tokio::test(flavor = "current_thread")]
    async fn clean_close_io_drains_inbound_to_eof_before_resolving() {
        use tokio::io::AsyncWriteExt;
        // 200 KiB pending inbound flood, under the 256 KiB DRAIN_CAP,
        // delivered in 4 KiB reads then EOF.
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

    /// F-SEC-1 v2 CORE PROPERTY (the bug the prior fix missed): the FIN
    /// is sent PROMPTLY (first poll — sending a FIN never causes an RST,
    /// only DROPPING the socket with unread inbound does), but
    /// `poll_shutdown` MUST NOT RESOLVE (`Poll::Ready`) while the peer
    /// has not yet closed its write half (`poll_read` → `Poll::Pending`)
    /// — because h2 drops the io the instant we resolve, and dropping
    /// with the flood still arriving is exactly the RST that discards
    /// the client's already-received GOAWAY (the phase3-final
    /// `BrokenPipe`). It resolves only after the peer reacts to the
    /// GOAWAY+FIN and closes (EOF). Driven with a manual waker so the
    /// assertion is deterministic and scheduler-independent.
    #[tokio::test(flavor = "current_thread")]
    async fn clean_close_io_does_not_resolve_while_peer_still_open() {
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        use std::task::Wake;

        // poll_read returns Pending until `release_eof` is set, then a
        // clean EOF (0 bytes). poll_shutdown records whether the inner
        // FIN was delegated.
        struct LingerProbe {
            release_eof: AtomicBool,
            shutdown_called: AtomicBool,
            reads: AtomicUsize,
        }

        struct NoopWake;
        impl Wake for NoopWake {
            fn wake(self: Arc<Self>) {}
        }

        // Arc-shared handle so the test can flip `release_eof` after
        // construction; all I/O methods are interior-mutable via atomics
        // so `&LingerProbe` suffices and no `&mut` aliasing is needed.
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
        // (it must NOT resolve, because resolving => h2 drops the io =>
        // RST while the flood is still arriving => GOAWAY discarded).
        // The inner FIN, however, IS sent promptly on the first poll —
        // a FIN never causes an RST; only the drop-with-unread-data
        // does, and that drop is what we are gating.
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
