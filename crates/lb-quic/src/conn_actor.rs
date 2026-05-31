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

use bytes::Bytes;

use crate::raw_proxy::{RawBackend, run_raw_proxy_actor};

use crate::h3_bridge::{
    BodyItem, H3_RESP_CHANNEL_DEPTH, H3Request, MAX_REQUEST_BODY_BYTES, MAX_RESPONSE_BODY_BYTES,
    FeedError, ReqBodyEvent, RespEvent, StreamRxBuf, encode_h3_response, h3_to_h1_stream_resp,
    h3_to_h2_stream_resp, h3_to_h3_stream_resp, validate_request_pseudo_headers,
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

/// SESSION 4 / P1-B (Q2 — team-lead ruling, approval condition C1):
/// the H3 application error code the actor puts on a `RESET_STREAM`
/// when an H1-upstream response is aborted mid-flight (upstream reset /
/// premature EOF / chunked-decode error / over-cap / bad head / client
/// cancel — every [`crate::h3_bridge::RespAbort`] variant).
///
/// `H3_INTERNAL_ERROR = 0x0102` is RFC 9114 §8.1's code for a
/// proxy/upstream-side failure to produce a faithful complete
/// response, which is exactly what every abort cause on this path is.
/// It is deliberately NOT [`H3_NO_ERROR`] (`0x0100`): signalling the
/// graceful-drain code on an abort would let a client/cache treat the
/// partial body as a complete response (truncated-as-complete /
/// cache-poisoning). It is deliberately NOT `H3_REQUEST_CANCELLED`
/// (`0x010c`): that implies the *requester* cancelled, which is the
/// distinct client-cancel path where the proxy does not RESET but
/// stops reading the upstream. A grep of `crates/lb-quic` found no
/// pre-existing reusable cancel/internal-error constant, so this is
/// the RFC-registered codepoint, not an invented value.
pub const H3_INTERNAL_ERROR: u64 = 0x0102;

/// SESSION 22 (h3spec #12–15) — RFC 9114 §8.1 `H3_MESSAGE_ERROR`
/// (`0x010e`): "a malformed request or response was received". Used to
/// **reset the request stream** when inbound HEADERS fail
/// [`crate::h3_bridge::validate_request_pseudo_headers`] (duplicate /
/// missing-mandatory / prohibited / mis-ordered pseudo-header). RFC 9114
/// §4.1.3 classifies a malformed message as a *stream* error, so this is
/// emitted via `stream_shutdown` (RESET_STREAM + STOP_SENDING), not a
/// connection close — the connection survives and other streams proceed.
pub const H3_MESSAGE_ERROR: u64 = 0x010e;

/// SESSION 22 (h3spec #11/#21) — RFC 9114 §8.1 `H3_FRAME_UNEXPECTED`
/// (`0x0105`): "a frame was received in a context where it is not
/// permitted". Emitted as a **connection** close (RFC 9114 §7.2 classifies
/// these as connection errors) when a request stream carries a
/// control-stream-only or out-of-sequence frame — DATA before HEADERS
/// (#11), or CANCEL_PUSH / SETTINGS / GOAWAY / MAX_PUSH_ID / PUSH_PROMISE
/// on a request stream (#21 + §7.2).
pub const H3_FRAME_UNEXPECTED: u64 = 0x0105;

/// SESSION 22 (h3spec #22) — RFC 9204 §8.3 `QPACK_DECOMPRESSION_FAILED`
/// (`0x0200`): the decoder failed to interpret an encoded field section
/// (e.g. an invalid static-table index, or a dynamic-table reference the
/// static-only decoder cannot satisfy). RFC 9204 §2.2 mandates this be a
/// **connection** error — emitted via `conn.close(true, …)` (an HTTP/3
/// application close).
pub const QPACK_DECOMPRESSION_FAILED: u64 = 0x0200;

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
    /// via [`h3_to_h3_stream_resp`] (bounded-incremental, R8) instead
    /// of the H1/TcpPool path. SESSION 7 / H3→H3 R8.
    pub h3_backend: Option<(QuicUpstreamPool, SocketAddr, String)>,
    /// Optional upstream H2 pool + single upstream H2 backend `(addr)`.
    /// When configured (and `h2_backend` is `Some`), the actor routes
    /// H3 requests via [`h3_to_h2_stream_resp`](crate::h3_bridge::h3_to_h2_stream_resp)
    /// (S6 R8 bounded-incremental). Takes precedence over `h3_backend`.
    /// PROTO-001 H3→H2 path.
    pub h2_backend: Option<(Http2Pool, SocketAddr)>,
    /// SESSION 16 / Mode B (terminate-and-re-originate) seam. When
    /// `Some`, [`run_actor`] dispatches to
    /// [`run_raw_proxy_actor`](crate::raw_proxy::run_raw_proxy_actor) at
    /// the very top — BEFORE any H3-specific local state is built — and
    /// the connection is proxied as raw QUIC (streams + datagrams) to a
    /// freshly re-originated upstream connection instead of being
    /// H3-terminated. When `None` (every existing caller) the H3
    /// termination path below runs byte-for-byte unchanged (R3
    /// no-regression). See `audit/quic/s16-plan.md` §1.
    pub raw_quic_backend: Option<RawBackend>,
    /// SESSION 19 / Mode B (B6) `quic_modeb_*` observability handles.
    /// `Some` only on a Mode-B actor spawned with a metrics registry;
    /// `None` on the H3-termination path (which never touches it — R3).
    /// Consumed by [`run_raw_proxy_actor`](crate::raw_proxy::run_raw_proxy_actor):
    /// the relay bumps the handles at its actor-lifetime + per-pass
    /// aggregate sites; the B4/B5 relay helpers are unaware of it.
    pub quic_modeb_metrics: Option<lb_observability::QuicModeBMetrics>,
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
    // SESSION 16 / Mode B splice point (plan §1). When a raw-QUIC
    // backend is configured this connection is terminated-and-
    // re-originated as raw QUIC, NOT H3-terminated: dispatch BEFORE any
    // H3-specific local state is built so the H3 path below stays
    // byte-for-byte unchanged when `raw_quic_backend` is `None` (R3).
    if params.raw_quic_backend.is_some() {
        return run_raw_proxy_actor(params).await;
    }

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
    // SESSION 4 / P1-B: per-stream bounded RESPONSE channels. The
    // `stream_h1_response` producer task owns the SENDER (the inverse
    // of the request side, where the actor owns the sender); the
    // RECEIVER stays here and is drained — under the §1.4.3
    // backpressure gate — into the stream's `Progressive` `StreamTx`.
    // Used by the H1 spawn site ONLY; H2/H3 + inline errors stay on the
    // legacy `request_tasks`/`task_wait` buffered path, untouched.
    let mut resp_rx_by_stream: HashMap<u64, mpsc::Receiver<RespEvent>> = HashMap::new();
    // SESSION 4 / P1-B: liveness handles for the response producer
    // tasks. NOT pushed into `request_tasks` (whose `(u64, Vec<u8>)`
    // type + its sole consumer, the legacy `finished` arm, must stay
    // untouched). Joined opportunistically to reap finished producers.
    let mut resp_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

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
        // SESSION 4 / P1-B: the same reasoning applies to an active
        // RESPONSE stream — the only thing that advances it is a tick
        // draining the bounded response channel into the `Progressive`
        // `StreamTx` as quiche frees send-window. Identical accepted S2
        // pattern; does NOT defeat backpressure (the §1.4.3 gate + the
        // bounded channel still cap in-flight bytes — we only poll the
        // gate more often so a backpressured response resumes promptly).
        if !body_tx_by_stream.is_empty()
            || !body_pending.is_empty()
            || !resp_rx_by_stream.is_empty()
        {
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
                &mut resp_rx_by_stream,
                &mut resp_tasks,
                &mut stream_response,
                &params.pool,
                &params.backends,
                params.h3_backend.as_ref(),
                params.h2_backend.as_ref(),
            );
        }

        // SESSION 5 / DEFECT-CLIENTGONE: detect client-cancel of the
        // response stream and tear it down (stops the upstream read;
        // binding C2 / §1.3.4). Must run before the §1.4.3 gate so the
        // cancelled stream is not re-driven this tick.
        reap_client_cancelled_responses(
            &mut params.conn,
            &mut resp_rx_by_stream,
            &mut stream_response,
        );

        // SESSION 4 / P1-B §1.4.3: the backpressure gate. Drain each
        // response receiver into its `Progressive` `StreamTx` ONLY
        // while that StreamTx's queue is empty — the memory bound and
        // the stall that propagates to the upstream. Then (§1.5) record
        // the retained-response gauge: this is the largest instant
        // (channel just refilled the StreamTx, bytes not yet handed to
        // quiche by the next `drain_streams_to_conn`).
        drain_resp_channels(&mut resp_rx_by_stream, &mut stream_response);

        // Reap finished response producers (liveness only; the actor
        // already observed their events / channel close).
        resp_tasks.retain(|h| !h.is_finished());
    }
    Ok(())
}

