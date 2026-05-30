//! SESSION 16 / Mode B — terminate-and-re-originate raw-QUIC proxy
//! actor (the B1 dual-connection skeleton + the seam).
//!
//! Mode A (S15, [`crate::passthrough`]) routes QUIC by Connection ID
//! WITHOUT decrypting. Mode B instead **terminates** the client QUIC
//! connection (reusing the entire client-facing termination machinery —
//! [`crate::router::InboundPacketRouter`] accept/Retry/0-RTT-guard +
//! the [`crate::conn_actor`] connection pump) and **re-originates** a
//! fresh, dedicated upstream QUIC connection, mirroring the client's
//! negotiated ALPN. Two distinct [`quiche::Connection`] objects — two
//! distinct SCIDs, two distinct TLS key schedules — bound 1:1 by this
//! actor's co-ownership. NOT a CID bridge. See `audit/quic/s16-plan.md`
//! §1 (seam), §2.1 (CID-based actor-owned 1:1 identity).
//!
//! ## B1 + B2 scope (this file)
//!
//! 1. (B1) Drive the CLIENT-facing connection (already
//!    `accept_with_retry`'d by the router, handed over in
//!    [`ActorParams::conn`]) to established, using the same low-level
//!    pump shape as [`crate::conn_actor::run_actor`] (recv inbound
//!    packets forwarded by the router over [`ActorParams::inbound`] →
//!    `conn.recv`; drain `conn.send` to the shared
//!    [`ActorParams::socket`]; `on_timeout`).
//! 2. (B1) On client `is_established()`: read the negotiated ALPN via
//!    `application_proto()` and dial a **dedicated** upstream connection
//!    ([`QuicUpstreamPool::dial_dedicated`]) on its OWN UDP socket,
//!    mirroring that ALPN.
//! 3. (B1) Run BOTH connection pumps concurrently in one `tokio::select!`
//!    loop (client inbound + upstream socket recv + both timeouts +
//!    cancel) until either side closes or idle-times-out, then close the
//!    other gracefully and return.
//! 4. (B2) Run the **bidirectional raw-STREAM relay** ([`relay_streams`])
//!    after every wake: copy raw QUIC STREAM bytes both directions under
//!    an **identity stream-ID map** (plan §2.2 — no translation table)
//!    with a **bounded per-stream pending window** ([`STREAM_RELAY_WINDOW`],
//!    the R8 memory-safety mechanism) and genuine end-to-end
//!    backpressure: a slow destination keeps the window full, the relay
//!    stops reading the source, and quiche stops extending that source
//!    stream's flow-control window (the source peer pauses). FIN is
//!    propagated only after all buffered bytes drain.
//! 5. (B3) **Propagate cancellation** ([`pump_dir`]'s reset/stop arms):
//!    a peer RESET_STREAM (surfaced as `stream_recv` →
//!    `Err(StreamReset(code))`) is relayed onward as a RESET_STREAM
//!    carrying the SAME code (`dst.stream_shutdown(sid, Shutdown::Write,
//!    code)`); a peer STOP_SENDING (surfaced as `stream_send` →
//!    `Err(StreamStopped(code))`) is relayed back toward the source as a
//!    STOP_SENDING carrying the code (`src.stream_shutdown(sid,
//!    Shutdown::Read, code)`). The B2 smuggling guard is KEPT — the
//!    affected half is still marked `done` with its pending bytes dropped
//!    and **never** a clean FIN — B3 only ADDS the positive propagation so
//!    the peer observes a real stream reset/stop (the F-MD-4 analog, plan
//!    §2.3). Only the affected unidirectional half is torn down; a bidi
//!    stream's other direction stays live. Datagrams are **B4**.
//!
//! ## The two connections + the relay seam
//!
//! * The two connections live in [`run_raw_proxy_actor`] as
//!   `params.conn` (client) and `upstream.conn` (backend, an owned
//!   [`lb_io::quic_pool::DedicatedQuic`]). [`relay_streams`] reads/writes
//!   both inside the single select loop — every arm + the relay has
//!   `&mut` access to both, so no mutex is needed (same rationale as the
//!   H3 actor keeping per-stream state inline).
//! * The per-stream relay state ([`RawStreamState`], two [`RelayHalf`]s)
//!   is the explicit bounded per-stream table (plan §2.2 / §3 R8).
//!   [`RelayHalf`] carries a `reset_code` so the B3 cancellation
//!   propagation is recorded + idempotent — see its docs.
//! * [`RawProxyOutcome`] (returned via the `io::Result` chain through a
//!   test hook) surfaces both connections' SCIDs + trace_ids so the
//!   verifier's two-connections proof can assert distinctness by
//!   mechanism rather than by a bridge assertion.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;

use lb_io::quic_pool::{DedicatedQuic, QuicUpstreamPool};

use crate::conn_actor::{ActorParams, drain_conn_send};

/// Application-layer `CONNECTION_CLOSE` error code emitted on graceful
/// shutdown of either raw-QUIC leg. `0x0000` is QUIC's transport-level
/// `NO_ERROR`; for an application close (`app = true`) it is the
/// conventional "orderly shutdown, no application error" signal. Mode B
/// proxies raw QUIC (no H3 layer), so unlike the H3 actor's
/// [`crate::conn_actor::H3_NO_ERROR`] (`0x0100`, an HTTP/3 code) the raw
/// path uses the bare application `0`.
pub const RAW_NO_ERROR: u64 = 0x0000;

/// Upper bound on how long the client handshake (Phase 1) is pumped
/// before giving up. The router already drove the Retry exchange; this
/// covers the remaining handshake flights. Mirrors the upstream dial
/// budget in [`lb_io::quic_pool`] (5 s) so neither leg out-waits the
/// other.
const CLIENT_HANDSHAKE_BUDGET: Duration = Duration::from_secs(5);

/// Upper bound on how long [`graceful_close`] pumps a connection after
/// `close()` before giving up — same rationale + value as the H3
/// actor's `GRACEFUL_SHUTDOWN_BUDGET` (quiche drains for `3 * PTO`;
/// 500 ms is comfortably above that on any sane link).
const GRACEFUL_CLOSE_BUDGET: Duration = Duration::from_millis(500);

/// Fallback tick when a connection reports no quiche timeout — keeps the
/// select loop from parking indefinitely on a connection that has no
/// timer armed yet. Matches the H3 actor's `unwrap_or(100ms)` default.
const IDLE_TICK: Duration = Duration::from_millis(100);

/// Construction parameters for a Mode B re-origination.
///
/// Cheap to [`Clone`] (an `Arc` config factory + `addr` + `sni`), so one
/// configured backend on [`crate::router::RouterParams::raw_quic_backend`]
/// fans out to every per-connection actor's
/// [`ActorParams::raw_quic_backend`]. Mirrors the dial inputs of
/// [`QuicUpstreamPool`]: the pool owns the `config_factory`; this struct
/// names the target + SNI for the dedicated dial.
#[derive(Clone)]
pub struct RawBackend {
    /// The upstream QUIC pool used to dial the dedicated upstream
    /// connection. Mode B uses [`QuicUpstreamPool::dial_dedicated`],
    /// which does NOT pool the result — the actor owns the connection
    /// 1:1 — but the pool is the home of the dial machinery + the
    /// `config_factory` (R12: reuse, don't duplicate).
    pub pool: QuicUpstreamPool,
    /// Resolved upstream backend address to re-originate to.
    pub addr: std::net::SocketAddr,
    /// SNI presented to the upstream on the re-originated handshake.
    pub sni: String,
    /// B4 — per-direction bounded DATAGRAM relay queue capacity (count of
    /// queued datagrams). Operator-configurable via
    /// `lb_config::RawQuicProxyConfig::dgram_queue_cap`; the binary passes
    /// the configured value here so the relay's [`BoundedDgramQueue`]
    /// bound is single-sourced with the `enable_dgram(true, cap, cap)`
    /// queue length advertised on the wire. Defaults to [`DGRAM_QUEUE_CAP`]
    /// (the config helper returns the same constant).
    pub dgram_queue_cap: usize,
    /// B5 — explicit, defense-in-depth ceiling on the per-connection relay
    /// stream table size. Operator-configurable via
    /// `lb_config::RawQuicProxyConfig::max_relay_streams`; passed here so
    /// [`relay_streams`]/[`admit_or_refuse`] gate on the configured value
    /// (single-sourced, not the bare const). Defaults to
    /// [`MAX_RELAY_STREAMS`] (the config helper returns the same constant).
    pub max_relay_streams: usize,
}

impl std::fmt::Debug for RawBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawBackend")
            .field("addr", &self.addr)
            .field("sni", &self.sni)
            .field("dgram_queue_cap", &self.dgram_queue_cap)
            .field("max_relay_streams", &self.max_relay_streams)
            .finish_non_exhaustive()
    }
}

/// Mechanism-level summary of an established Mode B proxy — the
/// two-connections proof handle (plan §2.1 / §5).
///
/// Holds each leg's source Connection ID bytes + quiche `trace_id`.
/// Distinct `client_scid` vs `upstream_scid` (and distinct trace_ids)
/// prove two genuinely separate `quiche::Connection` objects with
/// independent key schedules — NOT a CID bridge. Surfaced to the
/// verifier via the test hook [`run_raw_proxy_actor_for_test`].
#[derive(Debug, Clone)]
pub struct RawProxyOutcome {
    /// Client-facing connection's source CID bytes (the SCID the LB
    /// chose as server, registered in the router's dispatch table).
    pub client_scid: Vec<u8>,
    /// Upstream connection's source CID bytes (the SCID the LB chose as
    /// client when re-originating).
    pub upstream_scid: Vec<u8>,
    /// Client-facing connection's quiche trace id.
    pub client_trace_id: String,
    /// Upstream connection's quiche trace id.
    pub upstream_trace_id: String,
    /// Negotiated ALPN that was mirrored upstream.
    pub negotiated_alpn: Vec<u8>,
}

/// Drive a Mode B (terminate-and-re-originate) raw-QUIC proxy connection.
///
/// See the [module docs](self) for the B1 contract. Dispatched from
/// [`crate::conn_actor::run_actor`] when
/// [`ActorParams::raw_quic_backend`] is `Some`.
///
/// # Errors
///
/// Never surfaces an error to the caller for an operational fault — like
/// [`run_actor`](crate::conn_actor::run_actor) the actor swallows +
/// logs faults and returns the success variant on graceful close. The
/// `io::Result<()>` shape exists for call-site chaining. (The fallible
/// internal [`run_raw_proxy_actor_inner`] is where the upstream-dial /
/// handshake error is surfaced for the test hook; the public entrypoint
/// logs + discards it.)
pub async fn run_raw_proxy_actor(params: ActorParams) -> std::io::Result<()> {
    match run_raw_proxy_actor_inner(params).await {
        Ok(_outcome) => Ok(()),
        Err(e) => {
            tracing::warn!(error = %e, "Mode B raw-proxy actor exited with error");
            // Parity with `run_actor`: faults are swallowed after
            // logging so the spawned task's `JoinHandle` is always Ok.
            Ok(())
        }
    }
}

/// Test hook: identical to [`run_raw_proxy_actor`] but returns the
/// [`RawProxyOutcome`] (the two-connections proof handle) instead of
/// swallowing it. The verifier's wire test drives this to assert two
/// distinct connections by mechanism (distinct SCIDs / trace_ids), then
/// the connections close gracefully. Not used in production (the router
/// always calls [`run_raw_proxy_actor`]).
///
/// # Errors
///
/// Surfaces the upstream-dial / handshake / pump error verbatim (unlike
/// the public entrypoint which logs + discards it).
#[cfg(any(test, feature = "test-gauges"))]
pub async fn run_raw_proxy_actor_for_test(params: ActorParams) -> std::io::Result<RawProxyOutcome> {
    run_raw_proxy_actor_inner(params).await
}

/// The fallible core. Phase 1 drives the client handshake + dials the
/// dedicated upstream; Phase 2 runs both pumps concurrently until either
/// side finishes, then closes the other. Returns the two-connections
/// proof on graceful completion.
async fn run_raw_proxy_actor_inner(mut params: ActorParams) -> std::io::Result<RawProxyOutcome> {
    // The seam guarantees this is `Some` (the dispatch in `run_actor`
    // only routes here when set), but match defensively rather than
    // unwrap — the crate denies `unwrap`/`expect`.
    let Some(backend) = params.raw_quic_backend.clone() else {
        return Err(std::io::Error::other(
            "run_raw_proxy_actor invoked without a raw_quic_backend",
        ));
    };

    let mut out_buf = vec![0u8; 65_535];

    // ---- Phase 1: drive the CLIENT-facing connection to established --
    //
    // Same low-level pump shape as `run_actor`: drain quiche's outbound
    // packets to the shared listener socket, recv router-forwarded
    // inbound packets, tick `on_timeout`. No H3/stream state.
    let established = drive_client_to_established(
        &mut params.conn,
        &params.socket,
        &mut params.inbound,
        &params.cancel,
        &mut out_buf,
    )
    .await;
    if !established {
        // Cancelled or closed before established. Best-effort graceful
        // close of the client, then return without dialing upstream.
        graceful_close(&mut params.conn, &params.socket, &mut out_buf).await;
        return Err(std::io::Error::other(
            "Mode B client connection closed before established",
        ));
    }

    // Capture the negotiated ALPN BEFORE moving into the dial — the
    // `application_proto()` borrow must not overlap the dial await.
    let negotiated_alpn = params.conn.application_proto().to_vec();
    let client_scid = params.conn.source_id().as_ref().to_vec();
    let client_trace_id = params.conn.trace_id().to_owned();
    tracing::debug!(
        alpn = %String::from_utf8_lossy(&negotiated_alpn),
        client_trace_id = %client_trace_id,
        backend = %backend.addr,
        "Mode B: client established; dialing dedicated upstream"
    );

    // ---- Re-originate: dial a DEDICATED upstream, mirroring ALPN ------
    //
    // Mirror the negotiated client ALPN upstream. An empty negotiated
    // ALPN (peer advertised none) ⇒ pass `&[]` so the upstream config
    // factory's own ALPN is used (the pool default is `h3`/`h3-29`).
    let alpn_protos: Vec<&[u8]> = if negotiated_alpn.is_empty() {
        Vec::new()
    } else {
        vec![negotiated_alpn.as_slice()]
    };
    let mut upstream: DedicatedQuic = backend
        .pool
        .dial_dedicated(backend.addr, &backend.sni, &alpn_protos)
        .await?;
    let upstream_scid = upstream.conn.source_id().as_ref().to_vec();
    let upstream_trace_id = upstream.conn.trace_id().to_owned();
    tracing::info!(
        client_trace_id = %client_trace_id,
        upstream_trace_id = %upstream_trace_id,
        backend = %backend.addr,
        "Mode B: re-originated upstream connection established (two distinct conns)"
    );

    let outcome = RawProxyOutcome {
        client_scid,
        upstream_scid,
        client_trace_id,
        upstream_trace_id,
        negotiated_alpn,
    };

    // B6 metrics — the two-connection relay is now ESTABLISHED. Bump the
    // cumulative counter once and the active gauge up; the gauge is
    // decremented on EVERY return path below (RAII-style `ActiveConnGuard`)
    // so a graceful close, an early upstream/relay fault, or a cancel all
    // restore the gauge. The increments live HERE at the actor lifetime —
    // the B4/B5 relay helpers are never given the metrics (their
    // signatures are unchanged). `None` (tests / H3 path) ⇒ no-op.
    let modeb_metrics = params.quic_modeb_metrics.clone();
    if let Some(m) = modeb_metrics.as_ref() {
        m.connections_total.inc();
    }
    let _active_guard = ActiveConnGuard::new(modeb_metrics.clone());

    // ---- Phase 2: run BOTH pumps + the B2 bidirectional raw-STREAM
    // relay concurrently until either leg closes / idle-times-out.
    //
    // B6 (R14/R12): the relay's two memory bounds are single-sourced from
    // the operator config via `backend` — the B4 datagram-queue cap and the
    // B5 relay-stream-table cap. Defaults equal the `DGRAM_QUEUE_CAP` /
    // `MAX_RELAY_STREAMS` consts (the config helpers return them), so an
    // unconfigured deployment behaves exactly as before.
    run_dual_pump(
        &mut params,
        &mut upstream,
        &mut out_buf,
        modeb_metrics.as_ref(),
        backend.dgram_queue_cap,
        backend.max_relay_streams,
    )
    .await;

    // Either side closed / idle-timed-out: close the other gracefully.
    // (Both calls are idempotent — a no-op if the leg is already
    // closed.)
    graceful_close(&mut params.conn, &params.socket, &mut out_buf).await;
    graceful_close(&mut upstream.conn, &upstream.socket, &mut out_buf).await;

    Ok(outcome)
}

