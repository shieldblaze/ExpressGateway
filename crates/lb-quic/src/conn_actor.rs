//! Per-connection actor (`ConnectionActor`) driving one [`quiche::Connection`] to
//! established state, then pumping H3 requests through the
//! [`crate::h3_bridge`] to an H1 backend.
//!
//! The actor owns everything tied to one QUIC connection: the shared
//! `UdpSocket` the [`crate::router::InboundPacketRouter`] hands down, the
//! per-CID mpsc receiver, a cancellation token, the backend
//! [`lb_io::pool::TcpPool`], and the caller-supplied backend address
//! list. One tokio task per connection. The select! loop handles
//! inbound packets (forwarded by the router), the quiche timer, and
//! graceful cancellation.
//!
//! H3 ownership sits inside this actor rather than in a separate H3
//! driver because the quiche stream API is tightly coupled to the
//! connection's mutable state: every `stream_recv`/`stream_send` call
//! requires a `&mut quiche::Connection`, so splitting the actor in two
//! would require a mutex we do not want on the hot path. Instead we
//! keep per-stream state (read buffers, response queues) in `HashMap`s
//! inside the actor itself.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lb_io::http2_pool::Http2Pool;
use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;

use crate::h3_bridge::{
    BodyItem, H3Request, MAX_REQUEST_BODY_BYTES, ReqBodyEvent, StreamRxBuf, encode_h3_response,
    h3_to_h2_roundtrip, h3_to_h3_roundtrip, h3_to_h1_stream,
};

/// SESSION 2 / P1-A: depth of the per-stream bounded request-body
/// channel. With `h3_bridge::H3_BODY_CHUNK_MAX` (8 KiB) this caps the
/// max in-flight body to ≈ `H3_BODY_CHANNEL_DEPTH * 8 KiB` (≈ 64 KiB)
/// INDEPENDENT of the total request-body size — the memory-safety
/// mechanism. When the channel is full `poll_h3` stops calling
/// `stream_recv` for that stream, so quiche does not extend the QUIC
/// stream flow-control window and the H3 client is paused → genuine
/// end-to-end backpressure.
pub const H3_BODY_CHANNEL_DEPTH: usize = 8;

/// Application-layer error code emitted in the `CONNECTION_CLOSE`
/// frame the actor sends when the listener-wide cancel token fires.
///
/// `H3_NO_ERROR = 0x0100` is RFC 9114 §8.1's "graceful drain"
/// signal — every conformant H3 peer parses it as an orderly
/// shutdown rather than an abort (PROTO-2-11).
pub const H3_NO_ERROR: u64 = 0x0100;

/// Upper bound on how long [`graceful_h3_shutdown`] will pump the
/// connection after issuing `close()` before giving up. Quiche enters
/// the draining state for `3 * PTO` (RFC 9000 §10.1); 500 ms is
/// comfortably above that for any sane PTO on a loopback link and
/// puts a hard ceiling on shutdown latency in production.
const GRACEFUL_SHUTDOWN_BUDGET: Duration = Duration::from_millis(500);

/// Raw UDP packet forwarded from the router to a single actor.
#[derive(Debug)]
pub struct InboundPacket {
    /// Receive buffer (owned — one allocation per packet is acceptable
    /// at this scale; a pool is future work when profiling demands it).
    pub data: Vec<u8>,
    /// Peer address the packet came from.
    pub from: SocketAddr,
    /// Local address the packet came in on.
    pub to: SocketAddr,
}

/// Construction parameters for [`ConnectionActor`].
pub struct ActorParams {
    /// The `quiche::Connection` handed over by the router after
    /// `accept()` / token verification.
    pub conn: quiche::Connection,
    /// Shared outbound socket (all actors on one listener share this).
    pub socket: Arc<UdpSocket>,
    /// Bounded channel receiver; the router pushes packets into the
    /// paired sender when the DCID matches this actor's connection.
    pub inbound: mpsc::Receiver<InboundPacket>,
    /// Listener-wide cancellation token.
    pub cancel: CancellationToken,
    /// Backend TCP pool shared across all listeners.
    pub pool: TcpPool,
    /// Resolved backend addresses for H1 backends. Round-robin
    /// selection picks one per H3 request; 3b.3c-2 ships the simplest
    /// possible picker.
    pub backends: Arc<Vec<SocketAddr>>,
    /// Optional upstream H3 pool + single upstream H3 backend
    /// `(addr, sni)`. When configured, the actor routes H3 requests
    /// via [`h3_to_h3_roundtrip`] instead of the H1/TcpPool path.
    /// Pillar 3b.3c-3.
    pub h3_backend: Option<(QuicUpstreamPool, SocketAddr, String)>,
    /// Optional upstream H2 pool + single upstream H2 backend `(addr)`.
    /// When configured (and `h2_backend` is `Some`), the actor routes
    /// H3 requests via [`h3_to_h2_roundtrip`]. Takes precedence over
    /// `h3_backend`. PROTO-001 H3→H2 path.
    pub h2_backend: Option<(Http2Pool, SocketAddr)>,
}