/// SESSION 5 / DEFECT-CLIENTGONE: a client that STOP_SENDINGs (or
/// RESET_STREAMs) the H3 RESPONSE stream must stop the upstream read.
/// quiche surfaces a peer STOP_SENDING on a stream we are *writing* as
/// `Err(StreamStopped)` from `stream_writable` (via `stream_capacity`);
/// a peer reset as `Err(StreamReset)`. For every stream with a live
/// response receiver, poll write-side status; on a peer cancel drop the
/// receiver — the producer's next `tx.send().await` then returns
/// `Err(RespAbort::ClientGone)`, so `h3_to_h1_stream_resp` marks the
/// pooled upstream NON-reusable and returns (binding C2 / §1.3.4) —
/// and drop the `StreamTx`. The proxy does NOT emit RESET_STREAM here:
/// the client already cancelled (§1.3.4 ClientGone — distinct from the
/// H3_INTERNAL_ERROR=0x0102 abort path). Mirrors the S2 request-side
/// StreamReset|StreamStopped arms (~conn_actor.rs:861/:944).
fn reap_client_cancelled_responses(
    conn: &mut quiche::Connection,
    resp_rx_by_stream: &mut HashMap<u64, mpsc::Receiver<RespEvent>>,
    stream_response: &mut HashMap<u64, StreamTx>,
) {
    let mut cancelled: Vec<u64> = Vec::new();
    for &sid in resp_rx_by_stream.keys() {
        match conn.stream_writable(sid, 1) {
            Err(quiche::Error::StreamStopped(code)) | Err(quiche::Error::StreamReset(code)) => {
                tracing::debug!(
                    stream_id = sid,
                    code,
                    "SESSION 5 DEFECT-CLIENTGONE: client cancelled H3 response \
                     stream; dropping receiver to stop upstream read (ClientGone)"
                );
                cancelled.push(sid);
            }
            _ => {}
        }
    }
    for sid in cancelled {
        // Drop the Receiver ⇒ producer's next tx.send() ⇒ ClientGone ⇒
        // h3_to_h1_stream_resp sets pooled non-reusable + returns (C2).
        resp_rx_by_stream.remove(&sid);
        // Drop the StreamTx: never FIN, never RESET_STREAM (ClientGone).
        stream_response.remove(&sid);
    }
}

