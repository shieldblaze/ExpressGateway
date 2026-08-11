//! Per-connection actor driving one [`quiche::Connection`] to established, then
//! pumping H3 requests through the [`crate::h3_bridge`] to a backend.
//!
//! H3 ownership sits inside this actor rather than in a separate driver because
//! every `stream_recv`/`stream_send` needs `&mut quiche::Connection`: splitting
//! the actor in two would put a mutex on the hot path. Per-stream state
//! (read buffers, response queues) therefore lives in `HashMap`s inline.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use lb_io::http2_pool::Http2Pool;
use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;

use bytes::Bytes;

use crate::raw_proxy::{RawBackend, run_raw_proxy_actor};

use crate::h3_bridge::{
    H3_BODY_CHUNK_MAX, H3_RESP_CHANNEL_DEPTH, H3Request, MAX_REQUEST_BODY_BYTES,
    MAX_RESPONSE_BODY_BYTES, ReqBodyEvent, RespEvent, h3_to_h1_stream_resp, h3_to_h2_stream_resp,
    h3_to_h3_stream_resp, validate_request_pseudo_headers,
};

use crate::ws_tunnel::{
    H3TunnelEndpoints, H3WsTunnel, TunnelInbound, WsConnectRequest, WsRelayHandle, WsRelayLauncher,
    WsUpstreamOutcome,
};

/// Depth of the per-stream bounded request-body channel. With
/// `h3_bridge::H3_BODY_CHUNK_MAX` this caps in-flight body memory INDEPENDENT
/// of total body size — the memory-safety mechanism. When the channel is full
/// `poll_h3` stops calling `stream_recv`, so quiche does not extend the stream
/// flow-control window and the client is paused (end-to-end backpressure).
pub const H3_BODY_CHANNEL_DEPTH: usize = 8;

/// Application error code in the `CONNECTION_CLOSE` sent when the
/// listener-wide cancel token fires. RFC 9114 §8.1's "graceful drain" signal —
/// a conformant peer reads it as an orderly shutdown, not an abort.
pub const H3_NO_ERROR: u64 = 0x0100;

/// The code the actor puts on a `RESET_STREAM` when an upstream response is
/// aborted mid-flight (every [`crate::h3_bridge::RespAbort`] variant).
///
/// RFC 9114 §8.1 `H3_INTERNAL_ERROR` is the proxy-side "could not produce a
/// faithful complete response" code. Deliberately NOT [`H3_NO_ERROR`]
/// (`0x0100`): the graceful-drain code on an abort would let a client or cache
/// treat the partial body as complete (truncated-as-complete / cache
/// poisoning). Deliberately NOT `H3_REQUEST_CANCELLED` (`0x010c`), which
/// implies the *requester* cancelled — a distinct path where the proxy does
/// not RESET but stops reading the upstream.
pub const H3_INTERNAL_ERROR: u64 = 0x0102;

/// RFC 9114 §8.1 `H3_MESSAGE_ERROR` — a malformed request. Resets the request
/// stream when inbound HEADERS fail
/// [`crate::h3_bridge::validate_request_pseudo_headers`]. §4.1.3 classifies a
/// malformed message as a *stream* error, so this goes via `stream_shutdown`,
/// not a connection close: the connection survives and other streams proceed.
pub const H3_MESSAGE_ERROR: u64 = 0x010e;

/// RFC 9114 §8.1 `H3_FRAME_UNEXPECTED` — a frame in a context where it is not
/// permitted (DATA before HEADERS; a control-stream-only frame on a request
/// stream). Emitted as a **connection** close: §7.2 classifies these as
/// connection errors.
pub const H3_FRAME_UNEXPECTED: u64 = 0x0105;

/// RFC 9204 §8.3 `QPACK_DECOMPRESSION_FAILED` — the decoder could not
/// interpret an encoded field section. §2.2 mandates a **connection** error.
pub const QPACK_DECOMPRESSION_FAILED: u64 = 0x0200;

/// Budget for pumping the connection after `close()`. Quiche drains for
/// `3 * PTO` (RFC 9000 §10.1), comfortably under this.
const GRACEFUL_SHUTDOWN_BUDGET: Duration = Duration::from_millis(500);

/// RFC 9114 §8.1 `H3_REQUEST_CANCELLED`, emitted on the `RESET_STREAM` when a
/// WebSocket tunnel stream is torn down abnormally (RFC 9220 §3 mapping).
const H3_REQUEST_CANCELLED: u64 = 0x010c;

/// RFC 9114 §8.1 `H3_REQUEST_REJECTED` — "rejected by the server without
/// processing". Reset onto a request stream arriving AFTER the cap-triggered
/// GOAWAY: §5.2 lets the client retry such a stream on a fresh connection,
/// which is exactly the recycle semantics.
const H3_REQUEST_REJECTED: u64 = 0x010b;

/// Per-stream WebSocket tunnel state for a sid carrying a validated
/// `:protocol=websocket` extended CONNECT. The actor shuttles bytes between the
/// H3 stream and the injected relay over two bounded channels using only
/// non-blocking `try_send`/`try_recv`, so the sync poll loop never awaits.
struct WsTunnelState {
    /// Actor→relay (inbound: H3 stream DATA → `proxy_frames` reader).
    to_reader: Option<mpsc::Sender<TunnelInbound>>,
    /// Relay→actor (outbound: `proxy_frames` writer → H3 stream DATA).
    from_writer: mpsc::Receiver<Bytes>,
    /// Upstream-handshake readiness — resolves once, BEFORE the `200`.
    ready: Option<oneshot::Receiver<WsUpstreamOutcome>>,
    /// Response head still to encode before the tunnel activates.
    pending_ok: Option<WsPendingOk>,
    /// `true` once the `200` is on the wire — the tunnel-mode pump runs.
    activated: bool,
    /// Unsent tail of the chunk currently being written outbound (the R8
    /// retain-and-retry buffer).
    out_pending: Option<Bytes>,
    /// Set once we FIN the H3 stream outbound (the relay finished).
    fin_sent: bool,
    /// Marks the state for removal at the end of the tick.
    done: bool,
    /// The relay task (dial + upstream handshake + `proxy_frames`).
    task: tokio::task::JoinHandle<()>,
}

/// The success (`200`) response head queued for a WS extended CONNECT, held
/// until the upstream handshake resolves.
struct WsPendingOk {
    /// Extra response fields (e.g. the upstream-selected subprotocol).
    headers: Vec<(String, String)>,
}

/// Raw UDP packet forwarded from the router to a single actor.
#[derive(Debug)]
pub struct InboundPacket {
    /// Receive buffer (owned — one allocation per packet).
    pub data: Vec<u8>,
    /// Peer address the packet came from.
    pub from: SocketAddr,
    /// Local address the packet came in on.
    pub to: SocketAddr,
}