/// Drive one `quiche::Connection` to completion, terminating H3 and
/// forwarding to an H1 backend.
///
/// # Errors
///
/// Never — the actor swallows all errors after logging. The returned
/// `io::Result<()>` shape exists so the caller can chain without
/// bespoke error handling; the success variant is always returned.
pub async fn run_actor(mut params: ActorParams) -> std::io::Result<()> {
    let mut out_buf = vec![0u8; 65_535];
    let mut rx_buf_by_stream: HashMap<u64, StreamRxBuf> = HashMap::new();
    let mut stream_response: HashMap<u64, StreamTx> = HashMap::new();
    // SESSION 2 / P1-A: per-stream bounded request-body channels. The
    // sender lives here in the actor; the matching receiver is moved
    // into the spawned `h3_to_h1_stream` task. Bounded depth +
    // backpressure (poll_h3 skips `stream_recv` when full) is the
    // memory-safety mechanism.
    let mut body_tx_by_stream: HashMap<u64, mpsc::Sender<ReqBodyEvent>> = HashMap::new();
    // SESSION 2 / P1-A: decoded-but-not-yet-sent body events per
    // stream. A single DATA frame can decode into many ≤8 KiB chunks;
    // we never drop a decoded item — overflow past the channel's spare
    // capacity stays here and is flushed (with backpressure) next tick.
    let mut body_pending: HashMap<u64, VecDeque<ReqBodyEvent>> = HashMap::new();
    // `request_tasks` holds the bridge's H3→H1 jobs. We push each
    // spawned JoinHandle in, and await the first-completed inside the
    // select! so the actor wakes as soon as a response is ready — not
    // only on quiche's timeout or the next inbound packet.
    let mut request_tasks: Vec<tokio::task::JoinHandle<(u64, Vec<u8>)>> = Vec::new();

    loop {
        // Before waiting: push any outbound bytes from quiche + any
        // per-stream response bytes into quiche stream_send.
        drain_streams_to_conn(&mut params.conn, &mut stream_response);
        drain_conn_send(&params.socket, &mut params.conn, &mut out_buf).await;

        if params.conn.is_closed() {
            break;
        }

        let mut next_wait = params.conn.timeout().unwrap_or(Duration::from_millis(100));
        // SESSION 2 / P1-A: while a request body is actively streaming
        // the only thing that advances it is a `poll_h3` tick draining
        // decoded chunks into the (bounded) channel as the egress task
        // consumes them. quiche's idle timeout can be hundreds of ms,
        // which would throttle body forwarding to a crawl (and idle
        // out the client). Cap the wait to a short tick so a paused/
        // backpressured stream resumes promptly. This does NOT defeat
        // backpressure: the bounded channel + capacity gate still cap
        // in-flight bytes; we merely poll the gate more often.
        if !body_tx_by_stream.is_empty() || !body_pending.is_empty() {
            next_wait = next_wait.min(Duration::from_millis(2));
        }

        // Build the "task completed" future: the first finished one
        // among request_tasks. If none are outstanding, we push a
        // never-completing future so the select arm is inert.
        let task_wait = async {
            if request_tasks.is_empty() {
                std::future::pending::<Option<(u64, Vec<u8>)>>().await
            } else {
                // Poll every 5ms for any finished handle — cheap, and
                // decouples us from waking the loop on task completion.
                loop {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    if let Some(pos) = request_tasks
                        .iter()
                        .position(tokio::task::JoinHandle::is_finished)
                    {
                        let h = request_tasks.swap_remove(pos);
                        match h.await {
                            Ok(v) => return Some(v),
                            Err(e) => {
                                tracing::warn!(error = %e, "H3→H1 task join failure");
                                return None;
                            }
                        }
                    }
                }
            }
        };

        tokio::select! {
            biased;
            () = params.cancel.cancelled() => {
                graceful_h3_shutdown(&mut params.conn, &params.socket, &mut out_buf).await;
                break;
            }
            pkt = params.inbound.recv() => {
                let Some(mut pkt) = pkt else { break; };
                let info = quiche::RecvInfo { from: pkt.from, to: pkt.to };
                match params.conn.recv(&mut pkt.data, info) {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(e) => {
                        tracing::debug!(error = %e, "quiche recv");
                    }
                }
            }
            finished = task_wait => {
                if let Some((stream_id, response_bytes)) = finished {
                    stream_response.insert(stream_id, StreamTx::new(response_bytes));
                }
            }
            () = tokio::time::sleep(next_wait) => {
                params.conn.on_timeout();
            }
        }

        // Post-event: poll H3 streams if established.
        if params.conn.is_established() {
            poll_h3(
                &mut params.conn,
                &mut rx_buf_by_stream,
                &mut body_tx_by_stream,
                &mut body_pending,
                &mut request_tasks,
                &params.pool,
                &params.backends,
                params.h3_backend.as_ref(),
                params.h2_backend.as_ref(),
            );
        }
    }
    Ok(())
}