/// SESSION 4 / P1-B §1.4.3 — the response-side backpressure gate.
///
/// For each stream with a live response receiver, refill its
/// `Progressive` `StreamTx` from the bounded channel **only while that
/// StreamTx's queue is empty**. Refusing to pull while the queue still
/// holds bytes (i.e. while quiche's send window is full and
/// `drain_streams_to_conn` has not yet shipped them) is the memory
/// bound: the channel fills, `stream_h1_response`'s `tx.send().await`
/// blocks, and the upstream socket read pauses — genuine end-to-end
/// backpressure, in-flight bytes ≈ channel depth, body-size
/// independent. Mirrors the request-side `pending_empty`/`flush_pending`
/// gate.
///
/// Event mapping: `Bytes` ⇒ push to the queue; `End` ⇒ set `ended`;
/// `Reset`, or the channel closing with no prior `End`, ⇒ set `reset`
/// (a partial body is never presented as complete — the
/// response-splitting / cache-poisoning guard, parity with the
/// request-side P1-B abort).
fn drain_resp_channels(
    resp_rx_by_stream: &mut HashMap<u64, mpsc::Receiver<RespEvent>>,
    stream_response: &mut HashMap<u64, StreamTx>,
) {
    let sids: Vec<u64> = resp_rx_by_stream.keys().copied().collect();
    for sid in sids {
        // The H1 spawn site inserts an empty `Progressive` StreamTx
        // alongside the receiver, so this entry exists; if a legacy
        // `Buffered` somehow occupies the slot we leave it untouched.
        let tx = stream_response
            .entry(sid)
            .or_insert_with(StreamTx::progressive);
        let StreamTx::Progressive {
            queue,
            ended,
            reset,
            fin_sent,
        } = tx
        else {
            continue;
        };
        if *fin_sent || *reset || *ended {
            // Terminal already decided; nothing more to pull. (Keep
            // the receiver until the stream is dropped by
            // `drain_streams_to_conn` so a late Reset is not lost.)
            continue;
        }
        // The gate: only refill an EMPTY queue.
        if !queue.is_empty() {
            continue;
        }
        let Some(rx) = resp_rx_by_stream.get_mut(&sid) else {
            continue;
        };
        // Pull exactly ONE event: one chunk is the gate granularity.
        // The queue must drain to quiche before we pull more — that is
        // the backpressure point (a non-empty queue ⇒ no pull next
        // tick ⇒ channel fills ⇒ producer `send().await` blocks ⇒
        // upstream read pauses).
        match rx.try_recv() {
            Ok(RespEvent::Bytes(b)) => queue.push_back(b),
            Ok(RespEvent::End) => *ended = true,
            Ok(RespEvent::Reset) => *reset = true,
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {
                // Producer gone. If it never signalled End/Reset
                // explicitly, treat as Reset — NEVER FIN a possibly
                // truncated body (truncated-as-complete guard).
                *reset = true;
            }
        }
    }

    // SESSION 4 / P1-B §1.5 (test-gauge): non-vacuous memory proof —
    // recorded here, the largest instant (StreamTx just refilled from
    // the channels, before `drain_streams_to_conn` ships bytes to
    // quiche). Σ progressive-queue bytes + a sound UPPER bound on
    // channel occupancy (`used_slots × (H3_RESP_CHUNK_MAX +
    // H3_FRAME_HDR_MAX)` — each queued `RespEvent::Bytes` is a
    // pre-encoded frame: ≤chunk payload + frame-header varints; the
    // gauge must over- not under-count, parity with the request gauge).
    #[cfg(any(test, feature = "test-gauges"))]
    {
        let mut total: usize = 0;
        for tx in stream_response.values() {
            if let StreamTx::Progressive { queue, .. } = tx {
                total = total.saturating_add(queue.iter().map(Bytes::len).sum());
            }
        }
        for rx in resp_rx_by_stream.values() {
            let used = rx.max_capacity().saturating_sub(rx.capacity());
            total = total.saturating_add(used.saturating_mul(
                crate::h3_bridge::H3_RESP_CHUNK_MAX + crate::h3_bridge::H3_FRAME_HDR_MAX,
            ));
        }
        crate::h3_bridge::record_resp_retained(total);
    }
}