/// B6 — RAII guard for the `quic_modeb_connections` active gauge. Bumps
/// the gauge up on construction (one established two-conn relay) and back
/// down on `Drop`, so EVERY exit from [`run_raw_proxy_actor_inner`]'s
/// Phase 2 — graceful close, an upstream/relay error, a panic-unwind, or a
/// cancel — restores the gauge without scattering `dec()` calls across the
/// return paths. `None` (tests / no registry) ⇒ a no-op guard.
struct ActiveConnGuard {
    gauge: Option<lb_observability::IntGauge>,
}

impl ActiveConnGuard {
    fn new(metrics: Option<lb_observability::QuicModeBMetrics>) -> Self {
        let gauge = metrics.map(|m| m.connections);
        if let Some(g) = gauge.as_ref() {
            g.inc();
        }
        Self { gauge }
    }
}

impl Drop for ActiveConnGuard {
    fn drop(&mut self) {
        if let Some(g) = self.gauge.as_ref() {
            g.dec();
        }
    }
}

/// Phase 1 pump: drive ONLY the client-facing connection until it is
/// established. Returns `true` on established, `false` if the connection
/// closed or the cancel token fired first.
///
/// Mirrors the `run_actor` select skeleton (biased cancel → inbound →
/// timeout) restricted to the handshake (no stream/H3 work).
async fn drive_client_to_established(
    conn: &mut quiche::Connection,
    socket: &Arc<UdpSocket>,
    inbound: &mut tokio::sync::mpsc::Receiver<crate::conn_actor::InboundPacket>,
    cancel: &tokio_util::sync::CancellationToken,
    out_buf: &mut [u8],
) -> bool {
    let deadline = tokio::time::Instant::now() + CLIENT_HANDSHAKE_BUDGET;
    loop {
        drain_conn_send(socket, conn, out_buf).await;
        if conn.is_established() {
            return true;
        }
        if conn.is_closed() {
            return false;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::debug!("Mode B: client handshake budget exhausted");
            return false;
        }
        let quiche_timeout = conn.timeout().unwrap_or(IDLE_TICK);
        let residual = deadline.saturating_duration_since(tokio::time::Instant::now());
        let wait = quiche_timeout.min(residual);

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                return false;
            }
            pkt = inbound.recv() => {
                let Some(mut pkt) = pkt else { return false; };
                let info = quiche::RecvInfo { from: pkt.from, to: pkt.to };
                match conn.recv(&mut pkt.data, info) {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(e) => tracing::debug!(error = %e, "Mode B client quiche recv (handshake)"),
                }
            }
            () = tokio::time::sleep(wait) => {
                conn.on_timeout();
            }
        }
    }
}