/// Construction parameters for [`ConnectionActor`].
pub struct ActorParams {
    /// The `quiche::Connection` handed over by the router after accept.
    pub conn: quiche::Connection,
    /// Shared outbound socket (all actors on one listener share this).
    pub socket: Arc<UdpSocket>,
    /// Bounded channel receiver; the router pushes this CID's packets in.
    pub inbound: mpsc::Receiver<InboundPacket>,
    /// Listener-wide cancellation token.
    pub cancel: CancellationToken,
    /// Backend TCP pool shared across all listeners.
    pub pool: TcpPool,
    /// Resolved backend addresses for H1 backends (round-robin).
    pub backends: Arc<Vec<SocketAddr>>,
    /// Optional upstream H3 pool + backend `(addr, sni)`.
    pub h3_backend: Option<(QuicUpstreamPool, SocketAddr, String)>,
    /// Optional upstream H2 pool + backend `(addr)`.
    pub h2_backend: Option<(Http2Pool, SocketAddr)>,
    /// Mode B (terminate-and-re-originate) seam. When `Some`, [`run_actor`]
    /// dispatches to [`crate::raw_proxy`] BEFORE any H3 state is built, so the
    /// H3 path is byte-identical when it is `None`.
    pub raw_quic_backend: Option<RawBackend>,
    /// Mode B `quic_modeb_*` observability handles; `None` ⇒ every update is
    /// a no-op.
    pub quic_modeb_metrics: Option<lb_observability::QuicModeBMetrics>,
    /// WS-over-H3 (RFC 9220) Stage A: whether this listener accepts extended
    /// CONNECT. When `false` `:protocol` is rejected as an unregistered
    /// pseudo-header, byte-identically to a pre-WS listener.
    pub ws_enabled: bool,
    /// WS-over-H3 Stage C: the injected relay launcher. `lb-quic` cannot
    /// depend on the L7 relay (dependency cycle), so the binary injects the
    /// closure — the same seam as `config_factory`. `None` ⇒ no WS relay.
    pub ws_relay_launcher: Option<crate::ws_tunnel::WsRelayLauncher>,
    /// S36-A: per-connection H3 request cap. Non-zero ⇒ after this many
    /// request streams the connection emits a GOAWAY and recycles. `0`
    /// disables recycling entirely (byte-identical to the pre-S36 front).
    pub max_requests_per_h3_connection: u32,
    /// S36-A: the `h3_*` recycle metric handles; `None` ⇒ no-op.
    pub h3_recycle_metrics: Option<lb_observability::QuicH3RecycleMetrics>,
}

/// Drive one `quiche::Connection` to completion, terminating H3 and forwarding
/// to a backend.
///
/// # Errors
///
/// Never — all errors are logged and swallowed. The `io::Result<()>` shape
/// exists so callers can chain.
pub async fn run_actor(mut params: ActorParams) -> std::io::Result<()> {
    // Mode B splice point: dispatch BEFORE any H3-specific local state is
    // built, so the H3 path below stays byte-identical when this is `None`.
    if params.raw_quic_backend.is_some() {
        return run_raw_proxy_actor(params).await;
    }

    let mut out_buf = vec![0u8; 65_535];
    // H3 ingress rides `quiche::h3::Connection`, built lazily once the
    // connection is established.
    let mut h3: Option<quiche::h3::Connection> = None;
    let mut stream_response: HashMap<u64, StreamTx> = HashMap::new();
    // Per-stream bounded request-body channels — the R8 memory bound.
    let mut body_tx_by_stream: HashMap<u64, mpsc::Sender<ReqBodyEvent>> = HashMap::new();
    // Cumulative request-body bytes per stream, for the F-CAP-1 413 cap.
    let mut body_seen: HashMap<u64, usize> = HashMap::new();
    // Request trailers (RFC 9114 §4.1) arrive as a second HEADERS event, so
    // they are staged here until the stream's clean end.
    let mut pending_trailers: HashMap<u64, Vec<(String, String)>> = HashMap::new();
    // Per-stream bounded RESPONSE channels — the R8 response-side bound.
    let mut resp_rx_by_stream: HashMap<u64, mpsc::Receiver<RespEvent>> = HashMap::new();
    // Liveness handles for the response producer tasks.
    let mut resp_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    // Per-stream WebSocket tunnel state; empty ⇒ every WS branch is inert.
    let mut ws_tunnels: HashMap<u64, WsTunnelState> = HashMap::new();

    // S36-A connection-recycling state. `cap == 0` disables the whole block.
    // `goaway_pending` and `goaway_sent` are DELIBERATELY separate:
    // admission must stop the instant the cap trips, but the recycle must wait
    // until the GOAWAY frame is actually queued. Collapsing them re-opens the
    // window in which a request is admitted past the boundary.
    let cap = params.max_requests_per_h3_connection;
    let mut requests_served: u64 = 0;
    let mut goaway_pending = false;
    let mut goaway_sent = false;
    let mut goaway_last_id: u64 = 0;

    loop {
        // Push outbound bytes + progressive response bytes before waiting.
        drain_streams_to_conn(&mut params.conn, h3.as_mut(), &mut stream_response);
        drain_conn_send(&params.socket, &mut params.conn, &mut out_buf).await;

        if params.conn.is_closed() {
            break;
        }

        let mut next_wait = params.conn.timeout().unwrap_or(Duration::from_millis(100));
        // While a request body is actively streaming, cap the wait at a short
        // tick: quiche's idle timeout can be hundreds of ms and would throttle
        // the body relay. This does not defeat backpressure — the bounded
        // channel still caps in-flight bytes; we only poll the gate more often.
        if !body_tx_by_stream.is_empty() || !resp_rx_by_stream.is_empty() || !ws_tunnels.is_empty()
        {
            next_wait = next_wait.min(Duration::from_millis(2));
        }

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
            () = tokio::time::sleep(next_wait) => {
                params.conn.on_timeout();
            }
        }

        // Post-event: poll H3 streams if established.
        if params.conn.is_established() {
            // Build the `quiche::h3::Connection` once, post-establishment.
            if h3.is_none() {
                // Static-only QPACK config (see `crate::h3_config`).
                match crate::h3_config::build_server_h3_config(params.ws_enabled)
                    .and_then(|cfg| quiche::h3::Connection::with_transport(&mut params.conn, &cfg))
                {
                    Ok(c) => h3 = Some(c),
                    Err(e) => {
                        tracing::warn!(error = %e, "INC-2: h3 init (config/with_transport) failed; closing connection");
                        match params.conn.close(true, H3_INTERNAL_ERROR, b"h3 init") {
                            Ok(()) | Err(quiche::Error::Done) => {}
                            Err(e) => tracing::debug!(error = %e, "conn.close (h3 init)"),
                        }
                    }
                }
            }
            if let Some(h3c) = h3.as_mut() {
                poll_h3(
                    &mut params.conn,
                    h3c,
                    &mut body_tx_by_stream,
                    &mut body_seen,
                    &mut pending_trailers,
                    &mut resp_rx_by_stream,
                    &mut resp_tasks,
                    &mut stream_response,
                    &params.pool,
                    &params.backends,
                    params.h3_backend.as_ref(),
                    params.h2_backend.as_ref(),
                    params.ws_enabled,
                    &mut ws_tunnels,
                    params.ws_relay_launcher.as_ref(),
                    // S36-A: connection-recycling cap + state + metrics.
                    cap,
                    &mut requests_served,
                    &mut goaway_pending,
                    &mut goaway_sent,
                    &mut goaway_last_id,
                    params.h3_recycle_metrics.as_ref(),
                );
                // S36-A: retry a cap GOAWAY whose first send hit a full
                // control-stream window — the triggering client may send
                // nothing more, so the retry cannot live only in `poll_h3`.
                try_send_pending_goaway(
                    &mut params.conn,
                    h3c,
                    &mut goaway_pending,
                    &mut goaway_sent,
                    goaway_last_id,
                    params.h3_recycle_metrics.as_ref(),
                );
            }
        }

        // DEFECT-CLIENTGONE: detect a client cancel of the response stream and
        // stop the upstream read.
        reap_client_cancelled_responses(
            &mut params.conn,
            &mut resp_rx_by_stream,
            &mut stream_response,
        );

        // §1.4.3: the response backpressure gate — refill each `StreamTx`
        // ONLY while its queue is empty.
        drain_resp_channels(&mut resp_rx_by_stream, &mut stream_response);

        // Reap finished response producers (liveness only).
        resp_tasks.retain(|h| !h.is_finished());

        // S36-A DRAIN-THEN-RECYCLE: once the cap GOAWAY is out, close the
        // connection only after every in-flight response has drained, so a
        // recycle never truncates a response already being served.
        if goaway_sent
            && body_tx_by_stream.is_empty()
            && resp_rx_by_stream.is_empty()
            && stream_response.is_empty()
            && ws_tunnels.is_empty()
        {
            graceful_h3_shutdown(&mut params.conn, &params.socket, &mut out_buf).await;
            if let Some(m) = params.h3_recycle_metrics.as_ref() {
                m.connections_recycled_total.inc();
            }
            tracing::debug!(
                requests_served,
                goaway_last_id,
                "S36-A: H3 connection recycled after cap GOAWAY drained all in-flight streams"
            );
            break;
        }
    }
    // WS Stage C: the connection is closing — abort any live tunnel relays.
    for (_, st) in ws_tunnels {
        st.task.abort();
    }
    Ok(())
}