/// Per-stream outbound cursor.
///
/// Two variants. `Buffered` is the LEGACY shape (a single pre-built
/// `Vec`, byte cursor + FIN-on-empty) — it is **unchanged** and still
/// serves H2/H3 round-trips and the inline 400/502/413 error responses
/// (bit-for-bit identical wire behaviour; SESSION 4 adds no buffering
/// to that path). `Progressive` is the SESSION 4 / P1-B incremental
/// H1-response egress: a bounded queue of pre-encoded H3 frame chunks
/// fed by [`stream_h1_response`] over a bounded channel, drained into
/// quiche as flow control allows. The queue + the channel are the
/// memory bound (≈ `H3_RESP_CHANNEL_DEPTH` × chunk), independent of
/// total response size.
enum StreamTx {
    /// Legacy: one pre-built `Vec`, byte cursor + FIN-on-empty.
    Buffered {
        bytes: Vec<u8>,
        sent: usize,
        finished: bool,
    },
    /// SESSION 4 / P1-B: progressive H1 response egress.
    ///
    /// `queue` holds pre-encoded H3 wire chunks not yet handed to
    /// quiche. `ended` ⇒ once `queue` drains, set FIN. `reset` ⇒
    /// `RESET_STREAM` (never FIN) — a partial body is never presented
    /// as complete (response-splitting / cache-poisoning guard).
    /// `fin_sent` guards the one-shot FIN/shutdown.
    Progressive {
        queue: VecDeque<Bytes>,
        ended: bool,
        reset: bool,
        fin_sent: bool,
    },
}

impl StreamTx {
    /// Construct the LEGACY buffered cursor. Unchanged signature +
    /// behaviour so every existing caller (`conn_actor.rs:206`, the
    /// H2/H3 + inline-error path) is bit-for-bit unaffected.
    const fn new(bytes: Vec<u8>) -> Self {
        Self::Buffered {
            bytes,
            sent: 0,
            finished: false,
        }
    }

    /// Construct an empty SESSION 4 / P1-B progressive egress cursor.
    fn progressive() -> Self {
        Self::Progressive {
            queue: VecDeque::new(),
            ended: false,
            reset: false,
            fin_sent: false,
        }
    }
}