/// Phase 2 pump: drive BOTH legs concurrently in one `tokio::select!`
/// loop and run the **bidirectional raw-STREAM relay** (B2) after every
/// wake. Keeps both connections alive (recv inbound, drain outbound,
/// tick timeouts) until either leg closes or idle-times-out, or the
/// cancel token fires.
///
/// ## The select-loop shape
///
/// Each turn: drain both legs' outbound packets, check both for
/// `is_closed()`, then `select!` over five events:
/// 1. `cancel.cancelled()` (biased first) — listener shutdown.
/// 2. client inbound packet (router-forwarded mpsc) → `client.recv`.
/// 3. upstream socket `recv_from` → `upstream.recv`.
/// 4. client timeout → `client.on_timeout`.
/// 5. upstream timeout → `upstream.on_timeout`.
///
/// Immediately AFTER the `select!` (where the H3 actor runs `poll_h3`)
/// the relay runs [`relay_streams`] over BOTH connections — both are
/// `&mut` in scope, so no mutex (same reasoning as the H3 actor keeping
/// per-stream state inline). The relay both **reads new readable data**
/// and **flushes still-pending bytes** of a stream that was
/// backpressured on a previous turn (so a stream that could not drain to
/// a full destination last turn resumes the moment that destination
/// frees window).
///
/// ## Wake cadence (mirrors the H3 actor's S2/S4 short-tick)
///
/// quiche's idle timeout can be hundreds of ms; relying on it alone
/// would throttle a mid-transfer stream to a crawl. While the relay has
/// any in-flight per-stream state, the select wait is capped at
/// [`RELAY_TICK`] so a backpressured/partial transfer resumes promptly.
/// This does NOT defeat backpressure: the bounded per-stream window
/// ([`STREAM_RELAY_WINDOW`]) still caps in-flight bytes — we merely poll
/// the gate more often. When idle (no relay state) the loop parks on the
/// real quiche timeout, so there is no busy-spin.
async fn run_dual_pump(
    params: &mut ActorParams,
    upstream: &mut DedicatedQuic,
    out_buf: &mut [u8],
    // B6: optional `quic_modeb_*` handles. The relay bumps them ONLY here
    // at the per-pass aggregate level (datagrams-dropped delta +
    // streams-active gauge); the B4 datagram helper + `pump_dir` are never
    // given the metrics, so `relay_datagrams`/`pump_dir` keep their existing
    // signatures + tests. `None` ⇒ every update is a no-op.
    metrics: Option<&lb_observability::QuicModeBMetrics>,
    // B6 (R14/R12): operator-configured B4 datagram-queue cap, single-
    // sourced from `RawBackend`/`lb_config`. Defaults to `DGRAM_QUEUE_CAP`.
    dgram_queue_cap: usize,
    // B6 (R14/R12): operator-configured B5 relay-stream-table cap, single-
    // sourced from `RawBackend`/`lb_config`. Defaults to `MAX_RELAY_STREAMS`.
    // Threaded into `relay_streams` → `admit_or_refuse`.
    max_relay_streams: usize,
) {
    // The upstream recv needs its own inbound buffer (the client side
    // uses owned `Vec`s forwarded by the router; the upstream side
    // recv_from's straight off its dedicated socket).
    let mut up_in_buf = vec![0u8; 65_535];
    let upstream_local = upstream.local;

    // B2: the bounded per-stream relay state table (R8). Empty until the
    // first stream carries data. An entry lives until BOTH directions
    // are terminally done (FIN flushed, or dropped on a reset for B3).
    let mut streams: HashMap<u64, RawStreamState> = HashMap::new();

    // B4: the two bounded datagram relay queues (R8 — drop-newest when
    // full). Datagrams (RFC 9221) are independent of streams: no FIN, no
    // reset, no ordering guarantee, so they live OUTSIDE the stream table
    // and never touch stream state. `c2u_q` carries client→upstream
    // datagrams, `u2c_q` upstream→client.
    let mut c2u_q = BoundedDgramQueue::new(dgram_queue_cap);
    let mut u2c_q = BoundedDgramQueue::new(dgram_queue_cap);

    // B6: last observed total drop count across both queues, so we can
    // surface only the DELTA into the cumulative `quic_modeb_datagrams_\
    // dropped_total` counter each pass (the queues own a monotonic
    // per-lifetime `dropped` accessor; the counter is process-cumulative).
    let mut last_dropped_total: u64 = 0;

    loop {
        // Drain any queued outbound on both legs first (parity with the
        // H3 actor draining before the wait).
        drain_conn_send(&params.socket, &mut params.conn, out_buf).await;
        drain_conn_send(&upstream.socket, &mut upstream.conn, out_buf).await;

        if params.conn.is_closed() || upstream.conn.is_closed() {
            break;
        }

        let mut client_wait = params.conn.timeout().unwrap_or(IDLE_TICK);
        let mut upstream_wait = upstream.conn.timeout().unwrap_or(IDLE_TICK);
        // While any stream is mid-transfer OR a datagram is queued
        // (B4: a `dgram_send` previously returned `Done` and we are
        // holding a payload to retry, or a fresh recv-drain enqueued one),
        // poll the relay gate often so a backpressured/partial stream
        // resumes promptly AND datagram-only traffic (no streams at all)
        // is still pumped without waiting out quiche's idle timeout. This
        // does NOT defeat the bounded window/queue — see fn docs. When
        // fully idle (no streams, no queued datagrams), fall through to
        // the real quiche timeouts (no busy-spin).
        if !streams.is_empty() || !c2u_q.is_empty() || !u2c_q.is_empty() {
            client_wait = client_wait.min(RELAY_TICK);
            upstream_wait = upstream_wait.min(RELAY_TICK);
        }

        tokio::select! {
            biased;
            () = params.cancel.cancelled() => {
                break;
            }
            pkt = params.inbound.recv() => {
                let Some(mut pkt) = pkt else { break; };
                let info = quiche::RecvInfo { from: pkt.from, to: pkt.to };
                match params.conn.recv(&mut pkt.data, info) {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(e) => tracing::debug!(error = %e, "Mode B client quiche recv"),
                }
            }
            r = upstream.socket.recv_from(&mut up_in_buf) => {
                match r {
                    Ok((n, from)) => {
                        let slice = up_in_buf.get_mut(..n).unwrap_or(&mut []);
                        let info = quiche::RecvInfo { from, to: upstream_local };
                        match upstream.conn.recv(slice, info) {
                            Ok(_) | Err(quiche::Error::Done) => {}
                            Err(e) => {
                                tracing::debug!(error = %e, "Mode B upstream quiche recv");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "Mode B upstream recv_from");
                    }
                }
            }
            () = tokio::time::sleep(client_wait) => {
                params.conn.on_timeout();
            }
            () = tokio::time::sleep(upstream_wait) => {
                upstream.conn.on_timeout();
            }
        }

        // B2 relay: copy raw STREAM data both directions over the two
        // `&mut` connections, with identity stream-ID mapping + the
        // bounded per-stream window. Runs every wake so both freshly
        // readable data AND previously-backpressured pending bytes make
        // progress. The follow-up `drain_conn_send` at the top of the
        // next turn ships whatever this relay handed to quiche.
        relay_streams(
            &mut params.conn,
            &mut upstream.conn,
            &mut streams,
            max_relay_streams,
        );

        // B4 relay: forward unreliable DATAGRAMs (RFC 9221) verbatim both
        // directions through the two bounded drop-newest queues. Runs
        // every wake right after the stream relay. Datagrams have no
        // FIN/reset/ordering and never touch stream state — a datagram
        // queue full simply drops the NEWEST payload (the bound is the R8
        // memory-safety mechanism), and a payload quiche could not accept
        // this turn (`dst` send queue full) stays queued (bounded by cap)
        // and is retried next wake. The follow-up `drain_conn_send` at the
        // top of the next turn ships whatever this relay handed to quiche.
        relay_datagrams(&mut params.conn, &mut upstream.conn, &mut c2u_q, &mut u2c_q);

        // B6 metrics (per-pass aggregate; relay helpers untouched):
        // * `streams_active` ← the B5 relay-table size after this pass
        //   (post-`streams.retain` reclamation inside `relay_streams`).
        // * `datagrams_dropped_total` ← the DELTA of both queues' B4
        //   drop-newest counters since last pass (monotonic → non-negative
        //   delta; `saturating_*` so a `usize`/`i64`/`u64` boundary can
        //   never panic under the crate's no-panic bar).
        if let Some(m) = metrics {
            #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
            let table_len = i64::try_from(streams.len()).unwrap_or(i64::MAX);
            m.streams_active.set(table_len);

            let dropped_total = c2u_q.dropped().saturating_add(u2c_q.dropped());
            let delta = dropped_total.saturating_sub(last_dropped_total);
            if delta > 0 {
                m.datagrams_dropped_total.inc_by(delta);
                last_dropped_total = dropped_total;
            }
        }
    }
}

/// Bounded per-stream relay window, in bytes, **per stream per
/// direction** (R8 — the memory-safety mechanism, NOT a body/total cap).
///
/// The relay reads from a source stream ONLY while that stream's pending
/// buffer for the corresponding direction holds fewer than this many
/// bytes. When a destination stalls (its QUIC flow-control / send buffer
/// is full so `stream_send` returns a short write or `Done`), the unsent
/// remainder stays pending; once pending reaches this window the relay
/// stops calling `stream_recv` on the source, so quiche stops extending
/// that source stream's flow-control window and the *peer* pauses —
/// genuine end-to-end backpressure. 256 KiB is a few BDPs on a LAN/short
/// RTT path (enough not to throttle a healthy transfer) while keeping
/// worst-case per-stream relay memory bounded and independent of the
/// total transfer size. Total per-connection relay memory is bounded by
/// `MAX_RELAY_STREAMS * 2 * STREAM_RELAY_WINDOW` ([`MAX_RELAY_STREAMS`]
/// caps the table size; the per-stream window is bounded here).
const STREAM_RELAY_WINDOW: usize = 256 * 1024;

/// B5 — explicit, defense-in-depth CEILING on the per-connection relay
/// stream table ([`run_dual_pump`]'s `streams: HashMap<u64,
/// RawStreamState>`). Together with the per-stream [`STREAM_RELAY_WINDOW`]
/// it makes the worst-case per-connection relay memory a HARD constant:
///
/// ```text
///   per-conn relay memory  ≤  MAX_RELAY_STREAMS * 2 * STREAM_RELAY_WINDOW
///                          =  256 * 2 * 256 KiB  =  128 MiB  (never approached)
/// ```
///
/// ## Why an explicit cap on top of quiche's `max_streams`
///
/// In normal operation the table is already bounded WELL below this by
/// quiche's negotiated stream grant: the client-facing
/// [`crate::listener::build_server_config`] sets
/// `initial_max_streams_bidi(16)` + `initial_max_streams_uni(16)`, so a
/// conforming client can have at most ~32 streams concurrently OPEN, and
/// the existing reclamation ([`relay_streams`]'s `streams.retain`) evicts
/// each stream the moment BOTH its directions finish — the table grows
/// with the *concurrent* count, NOT the *total* stream count over a
/// connection's life. This cap is therefore NOT the primary bound in
/// practice.
///
/// It exists as defense-in-depth so the relay's memory ceiling is
/// INDEPENDENT of the quiche config: were `max_streams` ever mis-set to a
/// huge value (operator error / a future config change), the negotiated
/// grant alone would no longer bound the table, and this constant still
/// would. `256` sits comfortably above the negotiated concurrent grant
/// (~32 — an 8× margin so a correctly-configured connection NEVER hits it)
/// while keeping the absolute memory ceiling sane (128 MiB worst-case,
/// itself never reached because completed streams are reclaimed). An R7
/// pre-auth-safe, conservatively-sized constant.
///
/// B6 (R14/R12): this is the **canonical DEFAULT** for the
/// operator-configurable cap. The runtime value is single-sourced from
/// [`RawBackend::max_relay_streams`] (set from
/// `lb_config::RawQuicProxyConfig::max_relay_streams`, whose serde default
/// helper returns this same `256`); [`relay_streams`]/[`admit_or_refuse`]
/// gate on that param, NOT this const directly. It is `pub` so the params
/// layer can use it as the documented fallback default and tests can pin
/// the value.
pub const MAX_RELAY_STREAMS: usize = 256;

/// Short poll interval used while ANY stream is mid-transfer, so a
/// partial/backpressured copy resumes without waiting out quiche's idle
/// timeout. Mirrors the H3 actor's `next_wait.min(2ms)` cadence
/// (`conn_actor.rs` S2/S4). 2 ms keeps latency low without busy-spinning
/// (the loop only ticks this fast while there is pending relay work).
const RELAY_TICK: Duration = Duration::from_millis(2);

/// One direction of a relayed raw stream: a BOUNDED pending byte buffer
/// (capped at [`STREAM_RELAY_WINDOW`]) plus FIN/cancellation bookkeeping.
/// The pending buffer is the R8 bound and the backpressure point; the FIN
/// flags ensure a clean stream end is only emitted AFTER every buffered
/// byte has been accepted by the destination (never a FIN ahead of data).
#[derive(Default)]
struct RelayHalf {
    /// Bytes read from the source but not yet accepted by the
    /// destination's `stream_send`. Capped at [`STREAM_RELAY_WINDOW`]:
    /// the source is not read while this is at/over the cap.
    pending: Vec<u8>,
    /// The source returned `fin=true`. The destination FIN is deferred
    /// until `pending` is fully drained (see [`Self::needs_work`]).
    src_fin_seen: bool,
    /// A clean FIN (`stream_send(.., &[], true)`) has been delivered to
    /// the destination — terminal for this direction.
    fin_sent: bool,
    /// This direction is finished (FIN sent, or dropped + cancellation
    /// propagated on a reset/stop — B3). No more reads or sends; the entry
    /// is reclaimed once both directions are done.
    done: bool,
    /// B3: set to the application error code once a cancellation has been
    /// PROPAGATED for this half — `Some(code)` after a peer RESET_STREAM
    /// (`stream_recv` → `Err(StreamReset(code))`) was relayed onward as a
    /// RESET_STREAM, or a peer STOP_SENDING (`stream_send` →
    /// `Err(StreamStopped(code))`) was relayed back as a STOP_SENDING.
    /// Records the propagated code (observability) and makes the
    /// propagation idempotent: a half is only ever shut down once (it is
    /// also `done` immediately, so it is not revisited, but this is the
    /// explicit guard against any double-propagation).
    reset_code: Option<u64>,
}

impl RelayHalf {
    /// B3 — propagate a stream cancellation onto `peer` ONCE and mark this
    /// half terminally done WITHOUT a clean FIN (the F-MD-4 smuggling
    /// guard is kept: a truncated transfer must never look complete).
    ///
    /// `dir_for_peer` selects the shutdown direction (counterintuitive in
    /// quiche — see `audit/quic/s16-quiche-api-notes.md`):
    /// * [`quiche::Shutdown::Write`] ⇒ emits **RESET_STREAM** toward
    ///   `peer` (used to relay a source RESET_STREAM onward to `dst`).
    /// * [`quiche::Shutdown::Read`] ⇒ emits **STOP_SENDING** toward
    ///   `peer` (used to relay a destination STOP_SENDING back to `src`).
    ///
    /// Idempotent: if this half already propagated a cancellation
    /// (`reset_code.is_some()`) it is a no-op. `stream_shutdown` returning
    /// `Err(Done)` (that side already reset/closed/unknown) is treated as
    /// success; any other error is logged and swallowed — never a panic.
    fn propagate_cancel(
        &mut self,
        peer: &mut quiche::Connection,
        sid: u64,
        code: u64,
        dir_for_peer: quiche::Shutdown,
        dir: Direction,
    ) {
        // Guard against double-propagation: only ever shut the peer down
        // once for this half. (`done` already prevents a revisit, but a
        // half can be reset in one direction while we are mid-pass; this
        // is the explicit idempotency latch.)
        if self.reset_code.is_some() {
            self.pending.clear();
            self.done = true;
            return;
        }
        match peer.stream_shutdown(sid, dir_for_peer, code) {
            // Propagated, or the peer side was already gone — either way
            // the cancellation is (or will be) reflected to the peer.
            Ok(()) | Err(quiche::Error::Done) => {}
            Err(e) => {
                // Do NOT panic: the half is failing anyway and the
                // connection pump continues. Log for observability.
                tracing::debug!(
                    stream_id = sid, dir = dir.as_str(), error = %e,
                    "Mode B B3: stream_shutdown while propagating cancellation \
                     (swallowed; half still dropped without a FIN)"
                );
            }
        }
        // Smuggling guard (B2, kept): drop unsent bytes, terminate this
        // half, NEVER a clean FIN.
        self.pending.clear();
        self.reset_code = Some(code);
        self.done = true;
    }
}

/// Bounded per-stream relay state (plan §2.2): identity stream-ID map, so
/// the SAME `sid` indexes both connections. Holds the two directions'
/// [`RelayHalf`]s. `c2u` = client→upstream, `u2c` = upstream→client.
///
/// ## B3 (cancellation) propagation
///
/// Each direction is an independent [`RelayHalf`] keyed by `sid`, so a
/// cancellation tears down ONLY the affected unidirectional half — a bidi
/// stream's other direction stays live. A `stream_recv` that returns
/// `Err(StreamReset(code))` (the peer reset its send side) relays a
/// RESET_STREAM onward to the destination via
/// [`RelayHalf::propagate_cancel`] with [`quiche::Shutdown::Write`]; a
/// `stream_send` that returns `Err(StreamStopped(code))` (the peer
/// STOP_SENDING'd our send side) relays a STOP_SENDING back toward the
/// source via [`RelayHalf::propagate_cancel`] with
/// [`quiche::Shutdown::Read`]. The B2 smuggling guard is KEPT: the half is
/// dropped (`pending` cleared, `done = true`) and **never** a clean FIN —
/// a truncated transfer must not be presented as complete. See the
/// `// B3:` arms in [`pump_dir`].
#[derive(Default)]
struct RawStreamState {
    /// client → upstream direction.
    c2u: RelayHalf,
    /// upstream → client direction.
    u2c: RelayHalf,
}

impl RawStreamState {
    /// Both directions terminally finished ⇒ the entry can be reclaimed.
    const fn is_complete(&self) -> bool {
        self.c2u.done && self.u2c.done
    }
}

/// B2 — one bidirectional raw-STREAM relay pass over the two connections.
///
/// Identity stream-ID mapping (plan §2.2): a client stream `sid` relays
/// to the upstream stream of the SAME `sid` and vice-versa — the
/// role-quadrants line up (LB is server to the client, client to the
/// backend), so no translation table.
///
/// The candidate set each turn is the union of:
/// * `client.readable()` — client streams with new bytes to forward;
/// * `upstream.readable()` — backend streams with new bytes to forward;
/// * every `sid` already in the state table — so a stream that was
///   backpressured (pending bytes the destination could not accept) or
///   is awaiting a deferred FIN is revisited and resumes the moment the
///   destination frees window.
///
/// `readable()` is a snapshot, so it is re-collected here every pass.
///
/// `max_relay_streams` is the B5 per-connection table ceiling, single-
/// sourced from the operator config ([`RawBackend::max_relay_streams`] →
/// `lb_config`); it defaults to [`MAX_RELAY_STREAMS`]. Threaded into
/// [`admit_or_refuse`].
fn relay_streams(
    client: &mut quiche::Connection,
    upstream: &mut quiche::Connection,
    streams: &mut HashMap<u64, RawStreamState>,
    max_relay_streams: usize,
) {
    // Union of readable streams on both legs + every sid with live relay
    // state (pending bytes / deferred FIN). De-dup via the state map: a
    // readable sid that is not yet tracked gets a default entry; an
    // already-tracked sid is revisited regardless of readability.
    //
    // B5 — explicit per-stream table cap (`max_relay_streams`, default
    // [`MAX_RELAY_STREAMS`], defense-in-depth): a NEW readable sid is
    // admitted only while the table is below the cap; an already-tracked sid
    // is ALWAYS re-processed (correctness — never drop a live stream
    // mid-transfer). Over-cap is only reachable if the quiche `max_streams`
    // grant is mis-configured far above the cap (a conforming client can
    // have ≤ ~32 streams open — see [`MAX_RELAY_STREAMS`]); when it happens
    // we refuse to TRACK the new sid (fail-safe, bounded) and log rate-aware,
    // rather than grow without bound.
    for sid in client.readable() {
        admit_or_refuse(streams, sid, max_relay_streams);
    }
    for sid in upstream.readable() {
        admit_or_refuse(streams, sid, max_relay_streams);
    }

    let sids: Vec<u64> = streams.keys().copied().collect();
    for sid in sids {
        let Some(state) = streams.get_mut(&sid) else {
            continue;
        };
        // client → upstream: read from `client`, write to `upstream`.
        pump_dir(
            sid,
            client,
            upstream,
            &mut state.c2u,
            Direction::ClientToUpstream,
        );
        // upstream → client: read from `upstream`, write to `client`.
        pump_dir(
            sid,
            upstream,
            client,
            &mut state.u2c,
            Direction::UpstreamToClient,
        );
    }

    // Reclaim entries whose BOTH directions are terminally done. This is the
    // lifetime reclamation that keeps the table bounded by the CONCURRENT
    // stream count (not the total over the connection's life) — the B5
    // stream-flood test's primary bound, and the eviction whose removal makes
    // the table grow with total streams (the load-bearing negative control).
    streams.retain(|_, st| !st.is_complete());
}

/// B5 — admit a NEW relay-stream `sid` into the per-connection table iff the
/// table is below `max_relay_streams` (the operator-configured ceiling,
/// default [`MAX_RELAY_STREAMS`]); an already-tracked `sid` is left
/// untouched (a no-op `or_default` would be too — but the explicit
/// `contains_key` short-circuit makes the always-process-existing invariant
/// unmistakable). Over the cap a NEW sid is REFUSED (not inserted) and logged
/// rate-aware — a fail-safe, bounded ceiling independent of the quiche
/// `max_streams` grant (see [`MAX_RELAY_STREAMS`]). Returns nothing; the
/// table mutation IS the effect.
fn admit_or_refuse(streams: &mut HashMap<u64, RawStreamState>, sid: u64, max_relay_streams: usize) {
    if streams.contains_key(&sid) {
        // Already tracked: ALWAYS re-processed this pass (never drop a live
        // stream). No table growth, so the cap does not apply.
        return;
    }
    if streams.len() < max_relay_streams {
        streams.entry(sid).or_default();
    } else {
        // Over the explicit ceiling — refuse to track this new sid. Only
        // reachable with a mis-configured huge `max_streams` (a conforming
        // peer cannot open this many concurrent streams). `debug!` keeps the
        // log rate-bounded under a flood (one line per over-cap readable sid
        // per pass; the conforming path never reaches here).
        tracing::debug!(
            stream_id = sid,
            table_len = streams.len(),
            cap = max_relay_streams,
            "Mode B B5: relay stream table at cap; refusing new stream (R8 bound \
             — only reachable with a mis-configured max_streams)"
        );
    }
}

/// B4 — maximum capacity (in datagrams) of ONE [`BoundedDgramQueue`], i.e.
/// per connection-pair per direction. This is the R8 memory-safety bound
/// for the datagram relay (NOT a body/total cap): worst-case relay memory
/// for one direction is `DGRAM_QUEUE_CAP * MAX_DGRAM_SIZE`, bounded and
/// independent of total traffic. `1024` matches quiche's own
/// recv/send-queue length default (`enable_dgram(true, 1024, 1024)` on the
/// Mode-B configs) and is an industry-safe pre-auth default per R7 — large
/// enough to absorb a normal burst, small enough that a flooding peer
/// cannot grow our memory without bound (over-cap arrivals are
/// drop-newest, see [`BoundedDgramQueue::push`]).
///
/// B6 (R14/R12): this is the **canonical DEFAULT** for the
/// operator-configurable cap. The runtime value is single-sourced from
/// [`RawBackend::dgram_queue_cap`] (set from
/// `lb_config::RawQuicProxyConfig::dgram_queue_cap`, whose serde default
/// helper returns this same `1024`); [`run_dual_pump`] builds both
/// [`BoundedDgramQueue`]s with that param, NOT this const directly. It is
/// `pub` so the params layer can use it as the documented fallback default
/// and tests can pin the value.
pub const DGRAM_QUEUE_CAP: usize = 1024;

/// B4 — scratch buffer size for one `dgram_recv` (the largest single
/// datagram payload we will copy out of quiche). A QUIC datagram payload
/// cannot exceed one UDP datagram's worth of bytes; `65_535` is the
/// absolute UDP payload ceiling (matches the crate-wide
/// `MAX_UDP_DATAGRAM_SIZE`). `dgram_recv` into a buffer this large can
/// therefore never return `BufferTooShort` in practice — that arm is
/// defensive only.
const MAX_DGRAM_SIZE: usize = 65_535;

/// B4 — outcome of a [`BoundedDgramQueue::push`]: either the payload was
/// queued, or the queue was at capacity and the payload was dropped
/// (drop-newest). Returned so the recv-drain (and the unit tests) can
/// observe the drop-newest decision by mechanism rather than by inspecting
/// the counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DgramPushOutcome {
    /// The payload was appended to the back of the queue.
    Queued,
    /// The queue was full (`len() >= cap`); the payload was DISCARDED and
    /// the `dropped` counter incremented (drop-newest policy).
    Dropped,
}

