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
//! ## B1 scope (this file)
//!
//! B1 ships the **skeleton**, not the application relay:
//!
//! 1. Drive the CLIENT-facing connection (already `accept_with_retry`'d
//!    by the router, handed over in [`ActorParams::conn`]) to
//!    established, using the same low-level pump shape as
//!    [`crate::conn_actor::run_actor`] (recv inbound packets forwarded
//!    by the router over [`ActorParams::inbound`] → `conn.recv`; drain
//!    `conn.send` to the shared [`ActorParams::socket`]; `on_timeout`).
//! 2. On client `is_established()`: read the negotiated ALPN via
//!    `application_proto()` and dial a **dedicated** upstream connection
//!    ([`QuicUpstreamPool::dial_dedicated`]) on its OWN UDP socket,
//!    mirroring that ALPN.
//! 3. Run BOTH connection pumps concurrently in one `tokio::select!`
//!    loop (client inbound + upstream socket recv + both timeouts +
//!    cancel) until either side closes or idle-times-out, then close the
//!    other gracefully and return.
//!
//! There is **no** application stream/datagram relay yet — that is B2
//! (streams), B3 (cancellation), B4 (datagrams). B1 just proves both
//! connections establish and both pumps stay alive.
//!
//! ## What B2 needs from here (the seam this file fixes)
//!
//! * The two connections live in [`run_raw_proxy_actor`] as
//!   `params.conn` (client) and `upstream.conn` (backend, an owned
//!   [`lb_io::quic_pool::DedicatedQuic`]). The relay reads/writes both
//!   inside the single select loop — every arm has `&mut` access to
//!   both, so no mutex is needed (same rationale as the H3 actor).
//! * The select-loop shape (see [`run_raw_proxy_actor`]) is the
//!   extension point: B2 adds `conn.readable()` / `stream_recv` /
//!   `stream_send` relay passes AFTER each event, exactly where the H3
//!   actor runs `poll_h3`. The identity stream-ID map (plan §2.2) means
//!   no translation table.
//! * [`RawProxyOutcome`] (returned via the `io::Result` chain through a
//!   test hook) surfaces both connections' SCIDs + trace_ids so the
//!   verifier's two-connections proof can assert distinctness by
//!   mechanism rather than by a bridge assertion.

use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;

use lb_io::quic_pool::{DedicatedQuic, QuicUpstreamPool};

use crate::conn_actor::ActorParams;

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
}

impl std::fmt::Debug for RawBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawBackend")
            .field("addr", &self.addr)
            .field("sni", &self.sni)
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
pub async fn run_raw_proxy_actor_for_test(
    params: ActorParams,
) -> std::io::Result<RawProxyOutcome> {
    run_raw_proxy_actor_inner(params).await
}

/// The fallible core. Phase 1 drives the client handshake + dials the
/// dedicated upstream; Phase 2 runs both pumps concurrently until either
/// side finishes, then closes the other. Returns the two-connections
/// proof on graceful completion.
async fn run_raw_proxy_actor_inner(
    mut params: ActorParams,
) -> std::io::Result<RawProxyOutcome> {
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

    // ---- Phase 2: run BOTH pumps concurrently (no relay yet, B2) -----
    run_dual_pump(&mut params, &mut upstream, &mut out_buf).await;

    // Either side closed / idle-timed-out: close the other gracefully.
    // (Both calls are idempotent — a no-op if the leg is already
    // closed.)
    graceful_close(&mut params.conn, &params.socket, &mut out_buf).await;
    graceful_close(&mut upstream.conn, &upstream.socket, &mut out_buf).await;

    Ok(outcome)
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
/// loop. No application relay yet (B2/B4) — this just keeps both
/// connections alive (recv inbound, drain outbound, tick timeouts) until
/// either leg closes or idle-times-out, or the cancel token fires.
///
/// ## The select-loop shape (B2 extension point)
///
/// Each turn: drain both legs' outbound packets, check both for
/// `is_closed()`, then `select!` over six events:
/// 1. `cancel.cancelled()` (biased first) — listener shutdown.
/// 2. client inbound packet (router-forwarded mpsc) → `client.recv`.
/// 3. upstream socket `recv_from` → `upstream.recv`.
/// 4. client timeout → `client.on_timeout`.
/// 5. upstream timeout → `upstream.on_timeout`.
///
/// B2 adds, immediately AFTER the `select!` (where the H3 actor runs
/// `poll_h3`), the bidirectional raw-stream relay passes over BOTH
/// connections — both are `&mut` in scope, so no mutex (same reasoning
/// as the H3 actor keeping per-stream state inline).
async fn run_dual_pump(
    params: &mut ActorParams,
    upstream: &mut DedicatedQuic,
    out_buf: &mut [u8],
) {
    // The upstream recv needs its own inbound buffer (the client side
    // uses owned `Vec`s forwarded by the router; the upstream side
    // recv_from's straight off its dedicated socket).
    let mut up_in_buf = vec![0u8; 65_535];
    let upstream_local = upstream.local;

    loop {
        // Drain any queued outbound on both legs first (parity with the
        // H3 actor draining before the wait).
        drain_conn_send(&params.socket, &mut params.conn, out_buf).await;
        drain_conn_send(&upstream.socket, &mut upstream.conn, out_buf).await;

        // B1 termination condition: either leg done. (B2+ keeps the
        // relay running across a half-close; B1 has no relay, so a
        // close on either side ends the actor.)
        if params.conn.is_closed() || upstream.conn.is_closed() {
            break;
        }

        let client_wait = params.conn.timeout().unwrap_or(IDLE_TICK);
        let upstream_wait = upstream.conn.timeout().unwrap_or(IDLE_TICK);

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

        // B2 extension point: bidirectional raw-stream + datagram relay
        // passes go HERE (both `params.conn` and `upstream.conn` are
        // `&mut` in scope). B1 ships no relay.
    }
}

/// Repeatedly call `quiche::Connection::send` and write the resulting
/// packets onto the UDP socket until quiche reports `Done`. Byte-for-
/// byte the same pump as [`crate::conn_actor`]'s private `drain_conn_send`
/// (kept local to this module rather than re-exported to avoid widening
/// `conn_actor`'s public surface for B1).
async fn drain_conn_send(socket: &UdpSocket, conn: &mut quiche::Connection, out_buf: &mut [u8]) {
    loop {
        match conn.send(out_buf) {
            Ok((n, info)) => {
                let slice = out_buf.get(..n).unwrap_or(&[]);
                if let Err(e) = socket.send_to(slice, info.to).await {
                    tracing::debug!(error = %e, "Mode B conn send_to");
                    break;
                }
            }
            Err(quiche::Error::Done) => break,
            Err(e) => {
                tracing::debug!(error = %e, "Mode B conn.send");
                break;
            }
        }
    }
}

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