/// Pump per-stream response bytes into quiche's send buffer. We send
/// incrementally because `stream_send` may refuse bytes when flow
/// control is saturated.
fn drain_streams_to_conn(conn: &mut quiche::Connection, streams: &mut HashMap<u64, StreamTx>) {
    let mut to_drop = Vec::new();
    for (&sid, tx) in streams.iter_mut() {
        match tx {
            // LEGACY buffered path — byte-for-byte the pre-SESSION-4
            // loop. H2/H3 + inline-error responses are unaffected.
            StreamTx::Buffered {
                bytes,
                sent,
                finished,
            } => {
                if *finished {
                    continue;
                }
                loop {
                    let remaining = bytes.get(*sent..).unwrap_or(&[]);
                    if remaining.is_empty() {
                        // All bytes in; send FIN separately via a
                        // zero-length send with fin=true.
                        match conn.stream_send(sid, &[], true) {
                            Ok(_) | Err(quiche::Error::Done) => {
                                *finished = true;
                            }
                            Err(e) => {
                                tracing::debug!(error = %e, stream_id = sid, "stream_send FIN");
                                *finished = true;
                            }
                        }
                        to_drop.push(sid);
                        break;
                    }
                    match conn.stream_send(sid, remaining, false) {
                        Ok(0) | Err(quiche::Error::Done) => break,
                        Ok(n) => {
                            *sent = sent.saturating_add(n);
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, stream_id = sid, "stream_send");
                            break;
                        }
                    }
                }
            }
            // SESSION 4 / P1-B progressive H1 egress.
            StreamTx::Progressive {
                queue,
                ended,
                reset,
                fin_sent,
            } => {
                if *fin_sent {
                    continue;
                }
                // Drain queued pre-encoded chunks front-to-back. On a
                // short/refused send, split the front chunk so the
                // unsent tail stays queued in order (no drop / reorder).
                while let Some(front) = queue.front_mut() {
                    match conn.stream_send(sid, front, false) {
                        Ok(0) | Err(quiche::Error::Done) => break,
                        Ok(n) if n >= front.len() => {
                            queue.pop_front();
                        }
                        Ok(n) => {
                            // Partial: advance past the sent prefix,
                            // keep the remainder at the queue front.
                            let _ = front.split_to(n);
                            break;
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, stream_id = sid, "stream_send (resp)");
                            break;
                        }
                    }
                }
                if *reset {
                    // Abort: RESET_STREAM, NEVER FIN — a partial body
                    // is never presented as a complete response (Q2 /
                    // C1: H3_INTERNAL_ERROR, not the graceful code).
                    match conn.stream_shutdown(sid, quiche::Shutdown::Write, H3_INTERNAL_ERROR) {
                        Ok(()) | Err(quiche::Error::Done) => {}
                        Err(e) => {
                            tracing::debug!(error = %e, stream_id = sid, "stream_shutdown (resp)");
                        }
                    }
                    *fin_sent = true;
                    to_drop.push(sid);
                } else if *ended && queue.is_empty() {
                    // Clean completion: FIN via a zero-length fin send
                    // (same mechanism as the legacy branch).
                    match conn.stream_send(sid, &[], true) {
                        Ok(_) | Err(quiche::Error::Done) => {}
                        Err(e) => {
                            tracing::debug!(error = %e, stream_id = sid, "stream_send FIN (resp)");
                        }
                    }
                    *fin_sent = true;
                    to_drop.push(sid);
                }
            }
        }
    }
    for sid in to_drop {
        // Mark terminal so subsequent calls skip it; remove lazily to
        // keep the allocation low (unchanged from the legacy policy).
        if let Some(tx) = streams.get_mut(&sid) {
            match tx {
                StreamTx::Buffered { finished, .. } => *finished = true,
                StreamTx::Progressive { fin_sent, .. } => *fin_sent = true,
            }
        }
    }
    streams.retain(|_, tx| match tx {
        StreamTx::Buffered { finished, .. } => !*finished,
        StreamTx::Progressive { fin_sent, .. } => !*fin_sent,
    });
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
///
/// SESSION 16 / Mode B (R12 single-source): exposed `pub(crate)` so
/// [`crate::raw_proxy`] drives both of its legs through the SAME pump
/// rather than carrying a byte-identical private copy. The low-level
/// send loop has no H3/Mode-A coupling — it is purely "flush quiche's
/// outbound packets to this socket until `Done`".
/// SESSION 22 — reset an H3 request stream with an application error
/// `code` (a STREAM error per RFC 9114 §4.1.3, e.g. `H3_MESSAGE_ERROR`).
/// Shuts the stream down in BOTH directions: `Write` emits `RESET_STREAM`
/// (we will send no response) and `Read` emits `STOP_SENDING` (we want no
/// more request bytes). Both frames are queued on `conn`; the actor loop's
/// next `drain_conn_send` pumps them to the peer (a queued reset is inert
/// until `conn.send()` runs — see the `quiche-reset-needs-a-flush-pump`
/// lesson). This works at H3-frame time because the connection is already
/// established (`recv_count > 0`), unlike the suppressed first-packet
/// transport close documented in `audit/h3spec/s22-findings.md` (#1–8).
fn reset_h3_stream(conn: &mut quiche::Connection, sid: u64, code: u64) {
    match conn.stream_shutdown(sid, quiche::Shutdown::Write, code) {
        Ok(()) | Err(quiche::Error::Done) => {}
        Err(e) => tracing::debug!(error = %e, stream_id = sid, "reset_h3_stream (RESET_STREAM)"),
    }
    match conn.stream_shutdown(sid, quiche::Shutdown::Read, code) {
        Ok(()) | Err(quiche::Error::Done) => {}
        Err(e) => tracing::debug!(error = %e, stream_id = sid, "reset_h3_stream (STOP_SENDING)"),
    }
}