/// B4 — a bounded FIFO of QUIC DATAGRAM (RFC 9221) payloads with an
/// explicit **drop-newest** full-policy (the R8 memory-safety bound for
/// the datagram relay).
///
/// One queue per connection-pair per direction. Payloads are owned
/// `Vec<u8>` stored **verbatim** (binary-safe; zero-length payloads are
/// preserved as empty `Vec`s — datagrams have no length-implied
/// semantics, so an empty datagram is a legitimate, distinct datagram).
///
/// ## Drop-newest policy
///
/// When the queue is at capacity (`len() >= cap`) an incoming
/// [`push`](Self::push) DISCARDS the *arriving* payload (the newest) and
/// increments [`dropped`](Self::dropped) — the already-queued (older)
/// payloads are kept in order. This mirrors quiche's own recv-queue
/// overflow behaviour (a full recv queue drops the arriving frame), so the
/// relay layer's policy is owned, documented, and unit-testable. Datagrams
/// are unreliable by contract (RFC 9221), so dropping the newest under
/// pressure is correct: there is no retransmission obligation and no
/// ordering guarantee to violate.
///
/// The alternative — an unbounded queue or drop-oldest — would either let
/// a flooding peer grow relay memory without bound (the R8 violation this
/// type prevents) or silently reorder by evicting in-flight head-of-line
/// payloads; drop-newest keeps memory bounded AND preserves the order of
/// what is retained.
struct BoundedDgramQueue {
    /// FIFO of datagram payloads (verbatim bytes, front = oldest).
    q: VecDeque<Vec<u8>>,
    /// Maximum number of queued payloads — the R8 bound. A `push` at this
    /// length drops the newest.
    cap: usize,
    /// Count of drop-newest events over this queue's lifetime. Surfaced
    /// (read-only via [`dropped`](Self::dropped)) so B6 can expose it as a
    /// `quic_modeb_datagrams_dropped_total`-class metric. Saturates rather
    /// than wraps (a `u64` of drops is not reachable in practice, but the
    /// increment is `saturating_add` to honour the crate's no-panic bar
    /// under any conceivable overflow).
    dropped: u64,
}

impl BoundedDgramQueue {
    /// Construct an empty queue bounded at `cap` payloads.
    fn new(cap: usize) -> Self {
        Self {
            q: VecDeque::new(),
            cap,
            dropped: 0,
        }
    }

    /// Enqueue `payload` (verbatim) unless the queue is full.
    ///
    /// **Drop-newest**: if `len() >= cap` the arriving `payload` is
    /// discarded and [`dropped`](Self::dropped) is incremented; the
    /// already-queued payloads are untouched. Otherwise `payload` is
    /// appended at the back (FIFO). Returns which branch was taken so the
    /// caller can observe the policy by mechanism.
    fn push(&mut self, payload: Vec<u8>) -> DgramPushOutcome {
        if self.q.len() >= self.cap {
            // Drop-newest: discard the arriving payload, count it. The
            // bound holds regardless of `cap == 0` (then every push is a
            // drop).
            self.dropped = self.dropped.saturating_add(1);
            DgramPushOutcome::Dropped
        } else {
            self.q.push_back(payload);
            DgramPushOutcome::Queued
        }
    }

    /// Borrow the front (oldest) payload without removing it, or `None` if
    /// empty. Used by the send-drain to peek the next payload before
    /// attempting `dgram_send` (so a `Done`/full-send-queue can leave it
    /// queued).
    fn front(&self) -> Option<&Vec<u8>> {
        self.q.front()
    }

    /// Remove and return the front (oldest) payload, or `None` if empty.
    fn pop_front(&mut self) -> Option<Vec<u8>> {
        self.q.pop_front()
    }

    /// Number of currently-queued payloads (never exceeds `cap`).
    fn len(&self) -> usize {
        self.q.len()
    }

    /// `true` iff no payloads are queued.
    fn is_empty(&self) -> bool {
        self.q.is_empty()
    }

    /// Total drop-newest events over this queue's lifetime (plumbed for
    /// the B6 metric).
    #[cfg_attr(not(test), allow(dead_code))]
    fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// B4 — one bidirectional DATAGRAM (RFC 9221) relay pass over the two
/// connections. Symmetric: [`pump_dgram_dir`] is run once per direction.
///
/// Datagrams are connectionless w.r.t. streams — no FIN, no reset, no
/// ordering guarantee — so this relay NEVER touches stream state and is
/// fully independent of [`relay_streams`]. Each direction recv-drains the
/// source connection's datagram queue into the bounded relay queue
/// (drop-newest when full) then send-drains that queue into the
/// destination connection.
fn relay_datagrams(
    client: &mut quiche::Connection,
    upstream: &mut quiche::Connection,
    c2u_q: &mut BoundedDgramQueue,
    u2c_q: &mut BoundedDgramQueue,
) {
    // client → upstream: drain client's recv'd datagrams, send to upstream.
    pump_dgram_dir(client, upstream, c2u_q, Direction::ClientToUpstream);
    // upstream → client: drain upstream's recv'd datagrams, send to client.
    pump_dgram_dir(upstream, client, u2c_q, Direction::UpstreamToClient);
}

/// B4 — relay ONE direction of DATAGRAM traffic for this turn: recv-drain
/// every datagram quiche has queued on `src` into the bounded `q`
/// (drop-newest when full), then send-drain `q` into `dst`.
///
/// ## Recv-drain (`src` → `q`)
///
/// Loop `src.dgram_recv(buf)` (buf sized to [`MAX_DGRAM_SIZE`]):
/// * `Ok(len)` → push `buf[..len].to_vec()` (verbatim) onto `q`
///   (drop-newest if `q` is at `cap`).
/// * `Err(Done)` → no more queued datagrams on `src`; stop draining.
/// * `Err(BufferTooShort)` → the datagram was larger than our max buffer.
///   With a [`MAX_DGRAM_SIZE`] (full UDP-payload-ceiling) buffer this is
///   not reachable in practice; treat it defensively — log and stop
///   draining this turn (do NOT spin).
///
/// ## Send-drain (`q` → `dst`), front-first (FIFO, preserve arrival order)
///
/// While `q.front()` is `Some`, attempt `dst.dgram_send(front)`:
/// * `Ok(())` → accepted; `pop_front` and continue.
/// * `Err(Done)` → `dst`'s OWN send queue is full → **stop this turn**,
///   leaving the payload queued (bounded by `cap`; retried next wake when
///   `dst` has drained). Do NOT drop — `Done` is transient backpressure.
/// * `Err(BufferTooShort)` → the payload exceeds `dst`'s peer
///   `max_datagram_frame_size` (it can NEVER be forwarded over this
///   connection) → drop THIS payload (`pop_front`, count) and continue to
///   the next (it would block the queue forever otherwise).
/// * `Err(InvalidState)` → `dst` never negotiated DATAGRAM (mis-wired:
///   negotiation is a config-time invariant). This direction cannot
///   forward anything → drain + discard the whole queue (counting each)
///   and log, so a non-negotiating `dst` cannot pin relay memory.
fn pump_dgram_dir(
    src: &mut quiche::Connection,
    dst: &mut quiche::Connection,
    q: &mut BoundedDgramQueue,
    dir: Direction,
) {
    // ── Recv-drain: pull every datagram quiche has queued on `src` into
    // the bounded relay queue (drop-newest when full).
    let mut buf = vec![0u8; MAX_DGRAM_SIZE];
    loop {
        match src.dgram_recv(&mut buf) {
            Ok(len) => {
                // Verbatim copy of exactly `len` bytes (binary-safe,
                // zero-length preserved). `get(..len)` cannot panic; on
                // the impossible None it yields an empty payload.
                let payload = buf.get(..len).unwrap_or(&[]).to_vec();
                if q.push(payload) == DgramPushOutcome::Dropped {
                    tracing::trace!(
                        dir = dir.as_str(),
                        dropped = q.dropped,
                        "Mode B B4: datagram relay queue full; dropped newest (R8 bound)"
                    );
                }
            }
            Err(quiche::Error::Done) => break,
            // Not reachable with a full-UDP-payload-sized buffer; defensive.
            Err(quiche::Error::BufferTooShort) => {
                tracing::debug!(
                    dir = dir.as_str(),
                    max = MAX_DGRAM_SIZE,
                    "Mode B B4: dgram_recv BufferTooShort (datagram exceeds max buf); \
                     stopping recv-drain this turn"
                );
                break;
            }
            Err(e) => {
                tracing::debug!(
                    dir = dir.as_str(), error = %e,
                    "Mode B B4: dgram_recv error; stopping recv-drain this turn"
                );
                break;
            }
        }
    }

    // ── Send-drain: forward queued datagrams to `dst`, front-first.
    while let Some(front) = q.front() {
        match dst.dgram_send(front) {
            Ok(()) => {
                // Accepted by quiche's send queue — drop the front.
                let _ = q.pop_front();
            }
            // `dst`'s send queue is full: transient backpressure. Leave
            // the payload queued (bounded by cap) and retry next wake.
            Err(quiche::Error::Done) => break,
            // The payload is larger than `dst`'s peer max writable: it can
            // NEVER be forwarded over this connection. Drop THIS one (it
            // would otherwise block the queue forever) and continue.
            Err(quiche::Error::BufferTooShort) => {
                let _ = q.pop_front();
                q.dropped = q.dropped.saturating_add(1);
                tracing::debug!(
                    dir = dir.as_str(),
                    "Mode B B4: dgram_send BufferTooShort (payload exceeds dst max \
                     writable); dropping this datagram"
                );
            }
            // `dst` never negotiated DATAGRAM (only reachable if mis-wired
            // — negotiation is a config-time invariant). This direction can
            // forward NOTHING, so drain + discard the whole queue (counting
            // each) so a non-negotiating peer cannot pin relay memory.
            Err(quiche::Error::InvalidState) => {
                let drained = q.len() as u64;
                while q.pop_front().is_some() {}
                q.dropped = q.dropped.saturating_add(drained);
                tracing::warn!(
                    dir = dir.as_str(),
                    drained,
                    "Mode B B4: dgram_send InvalidState (dst never negotiated DATAGRAM); \
                     draining + disabling this direction's datagram queue"
                );
                break;
            }
            Err(e) => {
                // Any other error: drop this datagram (datagrams are
                // unreliable; do not block the queue) and stop this turn.
                let _ = q.pop_front();
                q.dropped = q.dropped.saturating_add(1);
                tracing::debug!(
                    dir = dir.as_str(), error = %e,
                    "Mode B B4: dgram_send error; dropping this datagram, stopping \
                     send-drain this turn"
                );
                break;
            }
        }
    }
}

/// Relay direction — only used to disambiguate log lines (the relay
/// itself is symmetric).
#[derive(Clone, Copy)]
enum Direction {
    ClientToUpstream,
    UpstreamToClient,
}

impl Direction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ClientToUpstream => "c→u",
            Self::UpstreamToClient => "u→c",
        }
    }
}

