//! Mode B — terminate-and-re-originate raw-QUIC proxy actor.
//!
//! Unlike Mode A ([`crate::passthrough`], which routes by Connection ID without
//! decrypting), Mode B **terminates** the client QUIC connection and
//! **re-originates** a fresh, dedicated upstream one mirroring the negotiated
//! ALPN: two [`quiche::Connection`] objects, two SCIDs, two TLS key schedules,
//! bound 1:1 by this actor. NOT a CID bridge.
//!
//! Both connections live in [`run_raw_proxy_actor`] and both pumps run in one
//! `tokio::select!`, so the relay has `&mut` access to both and needs no mutex.
//! [`relay_streams`] copies raw STREAM bytes both ways under an identity
//! stream-ID map with a bounded per-stream window ([`STREAM_RELAY_WINDOW`] —
//! the R8 mechanism): a slow destination keeps the window full, the relay stops
//! reading the source, and quiche stops extending that stream's flow-control
//! window. FIN is propagated only after all buffered bytes drain, and a peer
//! RESET_STREAM / STOP_SENDING is propagated onward (B3) while the affected
//! half is still dropped without a FIN — a truncated transfer must never look
//! complete. [`relay_datagrams`] carries RFC 9221 DATAGRAMs independently.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;

use lb_io::quic_pool::{DedicatedQuic, QuicUpstreamPool};

use crate::conn_actor::{ActorParams, drain_conn_send};

/// Application `CONNECTION_CLOSE` code on graceful shutdown of either raw leg.
/// Mode B proxies raw QUIC with no H3 layer, so unlike
/// [`crate::conn_actor::H3_NO_ERROR`] (`0x0100`, an HTTP/3 code) it is a bare
/// application `0`.
pub const RAW_NO_ERROR: u64 = 0x0000;

/// Budget for pumping the client handshake (Phase 1). Matches the upstream
/// dial budget in [`lb_io::quic_pool`] so neither leg out-waits the other.
const CLIENT_HANDSHAKE_BUDGET: Duration = Duration::from_secs(5);

/// Budget for pumping a connection after `close()` — quiche drains for
/// `3 * PTO`, comfortably under this.
const GRACEFUL_CLOSE_BUDGET: Duration = Duration::from_millis(500);

/// Fallback tick when a connection reports no quiche timeout, so the select
/// loop never parks indefinitely on a connection with no timer armed.
const IDLE_TICK: Duration = Duration::from_millis(100);