pub(crate) async fn drain_conn_send(
    socket: &UdpSocket,
    conn: &mut quiche::Connection,
    out_buf: &mut [u8],
) {
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
    // SESSION 4 / P1-B: the H1 spawn site moves its response off the
    // legacy `request_tasks`/`task_wait` Vec path onto the bounded
    // RESPONSE channel. `resp_rx_by_stream` (actor-owned receivers) +
    // an empty `Progressive` `StreamTx` are registered here; the
    // producer `JoinHandle` goes to `resp_tasks` (NOT `request_tasks`).
    // The inline-400 / h2 / h3 spawns + the legacy Vec path are
    // untouched.
    resp_rx_by_stream: &mut HashMap<u64, mpsc::Receiver<RespEvent>>,
    resp_tasks: &mut Vec<tokio::task::JoinHandle<()>>,
    stream_response: &mut HashMap<u64, StreamTx>,
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
        // SESSION 22 — the HTTP/3 request decoder (incl. the §4.1/§7.2
        // frame-sequencing guards in `StreamRxBuf::feed`) applies ONLY to
        // client-initiated BIDIRECTIONAL streams (request streams, id % 4 ==
        // 0). Client UNIDIRECTIONAL streams (id % 4 == 2 — the H3 control
        // stream and the QPACK encoder/decoder streams) begin with a
        // stream-TYPE varint + control frames, NOT a request: feeding them
        // through the request decoder would mis-read the control-stream type
        // byte (0x00) as a DATA frame and wrongly close the connection with
        // H3_FRAME_UNEXPECTED. The gateway does not yet enforce client
        // control-stream rules (h3spec #16–20/#24 — carried to S23); until
        // then it DRAINS + discards client uni streams so they neither block
        // flow control nor trip the request-stream guards. (Server-initiated
        // streams never appear here — the gateway writes those.)
        if sid % 4 != 0 {
            let mut sink = [0u8; 4096];
            while let Ok((_n, _fin)) = conn.stream_recv(sid, &mut sink) {}
            continue;
        }
        let mut buf = [0u8; 8192];
        loop {
            match conn.stream_recv(sid, &mut buf) {
                Ok((n, fin)) => {
                    let rx = rx_by_stream.entry(sid).or_default();
                    match rx.feed(buf.get(..n).unwrap_or(&[])) {
                        Ok(Some(headers)) => {
                            // SESSION 22 (h3spec #12–15): RFC 9114 §4.3
                            // request pseudo-header validation BEFORE
                            // building the request or dialling any
                            // upstream. A malformed request is a STREAM
                            // error of type H3_MESSAGE_ERROR (§4.1.3): reset
                            // the stream and forward NOTHING upstream
                            // (request integrity — a smuggling/desync
                            // guard). Single ingress site ⇒ covers all
                            // H3-front cells (R12). The reset frames are
                            // pumped by the loop's next `drain_conn_send`.
                            if let Err(reason) = validate_request_pseudo_headers(&headers) {
                                tracing::warn!(
                                    stream_id = sid,
                                    reason,
                                    "SESSION 22: malformed H3 request rejected \
                                     (H3_MESSAGE_ERROR, RFC 9114 §4.1.3)"
                                );
                                reset_h3_stream(conn, sid, H3_MESSAGE_ERROR);
                                rx_by_stream.remove(&sid);
                                break;
                            }
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
                            // S6 / H3→H2 R8 (I3): H3→H2 takes precedence
                            // when h2_backend is configured. Was the
                            // BUFFERED, request-body-DROPPING
                            // `h3_to_h2_roundtrip` on the legacy
                            // `request_tasks` Vec path; now the SAME
                            // bounded-incremental request+response
                            // producer shape the H3→H1 cell proved —
                            // `h3_to_h2_stream_resp` on the
                            // `resp_tasks` path with the identical
                            // (btx/brx)+(resp_tx/resp_rx)+Progressive
                            // StreamTx registration and the shared
                            // `fin`/body-channel tail below (so a slow
                            // H2 upstream backpressures the H3 client,
                            // and the request body is forwarded — no
                            // longer silently deleted). H3→H3 + the
                            // inline errors keep the legacy Vec path.
                            if let Some((h2pool, addr)) = h2_backend {
                                let h2pool = h2pool.clone();
                                let addr = *addr;
                                let (btx, brx) =
                                    mpsc::channel::<ReqBodyEvent>(H3_BODY_CHANNEL_DEPTH);
                                let (resp_tx, resp_rx) =
                                    mpsc::channel::<RespEvent>(H3_RESP_CHANNEL_DEPTH);
                                resp_rx_by_stream.insert(sid, resp_rx);
                                stream_response.insert(sid, StreamTx::progressive());
                                resp_tasks.push(tokio::spawn(async move {
                                    if let Err(abort) = h3_to_h2_stream_resp(
                                        &req,
                                        addr,
                                        &h2pool,
                                        brx,
                                        resp_tx,
                                        MAX_RESPONSE_BODY_BYTES,
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            ?abort,
                                            stream_id = sid,
                                            "H3→H2 resp stream aborted"
                                        );
                                    }
                                }));
                                if fin {
                                    // Bodyless (HEADERS + FIN): the
                                    // egress peeks `End` first ⇒
                                    // legitimately bodyless request.
                                    let _ = btx.try_send(ReqBodyEvent::End {
                                        trailers: Vec::new(),
                                    });
                                } else {
                                    // Body to follow — identical
                                    // body-channel handover to the H1
                                    // streaming path (M-A pump,
                                    // unchanged).
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
                                // Stream is now in the Body phase; stop
                                // the headers recv loop for it.
                                break;
                            }
                            // SESSION 7 / H3→H3 R8 (J3): H3→H3 when
                            // h3_backend is configured. Was the
                            // BUFFERED, request-body-DROPPING H3→H3
                            // round-trip on the legacy
                            // `request_tasks` Vec path; now the SAME
                            // bounded-incremental request+response
                            // producer shape the H3→H1/H3→H2 cells
                            // proved — `h3_to_h3_stream_resp` on the
                            // `resp_tasks` path with the identical
                            // (btx/brx)+(resp_tx/resp_rx)+Progressive
                            // StreamTx registration and the shared
                            // `fin`/body-channel tail (so a slow H3
                            // upstream backpressures the H3 client, and
                            // the request body is forwarded — no longer
                            // silently deleted). This block is a
                            // token-for-token clone of the verified
                            // H3→H2 branch above; the ONLY deltas are
                            // the `sni` destructure, the spawned fn
                            // (`h3_to_h3_stream_resp` + `&sni`), and the
                            // warn label.
                            if let Some((qpool, addr, sni)) = h3_backend {
                                let qpool = qpool.clone();
                                let addr = *addr;
                                let sni = sni.clone();
                                let (btx, brx) =
                                    mpsc::channel::<ReqBodyEvent>(H3_BODY_CHANNEL_DEPTH);
                                let (resp_tx, resp_rx) =
                                    mpsc::channel::<RespEvent>(H3_RESP_CHANNEL_DEPTH);
                                resp_rx_by_stream.insert(sid, resp_rx);
                                stream_response.insert(sid, StreamTx::progressive());
                                resp_tasks.push(tokio::spawn(async move {
                                    if let Err(abort) = h3_to_h3_stream_resp(
                                        &req,
                                        addr,
                                        &sni,
                                        &qpool,
                                        brx,
                                        resp_tx,
                                        MAX_RESPONSE_BODY_BYTES,
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            ?abort,
                                            stream_id = sid,
                                            "H3→H3 resp stream aborted"
                                        );
                                    }
                                }));
                                if fin {
                                    // Bodyless (HEADERS + FIN): the
                                    // egress peeks `End` first ⇒
                                    // legitimately bodyless request.
                                    let _ = btx.try_send(ReqBodyEvent::End {
                                        trailers: Vec::new(),
                                    });
                                } else {
                                    // Body to follow — identical
                                    // body-channel handover to the H1
                                    // streaming path (M-A pump,
                                    // unchanged).
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
                                // Stream is now in the Body phase; stop
                                // the headers recv loop for it.
                                break;
                            }
                            let Some(backend) = select_backend(backends) else {
                                tracing::warn!("no backends available for H3 request");
                                continue;
                            };
                            // SESSION 4 / P1-B: spawn the INCREMENTAL
                            // request + INCREMENTAL response producer.
                            // The request-body channel (btx/brx) is the
                            // request-side memory mechanism (unchanged
                            // from S2). The NEW response channel
                            // (resp_tx/resp_rx) is the response-side
                            // one: the producer owns the sender, the
                            // actor owns the receiver (registered here)
                            // and drains it under the §1.4.3
                            // backpressure gate into an (empty)
                            // `Progressive` `StreamTx`. The producer
                            // `JoinHandle` goes to `resp_tasks`, NOT
                            // `request_tasks` — the legacy
                            // `request_tasks`/`task_wait` Vec path stays
                            // byte-for-byte and still serves H2/H3 +
                            // the inline-400/502/413 errors. C2: the
                            // producer owns the `PooledTcp` and marks
                            // it non-reusable on every abort/clean
                            // outcome (inside `h3_to_h1_stream_resp`).
                            let pool = pool.clone();
                            let (btx, brx) = mpsc::channel::<ReqBodyEvent>(H3_BODY_CHANNEL_DEPTH);
                            let (resp_tx, resp_rx) =
                                mpsc::channel::<RespEvent>(H3_RESP_CHANNEL_DEPTH);
                            resp_rx_by_stream.insert(sid, resp_rx);
                            stream_response.insert(sid, StreamTx::progressive());
                            resp_tasks.push(tokio::spawn(async move {
                                if let Err(abort) = h3_to_h1_stream_resp(
                                    &req,
                                    backend,
                                    &pool,
                                    brx,
                                    resp_tx,
                                    MAX_RESPONSE_BODY_BYTES,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        ?abort,
                                        stream_id = sid,
                                        "H3→H1 resp stream aborted"
                                    );
                                }
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
                        // SESSION 22 (h3spec #11/#21): a control-only or
                        // out-of-sequence frame on a request stream is a
                        // CONNECTION error of type H3_FRAME_UNEXPECTED
                        // (RFC 9114 §7.2). Close the whole connection; the
                        // loop's next `drain_conn_send` emits the
                        // CONNECTION_CLOSE and `is_closed()` then breaks the
                        // actor (the H3 conn is established, so the close is
                        // not suppressed — cf. the transport #1-8 case).
                        Err(FeedError::FrameUnexpected(reason)) => {
                            tracing::warn!(
                                stream_id = sid,
                                reason,
                                "SESSION 22: H3_FRAME_UNEXPECTED — closing connection (RFC 9114 §7.2)"
                            );
                            // `app = true`: H3_FRAME_UNEXPECTED is an HTTP/3
                            // APPLICATION error code (RFC 9114 §8.1), so it
                            // must ride an application CONNECTION_CLOSE
                            // (frame 0x1d), not a transport one (0x1c) — a
                            // transport close would carry the code in the
                            // wrong error space and h3spec would not see the
                            // expected H3 error. (Mirrors graceful_h3_shutdown's
                            // `conn.close(true, H3_NO_ERROR, …)`.)
                            match conn.close(true, H3_FRAME_UNEXPECTED, reason.as_bytes()) {
                                Ok(()) | Err(quiche::Error::Done) => {}
                                Err(e) => {
                                    tracing::debug!(error = %e, "conn.close (H3_FRAME_UNEXPECTED)");
                                }
                            }
                            rx_by_stream.remove(&sid);
                            break;
                        }
                        // SESSION 22 (h3spec #22): a QPACK field-section
                        // decode failure is a CONNECTION error
                        // QPACK_DECOMPRESSION_FAILED (RFC 9204 §2.2).
                        // app=true (application close — see the
                        // FrameUnexpected arm above).
                        Err(FeedError::QpackDecompressionFailed(e)) => {
                            tracing::warn!(
                                error = %e,
                                stream_id = sid,
                                "SESSION 22: QPACK_DECOMPRESSION_FAILED — closing connection (RFC 9204 §2.2)"
                            );
                            match conn.close(true, QPACK_DECOMPRESSION_FAILED, b"qpack decompression failed") {
                                Ok(()) | Err(quiche::Error::Done) => {}
                                Err(e) => {
                                    tracing::debug!(error = %e, "conn.close (QPACK_DECOMPRESSION_FAILED)");
                                }
                            }
                            rx_by_stream.remove(&sid);
                            break;
                        }
                        Err(FeedError::Decode(e)) => {
                            tracing::warn!(error = %e, stream_id = sid, "h3 decode");
                        }
                    }
                }
                Err(quiche::Error::Done) => break,
                // SESSION 2 / P1-B: peer reset/stopped the stream while
                // we were still accumulating HEADERS (no body channel
                // exists yet, so nothing to forward). Drop the partial
                // per-stream rx buffer so no state leaks, then stop the
                // recv loop for this stream. Other errors are handled
                // identically (fail safe).
                Err(quiche::Error::StreamReset(code)) | Err(quiche::Error::StreamStopped(code)) => {
                    tracing::debug!(
                        stream_id = sid,
                        code,
                        "SESSION 2 / P1-B: peer reset/stopped stream during HEADERS; \
                         dropping partial per-stream state"
                    );
                    rx_by_stream.remove(&sid);
                    break;
                }
                Err(e) => {
                    tracing::debug!(error = %e, stream_id = sid, "stream_recv");
                    rx_by_stream.remove(&sid);
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
            // SESSION 2 / P1-B: CLIENT CANCELS MID-BODY. When the H3
            // peer aborts the request stream (QUIC RESET_STREAM /
            // STOP_SENDING) before FIN, quiche surfaces it here as
            // `Err(quiche::Error::StreamReset(_))` (peer reset its send
            // side) or `Err(quiche::Error::StreamStopped(_))` (peer
            // STOP_SENDING our receive side). Either way the body is
            // incomplete and MUST NOT be presented to the backend as a
            // completed request (HTTP-request-smuggling / cache-poisoning
            // guard). We send `ReqBodyEvent::Reset` into the body channel
            // — `h3_to_h1_stream` aborts the upstream (marks the pooled
            // conn non-reusable, returns WITHOUT writing the `0\r\n\r\n`
            // chunked terminator) — and tear down ALL per-stream state so
            // nothing leaks. Any other `stream_recv` error is treated
            // identically (fail safe: never forward a partial body as
            // complete). Matched explicitly so the smuggling-relevant
            // reset path is unmistakable and regression-locked.
            Err(quiche::Error::StreamReset(code)) | Err(quiche::Error::StreamStopped(code)) => {
                tracing::debug!(
                    stream_id = sid,
                    code,
                    "SESSION 2 / P1-B: peer reset/stopped request stream mid-body; \
                     aborting upstream + tearing down per-stream state"
                );
                if let Some(tx) = body_tx_by_stream.remove(&sid) {
                    let _ = tx.try_send(ReqBodyEvent::Reset);
                }
                rx_by_stream.remove(&sid);
                body_pending.remove(&sid);
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
    let chan_used = body_tx_by_stream
        .get(&sid)
        .map_or(0, |tx| tx.max_capacity().saturating_sub(tx.capacity()));
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
                let is_end = matches!(ev, ReqBodyEvent::End { .. } | ReqBodyEvent::Reset);
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