/// Relay ONE direction of ONE stream for this turn: gate-read from `src`
/// into the bounded pending buffer, then drain pending into `dst`,
/// honouring partial writes / `Done` / `StreamLimit`, and propagating a
/// clean FIN only after all pending bytes are accepted.
///
/// ## Backpressure (R8 — the bounded-window mechanism)
///
/// * **Read gate**: `src.stream_recv` is called ONLY while
///   `half.pending.len() < STREAM_RELAY_WINDOW`. quiche extends a
///   stream's flow-control window (queues `MAX_STREAM_DATA`) as a side
///   effect of `stream_recv`; by NOT reading while pending is full we
///   stop extending the window, so the *source peer* (client or backend)
///   blocks once its in-flight credit is spent. A slow `dst` keeps
///   `pending` full ⇒ the relay stops reading `src` ⇒ the slow side
///   pauses the fast side. Symmetric for both directions.
/// * **Write**: a short write (`Ok(n) < pending.len()`) or `Ok(0)` /
///   `Err(Done)` means `dst`'s send buffer / flow-control window is
///   full; the unsent remainder STAYS in `pending` (drained front-first,
///   no reorder, no drop) and the relay stops pushing this turn.
/// * **`StreamLimit`**: opening the mirror stream is refused (peer's
///   MAX_STREAMS not yet granted). Keep the bytes pending and retry next
///   turn — never drop. This is stream-grant backpressure.
///
/// ## FIN
///
/// When `stream_recv` returns `fin=true` the FIN is recorded
/// (`src_fin_seen`) but NOT forwarded until `pending` is empty; only then
/// is `stream_send(sid, &[], true)` issued (one-shot, guarded by
/// `fin_sent`). A FIN therefore can never overtake buffered data.
///
/// ## Reset / stop (B3 — cancellation propagation, NO reset→clean-FIN bug)
///
/// `Err(StreamReset(code))` from `stream_recv` (peer reset its send side)
/// is RELAYED onward: a RESET_STREAM is emitted toward `dst` carrying the
/// same `code` (`dst.stream_shutdown(sid, Shutdown::Write, code)`).
/// `Err(StreamStopped(code))` from `stream_send` (peer STOP_SENDING'd our
/// send side) is RELAYED back toward `src` (`src.stream_shutdown(sid,
/// Shutdown::Read, code)`). Both then terminate THIS direction by marking
/// it `done` and dropping pending bytes — and **never** synthesise a clean
/// FIN, which would present a truncated transfer as complete (the F-MD-4
/// smuggling bug). Only the affected unidirectional half is torn down (a
/// bidi stream's reverse direction stays live). Propagation goes through
/// [`RelayHalf::propagate_cancel`] (idempotent; `stream_shutdown` errors
/// are logged + swallowed). A GENERIC (non-reset/stop) `stream_recv` /
/// `stream_send` error fails safe — half dropped, no FIN — but does NOT
/// synthesise a reset toward the peer (a generic fault is not a peer
/// cancellation with a meaningful app code and usually accompanies a
/// connection teardown).
fn pump_dir(
    sid: u64,
    src: &mut quiche::Connection,
    dst: &mut quiche::Connection,
    half: &mut RelayHalf,
    dir: Direction,
) {
    if half.done {
        return;
    }

    // ── Read gate: pull from src only while pending is below the window AND
    // the source FIN has not yet been observed. Loop so a burst is moved into
    // pending in one turn (still capped).
    //
    // CF-S16-RELAY-STALL: once the source FIN is read, quiche has COLLECTED the
    // stream (`stream.is_complete()`); re-issuing `stream_recv` on a collected
    // stream returns `Err(InvalidStreamState)`, which the generic read-error arm
    // below would treat as a fault and DROP the still-pending tail + the FIN.
    // There is nothing more to read after the FIN, so skip the read entirely and
    // let the pending tail drain + the deferred FIN forward on subsequent turns.
    while !half.src_fin_seen && half.pending.len() < STREAM_RELAY_WINDOW {
        let room = STREAM_RELAY_WINDOW.saturating_sub(half.pending.len());
        // Read at most `room` so pending never exceeds the window in a
        // single recv (the cap is the R8 bound).
        let mut buf = vec![0u8; room.min(MAX_RELAY_READ)];
        match src.stream_recv(sid, &mut buf) {
            Ok((n, fin)) => {
                half.pending.extend_from_slice(buf.get(..n).unwrap_or(&[]));
                if fin {
                    half.src_fin_seen = true;
                }
                if fin || n == 0 {
                    // FIN reached, or a spurious empty read — stop the
                    // read loop (a `(0,true)` means drained-at-FIN).
                    break;
                }
            }
            Err(quiche::Error::Done) => break,
            // B3: peer RESET_STREAM on its send side. The transfer is
            // TRUNCATED — must NOT become a clean FIN on `dst` (F-MD-4
            // smuggling guard). PROPAGATE the reset onward: emit a
            // RESET_STREAM toward `dst` carrying the SAME `code`
            // (`Shutdown::Write` ⇒ RESET_STREAM), then drop this half
            // without a FIN. Only THIS unidirectional half is torn down;
            // the reverse-direction half on this bidi stream stays live.
            Err(quiche::Error::StreamReset(code)) => {
                tracing::debug!(
                    stream_id = sid,
                    code,
                    dir = dir.as_str(),
                    "Mode B B3: src RESET_STREAM; propagating RESET_STREAM to dst \
                     (same code) — never a clean FIN"
                );
                half.propagate_cancel(dst, sid, code, quiche::Shutdown::Write, dir);
                return;
            }
            // Generic read error (NOT a peer RESET_STREAM). Fail safe:
            // drop this half WITHOUT a clean FIN. We deliberately do NOT
            // synthesise a reset toward `dst` here — a generic `stream_recv`
            // fault is not a peer cancellation with a meaningful app code,
            // and most such errors (`InvalidStreamState` on an
            // already-closed/unknown stream, etc.) mean `dst` is already
            // being torn down by the surrounding connection close. The
            // smuggling guard (no FIN) is what matters; the catch-all
            // reset/stop arms below cover the real cancellation cases.
            Err(e) => {
                tracing::debug!(
                    stream_id = sid, dir = dir.as_str(), error = %e,
                    "Mode B B3: src stream_recv error (not a reset); dropping relay \
                     half without a FIN (no synthetic reset for a generic fault)"
                );
                half.pending.clear();
                half.done = true;
                return;
            }
        }
    }

    // ── Drain pending into dst, front-first (preserve order, no drop).
    let mut accepted = 0usize;
    while accepted < half.pending.len() {
        let chunk = half.pending.get(accepted..).unwrap_or(&[]);
        match dst.stream_send(sid, chunk, false) {
            Ok(0) | Err(quiche::Error::Done) => break,
            Ok(n) => {
                accepted = accepted.saturating_add(n);
                if n < chunk.len() {
                    // Short write: dst flow-control / send buffer full.
                    break;
                }
            }
            // New mirror stream cannot be opened yet (peer MAX_STREAMS
            // not granted). Keep the bytes pending and retry next turn —
            // stream-grant backpressure, never a drop.
            Err(quiche::Error::StreamLimit) => {
                tracing::trace!(
                    stream_id = sid,
                    dir = dir.as_str(),
                    "Mode B B2: dst StreamLimit; holding pending bytes for retry"
                );
                break;
            }
            // B3: peer STOP_SENDING on the stream we are writing. The peer
            // asked us to stop producing on this direction. PROPAGATE it
            // back toward `src`: emit a STOP_SENDING toward `src` carrying
            // the SAME `code` (`Shutdown::Read` ⇒ STOP_SENDING) so the
            // source stops producing, then drop this half without a FIN
            // (smuggling guard). Only THIS unidirectional half is torn
            // down; the reverse-direction half stays live.
            Err(quiche::Error::StreamStopped(code)) => {
                tracing::debug!(
                    stream_id = sid,
                    code,
                    dir = dir.as_str(),
                    "Mode B B3: dst STOP_SENDING; propagating STOP_SENDING to src \
                     (same code) — never a clean FIN"
                );
                half.propagate_cancel(src, sid, code, quiche::Shutdown::Read, dir);
                return;
            }
            // Generic write error (NOT a peer STOP_SENDING). Fail safe:
            // drop this half WITHOUT a clean FIN, no synthetic reset (same
            // rationale as the read-side generic arm above — a generic
            // `stream_send` fault is not a peer cancellation with a
            // meaningful code, and usually means `src`/the connection is
            // already tearing down).
            Err(e) => {
                tracing::debug!(
                    stream_id = sid, dir = dir.as_str(), error = %e,
                    "Mode B B3: dst stream_send error (not a stop); dropping relay \
                     half without a FIN (no synthetic reset for a generic fault)"
                );
                half.pending.clear();
                half.done = true;
                return;
            }
        }
    }
    // Drop the accepted prefix; the unsent tail (if any) stays pending in
    // order for the next turn (backpressure carry-over).
    if accepted > 0 {
        half.pending.drain(..accepted.min(half.pending.len()));
    }

    // ── FIN: only after ALL pending bytes are accepted by dst.
    if half.src_fin_seen && half.pending.is_empty() && !half.fin_sent {
        match dst.stream_send(sid, &[], true) {
            Ok(_) | Err(quiche::Error::Done) => {
                half.fin_sent = true;
                half.done = true;
            }
            // The mirror stream cannot be OPENED yet (peer MAX_STREAMS
            // not granted) — reachable for a zero-data FIN-only stream
            // whose first `stream_send` is this empty-FIN send. Do NOT
            // mark `done`/`fin_sent`: leave the half live so the next
            // relay turn retries the FIN once the peer grants stream
            // credit (the 2ms tick stays alive while the stream is
            // tracked). Dropping it here would silently lose the FIN and
            // the mirror stream would never be created/finished. Mirrors
            // the drain block's `StreamLimit` carry-over.
            Err(quiche::Error::StreamLimit) => {
                tracing::trace!(
                    stream_id = sid,
                    dir = dir.as_str(),
                    "Mode B B2: dst StreamLimit on FIN-only stream; retrying FIN next turn"
                );
            }
            // B3: dst STOP_SENDING on the FIN itself — the peer cancelled
            // its read side as we were about to clean-close. Propagate the
            // STOP_SENDING back toward `src` (same code) so the source
            // stops; the half is terminal anyway. (`pending` is already
            // empty here — this arm is only reached once drained.)
            Err(quiche::Error::StreamStopped(code)) => {
                tracing::debug!(
                    stream_id = sid,
                    code,
                    dir = dir.as_str(),
                    "Mode B B3: dst STOP_SENDING on FIN; propagating STOP_SENDING to src"
                );
                half.propagate_cancel(src, sid, code, quiche::Shutdown::Read, dir);
            }
            Err(e) => {
                tracing::debug!(
                    stream_id = sid, dir = dir.as_str(), error = %e,
                    "Mode B B3: dst stream_send FIN error; closing relay half"
                );
                half.done = true;
            }
        }
    }
}

/// Largest single `stream_recv` read in one [`pump_dir`] iteration. The
/// read loop is still capped by [`STREAM_RELAY_WINDOW`] (the R8 bound);
/// this just bounds the per-call scratch allocation (one UDP-payload-class
/// buffer) rather than allocating up to a full window per read.
const MAX_RELAY_READ: usize = 16 * 1024;

/// Emit an application `CONNECTION_CLOSE` ([`RAW_NO_ERROR`]) and pump the
/// connection until quiche reports closed or [`GRACEFUL_CLOSE_BUDGET`]
/// elapses. Idempotent: `close()` on an already-closed connection
/// returns `Error::Done`, treated as a no-op. Mirrors the H3 actor's
/// [`crate::conn_actor::graceful_h3_shutdown`] (raw application code `0`
/// instead of the HTTP/3 `H3_NO_ERROR`).
async fn graceful_close(conn: &mut quiche::Connection, socket: &UdpSocket, out_buf: &mut [u8]) {
    match conn.close(true, RAW_NO_ERROR, b"shutdown") {
        Ok(()) | Err(quiche::Error::Done) => {}
        Err(e) => tracing::debug!(error = %e, "Mode B conn.close (graceful_close)"),
    }
    let deadline = tokio::time::Instant::now() + GRACEFUL_CLOSE_BUDGET;
    loop {
        drain_conn_send(socket, conn, out_buf).await;
        if conn.is_closed() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::debug!("Mode B graceful_close: budget exhausted before is_closed()");
            return;
        }
        let quiche_timeout = conn.timeout().unwrap_or(Duration::from_millis(10));
        let residual = deadline.saturating_duration_since(tokio::time::Instant::now());
        let wait = quiche_timeout.min(residual);
        tokio::time::sleep(wait).await;
        conn.on_timeout();
    }
}

#[cfg(test)]
mod tests {
    //! Deterministic, socket-free unit coverage for the [`pump_dir`]
    //! FIN-retry logic (the B2-review defect: `StreamLimit` on the
    //! zero-data FIN-only `stream_send` must NOT drop the FIN — the half
    //! stays live and retries once the peer grants stream credit).
    //!
    //! These drive a REAL pair of `quiche::Connection`s but pump packets
    //! in-memory (no UDP), so the MAX_STREAMS limit is enforced exactly by
    //! quiche with no timing coupling. The full open-then-grant
    //! INTEGRATION path (a live wire transfer that exhausts then re-opens
    //! the upstream stream credit) is the VERIFIER's bar — here we prove
    //! the unit-level branch: refuse ⇒ retryable (not dropped); credit ⇒
    //! delivered (peer observes the stream finished).

    use super::{
        BoundedDgramQueue, DGRAM_QUEUE_CAP, DgramPushOutcome, Direction, MAX_DGRAM_SIZE,
        MAX_RELAY_STREAMS, RawStreamState, RelayHalf, admit_or_refuse, pump_dgram_dir, pump_dir,
        relay_streams,
    };

    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    const TEST_SNI: &str = "expressgateway.test";
    const ALPN: &[u8] = b"raw-b2";

    static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TestCerts {
        dir: PathBuf,
        cert: PathBuf,
        key: PathBuf,
        ca: PathBuf,
    }

    impl Drop for TestCerts {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn gen_certs() -> TestCerts {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "lb-quic-s16-b2-finretry-{}-{nanos}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut params = rcgen::CertificateParams::new(vec![TEST_SNI.to_string()]).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params
            .extended_key_usages
            .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        let ca_path = dir.join("ca.pem");
        std::fs::write(&cert_path, cert.pem().as_bytes()).unwrap();
        std::fs::write(&key_path, key_pair.serialize_pem().as_bytes()).unwrap();
        std::fs::write(&ca_path, cert.pem().as_bytes()).unwrap();
        TestCerts {
            dir,
            cert: cert_path,
            key: key_path,
            ca: ca_path,
        }
    }

    fn random_scid() -> [u8; quiche::MAX_CONN_ID_LEN] {
        use ring::rand::SecureRandom;
        let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
        ring::rand::SystemRandom::new().fill(&mut scid).unwrap();
        scid
    }