/// DEFECT-CLIENTGONE: a client that STOP_SENDINGs (or RESET_STREAMs) the H3
/// RESPONSE stream must stop the upstream read. quiche surfaces a peer
/// STOP_SENDING on a stream we are *writing* as `Err(StreamStopped)` from
/// `stream_capacity`, and a peer reset as `Err(StreamReset)`. On either, drop
/// the receiver — the producer's next `tx.send().await` returns
/// `Err(RespAbort::ClientGone)`, so the cell marks the pooled upstream
/// NON-reusable — and drop the `StreamTx`. The proxy does NOT emit
/// RESET_STREAM here: the client already cancelled (distinct from the
/// `H3_INTERNAL_ERROR` abort path).
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
        // Drop the Receiver ⇒ the producer's next send ⇒ ClientGone.
        resp_rx_by_stream.remove(&sid);
        // Drop the StreamTx: never FIN, never RESET_STREAM (ClientGone).
        stream_response.remove(&sid);
    }
}

/// §1.4.3 — the response-side backpressure gate.
///
/// Refill each stream's `StreamTx` from its bounded channel **only while that
/// StreamTx's queue is empty**. Refusing to pull while bytes are still queued
/// (quiche's send window full, `drain_streams_to_conn` has not shipped them) is
/// the memory bound: the channel fills, the producer's `tx.send().await`
/// blocks, and the upstream read pauses — in-flight bytes ≈ channel depth,
/// body-size independent.
///
/// `End` ⇒ set `ended`; `Reset`, or the channel closing with no prior `End`, ⇒
/// set `reset`, so a partial body is never presented as complete.
fn drain_resp_channels(
    resp_rx_by_stream: &mut HashMap<u64, mpsc::Receiver<RespEvent>>,
    stream_response: &mut HashMap<u64, StreamTx>,
) {
    let sids: Vec<u64> = resp_rx_by_stream.keys().copied().collect();
    for sid in sids {
        // F-S29-1 (gRPC-over-H3 large-response trailer drop): the spawn site
        // inserts the `Progressive` StreamTx alongside the receiver, but
        // `drain_streams_to_conn`'s `retain` REMOVES it the instant the stream
        // goes terminal, and a stale receiver can outlive it. Use `get_mut`,
        // NOT `entry().or_insert_with()`: a fresh StreamTx would replay the
        // leftover `End`, fire a spurious FIN + RESET, and `stream_shutdown`
        // would DISCARD a large response's still-buffered trailer+FIN (small
        // responses raced clear; large ones silently lost the trailing
        // `grpc-status` HEADERS — gRPC-fatal). A missing StreamTx means the
        // stream already terminated correctly: drop the stale receiver, skip.
        let Some(StreamTx::Progressive {
            queue,
            ended,
            reset,
            fin_sent,
            ..
        }) = stream_response.get_mut(&sid)
        else {
            resp_rx_by_stream.remove(&sid);
            continue;
        };
        if *fin_sent || *reset || *ended {
            // Terminal already decided; nothing more to pull.
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
        match rx.try_recv() {
            Ok(RespEvent::Head { status, headers }) => {
                queue.push_back(RespItem::Head { status, headers });
            }
            Ok(RespEvent::Body(b)) => queue.push_back(RespItem::Body(b)),
            Ok(RespEvent::Trailers(t)) => queue.push_back(RespItem::Trailers(t)),
            Ok(RespEvent::End) => *ended = true,
            Ok(RespEvent::Reset) => *reset = true,
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {
                // Producer gone with no End/Reset ⇒ treat as Reset.
                *reset = true;
            }
        }
    }

    // §1.5 test gauge: recorded at the largest instant (StreamTx just refilled,
    // before `drain_streams_to_conn` ships bytes). It sums queued `Body` bytes
    // plus `Head`/`Trailers` field bytes plus an UPPER bound on channel
    // occupancy — the gauge must over-, never under-count.
    #[cfg(any(test, feature = "test-gauges"))]
    {
        let mut total: usize = 0;
        for tx in stream_response.values() {
            let StreamTx::Progressive { queue, .. } = tx;
            for item in queue.iter() {
                total = total.saturating_add(match item {
                    RespItem::Body(b) => b.len(),
                    RespItem::Head { headers, .. } => {
                        headers.iter().map(|(n, v)| n.len() + v.len()).sum()
                    }
                    RespItem::Trailers(t) => t.iter().map(|(n, v)| n.len() + v.len()).sum(),
                });
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

/// One DECODED response item queued for a stream, encoded onto the
/// `quiche::h3::Connection` as flow control allows.
enum RespItem {
    /// The response head — encoded via `h3.send_response` (once).
    Head {
        /// `:status`.
        status: u16,
        /// Hop-by-hop-stripped non-pseudo response headers.
        headers: Vec<(String, String)>,
    },
    /// A body chunk (≤ `H3_RESP_CHUNK_MAX`) — encoded via `send_body`.
    Body(Bytes),
    /// The trailing field section — via `send_additional_headers`.
    Trailers(Vec<(String, String)>),
}

/// Per-stream outbound cursor: progressive response egress. A bounded queue of
/// DECODED response items fed over a bounded channel and encoded onto the
/// `quiche::h3::Connection` as flow control allows. The queue plus the channel
/// are the memory bound, independent of total response size.
enum StreamTx {
    /// `queue` holds DECODED items not yet encoded. `head_sent` guards the
    /// one-shot `send_response`; `ended` ⇒ FIN once `queue` drains; `reset` ⇒
    /// `RESET_STREAM` and NEVER a FIN, so a partial body is never presented as
    /// complete; `fin_sent` guards the one-shot FIN/shutdown.
    Progressive {
        queue: VecDeque<RespItem>,
        head_sent: bool,
        ended: bool,
        reset: bool,
        fin_sent: bool,
    },
}

impl StreamTx {
    /// Construct an empty progressive egress cursor.
    fn progressive() -> Self {
        Self::Progressive {
            queue: VecDeque::new(),
            head_sent: false,
            ended: false,
            reset: false,
            fin_sent: false,
        }
    }
}

/// Pump per-stream responses out: encode queued DECODED items onto the
/// `quiche::h3::Connection` via
/// `send_response`/`send_body`/`send_additional_headers`, incrementally,
/// because those calls refuse bytes (`Done` / `StreamBlocked`) when the send
/// window is saturated. `h3` is `None` until `with_transport` runs
/// post-establishment; while `None` this does nothing.
fn drain_streams_to_conn(
    conn: &mut quiche::Connection,
    mut h3: Option<&mut quiche::h3::Connection>,
    streams: &mut HashMap<u64, StreamTx>,
) {
    let mut to_drop = Vec::new();
    for (&sid, tx) in streams.iter_mut() {
        match tx {
            StreamTx::Progressive {
                queue,
                head_sent,
                ended,
                reset,
                fin_sent,
            } => {
                if *fin_sent {
                    continue;
                }
                // Can't send H3 responses before `with_transport` builds `h3`.
                let Some(h3c) = h3.as_deref_mut() else {
                    continue;
                };
                // Encode queued items front-to-back; a blocked send leaves
                // the rest queued for the next tick.
                while let Some(front) = queue.front_mut() {
                    match front {
                        RespItem::Head { status, headers } => {
                            if *head_sent {
                                // Defensive: a duplicate Head is impossible.
                                queue.pop_front();
                                continue;
                            }
                            let mut h3_headers: Vec<quiche::h3::Header> =
                                Vec::with_capacity(headers.len() + 1);
                            h3_headers.push(quiche::h3::Header::new(
                                b":status",
                                status.to_string().as_bytes(),
                            ));
                            for (n, v) in headers.iter() {
                                h3_headers
                                    .push(quiche::h3::Header::new(n.as_bytes(), v.as_bytes()));
                            }
                            match h3c.send_response(conn, sid, &h3_headers, false) {
                                Ok(()) => {
                                    *head_sent = true;
                                    queue.pop_front();
                                }
                                Err(quiche::h3::Error::StreamBlocked)
                                | Err(quiche::h3::Error::Done) => break,
                                Err(e) => {
                                    tracing::debug!(error = %e, stream_id = sid, "h3 send_response");
                                    *reset = true;
                                    break;
                                }
                            }
                        }
                        RespItem::Body(b) => {
                            match h3c.send_body(conn, sid, b, false) {
                                Ok(0) | Err(quiche::h3::Error::Done) => break,
                                Ok(n) if n >= b.len() => {
                                    queue.pop_front();
                                }
                                Ok(n) => {
                                    // Partial: keep the unsent tail queued.
                                    let _ = b.split_to(n);
                                    break;
                                }
                                Err(quiche::h3::Error::StreamBlocked) => break,
                                Err(e) => {
                                    tracing::debug!(error = %e, stream_id = sid, "h3 send_body");
                                    *reset = true;
                                    break;
                                }
                            }
                        }
                        RespItem::Trailers(t) => {
                            let h3_trailers: Vec<quiche::h3::Header> = t
                                .iter()
                                .map(|(n, v)| quiche::h3::Header::new(n.as_bytes(), v.as_bytes()))
                                .collect();
                            // The trailing field section is ALWAYS the last
                            // item, so it carries the FIN itself — a separate
                            // zero-length FIN would be a second terminal write.
                            match h3c.send_additional_headers(conn, sid, &h3_trailers, true, true) {
                                Ok(()) => {
                                    queue.pop_front();
                                    *fin_sent = true;
                                    to_drop.push(sid);
                                    break;
                                }
                                Err(quiche::h3::Error::StreamBlocked)
                                | Err(quiche::h3::Error::Done) => break,
                                Err(e) => {
                                    tracing::debug!(error = %e, stream_id = sid, "h3 send_additional_headers");
                                    *reset = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                if *reset {
                    // Abort: RESET_STREAM, NEVER FIN — a partial body must
                    // never be presentable as a complete response.
                    match conn.stream_shutdown(sid, quiche::Shutdown::Write, H3_INTERNAL_ERROR) {
                        Ok(()) | Err(quiche::Error::Done) => {}
                        Err(e) => {
                            tracing::debug!(error = %e, stream_id = sid, "stream_shutdown (resp)");
                        }
                    }
                    *fin_sent = true;
                    to_drop.push(sid);
                } else if *ended && queue.is_empty() && !*fin_sent {
                    // Clean completion: FIN via a zero-length `send_body`.
                    match h3c.send_body(conn, sid, &[], true) {
                        Ok(_) | Err(quiche::h3::Error::Done) => {}
                        Err(e) => {
                            tracing::debug!(error = %e, stream_id = sid, "h3 send_body FIN (resp)");
                        }
                    }
                    *fin_sent = true;
                    to_drop.push(sid);
                }
            }
        }
    }
    for sid in to_drop {
        // Mark terminal so later calls skip it; remove lazily.
        if let Some(StreamTx::Progressive { fin_sent, .. }) = streams.get_mut(&sid) {
            *fin_sent = true;
        }
    }
    streams.retain(|_, tx| {
        let StreamTx::Progressive { fin_sent, .. } = tx;
        !*fin_sent
    });
}

/// Emit an H3 `CONNECTION_CLOSE` (application-layer, carrying
/// [`H3_NO_ERROR`]) and pump `send`/`on_timeout` until quiche reports closed or
/// [`GRACEFUL_SHUTDOWN_BUDGET`] elapses. Idempotent: `close()` on an
/// already-closed connection returns `Done`, treated as a no-op.
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
        // Quiche's draining timer is per-connection; wait whichever is sooner.
        let quiche_timeout = conn.timeout().unwrap_or(Duration::from_millis(10));
        let residual = deadline.saturating_duration_since(tokio::time::Instant::now());
        let wait = quiche_timeout.min(residual);
        tokio::time::sleep(wait).await;
        conn.on_timeout();
    }
}

/// Repeatedly call `quiche::Connection::send` and push the resulting packets
/// onto the UDP socket until quiche reports `Done`.
///
/// `pub(crate)` (R12 single-source) so [`crate::raw_proxy`] drives both of its
/// legs through the SAME pump rather than a byte-identical private copy.
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

/// S36-A — attempt to emit the cap-triggered H3 GOAWAY (RFC 9114 §5.2).
///
/// Called when the cap trips and again each tick while
/// `*goaway_pending && !*goaway_sent`: the triggering client may send nothing
/// more, so the retry on a momentarily-full control-stream window CANNOT live
/// only in `poll_h3`. `goaway_last_id` is the highest admitted request stream
/// id — a client-initiated bidi stream, hence a multiple of 4, satisfying
/// `send_goaway`'s server-id precondition; calling only while `!*goaway_sent`
/// means its "id must not increase across calls" rule cannot bite.
///
/// `Err(StreamBlocked)`/`Err(Done)` leaves `goaway_sent` false so the caller
/// retries (admission is already stopped via `goaway_pending`). Any other error
/// flips `goaway_sent` so we do not spin on a doomed send — the subsequent
/// CONNECTION_CLOSE is the hard recycle signal the client always sees.
fn try_send_pending_goaway(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    goaway_pending: &mut bool,
    goaway_sent: &mut bool,
    goaway_last_id: u64,
    h3_recycle_metrics: Option<&lb_observability::QuicH3RecycleMetrics>,
) {
    if *goaway_sent || !*goaway_pending {
        return;
    }
    match h3.send_goaway(conn, goaway_last_id) {
        Ok(()) => {
            *goaway_sent = true;
            if let Some(m) = h3_recycle_metrics {
                m.goaway_sent_total.inc();
            }
            tracing::debug!(
                goaway_last_id,
                "S36-A: H3 connection reached request cap; sent GOAWAY, draining to recycle"
            );
        }
        Err(quiche::h3::Error::StreamBlocked) | Err(quiche::h3::Error::Done) => {
            // Control-stream window momentarily full — retry next tick.
        }
        Err(e) => {
            tracing::debug!(
                error = %e,
                goaway_last_id,
                "S36-A: send_goaway failed; proceeding to drain/close (CONNECTION_CLOSE is the hard recycle signal)"
            );
            *goaway_sent = true;
            if let Some(m) = h3_recycle_metrics {
                m.goaway_sent_total.inc();
            }
        }
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

/// Drain request-body bytes for ONE stream off the `quiche::h3::Connection`
/// into its bounded body channel.
///
/// **R8 backpressure:** `recv_body` runs only while the bounded channel has
/// spare capacity. When full we STOP reading, so quiche does not extend the
/// stream flow-control window and the client is paused; in-flight memory stays
/// ≈ `H3_BODY_CHANNEL_DEPTH * H3_BODY_CHUNK_MAX`, INDEPENDENT of body size
/// (quiche holds the remainder in its own flow-control-bounded buffer).
///
/// **F-CAP-1:** the cumulative `MAX_REQUEST_BODY_BYTES` cap is enforced here
/// via `body_seen`; on overflow emit `ReqBodyEvent::Reset` (⇒ 413).
///
/// **F-MD-4:** ANY `recv_body` error (a peer RESET_STREAM / STOP_SENDING
/// surfaces here) maps to `Reset`, NEVER a clean end — a truncated request must
/// never reach the backend as complete. The clean end comes from the `Finished`
/// event in [`poll_h3`], not here.
fn drain_request_body(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    sid: u64,
    body_tx_by_stream: &mut HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    body_seen: &mut HashMap<u64, usize>,
    pending_trailers: &mut HashMap<u64, Vec<(String, String)>>,
) {
    let mut scratch = [0u8; H3_BODY_CHUNK_MAX];
    loop {
        // Backpressure gate: do not read while the channel is full.
        match body_tx_by_stream.get(&sid) {
            Some(tx) if tx.capacity() > 0 => {}
            _ => return,
        }
        match h3.recv_body(conn, sid, &mut scratch) {
            Ok(0) => return,
            Ok(n) => {
                let seen = body_seen.entry(sid).or_default();
                *seen = seen.saturating_add(n);
                if *seen > MAX_REQUEST_BODY_BYTES {
                    // F-CAP-1: cumulative body over the cap → Reset (⇒ 413).
                    if let Some(tx) = body_tx_by_stream.remove(&sid) {
                        let _ = tx.try_send(ReqBodyEvent::Reset);
                    }
                    body_seen.remove(&sid);
                    pending_trailers.remove(&sid);
                    return;
                }
                // capacity > 0 was checked above and the actor is the sole
                // producer, so this send cannot fail.
                if let Some(tx) = body_tx_by_stream.get(&sid) {
                    let _ = tx.try_send(ReqBodyEvent::Chunk(Bytes::copy_from_slice(
                        scratch.get(..n).unwrap_or(&[]),
                    )));
                }
                #[cfg(any(test, feature = "test-gauges"))]
                record_req_retained(sid, body_tx_by_stream, n);
            }
            Err(quiche::h3::Error::Done) => return,
            Err(e) => {
                // F-MD-4: a mid-body stream error is NEVER a clean end.
                tracing::debug!(
                    error = %e,
                    stream_id = sid,
                    "INC-2: recv_body error mid-body; aborting upstream (Reset)"
                );
                if let Some(tx) = body_tx_by_stream.remove(&sid) {
                    let _ = tx.try_send(ReqBodyEvent::Reset);
                }
                body_seen.remove(&sid);
                pending_trailers.remove(&sid);
                return;
            }
        }
    }
}

/// Drive the `quiche::h3::Connection` ingress: poll events, decode request
/// HEADERS, run the pseudo-header + authority validation, and spawn the
/// H3→H1/H2/H3 cell task per request, streaming the body through the bounded
/// channel with R8 backpressure.
///
/// quiche `poll` is **edge-triggered**: `Data` fires once and re-arms only
/// after the stream drains to `Done`. Because the R8 gate stops `recv_body`
/// while the channel is full, `Data` will NOT re-fire — so PASS 1 re-attempts
/// the capacity-gated drain for every body-phase stream every tick,
/// independent of the poll events.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn poll_h3(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    body_tx_by_stream: &mut HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    body_seen: &mut HashMap<u64, usize>,
    pending_trailers: &mut HashMap<u64, Vec<(String, String)>>,
    resp_rx_by_stream: &mut HashMap<u64, mpsc::Receiver<RespEvent>>,
    resp_tasks: &mut Vec<tokio::task::JoinHandle<()>>,
    stream_response: &mut HashMap<u64, StreamTx>,
    pool: &TcpPool,
    backends: &Arc<Vec<SocketAddr>>,
    h3_backend: Option<&(QuicUpstreamPool, SocketAddr, String)>,
    h2_backend: Option<&(Http2Pool, SocketAddr)>,
    // WS Stage A: gates `:protocol` Extended-CONNECT acceptance.
    ws_enabled: bool,
    // WS Stage C: per-stream tunnel state + the injected relay launcher.
    ws_tunnels: &mut HashMap<u64, WsTunnelState>,
    ws_relay_launcher: Option<&WsRelayLauncher>,
    // S36-A: recycling cap + actor-local state + metrics; `cap == 0` ⇒ inert.
    cap: u32,
    requests_served: &mut u64,
    goaway_pending: &mut bool,
    goaway_sent: &mut bool,
    goaway_last_id: &mut u64,
    h3_recycle_metrics: Option<&lb_observability::QuicH3RecycleMetrics>,
) {
    // PASS 1 — re-arm / backpressure drain (see fn doc).
    let active: Vec<u64> = body_tx_by_stream.keys().copied().collect();
    for sid in active {
        drain_request_body(
            conn,
            h3,
            sid,
            body_tx_by_stream,
            body_seen,
            pending_trailers,
        );
    }

    // PASS 2 — event loop. One event per `poll` call until `Done`.
    loop {
        match h3.poll(conn) {
            Ok((sid, quiche::h3::Event::Headers { list, more_frames })) => {
                let headers: Vec<(String, String)> = list
                    .iter()
                    .map(|h| {
                        use quiche::h3::NameValue;
                        (
                            String::from_utf8_lossy(h.name()).into_owned(),
                            String::from_utf8_lossy(h.value()).into_owned(),
                        )
                    })
                    .collect();

                // A SECOND HEADERS frame on a body-phase stream is the
                // trailing field section, not a new request.
                if body_tx_by_stream.contains_key(&sid) {
                    // RFC 9114 §4.3: a pseudo-header in a trailing field
                    // section is malformed.
                    if headers.iter().any(|(n, _)| n.starts_with(':')) {
                        tracing::warn!(
                            stream_id = sid,
                            "INC-2: H3 trailer pseudo-header rejected (RFC 9114 §4.3)"
                        );
                        if let Some(tx) = body_tx_by_stream.remove(&sid) {
                            let _ = tx.try_send(ReqBodyEvent::Reset);
                        }
                        body_seen.remove(&sid);
                        pending_trailers.remove(&sid);
                        continue;
                    }
                    pending_trailers.insert(sid, headers);
                    continue;
                }

                // S36-A CONNECTION RECYCLING — runs ONCE per NEW request
                // stream. `cap == 0` short-circuits the whole block, so the
                // disabled path is byte-identical to the pre-S36 front.
                if cap != 0 {
                    // 1) Already recycling: reject any stream opened AFTER the
                    //    GOAWAY's last-processed id. The gate is
                    //    `goaway_pending`, NOT `goaway_sent`: admission stops
                    //    the moment the cap trips, even while the GOAWAY frame
                    //    waits on a full control-stream window, so nothing is
                    //    admitted past the boundary during the retry. RFC 9114
                    //    §5.2 lets the client retry it on a new connection.
                    if *goaway_pending && sid > *goaway_last_id {
                        tracing::debug!(
                            stream_id = sid,
                            goaway_last_id = *goaway_last_id,
                            "S36-A: rejecting new H3 request after cap GOAWAY (H3_REQUEST_REJECTED, RFC 9114 §5.2)"
                        );
                        reset_h3_stream(conn, sid, H3_REQUEST_REJECTED);
                        continue;
                    }
                    // 2) Count this stream. Counting BEFORE validation bounds
                    //    quiche's per-connection `collected` set against
                    //    malformed-request spam too: every new stream lands in
                    //    `collected`, so the cap must count rejects as well.
                    *requests_served = requests_served.saturating_add(1);
                    // This stream WILL be processed, so it is the new
                    // highest-processed id for any GOAWAY below. Set ONLY here,
                    // on an admitted request bidi stream — never from a uni /
                    // control stream — so `send_goaway`'s multiple-of-4
                    // precondition holds by construction.
                    *goaway_last_id = sid;
                    // 3) At the cap, flip `goaway_pending` (stop admitting
                    //    immediately) and try to emit the GOAWAY now; the outer
                    //    loop retries if the window is full.
                    if *requests_served >= u64::from(cap) {
                        *goaway_pending = true;
                        try_send_pending_goaway(
                            conn,
                            h3,
                            goaway_pending,
                            goaway_sent,
                            *goaway_last_id,
                            h3_recycle_metrics,
                        );
                    }
                }

                // Initial request HEADERS: pseudo-header validation runs
                // BEFORE any upstream is dialled.
                if let Err(reason) = validate_request_pseudo_headers(&headers, ws_enabled) {
                    tracing::warn!(
                        stream_id = sid,
                        reason,
                        "SESSION 22: malformed H3 request rejected (H3_MESSAGE_ERROR, RFC 9114 §4.1.3)"
                    );
                    reset_h3_stream(conn, sid, H3_MESSAGE_ERROR);
                    continue;
                }
                let req = H3Request::from_headers(headers);
                // ROUND8-L7-16: :authority sanitisation — reject (H3 400)
                // before the value can reach a backend.
                if !req.authority.is_empty() {
                    if let Err(e) = lb_core::authority::validate(&req.authority) {
                        tracing::warn!(
                            authority = %req.authority,
                            error = ?e,
                            stream_id = sid,
                            "ROUND8-L7-16: H3 :authority rejected before upstream selection"
                        );
                        // Emit the inline 400 through the decoded egress path.
                        spawn_inline_h3_response(
                            resp_tasks,
                            resp_rx_by_stream,
                            stream_response,
                            sid,
                            400,
                            "bad request",
                        );
                        continue;
                    }
                }

                // WS Stage C: intercept a validated `:protocol=websocket`
                // extended CONNECT before the normal cell dispatch — the
                // tunnel takes over this stream entirely.
                if ws_enabled {
                    let ws_protocol = req
                        .extra
                        .iter()
                        .find(|(n, _)| n == ":protocol")
                        .map(|(_, v)| v.clone());
                    if let Some(protocol) = ws_protocol {
                        setup_ws_tunnel(
                            sid,
                            req,
                            &protocol,
                            ws_relay_launcher,
                            ws_tunnels,
                            resp_tasks,
                            resp_rx_by_stream,
                            stream_response,
                        );
                        continue;
                    }
                }

                let bodyless = !more_frames;
                // Build the bounded request-body + response channels and spawn
                // the per-cell producer task.
                let (btx, brx) = mpsc::channel::<ReqBodyEvent>(H3_BODY_CHANNEL_DEPTH);
                let (resp_tx, resp_rx) = mpsc::channel::<RespEvent>(H3_RESP_CHANNEL_DEPTH);

                let spawned = if let Some((h2pool, addr)) = h2_backend {
                    let (h2pool, addr) = (h2pool.clone(), *addr);
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
                            tracing::warn!(?abort, stream_id = sid, "H3→H2 resp stream aborted");
                        }
                    }));
                    true
                } else if let Some((qpool, addr, sni)) = h3_backend {
                    let (qpool, addr, sni) = (qpool.clone(), *addr, sni.clone());
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
                            tracing::warn!(?abort, stream_id = sid, "H3→H3 resp stream aborted");
                        }
                    }));
                    true
                } else if let Some(backend) = select_backend(backends) {
                    let pool = pool.clone();
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
                            tracing::warn!(?abort, stream_id = sid, "H3→H1 resp stream aborted");
                        }
                    }));
                    true
                } else {
                    tracing::warn!("no backends available for H3 request");
                    false
                };
                if !spawned {
                    continue;
                }
                resp_rx_by_stream.insert(sid, resp_rx);
                stream_response.insert(sid, StreamTx::progressive());

                if bodyless {
                    // Bodyless (HEADERS + FIN): the consumer's first event must
                    // be `End`, so send it now rather than registering a
                    // body-phase channel.
                    let _ = btx.try_send(ReqBodyEvent::End {
                        trailers: Vec::new(),
                    });
                } else {
                    // Body to follow: register the body-phase channel and take
                    // the first capacity-gated drain now.
                    body_tx_by_stream.insert(sid, btx);
                    body_seen.insert(sid, 0);
                    drain_request_body(
                        conn,
                        h3,
                        sid,
                        body_tx_by_stream,
                        body_seen,
                        pending_trailers,
                    );
                }
            }
            Ok((sid, quiche::h3::Event::Data)) => {
                drain_request_body(
                    conn,
                    h3,
                    sid,
                    body_tx_by_stream,
                    body_seen,
                    pending_trailers,
                );
            }
            Ok((sid, quiche::h3::Event::Finished)) => {
                // WS Stage C: a tunnel-stream FIN is the client closing its
                // send half, not a request end.
                if let Some(st) = ws_tunnels.get_mut(&sid) {
                    ws_handle_client_fin(conn, h3, sid, st);
                    continue;
                }
                // F-MD-4 SMUGGLING GUARD. quiche's `poll` can return
                // `Event::Finished` for a request stream that was actually
                // RESET *after* its last DATA frame: `recv_body` on a reset
                // stream queues it as finished, and `poll`'s FIRST
                // `finished_streams` pop returns `Finished` WITHOUT the reset
                // re-check that only its SECOND pop performs. Treating that as
                // a clean end would present a truncated request to the backend
                // as complete. Probe the transport exactly as quiche's own
                // guard does — a zero-length `stream_recv` returns
                // `StreamReset` for a reset stream — and map that to `Reset`,
                // never `End`. A genuinely FIN'd stream returns `Ok((0, true))`
                // and takes the clean path.
                let was_reset = matches!(
                    conn.stream_recv(sid, &mut []),
                    Err(quiche::Error::StreamReset(_))
                );
                if let Some(tx) = body_tx_by_stream.remove(&sid) {
                    if was_reset {
                        tracing::debug!(
                            stream_id = sid,
                            "INC-2 F-MD-4: Finished event on a RESET request stream; \
                             Reset to upstream (not a clean End)"
                        );
                        let _ = tx.try_send(ReqBodyEvent::Reset);
                    } else {
                        let trailers = pending_trailers.remove(&sid).unwrap_or_default();
                        let _ = tx.try_send(ReqBodyEvent::End { trailers });
                    }
                }
                body_seen.remove(&sid);
                pending_trailers.remove(&sid);
            }
            Ok((sid, quiche::h3::Event::Reset(code))) => {
                // WS Stage C: a tunnel-stream reset is an abnormal close.
                if let Some(st) = ws_tunnels.get_mut(&sid) {
                    ws_handle_client_reset(sid, st);
                    continue;
                }
                // F-MD-4: the client reset the request stream mid-flight.
                tracing::debug!(
                    stream_id = sid,
                    code,
                    "INC-2 F-MD-4: client reset request stream; Reset to upstream"
                );
                if let Some(tx) = body_tx_by_stream.remove(&sid) {
                    let _ = tx.try_send(ReqBodyEvent::Reset);
                }
                body_seen.remove(&sid);
                pending_trailers.remove(&sid);
            }
            // GoAway / PriorityUpdate / H3 DATAGRAM — quiche handles these.
            Ok((_sid, _)) => {}
            Err(quiche::h3::Error::Done) => break,
            Err(e) => {
                // quiche enforces the control / QPACK / frame-sequence rules
                // itself and has already closed the conn.
                tracing::debug!(error = %e, "INC-2: h3.poll error (quiche closed the connection)");
                break;
            }
        }
    }

    // WS Stage C: after the event loop, advance every live tunnel.
    pump_ws_tunnels(
        conn,
        h3,
        ws_tunnels,
        resp_tasks,
        resp_rx_by_stream,
        stream_response,
    );
}