/// Construction parameters for a Mode B re-origination. Cheap to [`Clone`], so
/// one configured backend fans out to every per-connection actor.
#[derive(Clone)]
pub struct RawBackend {
    /// The upstream QUIC pool. Mode B uses [`QuicUpstreamPool::dial_dedicated`],
    /// which does NOT pool the result — the actor owns the connection 1:1 —
    /// but the pool owns the dial machinery + `config_factory` (R12).
    pub pool: QuicUpstreamPool,
    /// Resolved upstream backend address to re-originate to.
    pub addr: std::net::SocketAddr,
    /// SNI presented to the upstream on the re-originated handshake.
    pub sni: String,
    /// B4 — per-direction bounded DATAGRAM queue capacity. Single-sourced
    /// with the `enable_dgram` queue length advertised on the wire; defaults
    /// to [`DGRAM_QUEUE_CAP`].
    pub dgram_queue_cap: usize,
    /// B5 — ceiling on the per-connection relay stream table. Gated on here,
    /// not on the bare const, so it is single-sourced with `lb_config`;
    /// defaults to [`MAX_RELAY_STREAMS`].
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
/// two-connections proof handle. Distinct `client_scid` vs `upstream_scid`
/// (and distinct trace_ids) prove two genuinely separate connections with
/// independent key schedules, NOT a CID bridge.
#[derive(Debug, Clone)]
pub struct RawProxyOutcome {
    /// Client-facing connection's source CID bytes.
    pub client_scid: Vec<u8>,
    /// Upstream connection's source CID bytes.
    pub upstream_scid: Vec<u8>,
    /// Client-facing connection's quiche trace id.
    pub client_trace_id: String,
    /// Upstream connection's quiche trace id.
    pub upstream_trace_id: String,
    /// Negotiated ALPN that was mirrored upstream.
    pub negotiated_alpn: Vec<u8>,
}

/// Drive a Mode B (terminate-and-re-originate) raw-QUIC proxy connection.
/// Dispatched from [`crate::conn_actor::run_actor`] when
/// [`ActorParams::raw_quic_backend`] is `Some`.
///
/// # Errors
///
/// Never surfaces an operational fault — like
/// [`run_actor`](crate::conn_actor::run_actor) it logs and swallows them; the
/// `io::Result<()>` shape exists for call-site chaining.
pub async fn run_raw_proxy_actor(params: ActorParams) -> std::io::Result<()> {
    match run_raw_proxy_actor_inner(params).await {
        Ok(_outcome) => Ok(()),
        Err(e) => {
            tracing::warn!(error = %e, "Mode B raw-proxy actor exited with error");
            // Parity with `run_actor`: swallowed after logging so the
            // spawned task's `JoinHandle` is always Ok.
            Ok(())
        }
    }
}

/// Test hook: as [`run_raw_proxy_actor`] but returns the [`RawProxyOutcome`]
/// so the verifier can assert two distinct connections by mechanism. Not used
/// in production.
///
/// # Errors
///
/// Surfaces the dial / handshake / pump error verbatim.
#[cfg(any(test, feature = "test-gauges"))]
pub async fn run_raw_proxy_actor_for_test(params: ActorParams) -> std::io::Result<RawProxyOutcome> {
    run_raw_proxy_actor_inner(params).await
}

/// The fallible core: Phase 1 drives the client handshake and dials the
/// dedicated upstream; Phase 2 runs both pumps until either side finishes.
async fn run_raw_proxy_actor_inner(mut params: ActorParams) -> std::io::Result<RawProxyOutcome> {
    // The seam guarantees `Some`, but the crate denies `unwrap`/`expect`.
    let Some(backend) = params.raw_quic_backend.clone() else {
        return Err(std::io::Error::other(
            "run_raw_proxy_actor invoked without a raw_quic_backend",
        ));
    };

    let mut out_buf = vec![0u8; 65_535];

    // ---- Phase 1: drive the CLIENT-facing connection to established ----
    let established = drive_client_to_established(
        &mut params.conn,
        &params.socket,
        &mut params.inbound,
        &params.cancel,
        &mut out_buf,
    )
    .await;
    if !established {
        graceful_close(&mut params.conn, &params.socket, &mut out_buf).await;
        return Err(std::io::Error::other(
            "Mode B client connection closed before established",
        ));
    }

    // Capture the ALPN BEFORE the dial — the `application_proto()` borrow
    // must not overlap the dial await.
    let negotiated_alpn = params.conn.application_proto().to_vec();
    let client_scid = params.conn.source_id().as_ref().to_vec();
    let client_trace_id = params.conn.trace_id().to_owned();
    tracing::debug!(
        alpn = %String::from_utf8_lossy(&negotiated_alpn),
        client_trace_id = %client_trace_id,
        backend = %backend.addr,
        "Mode B: client established; dialing dedicated upstream"
    );

    // Mirror the negotiated client ALPN upstream. An empty one ⇒ pass `&[]`
    // so the upstream config factory's own ALPN is used.
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

    // B6: the gauge is decremented on EVERY return path by `ActiveConnGuard`,
    // so a graceful close, an early fault or a cancel all restore it.
    let modeb_metrics = params.quic_modeb_metrics.clone();
    if let Some(m) = modeb_metrics.as_ref() {
        m.connections_total.inc();
    }
    let _active_guard = ActiveConnGuard::new(modeb_metrics.clone());

    // ---- Phase 2: both pumps + the B2 raw-STREAM relay, until either leg
    // closes. The two memory bounds are single-sourced from the operator
    // config via `backend`. ----
    run_dual_pump(
        &mut params,
        &mut upstream,
        &mut out_buf,
        modeb_metrics.as_ref(),
        backend.dgram_queue_cap,
        backend.max_relay_streams,
    )
    .await;

    // Both calls are idempotent — a no-op if the leg is already closed.
    graceful_close(&mut params.conn, &params.socket, &mut out_buf).await;
    graceful_close(&mut upstream.conn, &upstream.socket, &mut out_buf).await;

    Ok(outcome)
}

/// B6 — RAII guard for the `quic_modeb_connections` active gauge, so every
/// exit from Phase 2 (close, fault, unwind, cancel) restores it without
/// scattered `dec()` calls. `None` ⇒ a no-op guard.
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

/// Phase 1 pump: drive ONLY the client-facing connection until established.
/// Returns `false` if it closed or the cancel token fired first.
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

/// Phase 2 pump: drive BOTH legs in one `tokio::select!` (biased cancel →
/// client inbound → upstream recv → both timeouts) and run [`relay_streams`]
/// plus [`relay_datagrams`] after every wake, where the H3 actor runs
/// `poll_h3`. The relay both reads newly readable data AND flushes bytes still
/// pending from a previous turn, so a stream backpressured against a full
/// destination resumes the moment that destination frees window.
///
/// While the relay holds any in-flight state the select wait is capped at
/// [`RELAY_TICK`]: quiche's idle timeout can be hundreds of ms and would
/// throttle a mid-transfer stream to a crawl. This does NOT defeat
/// backpressure — [`STREAM_RELAY_WINDOW`] still caps in-flight bytes; we
/// merely poll the gate more often. When idle the loop parks on the real
/// quiche timeout, so there is no busy-spin.
async fn run_dual_pump(
    params: &mut ActorParams,
    upstream: &mut DedicatedQuic,
    out_buf: &mut [u8],
    // B6: the relay bumps metrics ONLY here at the per-pass aggregate level,
    // so `relay_datagrams`/`pump_dir` keep their signatures. `None` ⇒ no-op.
    metrics: Option<&lb_observability::QuicModeBMetrics>,
    // B4 datagram-queue cap, single-sourced from `RawBackend`/`lb_config`.
    dgram_queue_cap: usize,
    // B5 relay-stream-table cap, single-sourced; threaded into
    // `relay_streams` → `admit_or_refuse`.
    max_relay_streams: usize,
) {
    // The upstream leg recv_from's straight off its dedicated socket, so it
    // needs its own inbound buffer (the client side gets owned `Vec`s).
    let mut up_in_buf = vec![0u8; 65_535];
    let upstream_local = upstream.local;

    // B2: the bounded per-stream relay table (R8). An entry lives until BOTH
    // directions are terminally done.
    let mut streams: HashMap<u64, RawStreamState> = HashMap::new();

    // B4: the two bounded drop-newest datagram queues. Datagrams have no FIN,
    // reset or ordering, so they live OUTSIDE the stream table.
    let mut c2u_q = BoundedDgramQueue::new(dgram_queue_cap);
    let mut u2c_q = BoundedDgramQueue::new(dgram_queue_cap);

    // B6: only the DELTA of the queues' monotonic per-lifetime `dropped` is
    // fed into the process-cumulative counter.
    let mut last_dropped_total: u64 = 0;

    loop {
        drain_conn_send(&params.socket, &mut params.conn, out_buf).await;
        drain_conn_send(&upstream.socket, &mut upstream.conn, out_buf).await;

        if params.conn.is_closed() || upstream.conn.is_closed() {
            break;
        }

        let mut client_wait = params.conn.timeout().unwrap_or(IDLE_TICK);
        let mut upstream_wait = upstream.conn.timeout().unwrap_or(IDLE_TICK);
        // While any stream is mid-transfer or a datagram is queued, poll the
        // relay gate often so a backpressured stream resumes promptly AND
        // datagram-only traffic is pumped without waiting out quiche's idle
        // timeout. The bounded window/queue still holds — see fn docs. Fully
        // idle ⇒ fall through to the real timeouts (no busy-spin).
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

        // B2 relay: runs every wake so both freshly readable data AND
        // previously-backpressured pending bytes make progress. Next turn's
        // `drain_conn_send` ships whatever it handed to quiche.
        relay_streams(
            &mut params.conn,
            &mut upstream.conn,
            &mut streams,
            max_relay_streams,
        );

        // B4 relay: forward RFC 9221 DATAGRAMs verbatim both ways. A full
        // queue drops the NEWEST payload (the R8 bound); a payload quiche
        // could not accept this turn stays queued and is retried next wake.
        relay_datagrams(&mut params.conn, &mut upstream.conn, &mut c2u_q, &mut u2c_q);

        // B6 per-pass aggregate: `streams_active` from the post-reclamation
        // table size, and the DELTA of both queues' drop-newest counters
        // (`saturating_*` so no boundary can panic under the no-panic bar).
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

/// Bounded per-stream relay window, in bytes, **per stream per direction**
/// (R8 — the memory-safety mechanism, NOT a body/total cap).
///
/// The relay reads a source stream ONLY while that direction's pending buffer
/// is under this. quiche extends a stream's flow-control window as a side
/// effect of `stream_recv`, so not reading stops the window growing and the
/// *peer* pauses — genuine end-to-end backpressure. 256 KiB is a few BDPs on a
/// short-RTT path while keeping per-stream memory bounded and independent of
/// transfer size; total per-connection memory is
/// `MAX_RELAY_STREAMS * 2 * STREAM_RELAY_WINDOW`.
const STREAM_RELAY_WINDOW: usize = 256 * 1024;

/// B5 — explicit, defense-in-depth CEILING on the per-connection relay stream
/// table, making worst-case per-connection relay memory a hard constant:
/// `MAX_RELAY_STREAMS * 2 * STREAM_RELAY_WINDOW = 256 * 2 * 256 KiB = 128 MiB`
/// (never approached).
///
/// This is NOT the primary bound in practice: quiche's negotiated grant
/// (`initial_max_streams_bidi/uni(16)`) already caps a conforming client at
/// ~32 concurrent streams, and [`relay_streams`] evicts each stream once BOTH
/// directions finish, so the table tracks the *concurrent*, not the *total*,
/// stream count. It exists so the ceiling is INDEPENDENT of the quiche config:
/// were `max_streams` ever mis-set huge, the grant would no longer bound the
/// table and this constant still would. `256` is an 8× margin over the
/// negotiated grant, so a correctly-configured connection never hits it.
///
/// This is the canonical DEFAULT; the runtime value is single-sourced from
/// [`RawBackend::max_relay_streams`]. `pub` so the params layer can use it as
/// the documented fallback and tests can pin it.
pub const MAX_RELAY_STREAMS: usize = 256;

/// Short poll interval while ANY stream is mid-transfer, so a partial or
/// backpressured copy resumes without waiting out quiche's idle timeout. The
/// loop only ticks this fast while there is pending relay work.
const RELAY_TICK: Duration = Duration::from_millis(2);

/// One direction of a relayed raw stream: a BOUNDED pending buffer (the R8
/// bound and the backpressure point) plus FIN/cancellation bookkeeping. The
/// FIN flags ensure a clean end is emitted only AFTER every buffered byte has
/// been accepted — never a FIN ahead of data.
#[derive(Default)]
struct RelayHalf {
    /// Read from the source, not yet accepted by `dst`. Capped at
    /// [`STREAM_RELAY_WINDOW`]: the source is not read while at/over the cap.
    pending: Vec<u8>,
    /// Source returned `fin=true`. The destination FIN is deferred until
    /// `pending` is fully drained.
    src_fin_seen: bool,
    /// A clean FIN was delivered to the destination — terminal.
    fin_sent: bool,
    /// Finished (FIN sent, or dropped with a cancellation propagated — B3).
    /// The entry is reclaimed once both directions are done.
    done: bool,
    /// B3: the application error code once a cancellation has been PROPAGATED
    /// for this half. Records the code and makes the propagation idempotent —
    /// a half is only ever shut down once.
    reset_code: Option<u64>,
}

impl RelayHalf {
    /// B3 — propagate a stream cancellation onto `peer` ONCE and mark this
    /// half terminally done WITHOUT a clean FIN (the smuggling guard: a
    /// truncated transfer must never look complete).
    ///
    /// `dir_for_peer` is COUNTERINTUITIVE in quiche — swapping the arms
    /// silently emits the wrong frame:
    /// * [`quiche::Shutdown::Write`] ⇒ **RESET_STREAM** toward `peer`
    ///   (relaying a source RESET_STREAM onward to `dst`).
    /// * [`quiche::Shutdown::Read`] ⇒ **STOP_SENDING** toward `peer`
    ///   (relaying a destination STOP_SENDING back to `src`).
    ///
    /// Idempotent; `Err(Done)` (that side already gone) counts as success and
    /// any other error is logged and swallowed — never a panic.
    fn propagate_cancel(
        &mut self,
        peer: &mut quiche::Connection,
        sid: u64,
        code: u64,
        dir_for_peer: quiche::Shutdown,
        dir: Direction,
    ) {
        // Explicit idempotency latch: a half can be reset in one direction
        // while we are mid-pass, so `done` alone is not enough.
        if self.reset_code.is_some() {
            self.pending.clear();
            self.done = true;
            return;
        }
        match peer.stream_shutdown(sid, dir_for_peer, code) {
            // Propagated, or the peer was already gone — either way it is
            // (or will be) reflected to the peer.
            Ok(()) | Err(quiche::Error::Done) => {}
            Err(e) => {
                // Do NOT panic: the half is failing anyway and the pump goes on.
                tracing::debug!(
                    stream_id = sid, dir = dir.as_str(), error = %e,
                    "Mode B B3: stream_shutdown while propagating cancellation \
                     (swallowed; half still dropped without a FIN)"
                );
            }
        }
        // Smuggling guard (B2, kept): drop unsent bytes, terminate this half,
        // NEVER a clean FIN.
        self.pending.clear();
        self.reset_code = Some(code);
        self.done = true;
    }
}

/// Bounded per-stream relay state: an identity stream-ID map, so the SAME
/// `sid` indexes both connections. Each direction is an independent
/// [`RelayHalf`], so a B3 cancellation tears down ONLY the affected
/// unidirectional half — a bidi stream's other direction stays live — and the
/// B2 smuggling guard is kept: the half is dropped and **never** given a clean
/// FIN. See the `// B3:` arms in [`pump_dir`].
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

/// B2 — one bidirectional raw-STREAM relay pass. Identity stream-ID mapping:
/// the role-quadrants line up (LB is server to the client, client to the
/// backend), so no translation table is needed.
///
/// The candidate set each turn is `client.readable()` ∪ `upstream.readable()`
/// ∪ every `sid` already tracked — so a stream backpressured last turn, or
/// awaiting a deferred FIN, is revisited and resumes when the destination
/// frees window. `readable()` is a snapshot, so it is re-collected every pass.
///
/// `max_relay_streams` is the B5 ceiling, single-sourced from the operator
/// config; it defaults to [`MAX_RELAY_STREAMS`].
fn relay_streams(
    client: &mut quiche::Connection,
    upstream: &mut quiche::Connection,
    streams: &mut HashMap<u64, RawStreamState>,
    max_relay_streams: usize,
) {
    // Union of readable streams on both legs + every sid with live relay state,
    // de-duped via the state map.
    //
    // B5: a NEW readable sid is admitted only while the table is below the cap;
    // an already-tracked sid is ALWAYS re-processed (correctness — never drop a
    // live stream mid-transfer). Over-cap is only reachable with a
    // mis-configured `max_streams` grant.
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
        pump_dir(
            sid,
            client,
            upstream,
            &mut state.c2u,
            Direction::ClientToUpstream,
        );
        pump_dir(
            sid,
            upstream,
            client,
            &mut state.u2c,
            Direction::UpstreamToClient,
        );
    }

    // Reclaim entries whose BOTH directions are done. This is what keeps the
    // table bounded by the CONCURRENT stream count rather than the total over
    // the connection's life — the load-bearing eviction.
    streams.retain(|_, st| !st.is_complete());
}

/// B5 — admit a NEW relay-stream `sid` iff the table is below
/// `max_relay_streams`; an already-tracked `sid` is left untouched (an
/// explicit `contains_key` short-circuit, so the always-process-existing
/// invariant is unmistakable). Over the cap a NEW sid is REFUSED — a fail-safe
/// ceiling independent of the quiche `max_streams` grant.
fn admit_or_refuse(streams: &mut HashMap<u64, RawStreamState>, sid: u64, max_relay_streams: usize) {
    if streams.contains_key(&sid) {
        // Already tracked: ALWAYS re-processed; no growth, so the cap
        // does not apply.
        return;
    }
    if streams.len() < max_relay_streams {
        streams.entry(sid).or_default();
    } else {
        // Over the ceiling — refuse to track this new sid. Only reachable with
        // a mis-configured huge `max_streams`. `debug!` keeps the log
        // rate-bounded under a flood.
        tracing::debug!(
            stream_id = sid,
            table_len = streams.len(),
            cap = max_relay_streams,
            "Mode B B5: relay stream table at cap; refusing new stream (R8 bound \
             — only reachable with a mis-configured max_streams)"
        );
    }
}

/// B4 — capacity (in datagrams) of ONE [`BoundedDgramQueue`], per direction.
/// The R8 bound for the datagram relay: worst-case memory for one direction is
/// `DGRAM_QUEUE_CAP * MAX_DGRAM_SIZE`, independent of total traffic. `1024`
/// matches quiche's own recv/send-queue default — large enough to absorb a
/// normal burst, small enough that a flooding peer cannot grow memory without
/// bound (over-cap arrivals are drop-newest).
///
/// This is the canonical DEFAULT; the runtime value is single-sourced from
/// [`RawBackend::dgram_queue_cap`]. `pub` so the params layer can use it as
/// the documented fallback and tests can pin it.
pub const DGRAM_QUEUE_CAP: usize = 1024;

/// B4 — scratch size for one `dgram_recv`. `65_535` is the absolute UDP
/// payload ceiling, so `BufferTooShort` is unreachable in practice and that
/// arm is defensive only.
const MAX_DGRAM_SIZE: usize = 65_535;

/// B4 — outcome of a [`BoundedDgramQueue::push`], returned so the recv-drain
/// and the tests observe the drop-newest decision by mechanism rather than by
/// inspecting the counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DgramPushOutcome {
    /// The payload was appended to the back of the queue.
    Queued,
    /// The queue was full; the payload was DISCARDED and `dropped`
    /// incremented (drop-newest).
    Dropped,
}

/// B4 — a bounded FIFO of QUIC DATAGRAM (RFC 9221) payloads with an explicit
/// **drop-newest** full-policy (the R8 bound for the datagram relay). Payloads
/// are stored verbatim; a zero-length datagram is a legitimate, distinct
/// datagram and is preserved as an empty `Vec`.
///
/// At capacity, [`push`](Self::push) DISCARDS the *arriving* payload and
/// increments [`dropped`](Self::dropped), keeping the older queued payloads in
/// order — mirroring quiche's own recv-queue overflow behaviour. Datagrams are
/// unreliable by contract, so there is no retransmission obligation and no
/// ordering to violate. The alternatives would either let a flooding peer grow
/// relay memory without bound (the R8 violation this type prevents) or
/// silently reorder by evicting head-of-line payloads.
struct BoundedDgramQueue {
    /// FIFO of datagram payloads (verbatim bytes, front = oldest).
    q: VecDeque<Vec<u8>>,
    /// Maximum queued payloads — the R8 bound. A `push` at this length drops
    /// the newest.
    cap: usize,
    /// Drop-newest events over this queue's lifetime, surfaced for the B6
    /// metric. `saturating_add` to honour the crate's no-panic bar.
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