/// Per-stream outbound cursor. Tracks how much of the encoded H3
/// response has been fed into quiche's send buffer and whether FIN has
/// been set.
struct StreamTx {
    bytes: Vec<u8>,
    sent: usize,
    finished: bool,
}

impl StreamTx {
    const fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            sent: 0,
            finished: false,
        }
    }
}

/// Pump per-stream response bytes into quiche's send buffer. We send
/// incrementally because `stream_send` may refuse bytes when flow
/// control is saturated.
fn drain_streams_to_conn(conn: &mut quiche::Connection, streams: &mut HashMap<u64, StreamTx>) {
    let mut to_drop = Vec::new();
    for (&sid, tx) in streams.iter_mut() {
        if tx.finished {
            continue;
        }
        loop {
            let remaining = tx.bytes.get(tx.sent..).unwrap_or(&[]);
            if remaining.is_empty() {
                // All bytes in; send FIN separately via a zero-length
                // send with fin=true.
                match conn.stream_send(sid, &[], true) {
                    Ok(_) | Err(quiche::Error::Done) => {
                        tx.finished = true;
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, stream_id = sid, "stream_send FIN");
                        tx.finished = true;
                    }
                }
                to_drop.push(sid);
                break;
            }
            match conn.stream_send(sid, remaining, false) {
                Ok(0) | Err(quiche::Error::Done) => break,
                Ok(n) => {
                    tx.sent = tx.sent.saturating_add(n);
                }
                Err(e) => {
                    tracing::debug!(error = %e, stream_id = sid, "stream_send");
                    break;
                }
            }
        }
    }
    for sid in to_drop {
        // Keep the StreamTx with finished=true so subsequent calls skip
        // it; remove lazily on next poll to keep the allocation low.
        if let Some(tx) = streams.get_mut(&sid) {
            tx.finished = true;
        }
    }
    streams.retain(|_, tx| !tx.finished);
}