/// Test gauge — record the per-stream retained request-body bytes at the
/// point the buffers are largest, so a whole-frame buffering regression fails.
#[cfg(any(test, feature = "test-gauges"))]
fn record_req_retained(
    sid: u64,
    body_tx_by_stream: &HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    last_read: usize,
) {
    let chan_used = body_tx_by_stream
        .get(&sid)
        .map_or(0, |tx| tx.max_capacity().saturating_sub(tx.capacity()));
    let chan_bytes = chan_used.saturating_mul(H3_BODY_CHUNK_MAX);
    crate::h3_bridge::record_retained(chan_bytes.saturating_add(last_read));
}

/// Pick a backend. Round-robin-ish: the first for now.
fn select_backend(backends: &Arc<Vec<SocketAddr>>) -> Option<SocketAddr> {
    backends.first().copied()
}

/// Spawn an inline H3 response (`status` + a short plain body) on `sid`
/// through the normal decoded response channel, so the WS error paths reuse
/// the same egress as every other response.
fn spawn_inline_h3_response(
    resp_tasks: &mut Vec<tokio::task::JoinHandle<()>>,
    resp_rx_by_stream: &mut HashMap<u64, mpsc::Receiver<RespEvent>>,
    stream_response: &mut HashMap<u64, StreamTx>,
    sid: u64,
    status: u16,
    msg: &'static str,
) {
    let (resp_tx, resp_rx) = mpsc::channel::<RespEvent>(H3_RESP_CHANNEL_DEPTH);
    resp_tasks.push(tokio::spawn(async move {
        let _ = resp_tx
            .send(RespEvent::Head {
                status,
                headers: Vec::new(),
            })
            .await;
        let _ = resp_tx
            .send(RespEvent::Body(Bytes::from_static(msg.as_bytes())))
            .await;
        let _ = resp_tx.send(RespEvent::End).await;
    }));
    resp_rx_by_stream.insert(sid, resp_rx);
    stream_response.insert(sid, StreamTx::progressive());
}