    /// Enqueue `payload` verbatim unless the queue is full, in which case the
    /// ARRIVING payload is discarded and `dropped` incremented (drop-newest).
    /// Returns which branch was taken so the caller observes the policy.
    fn push(&mut self, payload: Vec<u8>) -> DgramPushOutcome {
        if self.q.len() >= self.cap {
            // The bound holds even for `cap == 0` (then every push drops).
            self.dropped = self.dropped.saturating_add(1);
            DgramPushOutcome::Dropped
        } else {
            self.q.push_back(payload);
            DgramPushOutcome::Queued
        }
    }

    /// Borrow the front (oldest) payload, so the send-drain can peek before
    /// `dgram_send` and leave it queued on a full send queue.
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

    /// Total drop-newest events over this queue's lifetime (B6 metric).
    #[cfg_attr(not(test), allow(dead_code))]
    fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// B4 — one bidirectional DATAGRAM (RFC 9221) relay pass. Datagrams have no
/// FIN, reset or ordering guarantee, so this NEVER touches stream state and is
/// fully independent of [`relay_streams`].
fn relay_datagrams(
    client: &mut quiche::Connection,
    upstream: &mut quiche::Connection,
    c2u_q: &mut BoundedDgramQueue,
    u2c_q: &mut BoundedDgramQueue,
) {
    pump_dgram_dir(client, upstream, c2u_q, Direction::ClientToUpstream);
    pump_dgram_dir(upstream, client, u2c_q, Direction::UpstreamToClient);
}

/// B4 — relay ONE direction for this turn: recv-drain every datagram quiche
/// has queued on `src` into the bounded `q` (drop-newest when full), then
/// send-drain `q` into `dst` front-first.
///
/// The `dgram_send` arms are not interchangeable:
/// * `Err(Done)` — `dst`'s own send queue is full ⇒ **stop this turn**,
///   leaving the payload queued for the next wake. Transient backpressure, NOT
///   a drop.
/// * `Err(BufferTooShort)` — the payload exceeds `dst`'s peer
///   `max_datagram_frame_size` and can NEVER be forwarded ⇒ drop THIS payload
///   and continue, or it blocks the queue forever.
/// * `Err(InvalidState)` — `dst` never negotiated DATAGRAM (mis-wired;
///   negotiation is a config-time invariant) ⇒ drain and discard the whole
///   queue so a non-negotiating peer cannot pin relay memory.
fn pump_dgram_dir(
    src: &mut quiche::Connection,
    dst: &mut quiche::Connection,
    q: &mut BoundedDgramQueue,
    dir: Direction,
) {
    // Recv-drain into the bounded relay queue (drop-newest when full).
    let mut buf = vec![0u8; MAX_DGRAM_SIZE];
    loop {
        match src.dgram_recv(&mut buf) {
            Ok(len) => {
                // Verbatim copy of exactly `len` bytes (binary-safe,
                // zero-length preserved); `get(..len)` cannot panic.
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

    while let Some(front) = q.front() {
        match dst.dgram_send(front) {
            Ok(()) => {
                let _ = q.pop_front();
            }
            // `dst`'s send queue is full: transient backpressure, so leave
            // the payload queued and retry next wake.
            Err(quiche::Error::Done) => break,
            // Larger than `dst`'s peer max writable: it can NEVER be
            // forwarded, so drop THIS one or it blocks the queue forever.
            Err(quiche::Error::BufferTooShort) => {
                let _ = q.pop_front();
                q.dropped = q.dropped.saturating_add(1);
                tracing::debug!(
                    dir = dir.as_str(),
                    "Mode B B4: dgram_send BufferTooShort (payload exceeds dst max \
                     writable); dropping this datagram"
                );
            }
            // `dst` never negotiated DATAGRAM (mis-wired). Nothing can be
            // forwarded, so discard the whole queue rather than pin memory.
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
                // Datagrams are unreliable — drop this one, do not block.
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

/// Relay direction — only disambiguates log lines; the relay is symmetric.
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

/// Relay ONE direction of ONE stream for this turn: gate-read from `src` into
/// the bounded pending buffer, drain pending into `dst` honouring partial
/// writes / `Done` / `StreamLimit`, and propagate a clean FIN only after ALL
/// pending bytes are accepted — so a FIN can never overtake buffered data.
///
/// Backpressure (R8): `src.stream_recv` runs ONLY while
/// `half.pending.len() < STREAM_RELAY_WINDOW`. quiche extends a stream's
/// flow-control window as a side effect of `stream_recv`, so NOT reading stops
/// the window growing and the source peer blocks once its credit is spent. A
/// short write or `Done` leaves the remainder in `pending` (drained
/// front-first, no reorder, no drop). `StreamLimit` — the mirror stream cannot
/// be opened yet — keeps the bytes pending and retries; it is stream-grant
/// backpressure, never a drop.
///
/// Reset / stop (B3): `Err(StreamReset(code))` from `stream_recv` is RELAYED
/// onward as a RESET_STREAM toward `dst` with the same code;
/// `Err(StreamStopped(code))` from `stream_send` is RELAYED back toward `src`
/// as a STOP_SENDING. Both then drop THIS direction's pending bytes and mark
/// it done — and **never** synthesise a clean FIN, which would present a
/// truncated transfer as complete (the F-MD-4 smuggling bug). Only the
/// affected unidirectional half is torn down. A GENERIC (non-reset/stop) error
/// also fails safe with no FIN, but does NOT synthesise a reset toward the
/// peer: it is not a peer cancellation with a meaningful app code and usually
/// accompanies a connection teardown.
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

    // Read gate: pull from src only while pending is below the window AND the
    // source FIN has not been observed. Loop so a burst moves in one turn.
    //
    // CF-S16-RELAY-STALL: once the source FIN is read, quiche has COLLECTED the
    // stream, so re-issuing `stream_recv` returns `Err(InvalidStreamState)` and
    // the generic read-error arm below would DROP the still-pending tail + the
    // FIN. There is nothing more to read after the FIN — the `!src_fin_seen`
    // gate is the fix.
    while !half.src_fin_seen && half.pending.len() < STREAM_RELAY_WINDOW {
        let room = STREAM_RELAY_WINDOW.saturating_sub(half.pending.len());
        // Read at most `room` so pending never exceeds the window in one recv.
        let mut buf = vec![0u8; room.min(MAX_RELAY_READ)];
        match src.stream_recv(sid, &mut buf) {
            Ok((n, fin)) => {
                half.pending.extend_from_slice(buf.get(..n).unwrap_or(&[]));
                if fin {
                    half.src_fin_seen = true;
                }
                if fin || n == 0 {
                    // FIN reached, or a spurious empty read.
                    break;
                }
            }
            Err(quiche::Error::Done) => break,
            // B3: peer RESET_STREAM on its send side. The transfer is
            // TRUNCATED and must NOT become a clean FIN on `dst` (F-MD-4
            // smuggling guard). Propagate it onward with the SAME code; only
            // THIS half is torn down, the reverse direction stays live.
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
            // Generic read error (NOT a peer RESET_STREAM). Fail safe: drop
            // this half WITHOUT a clean FIN. Deliberately no synthetic reset —
            // a generic fault is not a peer cancellation with a meaningful app
            // code, and usually means `dst` is already being torn down.
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

    // Drain pending into dst, front-first (preserve order, no drop).
    let mut accepted = 0usize;
    while accepted < half.pending.len() {
        let chunk = half.pending.get(accepted..).unwrap_or(&[]);
        match dst.stream_send(sid, chunk, false) {
            Ok(0) | Err(quiche::Error::Done) => break,
            Ok(n) => {
                accepted = accepted.saturating_add(n);
                if n < chunk.len() {
                    break;
                }
            }
            // Mirror stream not openable yet (peer MAX_STREAMS not granted):
            // hold the bytes and retry — stream-grant backpressure, not a drop.
            Err(quiche::Error::StreamLimit) => {
                tracing::trace!(
                    stream_id = sid,
                    dir = dir.as_str(),
                    "Mode B B2: dst StreamLimit; holding pending bytes for retry"
                );
                break;
            }
            // B3: peer STOP_SENDING on the stream we are writing. Propagate it
            // back toward `src` with the SAME code so the source stops, then
            // drop this half without a FIN (smuggling guard).
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
            // Generic write error (NOT a peer STOP_SENDING). Fail safe: drop
            // the half without a FIN and no synthetic reset — same rationale
            // as the read-side generic arm.
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
    // The unsent tail stays pending, in order, for the next turn.
    if accepted > 0 {
        half.pending.drain(..accepted.min(half.pending.len()));
    }

    // FIN: only after ALL pending bytes are accepted by dst.
    if half.src_fin_seen && half.pending.is_empty() && !half.fin_sent {
        match dst.stream_send(sid, &[], true) {
            Ok(_) | Err(quiche::Error::Done) => {
                half.fin_sent = true;
                half.done = true;
            }
            // The mirror stream cannot be OPENED yet — reachable for a
            // zero-data FIN-only stream whose first send IS this empty FIN. Do
            // NOT mark done/fin_sent: leave the half live so a later turn
            // retries once credit is granted. Dropping here would silently lose
            // the FIN and never create the mirror stream.
            Err(quiche::Error::StreamLimit) => {
                tracing::trace!(
                    stream_id = sid,
                    dir = dir.as_str(),
                    "Mode B B2: dst StreamLimit on FIN-only stream; retrying FIN next turn"
                );
            }
            // B3: dst STOP_SENDING on the FIN itself. Propagate back toward
            // `src`; the half is terminal anyway (`pending` is already empty).
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

/// Largest single `stream_recv` read per [`pump_dir`] iteration — bounds the
/// per-call scratch allocation, not the window ([`STREAM_RELAY_WINDOW`] is
/// still the R8 bound).
const MAX_RELAY_READ: usize = 16 * 1024;

/// Emit an application `CONNECTION_CLOSE` ([`RAW_NO_ERROR`]) and pump until
/// quiche reports closed or [`GRACEFUL_CLOSE_BUDGET`] elapses. Idempotent:
/// `close()` on an already-closed connection returns `Done`.
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
    //! Deterministic, socket-free unit coverage for the [`pump_dir`] FIN-retry
    //! logic: `StreamLimit` on the zero-data FIN-only `stream_send` must NOT
    //! drop the FIN — the half stays live and retries once credit is granted.
    //! These drive REAL `quiche::Connection`s but pump packets in-memory, so
    //! the MAX_STREAMS limit is enforced exactly by quiche with no timing
    //! coupling.

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

    /// Server (= the LB's upstream PEER) config; `bidi_limit` is the granted
    /// client-initiated bidi stream count.
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

    /// Client (= the LB-as-client on the upstream leg) config.
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

    /// Drive a `connect` ⇄ `accept` pair to established entirely in memory.
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

    /// Ferry packets BOTH directions one round (no FIN/stream work).
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

    /// Build a relay `src` for stream 0: a server conn whose peer opened
    /// stream 0 with a zero-data FIN.
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
        peer_client.stream_send(0, &[], true).unwrap();
        pump_once(&mut peer_client, &mut src, caddr, saddr);
        assert!(
            src.readable().any(|s| s == 0) || src.stream_finished(0),
            "fixture: src must observe the FIN-only stream 0"
        );
        (src, peer_client)
    }

    /// THE DEFECT REGRESSION (refuse leg): a zero-data FIN-only stream whose
    /// mirror open is refused with `StreamLimit` MUST NOT drop the FIN — the
    /// half stays live so a later turn retries. Pre-fix the FIN block's
    /// catch-all `Err` arm set `done = true`, silently losing it.
    #[test]
    fn fin_only_stream_limit_does_not_drop_fin() {
        let certs = gen_certs();
        let (mut src, _peer) = src_server_with_fin_only_stream0(&certs);
        // `dst` = LB-as-client whose backend peer grants ZERO bidi streams.
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

        assert!(
            half.src_fin_seen,
            "the relay must have observed the source FIN (intent recorded)"
        );
        // The StreamLimit-refused FIN must NOT terminate the half.
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

    /// THE DEFECT REGRESSION (grant leg): with credit available the SAME
    /// FIN-only `pump_dir` delivers a clean FIN — together with the refuse leg
    /// this proves the retry is real.
    #[test]
    fn fin_only_delivered_when_stream_credit_available() {
        let certs = gen_certs();
        let (mut src, _peer) = src_server_with_fin_only_stream0(&certs);
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

        // Deliver the FIN STREAM frame and confirm the backend sees it.
        pump_once(&mut dst, &mut backend, caddr, saddr);
        assert!(
            backend.stream_finished(0),
            "the backend must observe stream 0 finished (clean FIN delivered)"
        );
    }

    /// Server config with a deliberately TINY per-stream window, so the
    /// relay's drain into `dst` short-writes.
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
        // The binding limit: how much the relay's `dst` may push per stream.
        cfg.set_initial_max_stream_data_bidi_remote(bidi_remote_window);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(4);
        cfg.set_initial_max_streams_uni(2);
        cfg.set_disable_active_migration(true);
        cfg
    }

    /// Like [`established_pair`] but the SERVER peer advertises a tiny window.
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

    /// Build a relay `src` for stream 0 carrying `payload` + FIN, so a single
    /// `pump_dir` read pulls `(payload.len(), fin=true)` and quiche COLLECTS
    /// the stream. (`payload` must fit the 64 KiB per-stream window.)
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

    /// CF-S16-RELAY-STALL — the post-FIN re-read drop regression. Turn 1 reads
    /// the whole payload + FIN in one `stream_recv` (so quiche COLLECTS the
    /// source) but short-writes into `dst`, leaving a pending tail. Turn 2's
    /// read gate is where the bug lived: PRE-FIX it re-issued `stream_recv` on
    /// the collected source, hit `Err(InvalidStreamState)`, and the generic
    /// read-error arm dropped the tail AND the FIN.
    ///
    /// Load-bearing: revert the one-line `!half.src_fin_seen` gate and this
    /// test FAILS (tail dropped, `fin_sent` false, backend never sees the full
    /// payload).
    #[test]
    fn post_fin_short_write_reread_does_not_drop_tail() {
        let certs = gen_certs();

        // A multi-KiB payload — larger than the backend's tiny window below, so
        // the drain necessarily short-writes.
        let payload: Vec<u8> = (0..10_240u32).map(|i| (i % 251) as u8).collect();

        // `src` = client-leg conn with stream 0 = payload + FIN, collected.
        let (mut src, mut peer) = src_server_with_payload_fin_stream0(&certs, &payload);
        let (src_caddr, src_saddr): (SocketAddr, SocketAddr) = (
            "127.0.0.1:5101".parse().unwrap(),
            "127.0.0.1:5102".parse().unwrap(),
        );

        // `dst` = LB-as-client whose backend grants a TINY per-stream window.
        let dst_window: u64 = 4 * 1024;
        let (mut dst, mut backend, caddr, saddr) =
            established_pair_small_dst_window(&certs, dst_window);

        let mut half = RelayHalf::default();

        // Turn 1: read payload+FIN (collects src), drain short-writes.
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

        // Complete the bidi stream 0 on `src` so quiche COLLECTS it — the
        // precondition for the pre-fix InvalidStreamState.
        src.stream_send(0, &[], true).unwrap();
        peer.stream_send(0, &[], true).ok();
        for _ in 0..8 {
            pump_once(&mut src, &mut peer, src_saddr, src_caddr);
        }
        {
            let mut sink = [0u8; 256];
            while let Ok((_n, _fin)) = peer.stream_recv(0, &mut sink) {}
        }
        for _ in 0..8 {
            pump_once(&mut src, &mut peer, src_saddr, src_caddr);
        }
        // Sanity: the source stream is now collected.
        assert!(
            src.stream_finished(0),
            "fixture: src stream 0 must be finished/collected before turn 2 \
             (so the buggy re-read trips InvalidStreamState)"
        );

        // Turn 2: THE buggy re-read turn. Pre-fix this dropped tail + FIN.
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

        // Open the dst window and drive the relay until the FIN is forwarded.
        let mut got = Vec::new();
        let mut backend_fin = false;
        let mut sink = vec![0u8; 65_535];
        for _ in 0..128 {
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
            pump_once(&mut dst, &mut backend, caddr, saddr);
            pump_dir(
                0,
                &mut src,
                &mut dst,
                &mut half,
                Direction::UpstreamToClient,
            );
            if half.fin_sent && half.pending.is_empty() && backend_fin {
                pump_once(&mut dst, &mut backend, caddr, saddr);
                break;
            }
        }
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

        // The tail must NOT have been dropped: a clean FIN was forwarded…
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

    /// (a) FIFO ORDER: front-first in arrival order, no reorder, no loss.
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

    /// (b) DROP-NEWEST NEGATIVE CONTROL: push `cap + K` and assert the OLDEST
    /// `cap` survived in order and `dropped == K`. An unbounded queue (the
    /// pre-fix shape) would hold all `cap + K` and report `dropped == 0` —
    /// this test fails it.
    #[test]
    fn dgram_queue_drop_newest_negative_control() {
        const CAP: usize = 16;
        const K: usize = 9;
        let mut q = BoundedDgramQueue::new(CAP);

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

        // The OLDEST cap survived, in order; the newest K were dropped.
        for i in 0..CAP {
            assert_eq!(
                q.pop_front(),
                Some(mk(i)),
                "the oldest cap payloads survived in arrival order; index {i}"
            );
        }
        assert!(q.is_empty(), "nothing beyond the oldest cap survived");
    }

    /// (c) BINARY / ZERO-LENGTH payloads round-trip VERBATIM — no UTF-8
    /// assumption, no length-implied truncation.
    #[test]
    fn dgram_queue_preserves_binary_and_zero_length_verbatim() {
        let mut q = BoundedDgramQueue::new(8);
        let empty: Vec<u8> = Vec::new();
        let zeros: Vec<u8> = vec![0u8; 64];
        let non_utf8: Vec<u8> = vec![0xff, 0xfe, 0x80, 0x00, 0x7f, 0xc0, 0xff];
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

    /// Pin the documented R8 bound so a silent change is caught.
    #[test]
    fn dgram_queue_cap_constant_is_documented_default() {
        assert_eq!(
            DGRAM_QUEUE_CAP, 1024,
            "the R8 datagram-queue bound is 1024 (matches quiche default)"
        );
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

    /// A `quiche::Config` pair granting MANY bidi streams (> the relay cap),
    /// so the relay table can be driven OVER [`MAX_RELAY_STREAMS`].
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

    /// Establish a peer→server pair where the peer opens `open_count` bidi
    /// streams (one byte + FIN each) and ferries them so `server` sees them all
    /// readable. `bidi_limit` (the grant) must be >= `open_count`.
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
        for i in 0..open_count {
            let sid = i * 4; // client-initiated bidi stream ids
            peer.stream_send(sid, &[(i % 251) as u8], true).unwrap();
        }
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
    /// streams over a grant that EXCEEDS the cap, so quiche is NOT the limiter.
    ///
    /// Load-bearing negative control: the fixture asserts the server observes
    /// ALL `OPEN` streams readable, so the table would reach `OPEN` without the
    /// `admit_or_refuse` ceiling. Remove the gate and the final assert flips
    /// from `== cap` to `== OPEN`.
    #[test]
    fn relay_table_clamped_to_max_relay_streams_under_flood() {
        let certs = gen_certs();
        let open: u64 = (MAX_RELAY_STREAMS as u64) + 64;
        // Grant strictly more than we open, so quiche is not the limiter.
        let grant: u64 = open + 16;
        let (mut server, _peer) = server_with_n_readable_streams(&certs, open, grant);

        let (mut upstream, _backend, _ca, _sa) = established_pair(&certs, 4);

        // Negative-control seed: the server really does see them all readable.
        let server_readable = server.readable().count();
        assert!(
            server_readable as u64 >= open,
            "seed: the source offers {open} readable streams (> the {MAX_RELAY_STREAMS} cap); \
             WITHOUT the cap the table would reach {server_readable}"
        );

        let mut streams: HashMap<u64, RawStreamState> = HashMap::new();
        // Several passes: each admits new readable sids up to the cap.
        for _ in 0..4 {
            relay_streams(&mut server, &mut upstream, &mut streams, MAX_RELAY_STREAMS);
            assert!(
                streams.len() <= MAX_RELAY_STREAMS,
                "B5: the relay table must never exceed MAX_RELAY_STREAMS ({MAX_RELAY_STREAMS}); \
                 got {}",
                streams.len()
            );
        }

        // The cap was actually REACHED, not merely under it for another reason.
        assert_eq!(
            streams.len(),
            MAX_RELAY_STREAMS,
            "B5: with > cap streams offered, the table must fill to exactly the cap \
             (the over-cap sids are refused, not inserted)"
        );
    }

    /// B5 — `admit_or_refuse` directly: an ALREADY-TRACKED sid is kept even AT
    /// the cap (the cap must never drop a live stream mid-transfer), while a
    /// genuinely NEW sid at the cap is REFUSED.
    #[test]
    fn admit_or_refuse_keeps_tracked_refuses_new_at_cap() {
        let mut streams: HashMap<u64, RawStreamState> = HashMap::new();
        // Fill the table to EXACTLY the cap with sids 0..cap.
        for sid in 0..(MAX_RELAY_STREAMS as u64) {
            admit_or_refuse(&mut streams, sid, MAX_RELAY_STREAMS);
        }
        assert_eq!(
            streams.len(),
            MAX_RELAY_STREAMS,
            "the first MAX_RELAY_STREAMS distinct sids fill the table to the cap"
        );

        // (a) An already-tracked sid offered again at the cap is a no-op.
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

        // (b) A genuinely NEW sid at the cap is REFUSED, table unchanged.
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

    /// B5 — pin the cap constant + the documented memory-ceiling arithmetic.
    #[test]
    fn max_relay_streams_constant_is_documented_default() {
        assert_eq!(
            MAX_RELAY_STREAMS, 256,
            "the B5 relay-table ceiling is 256 (8× the ~32 negotiated grant)"
        );
        let ceiling = MAX_RELAY_STREAMS * 2 * super::STREAM_RELAY_WINDOW;
        assert_eq!(
            ceiling,
            128 * 1024 * 1024,
            "documented per-conn relay memory ceiling = 128 MiB \
             (MAX_RELAY_STREAMS * 2 * STREAM_RELAY_WINDOW)"
        );
    }

    /// `quiche::Config` for a pair where DATAGRAM negotiation is independently
    /// switchable per side; `None` leaves it OFF (so `dgram_send` returns
    /// `InvalidState`).
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

    /// B4 — the `dgram_send` InvalidState arm: a `dst` that never negotiated
    /// DATAGRAM drains + counts the whole queue, so a non-negotiating peer
    /// cannot pin relay memory. Reachable only if mis-wired, but it must hold.
    #[test]
    fn pump_dgram_dir_invalid_state_drains_and_disables() {
        let certs = gen_certs();
        // quiche returns `InvalidState` when the LOCAL side's peer never
        // enabled DATAGRAM.
        let (mut src, mut dst, _caddr, _saddr) = dgram_pair(&certs, None, Some(1200));
        assert!(
            dst.dgram_max_writable_len().is_none(),
            "fixture: dst's peer must NOT have negotiated DATAGRAM (⇒ dgram_send InvalidState)"
        );

        // Pre-seed the relay queue as if these had been recv-drained.
        let mut q = BoundedDgramQueue::new(DGRAM_QUEUE_CAP);
        for i in 0..3u8 {
            assert_eq!(q.push(vec![i; 16]), DgramPushOutcome::Queued);
        }
        assert_eq!(q.len(), 3);
        let dropped_before = q.dropped();

        // One relay pass: recv-drain `src` (empty) then send-drain into `dst`.
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

    /// B4 — the `dgram_send` BufferTooShort arm: an oversized payload is
    /// dropped and counted while the send-drain CONTINUES, so it cannot block
    /// the queue forever — a normal payload queued after it still arrives.
    #[test]
    fn pump_dgram_dir_buffer_too_short_drops_one_continues() {
        let certs = gen_certs();
        // `dst` = server; its writable limit is what makes the payload oversized.
        let (mut peer_of_dst, mut dst, daddr_peer, daddr_dst) =
            dgram_pair(&certs, Some(1200), Some(1200));
        let max = dst
            .dgram_max_writable_len()
            .expect("fixture: dst negotiated DATAGRAM ⇒ Some writable len");
        assert!(max < MAX_DGRAM_SIZE, "fixture: writable len is bounded");

        let oversized = vec![0xABu8; max + 1]; // > dst writable ⇒ BufferTooShort
        let normal = vec![0xCDu8; max.min(64)]; // fits ⇒ delivered
        let mut q = BoundedDgramQueue::new(DGRAM_QUEUE_CAP);
        assert_eq!(q.push(oversized), DgramPushOutcome::Queued);
        assert_eq!(q.push(normal.clone()), DgramPushOutcome::Queued);
        let dropped_before = q.dropped();

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

        // The normal payload must actually have reached `dst`'s peer.
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