/// Emit a H3 `CONNECTION_CLOSE` frame and pump the connection until
/// quiche reports closed (PROTO-2-11).
///
/// The actor calls this from its cancel branch when the listener-wide
/// `CancellationToken` (derived from `lb_core::Shutdown::token()` —
/// the wiring of the listener-level token onto every actor is Wave-2c
/// code-owned work; here the actor merely emits the frame on whatever
/// cancel signal it receives) fires.
///
/// Behaviour:
/// 1. Call `quiche::Connection::close(true, H3_NO_ERROR, b"shutdown")`
///    so the wire frame is an application-layer `CONNECTION_CLOSE`
///    (frame type 0x1d, RFC 9000 §19.19) carrying RFC 9114 §8.1's
///    `H3_NO_ERROR = 0x0100`.
/// 2. Pump `conn.send()` → UDP, plus `on_timeout()` ticks at the PTO
///    cadence quiche requests, until either `is_closed()` becomes
///    true (quiche entered the closed state — CLOSE acknowledged or
///    draining timer elapsed) or [`GRACEFUL_SHUTDOWN_BUDGET`]
///    elapses.
///
/// If `close()` is called on an already-closed connection quiche
/// returns `Error::Done`; the helper treats that as a no-op so callers
/// can issue it idempotently.
pub async fn graceful_h3_shutdown(
    conn: &mut quiche::Connection,
    socket: &UdpSocket,
    out_buf: &mut [u8],
) {
    match conn.close(true, H3_NO_ERROR, b"shutdown") {
        Ok(()) | Err(quiche::Error::Done) => {}
        Err(e) => {
            tracing::debug!(error = %e, "conn.close (graceful_h3_shutdown)");
        }
    }
    let deadline = tokio::time::Instant::now() + GRACEFUL_SHUTDOWN_BUDGET;
    loop {
        drain_conn_send(socket, conn, out_buf).await;
        if conn.is_closed() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::debug!(
                "graceful_h3_shutdown: budget exhausted before is_closed(); abandoning"
            );
            return;
        }
        // Quiche's draining timer is per-connection; we wait whichever
        // is shorter between quiche's own timer suggestion and the
        // residual budget so we never sleep past the deadline.
        let quiche_timeout = conn.timeout().unwrap_or(Duration::from_millis(10));
        let residual = deadline.saturating_duration_since(tokio::time::Instant::now());
        let wait = quiche_timeout.min(residual);
        tokio::time::sleep(wait).await;
        conn.on_timeout();
    }
}

/// Repeatedly call `quiche::Connection::send` and send resulting
/// packets onto the UDP socket until quiche reports `Done`.
async fn drain_conn_send(socket: &UdpSocket, conn: &mut quiche::Connection, out_buf: &mut [u8]) {
    loop {
        match conn.send(out_buf) {
            Ok((n, info)) => {
                let slice = out_buf.get(..n).unwrap_or(&[]);
                if let Err(e) = socket.send_to(slice, info.to).await {
                    tracing::debug!(error = %e, "conn send_to");
                    break;
                }
            }
            Err(quiche::Error::Done) => break,
            Err(e) => {
                tracing::debug!(error = %e, "conn.send");
                break;
            }
        }
    }
}