/// Set up a WebSocket-over-H3 tunnel for a validated extended CONNECT: build
/// the bounded relay channels, launch the injected relay, and queue the `200`
/// head so it is sent only AFTER the upstream handshake succeeds
/// (upstream-before-200 — never a `200` the upstream never agreed to).
#[allow(clippy::too_many_arguments)]
fn setup_ws_tunnel(
    sid: u64,
    req: H3Request,
    protocol: &str,
    ws_relay_launcher: Option<&WsRelayLauncher>,
    ws_tunnels: &mut HashMap<u64, WsTunnelState>,
    resp_tasks: &mut Vec<tokio::task::JoinHandle<()>>,
    resp_rx_by_stream: &mut HashMap<u64, mpsc::Receiver<RespEvent>>,
    stream_response: &mut HashMap<u64, StreamTx>,
) {
    // RFC 8441/9220: `websocket` is the only `:protocol` value supported;
    // anything else draws a 501.
    if !protocol.eq_ignore_ascii_case("websocket") {
        tracing::debug!(
            stream_id = sid,
            protocol,
            "WS-H3: unsupported :protocol — 501"
        );
        spawn_inline_h3_response(
            resp_tasks,
            resp_rx_by_stream,
            stream_response,
            sid,
            501,
            "unsupported :protocol",
        );
        return;
    }
    let Some(launcher) = ws_relay_launcher else {
        // Fail closed: a WS listener always injects a launcher.
        tracing::warn!(
            stream_id = sid,
            "WS-H3: extended CONNECT but no relay launcher injected — 502"
        );
        spawn_inline_h3_response(
            resp_tasks,
            resp_rx_by_stream,
            stream_response,
            sid,
            502,
            "websocket relay unavailable",
        );
        return;
    };
    // The client's offered subprotocol list, forwarded to the upstream.
    let subprotocols = req
        .extra
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("sec-websocket-protocol"))
        .map(|(_, v)| v.clone());
    let connect_req = WsConnectRequest {
        authority: req.authority,
        path: req.path,
        subprotocols,
    };
    let (tunnel, endpoints) = H3WsTunnel::new();
    let WsRelayHandle { ready, task } = (launcher)(tunnel, connect_req);
    let H3TunnelEndpoints {
        to_reader,
        from_writer,
    } = endpoints;
    tracing::debug!(
        stream_id = sid,
        "WS-H3: extended CONNECT accepted; dialing upstream before 200"
    );
    ws_tunnels.insert(
        sid,
        WsTunnelState {
            to_reader: Some(to_reader),
            from_writer,
            ready: Some(ready),
            pending_ok: None,
            activated: false,
            out_pending: None,
            fin_sent: false,
            done: false,
            task,
        },
    );
}

