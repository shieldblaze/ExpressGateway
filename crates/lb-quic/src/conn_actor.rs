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

/// SESSION 28 / WS-over-H3 (RFC 9220) — `H3_REQUEST_CANCELLED` (`0x010c`,
/// RFC 9114 §8.1). Emitted on the `RESET_STREAM` when a WebSocket tunnel
/// stream is torn down abnormally (e.g. the gateway can no longer relay).
/// RFC 9220 §3 maps a WebSocket abnormal close over H3 to this code.
const H3_REQUEST_CANCELLED: u64 = 0x010c;

/// SESSION 28 / WS-over-H3 Stage C — per-stream WebSocket tunnel state.
///
/// Held in the actor's `ws_tunnels` map for a sid that carried a validated
/// `:protocol=websocket` extended CONNECT. The actor (sync poll loop)
/// shuttles bytes between the H3 stream and the injected relay via the two
/// bounded channels of the [`H3WsTunnel`] seam — all non-blocking
/// `try_send`/`try_recv`, so the actor never awaits.
struct WsTunnelState {
    /// Actor→relay (inbound: H3 stream DATA → `proxy_frames` reader).
    /// `None` once a terminal (clean EOF on client FIN, or `Reset`) has
    /// been relayed — the sender is dropped so the reader observes it.
    to_reader: Option<mpsc::Sender<TunnelInbound>>,
    /// Relay→actor (outbound: `proxy_frames` writer → H3 stream DATA).
    from_writer: mpsc::Receiver<Bytes>,
    /// Upstream-handshake readiness — resolves once, BEFORE the `200`
    /// (the H3 analog of the WS-H1 GHSA / WS-H2 F-S27-1 ordering). `None`
    /// once consumed.
    ready: Option<oneshot::Receiver<WsUpstreamOutcome>>,
    /// Response head still to encode before the tunnel activates (the
    /// `200` on success) — retried each tick until `send_response`
    /// accepts it. `None` once sent (or never set: an error response uses
    /// the inline Progressive path instead).
    pending_ok: Option<WsPendingOk>,
    /// `true` once the `200` is on the wire — the tunnel-mode pump runs.
    activated: bool,
    /// Unsent tail of the chunk currently being written outbound (the R8
    /// outbound backpressure point: a non-empty tail stops us pulling more
    /// from `from_writer`, which parks the relay's `PollSender`).
    out_pending: Option<Bytes>,
    /// Set once we FIN the H3 stream outbound (the relay finished).
    fin_sent: bool,
    /// Marks the state for removal at the end of the tick.
    done: bool,
    /// The relay task (dial + upstream handshake + `proxy_frames`).
    /// Aborted on teardown so a torn-down tunnel never leaks its task.
    task: tokio::task::JoinHandle<()>,
}