/// Walk readable streams, accumulate HEADERS for any that have not
/// started, and spawn an H3→H1 (or H3→H3 when configured) task per
/// completed request.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn poll_h3(
    conn: &mut quiche::Connection,
    rx_by_stream: &mut HashMap<u64, StreamRxBuf>,
    body_tx_by_stream: &mut HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    body_pending: &mut HashMap<u64, VecDeque<ReqBodyEvent>>,
    request_tasks: &mut Vec<tokio::task::JoinHandle<(u64, Vec<u8>)>>,
    pool: &TcpPool,
    backends: &Arc<Vec<SocketAddr>>,
    h3_backend: Option<&(QuicUpstreamPool, SocketAddr, String)>,
    h2_backend: Option<&(Http2Pool, SocketAddr)>,
) {
    // SESSION 2 / P1-A: flush any decoded-but-not-yet-sent body events
    // for EVERY active stream first, independent of `conn.readable()`.
    // Once a request stream is fully received + FIN, quiche stops
    // listing it as readable — but the per-stream pending queue may
    // still hold the tail chunks + the `End` event that the egress is
    // blocked waiting on. Without this pass those would never be
    // delivered and the request would hang. This is also where
    // backpressure releases: as the egress drains the bounded channel,
    // capacity frees and the queued tail flows out.
    let pending_sids: Vec<u64> = body_pending.keys().copied().collect();
    for sid in pending_sids {
        flush_pending(sid, body_tx_by_stream, body_pending);
    }

    let readable: Vec<u64> = conn.readable().collect();
    for sid in readable {
        // SESSION 2 / P1-A: streams that already have an active body
        // channel are in the Body phase. Drain them with channel
        // capacity backpressure: if the bounded channel is full we do
        // NOT call `stream_recv` this tick → quiche keeps the QUIC
        // stream flow-control window unextended → the H3 client is
        // paused. This is the genuine end-to-end backpressure / memory
        // mechanism (max in-flight = depth * chunk, body-size
        // independent).
        if body_tx_by_stream.contains_key(&sid) {
            drain_body_stream(conn, sid, rx_by_stream, body_tx_by_stream, body_pending);
            continue;
        }
        let mut buf = [0u8; 8192];
        loop {
            match conn.stream_recv(sid, &mut buf) {
                Ok((n, fin)) => {
                    let rx = rx_by_stream.entry(sid).or_default();
                    match rx.feed(buf.get(..n).unwrap_or(&[])) {
                        Ok(Some(headers)) => {
                            let req = H3Request::from_headers(headers);
                            // ROUND8-L7-16: authority value sanitisation
                            // choke point — the H3 leg of L7-09
                            // (HAProxy `BUG/MAJOR: http: forbid comma
                            // character in authority value`). This MUST
                            // run before ANY of the three upstream
                            // branches below so a comma / whitespace /
                            // control byte in `:authority` is rejected
                            // (H3 `:status 400`) and ZERO upstream
                            // connection is dialled. The predicate is
                            // `lb_core::authority::validate` — the
                            // EXACT same one the H1/H2 path
                            // (`lb_l7::authority`) calls, so the
                            // behaviour is byte-identical across all
                            // three protocols (no fork, no loopback
                            // exemption: value sanitisation only). An
                            // absent / empty `:authority` is NOT
                            // rejected here (PROTO-2-01's gate, not
                            // this predicate's).
                            if !req.authority.is_empty() {
                                if let Err(e) = lb_core::authority::validate(&req.authority) {
                                    tracing::warn!(
                                        authority = %req.authority,
                                        error = ?e,
                                        stream_id = sid,
                                        "ROUND8-L7-16: H3 :authority rejected \
                                         before upstream selection"
                                    );
                                    let resp =
                                        encode_h3_response(400, b"bad request").unwrap_or_default();
                                    request_tasks.push(tokio::spawn(async move { (sid, resp) }));
                                    continue;
                                }
                            }
                            // PROTO-001: H3→H2 takes precedence when
                            // h2_backend is configured.
                            if let Some((h2pool, addr)) = h2_backend {
                                let h2pool = h2pool.clone();
                                let addr = *addr;
                                request_tasks.push(tokio::spawn(async move {
                                    let bytes =
                                        Box::pin(h3_to_h2_roundtrip(&req, addr, &h2pool)).await;
                                    (sid, bytes)
                                }));
                                continue;
                            }
                            if let Some((qpool, addr, sni)) = h3_backend {
                                let qpool = qpool.clone();
                                let addr = *addr;
                                let sni = sni.clone();
                                request_tasks.push(tokio::spawn(async move {
                                    let bytes =
                                        Box::pin(h3_to_h3_roundtrip(&req, addr, &sni, &qpool))
                                            .await;
                                    (sid, bytes)
                                }));
                                continue;
                            }
                            let Some(backend) = select_backend(backends) else {
                                tracing::warn!("no backends available for H3 request");
                                continue;
                            };
                            // SESSION 2 / P1-A: spawn the INCREMENTAL
                            // streaming egress. The bounded body channel
                            // is the memory mechanism; the receiver is
                            // moved into the task, the sender stays here.
                            let pool = pool.clone();
                            let (btx, brx) =
                                mpsc::channel::<ReqBodyEvent>(H3_BODY_CHANNEL_DEPTH);
                            request_tasks.push(tokio::spawn(async move {
                                let bytes = match h3_to_h1_stream(
                                    &req,
                                    backend,
                                    &pool,
                                    brx,
                                    MAX_REQUEST_BODY_BYTES,
                                )
                                .await
                                {
                                    Ok(b) => b,
                                    Err(e) => {
                                        tracing::warn!(error = %e, "H3→H1 stream failed");
                                        Vec::new()
                                    }
                                };
                                (sid, bytes)
                            }));
                            if fin {
                                // Bodyless (HEADERS + FIN): the egress
                                // sees `End` first ⇒ byte-identical
                                // bodyless head. Don't register a body
                                // channel — request is complete.
                                let _ = btx.try_send(ReqBodyEvent::End {
                                    trailers: Vec::new(),
                                });
                            } else {
                                // Body to follow. Drain any post-HEADERS
                                // bytes that arrived coalesced in THIS
                                // recv, then hand the stream over to the
                                // body-phase drainer for later ticks.
                                body_tx_by_stream.insert(sid, btx);
                                body_pending.entry(sid).or_default();
                                decode_into_pending(
                                    sid,
                                    rx_by_stream,
                                    body_tx_by_stream,
                                    body_pending,
                                    &[],
                                    fin,
                                );
                                flush_pending(sid, body_tx_by_stream, body_pending);
                            }
                            // This stream is now in the Body phase;
                            // stop the headers recv loop for it.
                            break;
                        }
                        Ok(None) => {
                            if fin {
                                // FIN before a full HEADERS frame — a
                                // malformed/empty stream; nothing to
                                // forward. Drop any partial state.
                                rx_by_stream.remove(&sid);
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, stream_id = sid, "h3 decode");
                        }
                    }
                }
                Err(quiche::Error::Done) => break,
                Err(e) => {
                    tracing::debug!(error = %e, stream_id = sid, "stream_recv");
                    break;
                }
            }
        }
    }
}