/// Inbound pump (H3 stream DATA → `proxy_frames` reader), capacity-gated the
/// same way as [`drain_request_body`]: a full reader channel stops the read,
/// so quiche pauses the client (R8).
fn ws_drain_inbound(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    sid: u64,
    st: &mut WsTunnelState,
) {
    if !st.activated {
        return;
    }
    // Clone the sender so the error arm can drop the original and signal the
    // reader's terminal.
    let Some(tx) = st.to_reader.as_ref().cloned() else {
        return;
    };
    let mut scratch = [0u8; crate::ws_tunnel::H3_WS_TUNNEL_CHUNK_MAX];
    loop {
        // Backpressure gate: do not read while the reader channel is full.
        if tx.capacity() == 0 {
            return;
        }
        match h3.recv_body(conn, sid, &mut scratch) {
            Ok(0) => return,
            Ok(n) => {
                let chunk = Bytes::copy_from_slice(scratch.get(..n).unwrap_or(&[]));
                // capacity > 0 checked above + the actor is the sole producer.
                let _ = tx.try_send(TunnelInbound::Data(chunk));
            }
            Err(quiche::h3::Error::Done) => return,
            Err(e) => {
                tracing::debug!(error = %e, stream_id = sid, "WS-H3: recv_body error; Reset to relay");
                let _ = tx.try_send(TunnelInbound::Reset);
                st.to_reader = None;
                return;
            }
        }
    }
}