    /// Server (= the LB's upstream PEER) config. `bidi_limit` is the
    /// number of client-initiated bidi streams it grants the LB-as-client
    /// — set it to 0 to force `StreamLimit` on the first
    /// client-initiated stream open.
    fn server_config(certs: &TestCerts, bidi_limit: u64) -> quiche::Config {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        cfg.set_application_protos(&[ALPN]).unwrap();
        cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
            .unwrap();
        cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
            .unwrap();
        cfg.set_max_idle_timeout(5_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(bidi_limit);
        cfg.set_initial_max_streams_uni(2);
        cfg.set_disable_active_migration(true);
        cfg
    }

    /// Client (= the LB-as-client on the upstream leg, i.e. the relay
    /// `dst` for the client→upstream direction) config.
    fn client_config(certs: &TestCerts) -> quiche::Config {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        cfg.set_application_protos(&[ALPN]).unwrap();
        cfg.load_verify_locations_from_file(certs.ca.to_str().unwrap())
            .unwrap();
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(5_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(8);
        cfg.set_initial_max_streams_uni(2);
        cfg.set_disable_active_migration(true);
        cfg
    }

    /// Drive a `connect` ⇄ `accept` pair to established entirely
    /// in-memory (no sockets): ferry each side's `send()` output into the
    /// other's `recv()` until both report established. Deterministic.
    fn handshake_pair(
        client: &mut quiche::Connection,
        server: &mut quiche::Connection,
        client_addr: SocketAddr,
        server_addr: SocketAddr,
    ) {
        let mut buf = vec![0u8; 65_535];
        for _ in 0..64 {
            if client.is_established() && server.is_established() {
                return;
            }
            // client -> server
            loop {
                match client.send(&mut buf) {
                    Ok((n, _info)) => {
                        let info = quiche::RecvInfo {
                            from: client_addr,
                            to: server_addr,
                        };
                        let slice = buf.get_mut(..n).unwrap_or(&mut []);
                        let _ = server.recv(slice, info);
                    }
                    Err(quiche::Error::Done) => break,
                    Err(e) => panic!("client.send: {e:?}"),
                }
            }
            // server -> client
            loop {
                match server.send(&mut buf) {
                    Ok((n, _info)) => {
                        let info = quiche::RecvInfo {
                            from: server_addr,
                            to: client_addr,
                        };
                        let slice = buf.get_mut(..n).unwrap_or(&mut []);
                        let _ = client.recv(slice, info);
                    }
                    Err(quiche::Error::Done) => break,
                    Err(e) => panic!("server.send: {e:?}"),
                }
            }
        }
        assert!(
            client.is_established() && server.is_established(),
            "in-memory handshake did not establish"
        );
    }

    /// Ferry packets BOTH directions one round (no FIN/stream work),
    /// so a control frame (e.g. MAX_STREAMS / the FIN STREAM frame) is
    /// delivered to the peer for it to observe.
    fn pump_once(
        a: &mut quiche::Connection,
        b: &mut quiche::Connection,
        a_addr: SocketAddr,
        b_addr: SocketAddr,
    ) {
        let mut buf = vec![0u8; 65_535];
        loop {
            match a.send(&mut buf) {
                Ok((n, _)) => {
                    let info = quiche::RecvInfo {
                        from: a_addr,
                        to: b_addr,
                    };
                    let _ = b.recv(buf.get_mut(..n).unwrap_or(&mut []), info);
                }
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }
        loop {
            match b.send(&mut buf) {
                Ok((n, _)) => {
                    let info = quiche::RecvInfo {
                        from: b_addr,
                        to: a_addr,
                    };
                    let _ = a.recv(buf.get_mut(..n).unwrap_or(&mut []), info);
                }
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }
    }

    fn addrs() -> (SocketAddr, SocketAddr) {
        (
            "127.0.0.1:4001".parse().unwrap(),
            "127.0.0.1:4002".parse().unwrap(),
        )
    }

    fn established_pair(
        certs: &TestCerts,
        server_bidi_limit: u64,
    ) -> (
        quiche::Connection,
        quiche::Connection,
        SocketAddr,
        SocketAddr,
    ) {
        let (caddr, saddr) = addrs();
        let mut ccfg = client_config(certs);
        let mut scfg = server_config(certs, server_bidi_limit);
        let cscid = random_scid();
        let sscid = random_scid();
        let mut client = quiche::connect(
            Some(TEST_SNI),
            &quiche::ConnectionId::from_ref(&cscid),
            caddr,
            saddr,
            &mut ccfg,
        )
        .unwrap();
        let mut server = quiche::accept(
            &quiche::ConnectionId::from_ref(&sscid),
            None,
            saddr,
            caddr,
            &mut scfg,
        )
        .unwrap();
        handshake_pair(&mut client, &mut server, caddr, saddr);
        (client, server, caddr, saddr)
    }

    /// Build a realistic relay `src` for stream 0: a server conn whose
    /// peer (a client) opened stream 0 and sent a zero-data FIN. The
    /// returned server conn therefore has stream 0 readable-at-FIN, so
    /// the relay's read loop reads `(0, true)` then `Done` — exactly the
    /// FIN-only-source shape that drives `pump_dir`'s FIN-emit block.
    /// (The peer client is returned too so it stays alive / owned.)
    fn src_server_with_fin_only_stream0(
        certs: &TestCerts,
    ) -> (quiche::Connection, quiche::Connection) {
        let caddr: SocketAddr = "127.0.0.1:5001".parse().unwrap();
        let saddr: SocketAddr = "127.0.0.1:5002".parse().unwrap();
        let mut ccfg = client_config(certs);
        let mut scfg = server_config(certs, 4);
        let cscid = random_scid();
        let sscid = random_scid();
        let mut peer_client = quiche::connect(
            Some(TEST_SNI),
            &quiche::ConnectionId::from_ref(&cscid),
            caddr,
            saddr,
            &mut ccfg,
        )
        .unwrap();
        let mut src = quiche::accept(
            &quiche::ConnectionId::from_ref(&sscid),
            None,
            saddr,
            caddr,
            &mut scfg,
        )
        .unwrap();
        handshake_pair(&mut peer_client, &mut src, caddr, saddr);
        // Peer client opens stream 0 with a zero-data FIN, then ferry it
        // to `src` so `src` sees stream 0 finished/readable-at-FIN.
        peer_client.stream_send(0, &[], true).unwrap();
        pump_once(&mut peer_client, &mut src, caddr, saddr);
        assert!(
            src.readable().any(|s| s == 0) || src.stream_finished(0),
            "fixture: src must observe the FIN-only stream 0"
        );
        (src, peer_client)
    }

    /// THE DEFECT REGRESSION (refuse leg): a zero-data FIN-only stream
    /// whose mirror open on `dst` is refused with `StreamLimit` MUST NOT
    /// drop the FIN — `pump_dir` leaves the half live (`!done`,
    /// `!fin_sent`, `src_fin_seen`) so a later turn retries. Pre-fix this
    /// fell into the FIN block's catch-all `Err` arm and set `done =
    /// true`, silently losing the FIN (and never creating the mirror
    /// stream).
    #[test]
    fn fin_only_stream_limit_does_not_drop_fin() {
        let certs = gen_certs();
        // Realistic FIN-only source: stream 0 is at FIN on `src`.
        let (mut src, _peer) = src_server_with_fin_only_stream0(&certs);
        // `dst` = LB-as-client whose backend peer grants ZERO bidi
        // streams ⇒ the empty-FIN open of stream 0 returns StreamLimit.
        let (mut dst, _backend, _caddr, _saddr) = established_pair(&certs, 0);
        assert_eq!(
            dst.peer_streams_left_bidi(),
            0,
            "fixture: peer must grant zero bidi streams so the open is refused"
        );

        let mut half = RelayHalf::default();
        pump_dir(
            0,
            &mut src,
            &mut dst,
            &mut half,
            Direction::ClientToUpstream,
        );

        // The read loop saw the source FIN…
        assert!(
            half.src_fin_seen,
            "the relay must have observed the source FIN (intent recorded)"
        );
        // …but the StreamLimit-refused FIN must NOT terminate the half.
        assert!(
            !half.done,
            "StreamLimit on a FIN-only send must NOT mark the half done \
             (the FIN must be retried, not dropped)"
        );
        assert!(
            !half.fin_sent,
            "the FIN was refused (StreamLimit) so fin_sent must stay false"
        );
    }

    /// THE DEFECT REGRESSION (grant leg / retry succeeds): with stream
    /// credit available the SAME FIN-only `pump_dir` delivers a clean FIN
    /// and the destination peer observes stream 0 finished. Together with
    /// the refuse-leg test this proves the retry is real — a half left
    /// live by `StreamLimit` completes once credit exists.
    #[test]
    fn fin_only_delivered_when_stream_credit_available() {
        let certs = gen_certs();
        let (mut src, _peer) = src_server_with_fin_only_stream0(&certs);
        // `dst` = LB-as-client; backend grants >=1 bidi stream.
        let (mut dst, mut backend, caddr, saddr) = established_pair(&certs, 4);
        assert!(
            dst.peer_streams_left_bidi() >= 1,
            "fixture: peer must grant bidi credit for this leg"
        );

        let mut half = RelayHalf::default();
        pump_dir(
            0,
            &mut src,
            &mut dst,
            &mut half,
            Direction::ClientToUpstream,
        );

        assert!(
            half.fin_sent && half.done,
            "with stream credit the FIN-only send must succeed (fin_sent + done)"
        );

        // Deliver the FIN STREAM frame to the backend and confirm IT
        // observes stream 0 finished (the FIN was not lost). Here `dst`
        // is the LB-as-client, `backend` is the server peer.
        pump_once(&mut dst, &mut backend, caddr, saddr);
        assert!(
            backend.stream_finished(0),
            "the backend must observe stream 0 finished (clean FIN delivered)"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // CF-S16-RELAY-STALL regression (post-FIN re-read drop)
    // ─────────────────────────────────────────────────────────────────────

    /// Server (the LB's upstream PEER) config with a CUSTOM, deliberately
    /// SMALL `initial_max_stream_data_bidi_remote`. When the relay's `dst`
    /// (the LB-as-client) sends on a client-initiated bidi stream, the
    /// backend's `bidi_remote` limit caps how much it accepts — set it tiny
    /// to force `dst.stream_send` to SHORT-WRITE deterministically, leaving a
    /// pending tail in the relay half. (`initial_max_data` is left generous
    /// so the per-stream limit, not the conn limit, is the binding one.)
    fn server_config_small_stream_window(
        certs: &TestCerts,
        bidi_remote_window: u64,
    ) -> quiche::Config {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        cfg.set_application_protos(&[ALPN]).unwrap();
        cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
            .unwrap();
        cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
            .unwrap();
        cfg.set_max_idle_timeout(5_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
        // The binding limit: how much the relay's `dst` may push onto the
        // client-initiated bidi stream before the backend backpressures.
        cfg.set_initial_max_stream_data_bidi_remote(bidi_remote_window);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(4);
        cfg.set_initial_max_streams_uni(2);
        cfg.set_disable_active_migration(true);
        cfg
    }

    /// Like [`established_pair`] but the SERVER peer (`backend`) advertises a
    /// small `initial_max_stream_data_bidi_remote`, so the relay's `dst`
    /// (the returned client conn) short-writes on its first stream send.
    fn established_pair_small_dst_window(
        certs: &TestCerts,
        bidi_remote_window: u64,
    ) -> (
        quiche::Connection,
        quiche::Connection,
        SocketAddr,
        SocketAddr,
    ) {
        let (caddr, saddr) = addrs();
        let mut ccfg = client_config(certs);
        let mut scfg = server_config_small_stream_window(certs, bidi_remote_window);
        let cscid = random_scid();
        let sscid = random_scid();
        let mut client = quiche::connect(
            Some(TEST_SNI),
            &quiche::ConnectionId::from_ref(&cscid),
            caddr,
            saddr,
            &mut ccfg,
        )
        .unwrap();
        let mut server = quiche::accept(
            &quiche::ConnectionId::from_ref(&sscid),
            None,
            saddr,
            caddr,
            &mut scfg,
        )
        .unwrap();
        handshake_pair(&mut client, &mut server, caddr, saddr);
        (client, server, caddr, saddr)
    }

    /// Build a relay `src` for stream 0 carrying `payload` followed by a FIN:
    /// a server conn whose peer (a client) opened stream 0, wrote the full
    /// payload, and FIN'd it, then ferried it across. The returned `src`
    /// therefore has stream 0 readable with the whole payload AND the FIN
    /// available in a single drain — so a single `pump_dir` read pulls
    /// `(payload.len(), fin=true)`, which makes quiche COLLECT the stream
    /// (`stream.is_complete()`). The peer client is returned so it stays
    /// alive / owned. (`payload` must fit the 64 KiB per-stream window.)
    fn src_server_with_payload_fin_stream0(
        certs: &TestCerts,
        payload: &[u8],
    ) -> (quiche::Connection, quiche::Connection) {
        let caddr: SocketAddr = "127.0.0.1:5101".parse().unwrap();
        let saddr: SocketAddr = "127.0.0.1:5102".parse().unwrap();
        let mut ccfg = client_config(certs);
        let mut scfg = server_config(certs, 4);
        let cscid = random_scid();
        let sscid = random_scid();
        let mut peer_client = quiche::connect(
            Some(TEST_SNI),
            &quiche::ConnectionId::from_ref(&cscid),
            caddr,
            saddr,
            &mut ccfg,
        )
        .unwrap();
        let mut src = quiche::accept(
            &quiche::ConnectionId::from_ref(&sscid),
            None,
            saddr,
            caddr,
            &mut scfg,
        )
        .unwrap();
        handshake_pair(&mut peer_client, &mut src, caddr, saddr);
        // Peer client writes the full payload + FIN on stream 0 in one shot,
        // then ferry it to `src`.
        let sent = peer_client.stream_send(0, payload, true).unwrap();
        assert_eq!(
            sent,
            payload.len(),
            "fixture: the whole payload must fit the peer's stream window"
        );
        pump_once(&mut peer_client, &mut src, caddr, saddr);
        assert!(
            src.readable().any(|s| s == 0),
            "fixture: src must observe stream 0 readable with the payload+FIN"
        );
        (src, peer_client)
    }

    /// CF-S16-RELAY-STALL — THE post-FIN re-read drop regression.
    ///
    /// Reproduces the proven defect deterministically (Phase-1 diag): a
    /// CLEANLY-FINISHED source stream whose drain into `dst` SHORT-WRITES on
    /// the FIN-carrying turn (leaving a pending tail) must NOT have that tail
    /// + FIN dropped by a spurious post-FIN re-read.
    ///
    /// Mechanism this drives: turn 1's `pump_dir` reads the full payload +
    /// FIN in a single `stream_recv` (so `src_fin_seen=true` AND quiche
    /// COLLECTS the source stream), but the drain into `dst` short-writes
    /// against the backend's tiny per-stream window ⇒ `half.pending` is
    /// non-empty ⇒ the FIN-forward block is (correctly) skipped. Turn 2's
    /// read gate is where the bug lived: PRE-FIX it re-issued `stream_recv`
    /// on the now-collected source, hit `Err(InvalidStreamState)`, and the
    /// generic read-error arm ran `pending.clear(); done = true` — DROPPING
    /// the tail and the FIN. POST-FIX the read gate is short-circuited by
    /// `!half.src_fin_seen`, so turn 2 only drains; once the dst window is
    /// opened the full byte-identical payload + a clean FIN are delivered.
    ///
    /// Load-bearing: with the one-line fix reverted, this test FAILS (the
    /// tail is dropped: `half.done` true via drop, `fin_sent` false, the
    /// backend never sees the full payload / FIN). It PASSES only with the
    /// `!half.src_fin_seen` read-gate condition in place.
    #[test]
    fn post_fin_short_write_reread_does_not_drop_tail() {
        let certs = gen_certs();

        // A multi-KiB payload (> the backend's tiny stream window below, so
        // the first drain CANNOT clear it). Kept comfortably under the src
        // peer's per-stream window so the whole payload + FIN is buffered in
        // a single `stream_send` (the peer's effective initial credit after
        // the handshake is < 64 KiB). Distinct byte pattern so a
        // dropped/duplicated/reordered tail is caught by a byte-exact check.
        let payload: Vec<u8> = (0..10_240u32).map(|i| (i % 251) as u8).collect();

        // `src` = client-leg conn with stream 0 = payload + FIN, collected on
        // the read.
        let (mut src, mut peer) = src_server_with_payload_fin_stream0(&certs, &payload);
        let (src_caddr, src_saddr): (SocketAddr, SocketAddr) = (
            "127.0.0.1:5101".parse().unwrap(),
            "127.0.0.1:5102".parse().unwrap(),
        );

        // `dst` = LB-as-client whose backend grants a TINY per-stream window
        // (4 KiB) so the relay's `dst.stream_send` short-writes, leaving a
        // pending tail with `src_fin_seen` already true.
        let dst_window: u64 = 4 * 1024;
        let (mut dst, mut backend, caddr, saddr) =
            established_pair_small_dst_window(&certs, dst_window);

        let mut half = RelayHalf::default();

        // ── Turn 1: read payload+FIN (collects src), drain short-writes.
        pump_dir(
            0,
            &mut src,
            &mut dst,
            &mut half,
            Direction::UpstreamToClient,
        );
        assert!(
            half.src_fin_seen,
            "turn 1 must read the source FIN (it carried payload+FIN in one recv)"
        );
        assert!(
            !half.pending.is_empty(),
            "turn 1's drain must SHORT-WRITE against the tiny dst window, \
             leaving a pending tail (the precondition for the bug)"
        );
        assert!(
            !half.fin_sent,
            "the FIN must NOT be forwarded while a tail is still pending"
        );
        assert!(
            !half.done,
            "the half must still be live after turn 1 (a tail remains to drain)"
        );

        // Complete the bidi stream 0 on `src` so quiche COLLECTS it (matching
        // the production wire path, where the reverse relay leg's FIN finishes
        // the send side). `src`'s recv side is already finished (read the FIN
        // on turn 1); finishing its SEND side makes the stream `is_complete()`,
        // so the next `stream_recv(0)` returns `InvalidStreamState` — the exact
        // post-collection re-read the bug trips on. The peer client closes its
        // own send+read sides too and is pumped so all FINs/acks settle.
        src.stream_send(0, &[], true).unwrap();
        peer.stream_send(0, &[], true).ok();
        for _ in 0..8 {
            pump_once(&mut src, &mut peer, src_saddr, src_caddr);
        }
        // Drain the peer's recv of stream 0 so its recv side completes too.
        {
            let mut sink = [0u8; 256];
            while let Ok((_n, _fin)) = peer.stream_recv(0, &mut sink) {}
        }
        for _ in 0..8 {
            pump_once(&mut src, &mut peer, src_saddr, src_caddr);
        }
        // Sanity: the source stream is now collected — a direct read would
        // return InvalidStreamState (proven below by the negative control).
        assert!(
            src.stream_finished(0),
            "fixture: src stream 0 must be finished/collected before turn 2 \
             (so the buggy re-read trips InvalidStreamState)"
        );

        // ── Turn 2: THE buggy re-read turn. Pre-fix this re-issues
        // stream_recv on the collected source → InvalidStreamState → the
        // generic arm drops the tail + FIN. Post-fix the read gate is
        // skipped (src_fin_seen) and only the drain runs.
        pump_dir(
            0,
            &mut src,
            &mut dst,
            &mut half,
            Direction::UpstreamToClient,
        );
        assert!(
            !half.done || half.fin_sent,
            "turn 2 must NOT drop the half via a spurious post-FIN re-read \
             (CF-S16-RELAY-STALL): if done, it must be via a clean FIN, not a drop"
        );

        // ── Open the dst window (deliver the backend's flow-control updates)
        // and pump the relay to completion. Each round: drain whatever the
        // backend can read (accumulating into `got` AND freeing per-stream
        // credit so the backend issues MAX_STREAM_DATA), ferry packets so
        // that credit reaches `dst`, then drive `pump_dir` to drain the
        // carried-over tail and finally forward the FIN.
        let mut got = Vec::new();
        let mut backend_fin = false;
        let mut sink = vec![0u8; 65_535];
        for _ in 0..128 {
            // Deliver any pending relay output to the backend first.
            pump_once(&mut dst, &mut backend, caddr, saddr);
            // Drain the backend stream (accumulate + free credit).
            loop {
                match backend.stream_recv(0, &mut sink) {
                    Ok((n, fin)) => {
                        got.extend_from_slice(sink.get(..n).unwrap_or(&[]));
                        backend_fin |= fin;
                    }
                    Err(quiche::Error::Done) => break,
                    Err(_) => break,
                }
            }
            // Ferry the freed credit (MAX_STREAM_DATA) back to `dst`.
            pump_once(&mut dst, &mut backend, caddr, saddr);
            // Drive the relay: drain the carried-over tail, forward the FIN.
            pump_dir(
                0,
                &mut src,
                &mut dst,
                &mut half,
                Direction::UpstreamToClient,
            );
            if half.fin_sent && half.pending.is_empty() && backend_fin {
                // Flush the FIN frame to the backend before stopping.
                pump_once(&mut dst, &mut backend, caddr, saddr);
                break;
            }
        }
        // Final flush + drain so the backend definitely observes the FIN.
        pump_once(&mut dst, &mut backend, caddr, saddr);
        loop {
            match backend.stream_recv(0, &mut sink) {
                Ok((n, fin)) => {
                    got.extend_from_slice(sink.get(..n).unwrap_or(&[]));
                    backend_fin |= fin;
                }
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }

        // ── The tail must NOT have been dropped: a clean FIN was forwarded…
        assert!(
            half.fin_sent,
            "the relay must forward the deferred FIN (tail drained, not dropped) \
             — CF-S16-RELAY-STALL"
        );
        assert!(
            half.pending.is_empty(),
            "no bytes may be left stranded in pending after completion"
        );

        // …and the backend received the FULL, byte-identical payload + FIN.
        assert_eq!(
            got.len(),
            payload.len(),
            "the backend must receive the WHOLE payload (no dropped tail): \
             got {} of {} bytes",
            got.len(),
            payload.len()
        );
        assert_eq!(
            got, payload,
            "the backend must receive the byte-identical payload (order preserved)"
        );
        assert!(
            backend_fin,
            "the backend must observe the FIN on stream 0 (the FIN was forwarded, \
             not dropped) — CF-S16-RELAY-STALL"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // B4 — BoundedDgramQueue unit coverage (the R13(c) seed; the verifier
    // owns the authoritative flood/burst wire tests).
    // ─────────────────────────────────────────────────────────────────────

    /// (a) FIFO ORDER: payloads dequeue front-first in arrival order, with
    /// no reordering and no loss while under capacity.
    #[test]
    fn dgram_queue_preserves_fifo_order() {
        let mut q = BoundedDgramQueue::new(8);
        let payloads: Vec<Vec<u8>> = (0..5u8).map(|i| vec![i, i.wrapping_add(100)]).collect();
        for p in &payloads {
            assert_eq!(
                q.push(p.clone()),
                DgramPushOutcome::Queued,
                "under capacity every push must be Queued"
            );
        }
        assert_eq!(q.len(), payloads.len());
        assert_eq!(q.dropped(), 0, "no drops while under capacity");

        // Dequeue and confirm exact arrival order, front-first.
        for expected in &payloads {
            assert_eq!(
                q.front(),
                Some(expected),
                "front must be the oldest payload"
            );
            assert_eq!(q.pop_front().as_ref(), Some(expected));
        }
        assert!(q.is_empty());
        assert_eq!(q.pop_front(), None, "pop on empty yields None");
    }

    /// (b) DROP-NEWEST NEGATIVE CONTROL (R13(c) seed): push `cap + K` and
    /// assert `len() == cap`, the OLDEST `cap` survived IN ORDER, the K
    /// NEWEST were dropped, and `dropped == K`. An unbounded queue (the
    /// pre-fix shape) would hold all `cap + K` and report `dropped == 0` —
    /// this test fails it. The bounded drop-newest passes.
    #[test]
    fn dgram_queue_drop_newest_negative_control() {
        const CAP: usize = 16;
        const K: usize = 9;
        let mut q = BoundedDgramQueue::new(CAP);

        // Tag each payload by its arrival index so we can prove WHICH ones
        // survived. (Two bytes so the index round-trips even past 255.)
        let mk = |i: usize| -> Vec<u8> { vec![(i & 0xff) as u8, ((i >> 8) & 0xff) as u8] };

        for i in 0..(CAP + K) {
            let outcome = q.push(mk(i));
            if i < CAP {
                assert_eq!(
                    outcome,
                    DgramPushOutcome::Queued,
                    "the first cap pushes fill the queue"
                );
            } else {
                assert_eq!(
                    outcome,
                    DgramPushOutcome::Dropped,
                    "every push past cap is drop-newest"
                );
            }
        }

        // The bound held: never more than cap retained.
        assert_eq!(q.len(), CAP, "len must be clamped to cap (the R8 bound)");
        // Exactly the K newest were dropped.
        assert_eq!(
            q.dropped(),
            K as u64,
            "exactly the K newest arrivals were dropped"
        );

        // The OLDEST cap survived, in order (0..CAP). The newest K
        // (CAP..CAP+K) are gone.
        for i in 0..CAP {
            assert_eq!(
                q.pop_front(),
                Some(mk(i)),
                "the oldest cap payloads survived in arrival order; index {i}"
            );
        }
        assert!(q.is_empty(), "nothing beyond the oldest cap survived");
    }

    /// (c) BINARY / ZERO-LENGTH payloads are preserved VERBATIM (no UTF-8
    /// assumption, no length-implied truncation): a zero-length datagram,
    /// an all-zero-bytes payload, a high-bit non-UTF8 payload, and a near-
    /// MAX_DGRAM_SIZE payload all round-trip byte-identical.
    #[test]
    fn dgram_queue_preserves_binary_and_zero_length_verbatim() {
        let mut q = BoundedDgramQueue::new(8);
        let empty: Vec<u8> = Vec::new();
        let zeros: Vec<u8> = vec![0u8; 64];
        let non_utf8: Vec<u8> = vec![0xff, 0xfe, 0x80, 0x00, 0x7f, 0xc0, 0xff];
        // A large payload exercising verbatim copy of a big buffer.
        let large: Vec<u8> = (0..50_000usize)
            .map(|i| ((i * 37 + 11) % 256) as u8)
            .collect();

        for p in [&empty, &zeros, &non_utf8, &large] {
            assert_eq!(q.push(p.clone()), DgramPushOutcome::Queued);
        }

        assert_eq!(
            q.pop_front().as_ref(),
            Some(&empty),
            "a zero-length datagram is a distinct, preserved payload (empty Vec)"
        );
        assert_eq!(
            q.pop_front().as_ref(),
            Some(&zeros),
            "all-zero bytes preserved verbatim"
        );
        assert_eq!(
            q.pop_front().as_ref(),
            Some(&non_utf8),
            "non-UTF8 bytes preserved verbatim"
        );
        assert_eq!(
            q.pop_front().as_ref(),
            Some(&large),
            "large payload preserved verbatim"
        );
        assert!(q.is_empty());
    }

    /// The production cap constant is the documented R8 bound. Pin it so a
    /// silent change is caught (it is plumbed for the B6 metric default).
    #[test]
    fn dgram_queue_cap_constant_is_documented_default() {
        assert_eq!(
            DGRAM_QUEUE_CAP, 1024,
            "the R8 datagram-queue bound is 1024 (matches quiche default)"
        );
        // The constant is usable as a real cap (a queue built with it
        // accepts up to cap then drops-newest).
        let mut q = BoundedDgramQueue::new(DGRAM_QUEUE_CAP);
        for _ in 0..DGRAM_QUEUE_CAP {
            assert_eq!(q.push(vec![1, 2, 3]), DgramPushOutcome::Queued);
        }
        assert_eq!(q.len(), DGRAM_QUEUE_CAP);
        assert_eq!(
            q.push(vec![4]),
            DgramPushOutcome::Dropped,
            "the cap+1'th push is drop-newest"
        );
        assert_eq!(q.dropped(), 1);
    }

    // ─────────────────────────────────────────────────────────────────────
    // B5 — explicit per-stream relay-table cap (MAX_RELAY_STREAMS).
    // Deterministic, socket-free: a real quiche pair whose grant EXCEEDS the
    // cap opens > cap streams; `relay_streams` must clamp the table to the
    // cap. Plus the load-bearing negative-control seed (without the cap the
    // table would reach the higher opened-count). The verifier owns the
    // authoritative real-wire flood/burst.
    // ─────────────────────────────────────────────────────────────────────

    /// A `quiche::Config` pair granting MANY bidi streams (> the relay cap),
    /// so the relay table can be driven OVER [`MAX_RELAY_STREAMS`] in a unit
    /// test. The server (acting as the relay's `client` arg) grants
    /// `bidi_limit` client-initiated bidi streams; the peer opens that many.
    /// Generous per-stream + conn flow control so every opened stream carries
    /// its byte and becomes readable in one drain.
    fn over_cap_server_config(certs: &TestCerts, bidi_limit: u64) -> quiche::Config {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        cfg.set_application_protos(&[ALPN]).unwrap();
        cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
            .unwrap();
        cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
            .unwrap();
        cfg.set_max_idle_timeout(5_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(8 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(bidi_limit);
        cfg.set_initial_max_streams_uni(2);
        cfg.set_disable_active_migration(true);
        cfg
    }

    fn over_cap_client_config(certs: &TestCerts, bidi_limit: u64) -> quiche::Config {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        cfg.set_application_protos(&[ALPN]).unwrap();
        cfg.load_verify_locations_from_file(certs.ca.to_str().unwrap())
            .unwrap();
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(5_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(8 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(bidi_limit);
        cfg.set_initial_max_streams_uni(2);
        cfg.set_disable_active_migration(true);
        cfg
    }

    /// Establish a peer→server pair where the peer (a client) opens
    /// `open_count` client-initiated bidi streams, each carrying one byte +
    /// FIN, then ferries them so `server` observes all `open_count` streams
    /// readable. `server` is returned to play the relay's `client` arg; the
    /// peer is returned so it stays alive/owned. `bidi_limit` (the grant) must
    /// be >= `open_count`.
    fn server_with_n_readable_streams(
        certs: &TestCerts,
        open_count: u64,
        bidi_limit: u64,
    ) -> (quiche::Connection, quiche::Connection) {
        assert!(
            bidi_limit >= open_count,
            "fixture: the grant must allow opening all requested streams"
        );
        let caddr: SocketAddr = "127.0.0.1:5301".parse().unwrap();
        let saddr: SocketAddr = "127.0.0.1:5302".parse().unwrap();
        let mut ccfg = over_cap_client_config(certs, bidi_limit);
        let mut scfg = over_cap_server_config(certs, bidi_limit);
        let cscid = random_scid();
        let sscid = random_scid();
        let mut peer = quiche::connect(
            Some(TEST_SNI),
            &quiche::ConnectionId::from_ref(&cscid),
            caddr,
            saddr,
            &mut ccfg,
        )
        .unwrap();
        let mut server = quiche::accept(
            &quiche::ConnectionId::from_ref(&sscid),
            None,
            saddr,
            caddr,
            &mut scfg,
        )
        .unwrap();
        handshake_pair(&mut peer, &mut server, caddr, saddr);
        // Open `open_count` client-initiated bidi streams (ids 0,4,8,…), each
        // a single distinct byte + FIN.
        for i in 0..open_count {
            let sid = i * 4; // client-initiated bidi stream ids
            peer.stream_send(sid, &[(i % 251) as u8], true).unwrap();
        }
        // Ferry until the server has all streams readable (a few rounds cover
        // multi-packet fan-out of many small streams).
        for _ in 0..16 {
            pump_once(&mut peer, &mut server, caddr, saddr);
            let readable = server.readable().count() as u64;
            if readable >= open_count {
                break;
            }
        }
        let readable = server.readable().count() as u64;
        assert!(
            readable >= open_count,
            "fixture: server must observe all {open_count} opened streams readable (got {readable})"
        );
        (server, peer)
    }

    /// B5 — THE per-stream cap holds: a peer opens `OPEN > MAX_RELAY_STREAMS`
    /// streams over a grant that EXCEEDS the cap; `relay_streams` must clamp
    /// the relay table to `MAX_RELAY_STREAMS` (never insert the over-cap
    /// sids).
    ///
    /// Load-bearing negative control (seed): the peer opened `OPEN` distinct
    /// streams and the server observes them ALL readable (asserted in the
    /// fixture), so the relay table would reach `OPEN` (> cap) WITHOUT the
    /// `admit_or_refuse` ceiling — the `streams.len() < MAX_RELAY_STREAMS`
    /// gate is the only thing keeping it at the cap. Remove the gate (always
    /// `or_default`) and the final assert below flips from `== cap` to
    /// `== OPEN`. The verifier authors the authoritative cap-removed control.
    #[test]
    fn relay_table_clamped_to_max_relay_streams_under_flood() {
        let certs = gen_certs();
        // Open comfortably above the cap so the clamp is unambiguous.
        let open: u64 = (MAX_RELAY_STREAMS as u64) + 64;
        // Grant strictly more than we open (so quiche is NOT the limiter —
        // the relay-side cap is the one under test).
        let grant: u64 = open + 16;
        let (mut server, _peer) = server_with_n_readable_streams(&certs, open, grant);

        // The relay's `upstream` arg: a quiet established conn with no
        // readable streams of its own (so the readable union is exactly the
        // server's `open` streams). Reuse the standard established pair.
        let (mut upstream, _backend, _ca, _sa) = established_pair(&certs, 4);

        // Pre-condition (the negative-control seed): the server really does
        // have > cap readable streams to offer.
        let server_readable = server.readable().count();
        assert!(
            server_readable as u64 >= open,
            "seed: the source offers {open} readable streams (> the {MAX_RELAY_STREAMS} cap); \
             WITHOUT the cap the table would reach {server_readable}"
        );

        let mut streams: HashMap<u64, RawStreamState> = HashMap::new();
        // Several passes: each pass admits new readable sids up to the cap and
        // pumps. The cap must hold on every pass.
        for _ in 0..4 {
            // B6 (R14/R12): the cap is now a param (single-sourced from
            // config); the B5 proof drives it with the `MAX_RELAY_STREAMS`
            // const default, so the asserted behaviour is byte-identical.
            relay_streams(&mut server, &mut upstream, &mut streams, MAX_RELAY_STREAMS);
            assert!(
                streams.len() <= MAX_RELAY_STREAMS,
                "B5: the relay table must never exceed MAX_RELAY_STREAMS ({MAX_RELAY_STREAMS}); \
                 got {}",
                streams.len()
            );
        }

        // The cap was actually REACHED (not merely under it for some other
        // reason): the source offered > cap streams, so the table filled to
        // exactly the cap. This is what proves the ceiling is load-bearing
        // here (vs. a vacuous "never exceeded" on an empty table).
        assert_eq!(
            streams.len(),
            MAX_RELAY_STREAMS,
            "B5: with > cap streams offered, the table must fill to exactly the cap \
             (the over-cap sids are refused, not inserted)"
        );
    }

    /// B5 — `admit_or_refuse` (the cap gate) directly: an ALREADY-TRACKED sid
    /// is ALWAYS kept even when the table is AT the cap (correctness — the cap
    /// must never drop a live stream mid-transfer), while a genuinely NEW sid
    /// at the cap is REFUSED (not inserted). This is the pure-logic
    /// counterpart to the wire-level clamp test — no quiche needed.
    #[test]
    fn admit_or_refuse_keeps_tracked_refuses_new_at_cap() {
        let mut streams: HashMap<u64, RawStreamState> = HashMap::new();
        // Fill the table to EXACTLY the cap with sids 0..cap. The cap is now
        // a param (single-sourced from config); the proof drives it with the
        // `MAX_RELAY_STREAMS` const default ⇒ byte-identical behaviour.
        for sid in 0..(MAX_RELAY_STREAMS as u64) {
            admit_or_refuse(&mut streams, sid, MAX_RELAY_STREAMS);
        }
        assert_eq!(
            streams.len(),
            MAX_RELAY_STREAMS,
            "the first MAX_RELAY_STREAMS distinct sids fill the table to the cap"
        );

        // (a) An ALREADY-TRACKED sid offered again at the cap is a no-op —
        // kept, table unchanged (NEVER dropped because the table is full).
        let tracked = 7u64;
        assert!(streams.contains_key(&tracked));
        admit_or_refuse(&mut streams, tracked, MAX_RELAY_STREAMS);
        assert_eq!(
            streams.len(),
            MAX_RELAY_STREAMS,
            "re-offering a tracked sid at the cap must not change the table"
        );
        assert!(
            streams.contains_key(&tracked),
            "the cap must NEVER drop an already-tracked (live) stream"
        );

        // (b) A genuinely NEW sid at the cap is REFUSED — not inserted, table
        // size unchanged (the fail-safe bound; only reachable with a
        // mis-configured huge max_streams).
        let fresh = 999_999u64;
        assert!(!streams.contains_key(&fresh));
        admit_or_refuse(&mut streams, fresh, MAX_RELAY_STREAMS);
        assert!(
            !streams.contains_key(&fresh),
            "a new sid over the cap must be REFUSED (not inserted)"
        );
        assert_eq!(
            streams.len(),
            MAX_RELAY_STREAMS,
            "refusing a new over-cap sid must not grow the table (the R8 bound)"
        );
    }

    /// B5 — pin the cap constant + the documented memory-ceiling arithmetic so
    /// a silent change is caught (it is the B6 `max_relay_streams` default).
    #[test]
    fn max_relay_streams_constant_is_documented_default() {
        assert_eq!(
            MAX_RELAY_STREAMS, 256,
            "the B5 relay-table ceiling is 256 (8× the ~32 negotiated grant)"
        );
        // The documented worst-case memory ceiling: cap * 2 dirs * window.
        let ceiling = MAX_RELAY_STREAMS * 2 * super::STREAM_RELAY_WINDOW;
        assert_eq!(
            ceiling,
            128 * 1024 * 1024,
            "documented per-conn relay memory ceiling = 128 MiB \
             (MAX_RELAY_STREAMS * 2 * STREAM_RELAY_WINDOW)"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // B4 — `pump_dgram_dir` REACHABLE defensive send-drain arms (S19 B6
    // coverage close). Deterministic, socket-free: a real established quiche
    // pair drives the send-drain against a `dst` (a) that never negotiated
    // DATAGRAM (`dgram_send` → InvalidState → drain+disable) and (b) whose
    // writable limit is exceeded by a payload (`dgram_send` → BufferTooShort
    // → drop-this-one). The recv-side `BufferTooShort` arm is genuinely
    // unreachable with the MAX_DGRAM_SIZE (65535) recv buffer and is left for
    // the verifier to document. Asserts the BOUNDED outcome (queue drained /
    // payload dropped + counted), not mere "no panic".
    // ─────────────────────────────────────────────────────────────────────

    /// `quiche::Config` for an established pair where DATAGRAM negotiation is
    /// independently switchable per side. `*_dgram = Some(max)` enables
    /// `enable_dgram(true, max, max)`; `None` leaves DATAGRAM OFF (so a local
    /// `dgram_send` returns `InvalidState`).
    fn dgram_pair(
        certs: &TestCerts,
        client_dgram: Option<usize>,
        server_dgram: Option<usize>,
    ) -> (
        quiche::Connection,
        quiche::Connection,
        SocketAddr,
        SocketAddr,
    ) {
        let (caddr, saddr) = (
            "127.0.0.1:6001".parse::<SocketAddr>().unwrap(),
            "127.0.0.1:6002".parse::<SocketAddr>().unwrap(),
        );
        let mut ccfg = client_config(certs);
        if let Some(max) = client_dgram {
            ccfg.enable_dgram(true, max, max);
        }
        let mut scfg = server_config(certs, 4);
        if let Some(max) = server_dgram {
            scfg.enable_dgram(true, max, max);
        }
        let cscid = random_scid();
        let sscid = random_scid();
        let mut client = quiche::connect(
            Some(TEST_SNI),
            &quiche::ConnectionId::from_ref(&cscid),
            caddr,
            saddr,
            &mut ccfg,
        )
        .unwrap();
        let mut server = quiche::accept(
            &quiche::ConnectionId::from_ref(&sscid),
            None,
            saddr,
            caddr,
            &mut scfg,
        )
        .unwrap();
        handshake_pair(&mut client, &mut server, caddr, saddr);
        (client, server, caddr, saddr)
    }

    /// B4 — `dgram_send` InvalidState arm: a `dst` that NEVER negotiated
    /// DATAGRAM cannot forward anything, so the whole queue is drained +
    /// every payload counted as dropped (a non-negotiating peer must not be
    /// able to pin relay memory). Reachable only if mis-wired — negotiation
    /// is a config-time invariant — but the arm exists and must hold.
    #[test]
    fn pump_dgram_dir_invalid_state_drains_and_disables() {
        let certs = gen_certs();
        // quiche: `dgram_send` returns `InvalidState` when the LOCAL side's
        // PEER did not advertise DATAGRAM support (`dgram_max_writable_len()`
        // is `None`). So to make `dst`'s send fail with InvalidState, `dst`'s
        // PEER must have DATAGRAM OFF. Here `dst` = the server conn; its peer
        // is the client conn → give the CLIENT `None`. `src` is irrelevant to
        // the send-drain (it has no queued datagrams ⇒ recv-drain is a no-op),
        // so we reuse the client conn as `src`.
        let (mut src, mut dst, _caddr, _saddr) = dgram_pair(&certs, None, Some(1200));
        // dst's peer never negotiated DATAGRAM ⇒ dst cannot send any datagram.
        assert!(
            dst.dgram_max_writable_len().is_none(),
            "fixture: dst's peer must NOT have negotiated DATAGRAM (⇒ dgram_send InvalidState)"
        );

        // Pre-seed the relay queue with several payloads as if they had been
        // recv-drained from `src` on a prior turn.
        let mut q = BoundedDgramQueue::new(DGRAM_QUEUE_CAP);
        for i in 0..3u8 {
            assert_eq!(q.push(vec![i; 16]), DgramPushOutcome::Queued);
        }
        assert_eq!(q.len(), 3);
        let dropped_before = q.dropped();

        // One relay pass: recv-drain `src` (nothing queued) then send-drain
        // into `dst` → InvalidState → drain + disable.
        pump_dgram_dir(&mut src, &mut dst, &mut q, Direction::ClientToUpstream);

        assert_eq!(
            q.len(),
            0,
            "InvalidState must drain the whole queue (a non-negotiating dst cannot forward)"
        );
        assert_eq!(
            q.dropped(),
            dropped_before + 3,
            "every drained payload must be counted as dropped"
        );
    }

    /// B4 — `dgram_send` BufferTooShort arm: a payload larger than `dst`'s
    /// negotiated writable limit can NEVER be forwarded over this connection,
    /// so it is dropped (and counted) and the send-drain CONTINUES to the
    /// next payload (it must not block the queue forever). Asserts the
    /// oversized payload is dropped while a normal-sized one queued AFTER it
    /// is still delivered (reaches `dst`'s peer).
    #[test]
    fn pump_dgram_dir_buffer_too_short_drops_one_continues() {
        let certs = gen_certs();
        // Both sides negotiate DATAGRAM. `dst` = server; its writable limit
        // is what `dst`'s PEER (the client) advertised.
        let (mut peer_of_dst, mut dst, daddr_peer, daddr_dst) =
            dgram_pair(&certs, Some(1200), Some(1200));
        let max = dst
            .dgram_max_writable_len()
            .expect("fixture: dst negotiated DATAGRAM ⇒ Some writable len");
        assert!(max < MAX_DGRAM_SIZE, "fixture: writable len is bounded");

        // `src` is irrelevant to the send-drain; reuse `peer_of_dst` as the
        // (datagram-free) source — it has nothing queued so the recv-drain is
        // a no-op and we go straight to the send-drain into `dst`.
        let oversized = vec![0xABu8; max + 1]; // > dst writable ⇒ BufferTooShort
        let normal = vec![0xCDu8; max.min(64)]; // fits ⇒ delivered
        let mut q = BoundedDgramQueue::new(DGRAM_QUEUE_CAP);
        assert_eq!(q.push(oversized), DgramPushOutcome::Queued);
        assert_eq!(q.push(normal.clone()), DgramPushOutcome::Queued);
        let dropped_before = q.dropped();

        // Send-drain `dst` (note: src=`peer_of_dst` has no queued datagrams).
        pump_dgram_dir(
            &mut peer_of_dst,
            &mut dst,
            &mut q,
            Direction::UpstreamToClient,
        );

        assert_eq!(
            q.len(),
            0,
            "the oversized payload is dropped and the normal one is accepted ⇒ queue empties"
        );
        assert_eq!(
            q.dropped(),
            dropped_before + 1,
            "exactly the one oversized payload is counted as dropped"
        );

        // The normal payload must actually have reached `dst`'s peer: flush
        // `dst` → recv on `peer_of_dst` → `dgram_recv` yields the bytes.
        let mut buf = vec![0u8; MAX_DGRAM_SIZE];
        loop {
            match dst.send(&mut buf) {
                Ok((n, _)) => {
                    let info = quiche::RecvInfo {
                        from: daddr_dst,
                        to: daddr_peer,
                    };
                    let _ = peer_of_dst.recv(buf.get_mut(..n).unwrap_or(&mut []), info);
                }
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }
        let mut got = vec![0u8; MAX_DGRAM_SIZE];
        let recvd = peer_of_dst
            .dgram_recv(&mut got)
            .expect("dst's peer must receive the normal-sized datagram");
        assert_eq!(
            got.get(..recvd).unwrap_or(&[]),
            normal.as_slice(),
            "the post-oversized normal payload is forwarded byte-identically (send-drain continued)"
        );
    }
}