/// SESSION 2 / P1-A — Body-phase drain for a stream that already has
/// an active bounded body channel. Backpressure: if the channel has
/// no spare capacity (and the pending queue is non-empty) we return
/// WITHOUT calling `stream_recv`, so quiche does not extend the QUIC
/// stream flow-control window and the H3 client is paused (genuine
/// end-to-end backpressure; max in-flight stays ≈ depth*chunk
/// regardless of total body size).
fn drain_body_stream(
    conn: &mut quiche::Connection,
    sid: u64,
    rx_by_stream: &mut HashMap<u64, StreamRxBuf>,
    body_tx_by_stream: &mut HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    body_pending: &mut HashMap<u64, VecDeque<ReqBodyEvent>>,
) {
    let mut buf = [0u8; 8192];
    loop {
        // First, push out anything already decoded-but-not-sent.
        flush_pending(sid, body_tx_by_stream, body_pending);
        if !body_tx_by_stream.contains_key(&sid) {
            // Stream completed / aborted while flushing.
            return;
        }
        // Backpressure gate: if items are still pending the channel is
        // full — do NOT pull more bytes off quiche this tick.
        let pending_empty = body_pending.get(&sid).is_none_or(VecDeque::is_empty);
        if !pending_empty {
            return;
        }
        match conn.stream_recv(sid, &mut buf) {
            Ok((n, fin)) => {
                decode_into_pending(
                    sid,
                    rx_by_stream,
                    body_tx_by_stream,
                    body_pending,
                    buf.get(..n).unwrap_or(&[]),
                    fin,
                );
                if !body_tx_by_stream.contains_key(&sid) {
                    flush_pending(sid, body_tx_by_stream, body_pending);
                    return;
                }
            }
            Err(quiche::Error::Done) => {
                flush_pending(sid, body_tx_by_stream, body_pending);
                return;
            }
            Err(e) => {
                tracing::debug!(error = %e, stream_id = sid, "stream_recv (body)");
                if let Some(tx) = body_tx_by_stream.remove(&sid) {
                    let _ = tx.try_send(ReqBodyEvent::Reset);
                }
                rx_by_stream.remove(&sid);
                body_pending.remove(&sid);
                return;
            }
        }
    }
}

/// SESSION 2 / P1-A — decode freshly-received Body-phase bytes into
/// ordered events and APPEND them to the per-stream pending queue. No
/// decoded item is ever dropped: a large DATA frame splits into many
/// ≤8 KiB chunks that wait here for channel capacity. On `TooLarge`
/// the channel is `Reset` and torn down; on `fin` an `End` event is
/// appended after the data so the egress terminates cleanly.
fn decode_into_pending(
    sid: u64,
    rx_by_stream: &mut HashMap<u64, StreamRxBuf>,
    body_tx_by_stream: &mut HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    body_pending: &mut HashMap<u64, VecDeque<ReqBodyEvent>>,
    chunk: &[u8],
    fin: bool,
) {
    let Some(rx) = rx_by_stream.get_mut(&sid) else {
        return;
    };
    let items = match rx.feed_body(chunk, MAX_REQUEST_BODY_BYTES) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, stream_id = sid, "h3 body decode");
            if let Some(tx) = body_tx_by_stream.remove(&sid) {
                let _ = tx.try_send(ReqBodyEvent::Reset);
            }
            rx_by_stream.remove(&sid);
            body_pending.remove(&sid);
            return;
        }
    };
    let q = body_pending.entry(sid).or_default();
    let mut trailers: Vec<(String, String)> = Vec::new();
    for item in items {
        match item {
            BodyItem::Data(b) => q.push_back(ReqBodyEvent::Chunk(b)),
            BodyItem::Trailers(t) => trailers = t,
            BodyItem::TooLarge => {
                // Cap exceeded: discard anything queued, signal the
                // egress to 413 + abort the upstream, and tear down.
                if let Some(tx) = body_tx_by_stream.remove(&sid) {
                    let _ = tx.try_send(ReqBodyEvent::Reset);
                }
                rx_by_stream.remove(&sid);
                body_pending.remove(&sid);
                return;
            }
        }
    }
    if fin {
        body_pending
            .entry(sid)
            .or_default()
            .push_back(ReqBodyEvent::End { trailers });
        rx_by_stream.remove(&sid);
    }
    // SESSION 2 / P1-A FIX (test-gauge): record the TOTAL per-stream
    // retained body memory at the largest point — right after the
    // decode and BEFORE flush drains anything: `StreamRxBuf` internal
    // buffer + every byte queued in `body_pending` + the bounded
    // channel occupancy (upper-bounded by used slots * chunk-max).
    // This FAILS if the body-phase parser buffered a whole DATA frame.
    #[cfg(any(test, feature = "test-gauges"))]
    record_retained_for_stream(sid, rx_by_stream, body_tx_by_stream, body_pending);
}