/// The client FIN'd its WS send half: drain any coalesced DATA first (so no
/// inbound bytes are lost), then drop the sender so the reader sees the
/// terminal.
fn ws_handle_client_fin(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    sid: u64,
    st: &mut WsTunnelState,
) {
    // Drain DATA coalesced with the FIN first (no inbound bytes lost).
    ws_drain_inbound(conn, h3, sid, st);
    let Some(tx) = st.to_reader.take() else {
        return; // a terminal was already relayed (e.g. by ws_drain_inbound).
    };
    let was_reset = matches!(
        conn.stream_recv(sid, &mut []),
        Err(quiche::Error::StreamReset(_))
    );
    if was_reset {
        tracing::debug!(
            stream_id = sid,
            "WS-H3 F-MD-4: Finished on a RESET tunnel stream; Reset (not clean EOF)"
        );
        let _ = tx.try_send(TunnelInbound::Reset);
    } else {
        tracing::debug!(stream_id = sid, "WS-H3: client FIN; clean EOF to relay");
    }
    // `tx` dropped here ⇒ the reader observes the terminal.
}

/// The client RESET the WS tunnel stream — surface an abnormal drop to the
/// relay rather than a clean close.
fn ws_handle_client_reset(sid: u64, st: &mut WsTunnelState) {
    if let Some(tx) = st.to_reader.take() {
        tracing::debug!(
            stream_id = sid,
            "WS-H3: client reset tunnel stream; Reset to relay"
        );
        let _ = tx.try_send(TunnelInbound::Reset);
    }
}