/// SESSION 28 / WS-over-H3 Stage C — the success (`200`) response head the
/// pump sends before activating the tunnel. `head_sent` is a retry latch
/// for a `send_response` that returns `StreamBlocked`/`Done` under a full
/// send window.
struct WsPendingOk {
    /// Extra response fields (e.g. the upstream-selected
    /// `sec-websocket-protocol`) to emit alongside `:status 200`.
    headers: Vec<(String, String)>,
}

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
    /// SESSION 27 / WS-over-H3 (RFC 9220) Stage A: whether this listener
    /// opted into WebSocket (a `[listeners.websocket]` block was present).
    /// Threaded from [`crate::listener::QuicListenerParams::ws_enabled`]
    /// via [`crate::router::RouterParams::ws_enabled`]. Gates BOTH the
    /// `SETTINGS_ENABLE_CONNECT_PROTOCOL` advertisement (in
    /// [`crate::h3_config::build_server_h3_config`]) AND the
    /// `:protocol` Extended-CONNECT accept path in
    /// [`crate::h3_bridge::validate_request_pseudo_headers`]. `false`
    /// (every pre-S27 caller) keeps the H3 front byte-identical (R3).
    pub ws_enabled: bool,
    /// SESSION 28 / WS-over-H3 (RFC 9220) Stage C: the injected relay
    /// launcher (dependency inversion — the `lb` binary builds it because
    /// `lb-quic` cannot import `lb_l7::ws_proxy::proxy_frames`). `Some`
    /// only on a WebSocket-opted-in listener; the actor calls it on a
    /// validated `:protocol=websocket` extended CONNECT to dial the H1
    /// backend, complete the upstream handshake before the `200`, and run
    /// the single-sourced frame relay over the tunnel. `None` (every
    /// non-WS listener) ⇒ the actor never builds a tunnel and the H3 front
    /// is byte-identical (R3). Threaded from
    /// [`crate::router::RouterParams::ws_relay_launcher`].
    pub ws_relay_launcher: Option<crate::ws_tunnel::WsRelayLauncher>,
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
    // SESSION 24 / INC-2: H3 ingress now rides `quiche::h3::Connection`
    // (built lazily via `with_transport` once established); the old
    // hand-rolled `StreamRxBuf` request decoder + uni-stream drain are
    // gone. Egress is decoded `RespEvent` → `quiche::h3::send_*` (INC-3).
    let mut h3: Option<quiche::h3::Connection> = None;
    let mut stream_response: HashMap<u64, StreamTx> = HashMap::new();
    // SESSION 2 / P1-A: per-stream bounded request-body channels. The
    // sender lives here in the actor; the matching receiver is moved
    // into the spawned `h3_to_h1_stream` task. Bounded depth +
    // backpressure (poll_h3 skips `stream_recv` when full) is the
    // memory-safety mechanism.
    let mut body_tx_by_stream: HashMap<u64, mpsc::Sender<ReqBodyEvent>> = HashMap::new();
    // SESSION 24 / INC-2: cumulative request-body bytes per stream —
    // enforces MAX_REQUEST_BODY_BYTES (64 MiB → 413, F-CAP-1); this cap
    // previously lived inside the deleted `StreamRxBuf::feed_body`.
    let mut body_seen: HashMap<u64, usize> = HashMap::new();
    // SESSION 24 / INC-2: request trailers (RFC 9114 §4.1) now arrive as
    // a SECOND `Event::Headers` on a body-phase stream; stashed here until
    // `Finished` attaches them to `End`.
    let mut pending_trailers: HashMap<u64, Vec<(String, String)>> = HashMap::new();
    // SESSION 4 / P1-B: per-stream bounded RESPONSE channels. The cell
    // producer task owns the SENDER (the inverse of the request side,
    // where the actor owns the sender); the RECEIVER stays here and is
    // drained — under the §1.4.3 backpressure gate — into the stream's
    // `Progressive` `StreamTx`. SESSION 25 / INC-5: this DECODED Progressive
    // path is now the SOLE response egress — the inline-400 authority reject
    // joined it, and the legacy raw-byte `request_tasks`/`task_wait`/
    // `Buffered` path is deleted (production is 100% on quiche::h3).
    let mut resp_rx_by_stream: HashMap<u64, mpsc::Receiver<RespEvent>> = HashMap::new();
    // SESSION 4 / P1-B: liveness handles for the response producer tasks.
    // Joined opportunistically to reap finished producers.
    let mut resp_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    // SESSION 28 / WS-over-H3 Stage C: per-stream WebSocket tunnel state.
    // Populated when a validated `:protocol=websocket` extended CONNECT
    // arrives and the relay launcher is injected; drained each tick by
    // `pump_ws_tunnels`. Empty (always, on a non-WS listener) ⇒ the H3
    // front is byte-identical (R3).
    let mut ws_tunnels: HashMap<u64, WsTunnelState> = HashMap::new();

    loop {
        // Before waiting: push any outbound bytes from quiche + any
        // per-stream response items out. SESSION 24 / INC-3: the
        // Progressive arm now encodes via quiche::h3 (`h3.as_mut()`); at
        // the top-of-loop `h3` may still be `None` (pre-establishment) —
        // that's fine, nothing to send yet. The later `poll_h3` borrow
        // of `h3` is sequential, so the borrow checker is satisfied.
        drain_streams_to_conn(&mut params.conn, h3.as_mut(), &mut stream_response);
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
        // SESSION 28 / WS-over-H3 Stage C: an active WS tunnel is advanced
        // only by a `pump_ws_tunnels` tick (nothing wakes the select! when
        // the relay pushes an outbound frame into `from_writer`), so cap the
        // wait the same way an active body/response stream does. Bounded
        // busy-poll: a truly-idle tunnel is reaped by `proxy_frames`' own
        // idle timeout (default 60 s). Does NOT defeat backpressure — the
        // bounded channels still cap in-flight bytes; we only poll oftener.
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
            // SESSION 24 / INC-2: build the `quiche::h3::Connection` once
            // the transport handshake completes (`with_transport` takes
            // over SETTINGS + the server control/QPACK uni streams).
            if h3.is_none() {
                // Build the H3 config (INC-0; static-only QPACK, behaviour-
                // matching defaults) then wrap the established transport.
                // Both fallible steps fail safe: log + app-close so the
                // actor loop's next `is_closed()` breaks cleanly.
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
                );
            }
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
    // SESSION 28 / WS-over-H3 Stage C: the connection is closing — abort any
    // live relay tasks so a dead connection never leaves a tunnel relay (and
    // its pooled upstream TCP socket) running. Natural teardown (the dropped
    // channels signal the relay) would also stop them, but a backend wedged
    // past its read-frame budget could linger; the explicit abort bounds it.
    for (_, st) in ws_tunnels {
        st.task.abort();
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
        // The cell spawn site inserts an empty `Progressive` StreamTx
        // alongside the receiver, so this entry exists. INC-5: `Progressive`
        // is the only variant.
        let StreamTx::Progressive {
            queue,
            ended,
            reset,
            fin_sent,
            ..
        } = stream_response
            .entry(sid)
            .or_insert_with(StreamTx::progressive);
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
            // SESSION 24 / INC-3: push the DECODED item; the actor's
            // `drain_streams_to_conn` Progressive arm encodes it onto
            // the `quiche::h3::Connection`.
            Ok(RespEvent::Head { status, headers }) => {
                queue.push_back(RespItem::Head { status, headers });
            }
            Ok(RespEvent::Body(b)) => queue.push_back(RespItem::Body(b)),
            Ok(RespEvent::Trailers(t)) => queue.push_back(RespItem::Trailers(t)),
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
    // quiche). SESSION 24 / INC-3: the queue now holds DECODED items;
    // sum `Body` bytes (the load-bearing quantity, each ≤
    // `H3_RESP_CHUNK_MAX`) plus `Head`/`Trailers` field bytes (tiny /
    // bounded — counted too for a sound OVER-estimate, parity with the
    // request gauge) + a sound UPPER bound on channel occupancy
    // (`used_slots × (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX)`). The gauge
    // must over- not under-count.
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

/// SESSION 24 / INC-3: one DECODED response item queued for a
/// `Progressive` stream. The actor encodes it onto the
/// `quiche::h3::Connection` via `send_response` / `send_body` /
/// `send_additional_headers`. The queue holds only bounded decoded
/// items (the R8 egress bound) — never the whole response.
enum RespItem {
    /// The response head — encoded via `h3.send_response` (once).
    Head {
        /// `:status`.
        status: u16,
        /// Hop-by-hop-stripped non-pseudo response headers.
        headers: Vec<(String, String)>,
    },
    /// A body chunk (≤ `H3_RESP_CHUNK_MAX`) — encoded via
    /// `h3.send_body`; a partial write keeps the unsent tail at front.
    Body(Bytes),
    /// The trailing field section — encoded via
    /// `h3.send_additional_headers(.., is_trailer=true, ..)`.
    Trailers(Vec<(String, String)>),
}

/// Per-stream outbound cursor.
///
/// Two variants. `Buffered` is the LEGACY shape (a single pre-built
/// `Vec`, byte cursor + FIN-on-empty) — it is **unchanged** and still
/// serves H2/H3 round-trips and the inline 400/502/413 error responses
/// (bit-for-bit identical wire behaviour; SESSION 4 adds no buffering
/// to that path). `Progressive` is the SESSION 4 / P1-B incremental
/// H1-response egress: a bounded queue of DECODED response items fed by
/// [`stream_h1_response`] over a bounded channel, encoded into the
/// `quiche::h3::Connection` (INC-3) as flow control allows. The queue +
/// the channel are the memory bound (≈ `H3_RESP_CHANNEL_DEPTH` × chunk),
/// independent of total response size.
enum StreamTx {
    /// SESSION 4 / P1-B: progressive response egress (INC-3: via
    /// `quiche::h3`). SESSION 25 / INC-5: the SOLE egress — the legacy
    /// `Buffered` raw-`stream_send` cursor was deleted once the inline-400
    /// authority reject joined this decoded path (production is 100% on
    /// quiche::h3).
    ///
    /// `queue` holds DECODED response items not yet encoded onto the
    /// h3 connection. `head_sent` guards the one-shot `send_response`.
    /// `ended` ⇒ once `queue` drains, set FIN (`send_body(.., true)`).
    /// `reset` ⇒ `RESET_STREAM` (never FIN) — a partial body is never
    /// presented as complete (response-splitting / cache-poisoning
    /// guard). `fin_sent` guards the one-shot FIN/shutdown.
    Progressive {
        queue: VecDeque<RespItem>,
        head_sent: bool,
        ended: bool,
        reset: bool,
        fin_sent: bool,
    },
}

impl StreamTx {
    /// Construct an empty SESSION 4 / P1-B progressive egress cursor.
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

/// Pump per-stream responses out. The `Buffered` arm raw-`stream_send`s
/// pre-encoded bytes (INC-1 Exp4: raw bidi egress coexists with
/// quiche::h3 ingress on one conn — unchanged). The `Progressive` arm
/// (INC-3) encodes DECODED items onto the `quiche::h3::Connection` via
/// `send_response`/`send_body`/`send_additional_headers`; we send
/// incrementally because those calls may refuse bytes (`Done` /
/// `StreamBlocked`) when the send window is saturated. `h3` is `None`
/// until `with_transport` builds it post-establishment; while `None`
/// the Progressive arm does nothing this tick (no h3 responses can be
/// sent before the h3 conn exists).
fn drain_streams_to_conn(
    conn: &mut quiche::Connection,
    mut h3: Option<&mut quiche::h3::Connection>,
    streams: &mut HashMap<u64, StreamTx>,
) {
    let mut to_drop = Vec::new();
    for (&sid, tx) in streams.iter_mut() {
        match tx {
            // SESSION 24 / INC-3: progressive egress via quiche::h3 (SESSION
            // 25 / INC-5: the SOLE egress arm).
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
                // Can't send H3 responses before `with_transport` builds
                // the h3 conn. Do nothing this tick; the channel-refill
                // gate keeps the (bounded) queue intact for the next.
                let Some(h3c) = h3.as_deref_mut() else {
                    continue;
                };
                // Encode queued DECODED items front-to-back. A blocked
                // send (`Done` / `StreamBlocked`) leaves the item at the
                // front for next tick — this partial-write/Done retry IS
                // the egress R8 gate (never force-drain). A genuine error
                // latches `reset`.
                while let Some(front) = queue.front_mut() {
                    match front {
                        RespItem::Head { status, headers } => {
                            if *head_sent {
                                // Defensive: a duplicate Head can't be
                                // sent twice — drop it.
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
                                    // Partial: keep the unsent tail at the
                                    // front (R8 gate — do NOT force-drain).
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
                            // The trailing field section is ALWAYS the
                            // terminal frame on the stream (the RespEvent
                            // ordering contract emits Trailers only after
                            // the last Body and immediately before End;
                            // nothing follows it), and quiche rejects any
                            // DATA after it. So the FIN rides on this
                            // HEADERS frame: `fin=true`, mark terminal. A
                            // later `End` event just sets `ended` and is
                            // a no-op (the arm is `fin_sent`-guarded).
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
                    // Abort: RESET_STREAM, NEVER FIN — a partial body
                    // is never presented as a complete response (Q2 /
                    // C1: H3_INTERNAL_ERROR, not the graceful code). The
                    // transport-level shutdown is UNCHANGED.
                    match conn.stream_shutdown(sid, quiche::Shutdown::Write, H3_INTERNAL_ERROR) {
                        Ok(()) | Err(quiche::Error::Done) => {}
                        Err(e) => {
                            tracing::debug!(error = %e, stream_id = sid, "stream_shutdown (resp)");
                        }
                    }
                    *fin_sent = true;
                    to_drop.push(sid);
                } else if *ended && queue.is_empty() && !*fin_sent {
                    // Clean completion: FIN via a zero-length
                    // `send_body(.., true)`. Skipped when the FIN already
                    // rode on a terminal trailer section above (quiche
                    // rejects DATA after the trailing field section).
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
        // Mark terminal so subsequent calls skip it; remove lazily to
        // keep the allocation low (unchanged from the legacy policy).
        if let Some(StreamTx::Progressive { fin_sent, .. }) = streams.get_mut(&sid) {
            *fin_sent = true;
        }
    }
    streams.retain(|_, tx| {
        let StreamTx::Progressive { fin_sent, .. } = tx;
        !*fin_sent
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

/// SESSION 24 / INC-2 — drain request-body bytes for ONE stream off the
/// `quiche::h3::Connection` into its bounded body channel.
///
/// **R8 backpressure (the load-bearing memory mechanism):** we only call
/// `recv_body` while the bounded channel has spare capacity. When it is
/// full we STOP reading — quiche then does not extend the QUIC stream
/// flow-control window, so the H3 client is paused. In-flight request-body
/// memory stays ≈ `H3_BODY_CHANNEL_DEPTH * H3_BODY_CHUNK_MAX`, INDEPENDENT
/// of total body size (quiche holds the un-read remainder in its own
/// flow-control-bounded receive buffer — never an unbounded proxy buffer).
///
/// **F-CAP-1 (413):** the cumulative `MAX_REQUEST_BODY_BYTES` cap (which
/// previously lived inside `StreamRxBuf::feed_body`) is enforced here via
/// `body_seen`; on overflow we emit `ReqBodyEvent::Reset` (the consumer
/// maps `Reset` → `413`) and tear the stream down.
///
/// **F-MD-4 (smuggling guard):** any `recv_body` error (a peer
/// RESET_STREAM / STOP_SENDING surfaces here) maps to
/// `ReqBodyEvent::Reset`, NEVER a clean end — a truncated request must
/// never reach the backend as complete. The clean end is emitted by the
/// `Finished` event in [`poll_h3`], not here.
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
                    // F-CAP-1: cumulative body exceeded the cap → Reset
                    // (the consumer maps Reset → 413) + tear down. Do NOT
                    // forward any further body.
                    if let Some(tx) = body_tx_by_stream.remove(&sid) {
                        let _ = tx.try_send(ReqBodyEvent::Reset);
                    }
                    body_seen.remove(&sid);
                    pending_trailers.remove(&sid);
                    return;
                }
                // capacity > 0 was checked above and the actor is the sole
                // producer, so try_send accepts this chunk.
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
                // F-MD-4: a mid-body stream error (peer reset/stopped, or a
                // transport fault) MUST relay as a reset, never a clean EOF.
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

/// SESSION 24 / INC-2 — drive the `quiche::h3::Connection` ingress: poll
/// events, decode request HEADERS, run the KEEP-surface validation
/// (pseudo-headers #12–15, authority sanitisation) and spawn the
/// H3→H1/H2/H3 cell task per request, streaming the request body through
/// the bounded channel with R8 backpressure. Replaces the hand-rolled
/// `StreamRxBuf` / `stream_recv` request decoder + uni-stream drain.
///
/// quiche `poll` is **edge-triggered** (`Data` fires once and re-arms only
/// after the stream is drained to `Done`); because the R8 gate stops
/// `recv_body` while the channel is full (not draining to `Done`), the
/// `Data` event will not re-fire — so PASS 1 re-attempts the capacity-
/// gated drain for every body-phase stream every tick, independent of the
/// poll events (exactly what the old `drain_body_stream` did).
///
/// The response egress is the decoded `RespEvent::{Head,Body,Trailers}` →
/// `quiche::h3::send_response`/`send_body`/`send_additional_headers` path
/// (`drain_resp_channels` + `drain_streams_to_conn`, SESSION 24 / INC-3).
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
    // SESSION 27 / WS-over-H3 Stage A: gates `:protocol` Extended-CONNECT
    // acceptance in `validate_request_pseudo_headers`. `false` ⇒ unchanged.
    ws_enabled: bool,
    // SESSION 28 / WS-over-H3 Stage C: per-stream WS tunnel state + the
    // injected relay launcher. Both inert when no WS tunnel exists (R3).
    ws_tunnels: &mut HashMap<u64, WsTunnelState>,
    ws_relay_launcher: Option<&WsRelayLauncher>,
) {
    // PASS 1 — re-arm / backpressure drain (see fn doc): every body-phase
    // stream gets a capacity-gated `recv_body` drain this tick, regardless
    // of whether `poll` surfaces a fresh `Data` event for it.
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

                // A SECOND HEADERS frame on a stream already in the body
                // phase is the RFC 9114 §4.1 trailing field section.
                if body_tx_by_stream.contains_key(&sid) {
                    // RFC 9114 §4.3: a pseudo-header in the trailing field
                    // section is malformed → Reset (never a forwarded body).
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

                // Initial request HEADERS. (#12–15) pseudo-header
                // validation FIRST — before building the request or
                // dialling any upstream (smuggling / desync guard, R12:
                // single ingress ⇒ covers all H3-front cells).
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
                // before ANY upstream is dialled. Same predicate as the
                // H1/H2 path (byte-identical across protocols).
                if !req.authority.is_empty() {
                    if let Err(e) = lb_core::authority::validate(&req.authority) {
                        tracing::warn!(
                            authority = %req.authority,
                            error = ?e,
                            stream_id = sid,
                            "ROUND8-L7-16: H3 :authority rejected before upstream selection"
                        );
                        // SESSION 25 / INC-5: emit the inline 400 via the
                        // DECODED Progressive egress (quiche::h3). SESSION 28:
                        // single-sourced as `spawn_inline_h3_response` (shared
                        // with the WS extended-CONNECT rejects). Same 400 +
                        // "bad request" body, byte-for-byte.
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

                // SESSION 28 / WS-over-H3 (RFC 9220) Stage C: intercept a
                // validated `:protocol=websocket` extended CONNECT BEFORE the
                // normal request cell is spawned.
                // `validate_request_pseudo_headers` (under `ws_enabled`)
                // already guaranteed a `:protocol` implies `:method=CONNECT`
                // + `:scheme` + `:path` + `:authority`; `:protocol` lands in
                // `req.extra` (HeaderValue not one of the parsed pseudo
                // names). Build the bounded tunnel, call the injected relay
                // launcher (which dials the H1 backend + completes the
                // upstream RFC 6455 handshake BEFORE the 200 — the H3 analog
                // of the WS-H1 GHSA / WS-H2 F-S27-1 ordering), and register
                // the per-stream tunnel state. The `pump_ws_tunnels` tick
                // then sends the 200 on upstream-ready and relays frames.
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
                // Build the bounded request-body + response channels and
                // spawn the cell producer. H3→H2 takes precedence, then
                // H3→H3, else H3→H1. (Identical cell selection to the
                // pre-INC-2 path; only the framing of the read side moved.)
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
                    // Bodyless (HEADERS + FIN): the consumer's first
                    // `body_rx.recv()` must see `End` ⇒ send it now and let
                    // `btx` drop (matches the pre-INC-2 bodyless contract
                    // exactly; do NOT register the stream as body-phase).
                    let _ = btx.try_send(ReqBodyEvent::End {
                        trailers: Vec::new(),
                    });
                } else {
                    // Body to follow: register the body-phase channel +
                    // cap counter, then drain any DATA that arrived
                    // coalesced with the head this tick. The clean `End`
                    // is emitted later by the `Finished` event.
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
                // SESSION 28 / WS-over-H3 Stage C: a WS tunnel stream FIN is
                // the client closing its WS send half — map it (clean EOF vs
                // reset-on-finish) onto the tunnel reader, NOT the request-
                // body path.
                if let Some(st) = ws_tunnels.get_mut(&sid) {
                    ws_handle_client_fin(conn, h3, sid, st);
                    continue;
                }
                // F-MD-4 SMUGGLING GUARD. quiche's `poll` can return
                // `Event::Finished` for a request stream that was actually
                // RESET *after* its last DATA frame: `recv_body` on a reset
                // stream calls `process_finished_stream` (it is
                // `stream_finished()`), queueing it, and `poll`'s FIRST
                // `finished_streams` pop (quiche-0.28 mod.rs:2072) returns
                // `Finished` WITHOUT the reset re-check that only its SECOND
                // pop (:2106-2114) performs. Treating that as a clean end
                // would write the chunked `0\r\n\r\n` terminator and present
                // a truncated request to the backend as complete. So probe
                // the transport exactly as quiche's own guard does — a
                // zero-length `stream_recv` returns `StreamReset` for a reset
                // stream — and map that to `Reset`, never `End`. A genuinely
                // FIN'd stream returns `Ok((0, true))` and takes the clean
                // path (the in-test liveness request proves this is not a
                // blanket Reset).
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
                // SESSION 28 / WS-over-H3 Stage C: a WS tunnel stream reset is
                // an abnormal WS drop — surface it to the tunnel reader as
                // `Reset` (ConnectionReset, distinct from a clean Close), the
                // reset-vs-EOF mapping.
                if let Some(st) = ws_tunnels.get_mut(&sid) {
                    ws_handle_client_reset(sid, st);
                    continue;
                }
                // F-MD-4: the client reset the request stream mid-flight.
                // Relay as a backend reset (never a clean EOF) + tear down.
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
            // GoAway / PriorityUpdate / (H3 DATAGRAM) — quiche handles the
            // control-stream rules natively; nothing to do here.
            Ok((_sid, _)) => {}
            Err(quiche::h3::Error::Done) => break,
            Err(e) => {
                // quiche enforces #11 / #16–22 / #24 itself: on a control /
                // QPACK / frame-sequence violation it has already issued
                // `conn.close(true, …)`. Stop polling this tick; the actor
                // loop's next `drain_conn_send` ships the CONNECTION_CLOSE
                // and `is_closed()` then breaks the actor.
                tracing::debug!(error = %e, "INC-2: h3.poll error (quiche closed the connection)");
                break;
            }
        }
    }

    // SESSION 28 / WS-over-H3 Stage C: after the event loop, advance every
    // WS tunnel one tick — resolve upstream readiness (send the 200 or the
    // error), then relay frames both directions (inbound re-arm + outbound
    // drain, both bounded — R8). Inert when `ws_tunnels` is empty (R3).
    pump_ws_tunnels(
        conn,
        h3,
        ws_tunnels,
        resp_tasks,
        resp_rx_by_stream,
        stream_response,
    );
}

/// SESSION 24 / INC-2 (test-gauge) — record the per-stream retained
/// request-body memory at its largest instant. After the migration the
/// proxy retains only the bounded channel occupancy + the just-read
/// scratch chunk; quiche holds the un-read remainder in its own
/// flow-control-bounded receive buffer (NOT a proxy buffer). This is an
/// UPPER bound (used slots × chunk-max + last read) and is body-size
/// INDEPENDENT — a buffering ingress would make it grow with body size.
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

/// Round-robin-ish: pick the first backend for now. 3b.3c-2 does not
/// plumb a real balancer state through the router; the balancer crate
/// will own that when the router moves into the main binary path.
fn select_backend(backends: &Arc<Vec<SocketAddr>>) -> Option<SocketAddr> {
    backends.first().copied()
}

// ─── SESSION 28 / WS-over-H3 (RFC 9220) Stage C — tunnel-mode helpers ───

/// Spawn an inline H3 response (`status` + a short plain `msg` body) on
/// `sid` via the DECODED Progressive egress. Single-sources the "small
/// synthetic response" pattern shared by the inline 400 (:authority
/// reject) and the WS extended-CONNECT rejects (501 unsupported
/// `:protocol`, 502 relay-unavailable / upstream-failure). The body is a
/// `'static` string so the spawned task captures no borrowed state.
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

/// Set up a WebSocket-over-H3 tunnel for a validated extended CONNECT.
///
/// Enforces the WS-only `:protocol` value (RFC 8441/9220 — unknown
/// protocol → `501`), requires the injected relay launcher (fail-closed
/// `502` if absent — the binary pairs `ws_enabled` with a launcher), then
/// builds the bounded [`H3WsTunnel`] seam, hands the tunnel to the
/// launcher (which dials the H1 backend + completes the upstream RFC 6455
/// handshake BEFORE the 200), and registers the per-stream tunnel state.
/// The `200`/error is sent later by [`pump_ws_tunnels`] once the launcher
/// signals readiness — the upstream-before-200 ordering (R12, shared with
/// WS-H1/H2).
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
    // RFC 8441/9220: `websocket` is the only `:protocol` value the gateway
    // bootstraps. Any other registered protocol → 501 Not Implemented.
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
        // Fail closed: a WS listener always injects a launcher. Reaching
        // here means a misconfiguration — never tunnel without a relay.
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
    // The client's offered subprotocol list (forwarded to the upstream;
    // the upstream's selection is echoed back in the 200, RFC 8441 §5).
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

/// Inbound pump (H3 stream DATA → `proxy_frames` reader). Reads DATA off
/// the tunnel stream while the bounded `to_reader` channel has capacity; a
/// full channel STOPS the read so quiche does not extend the QUIC flow-
/// control window — R8 inbound backpressure, in-flight bytes bounded
/// independent of message volume. Any `recv_body` error maps to
/// [`TunnelInbound::Reset`] (abnormal drop, NOT a clean EOF — the
/// F-MD-4-adjacent guard). No-op before the tunnel is active.
fn ws_drain_inbound(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    sid: u64,
    st: &mut WsTunnelState,
) {
    if !st.activated {
        return;
    }
    // Clone the sender handle so the error arm can drop the original
    // (`st.to_reader = None`) after delivering the terminal Reset.
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

/// The client FIN'd its WS send half. Drain any coalesced DATA, then map
/// the terminal: a genuine FIN → drop the reader sender (clean EOF →
/// `proxy_frames` forwards a WS Close to the backend); a Finished-on-reset
/// (quiche can surface `Event::Finished` for a stream reset after its last
/// DATA — the migration's F-MD-4 concern, probed with a zero-length
/// `stream_recv`) → [`TunnelInbound::Reset`] (abnormal drop).
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

/// The client RESET the WS tunnel stream. Surface an abnormal drop to the
/// reader as [`TunnelInbound::Reset`] (distinct from a clean Close — the
/// reset-vs-EOF mapping).
fn ws_handle_client_reset(sid: u64, st: &mut WsTunnelState) {
    if let Some(tx) = st.to_reader.take() {
        tracing::debug!(
            stream_id = sid,
            "WS-H3: client reset tunnel stream; Reset to relay"
        );
        let _ = tx.try_send(TunnelInbound::Reset);
    }
}

/// Abort a tunnel stream abnormally: `RESET_STREAM` +`STOP_SENDING` with
/// `H3_REQUEST_CANCELLED` (RFC 9220 §3) and drop the reader sender. Used
/// when a `send_response`/`send_body` on the tunnel stream fails.
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

/// Per-tick WebSocket tunnel pump. For each tunnel:
///   1. resolve upstream readiness — `Ready` queues the 200, `Failed`
///      (or a dropped sender) emits the inline error + tears down;
///   2. send the queued 200 head (retry under a full send window);
///   3. once active, pump inbound ([`ws_drain_inbound`] — Data is edge-
///      triggered so this re-arms each tick) + outbound (relay →
///      `send_body`, R8: retain the unsent tail, a full send window stops
///      us pulling from `from_writer` which parks the relay's `PollSender`);
///   4. when the relay finishes (`from_writer` closed) FIN the H3 stream
///      and remove the tunnel.
///
/// Inert when `ws_tunnels` is empty (R3).
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
                    // Still dialing — nothing else to advance this tick.
                    continue;
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    // Launcher dropped the sender without a verdict — treat
                    // as a failed upstream (fail closed, no tunnel).
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

        // (3b) Outbound: relay → H3 stream DATA. R8: retain the unsent tail;
        // a full send window stops us pulling more from `from_writer`.
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
            // The relay task is already finished on the FIN/Failed paths;
            // abort is a no-op there and bounds any lingering task on the
            // send-error teardown path. Dropping `st` drops the channels.
            st.task.abort();
        }
    }
}