/// SESSION 2 / P1-A FIX (test-gauge): sum the per-stream retained body
/// memory and feed it to [`crate::h3_bridge::record_retained`].
#[cfg(any(test, feature = "test-gauges"))]
fn record_retained_for_stream(
    sid: u64,
    rx_by_stream: &HashMap<u64, StreamRxBuf>,
    body_tx_by_stream: &HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    body_pending: &HashMap<u64, VecDeque<ReqBodyEvent>>,
) {
    let rx_bytes = rx_by_stream
        .get(&sid)
        .map_or(0, crate::h3_bridge::StreamRxBuf::retained_bytes);
    let pending_bytes: usize = body_pending.get(&sid).map_or(0, |q| {
        q.iter()
            .map(|ev| match ev {
                ReqBodyEvent::Chunk(b) => b.len(),
                ReqBodyEvent::End { .. } | ReqBodyEvent::Reset => 0,
            })
            .sum()
    });
    // Channel occupancy is not byte-introspectable; every queued event
    // is a Chunk of <= H3_BODY_CHUNK_MAX, so used slots * chunk-max is a
    // sound UPPER bound (the gauge must over- not under-estimate).
    let chan_used = body_tx_by_stream.get(&sid).map_or(0, |tx| {
        tx.max_capacity().saturating_sub(tx.capacity())
    });
    let chan_bytes = chan_used.saturating_mul(crate::h3_bridge::H3_BODY_CHUNK_MAX);
    crate::h3_bridge::record_retained(rx_bytes + pending_bytes + chan_bytes);
}

/// SESSION 2 / P1-A — push as many pending events as the bounded
/// channel will currently accept (`try_reserve`), in order. Stops at
/// the first full / closed signal so the rest stay queued — that is
/// the backpressure point. Removes per-stream state once `End`/`Reset`
/// has been delivered.
fn flush_pending(
    sid: u64,
    body_tx_by_stream: &mut HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    body_pending: &mut HashMap<u64, VecDeque<ReqBodyEvent>>,
) {
    let Some(tx) = body_tx_by_stream.get(&sid) else {
        body_pending.remove(&sid);
        return;
    };
    let Some(q) = body_pending.get_mut(&sid) else {
        return;
    };
    let mut terminated = false;
    while !q.is_empty() {
        match tx.try_reserve() {
            Ok(permit) => {
                // Loop guard `!q.is_empty()` guarantees an element; the
                // `else` arm is unreachable but handled without panic.
                let Some(ev) = q.pop_front() else { break };
                let is_end =
                    matches!(ev, ReqBodyEvent::End { .. } | ReqBodyEvent::Reset);
                permit.send(ev);
                if is_end {
                    terminated = true;
                    break;
                }
            }
            Err(mpsc::error::TrySendError::Full(())) => break,
            Err(mpsc::error::TrySendError::Closed(())) => {
                // Egress task gone (upstream failed) — stop forwarding.
                terminated = true;
                break;
            }
        }
    }
    if terminated {
        body_tx_by_stream.remove(&sid);
        body_pending.remove(&sid);
    }
}

/// Round-robin-ish: pick the first backend for now. 3b.3c-2 does not
/// plumb a real balancer state through the router; the balancer crate
/// will own that when the router moves into the main binary path.
fn select_backend(backends: &Arc<Vec<SocketAddr>>) -> Option<SocketAddr> {
    backends.first().copied()
}