/// Abort a tunnel stream abnormally: `RESET_STREAM` + `STOP_SENDING` with
/// [`H3_REQUEST_CANCELLED`], then drop the tunnel state.
fn ws_teardown(conn: &mut quiche::Connection, sid: u64, st: &mut WsTunnelState) {
    st.to_reader = None;
    if !st.fin_sent {
        match conn.stream_shutdown(sid, quiche::Shutdown::Write, H3_REQUEST_CANCELLED) {
            Ok(()) | Err(quiche::Error::Done) => {}
            Err(e) => tracing::debug!(error = %e, stream_id = sid, "WS-H3 teardown RESET (write)"),
        }
        match conn.stream_shutdown(sid, quiche::Shutdown::Read, H3_REQUEST_CANCELLED) {
            Ok(()) | Err(quiche::Error::Done) => {}
            Err(e) => tracing::debug!(error = %e, stream_id = sid, "WS-H3 teardown RESET (read)"),
        }
        st.fin_sent = true;
    }
}

/// Per-tick WebSocket tunnel pump: resolve upstream readiness (`Ready` queues
/// the 200, `Failed` emits the inline error and tears down), send the queued
/// 200 (retrying under a full send window), then pump inbound (re-arming each
/// tick because `Data` is edge-triggered) and outbound (R8: retain the unsent
/// tail; a full send window stops us pulling from `from_writer`, which parks
/// the relay's `PollSender`). When the relay finishes, FIN the stream.
///
/// Inert when `ws_tunnels` is empty.
fn pump_ws_tunnels(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    ws_tunnels: &mut HashMap<u64, WsTunnelState>,
    resp_tasks: &mut Vec<tokio::task::JoinHandle<()>>,
    resp_rx_by_stream: &mut HashMap<u64, mpsc::Receiver<RespEvent>>,
    stream_response: &mut HashMap<u64, StreamTx>,
) {
    if ws_tunnels.is_empty() {
        return;
    }
    let sids: Vec<u64> = ws_tunnels.keys().copied().collect();
    let mut to_remove: Vec<u64> = Vec::new();
    for sid in sids {
        let Some(st) = ws_tunnels.get_mut(&sid) else {
            continue;
        };

        // (1) Upstream readiness — gates the 200 (upstream-before-200).
        if let Some(ready) = st.ready.as_mut() {
            match ready.try_recv() {
                Ok(WsUpstreamOutcome::Ready { headers }) => {
                    st.ready = None;
                    st.pending_ok = Some(WsPendingOk { headers });
                }
                Ok(WsUpstreamOutcome::Failed { status }) => {
                    st.ready = None;
                    spawn_inline_h3_response(
                        resp_tasks,
                        resp_rx_by_stream,
                        stream_response,
                        sid,
                        status,
                        "websocket upstream failed",
                    );
                    to_remove.push(sid);
                    continue;
                }
                Err(oneshot::error::TryRecvError::Empty) => {
                    continue;
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    // Launcher dropped the sender without a verdict — treat
                    // as a failure, never a silent 200.
                    st.ready = None;
                    spawn_inline_h3_response(
                        resp_tasks,
                        resp_rx_by_stream,
                        stream_response,
                        sid,
                        502,
                        "websocket upstream failed",
                    );
                    to_remove.push(sid);
                    continue;
                }
            }
        }

        // (2) Send the queued 200 head (retry under a full send window).
        if let Some(ok) = st.pending_ok.as_ref() {
            let mut h3_headers: Vec<quiche::h3::Header> = Vec::with_capacity(ok.headers.len() + 1);
            h3_headers.push(quiche::h3::Header::new(b":status", b"200"));
            for (n, v) in &ok.headers {
                h3_headers.push(quiche::h3::Header::new(n.as_bytes(), v.as_bytes()));
            }
            match h3.send_response(conn, sid, &h3_headers, false) {
                Ok(()) => {
                    st.pending_ok = None;
                    st.activated = true;
                    tracing::debug!(stream_id = sid, "WS-H3: 200 sent; tunnel active");
                }
                Err(quiche::h3::Error::StreamBlocked) | Err(quiche::h3::Error::Done) => {
                    continue; // retry next tick
                }
                Err(e) => {
                    tracing::debug!(error = %e, stream_id = sid, "WS-H3: send_response(200) failed; tearing down");
                    ws_teardown(conn, sid, st);
                    to_remove.push(sid);
                    continue;
                }
            }
        }

        if !st.activated {
            continue;
        }

        // (3a) Inbound re-arm (Data is edge-triggered).
        ws_drain_inbound(conn, h3, sid, st);

        // (3b) Outbound: relay → H3 DATA. R8: retain the unsent tail.
        loop {
            if st.out_pending.is_none() {
                match st.from_writer.try_recv() {
                    Ok(chunk) => st.out_pending = Some(chunk),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        // (4) Relay finished → FIN the H3 stream + remove.
                        if !st.fin_sent {
                            match h3.send_body(conn, sid, &[], true) {
                                Ok(_) | Err(quiche::h3::Error::Done) => {}
                                Err(e) => {
                                    tracing::debug!(error = %e, stream_id = sid, "WS-H3: FIN send_body");
                                }
                            }
                            st.fin_sent = true;
                        }
                        st.done = true;
                        break;
                    }
                }
            }
            if let Some(buf) = st.out_pending.as_mut() {
                match h3.send_body(conn, sid, buf, false) {
                    Ok(n) if n >= buf.len() => {
                        st.out_pending = None; // fully sent; loop for the next chunk
                    }
                    Ok(0)
                    | Err(quiche::h3::Error::Done)
                    | Err(quiche::h3::Error::StreamBlocked) => break, // window full → retain (R8)
                    Ok(n) => {
                        let _ = buf.split_to(n); // partial → retain the tail
                        break;
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, stream_id = sid, "WS-H3: send_body error; tearing down");
                        ws_teardown(conn, sid, st);
                        st.done = true;
                        break;
                    }
                }
            } else {
                break;
            }
        }

        if st.done {
            to_remove.push(sid);
        }
    }
    for sid in to_remove {
        if let Some(st) = ws_tunnels.remove(&sid) {
            // The relay task is already finished on the FIN/Failed paths.
            st.task.abort();
        }
    }
}
