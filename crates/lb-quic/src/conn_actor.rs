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
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;

use crate::h3_bridge::{H3Request, StreamRxBuf, h3_to_h1_roundtrip, h3_to_h3_roundtrip};

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

        let next_wait = params.conn.timeout().unwrap_or(Duration::from_millis(100));

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
                let _ = params.conn.close(false, 0x0, b"shutdown");
                drain_conn_send(&params.socket, &mut params.conn, &mut out_buf).await;
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
                &mut request_tasks,
                &params.pool,
                &params.backends,
                params.h3_backend.as_ref(),
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
fn poll_h3(
    conn: &mut quiche::Connection,
    rx_by_stream: &mut HashMap<u64, StreamRxBuf>,
    request_tasks: &mut Vec<tokio::task::JoinHandle<(u64, Vec<u8>)>>,
    pool: &TcpPool,
    backends: &Arc<Vec<SocketAddr>>,
    h3_backend: Option<&(QuicUpstreamPool, SocketAddr, String)>,
) {
    let readable: Vec<u64> = conn.readable().collect();
    for sid in readable {
        let mut buf = [0u8; 8192];
        loop {
            match conn.stream_recv(sid, &mut buf) {
                Ok((n, _fin)) => {
                    let rx = rx_by_stream.entry(sid).or_default();
                    match rx.feed(buf.get(..n).unwrap_or(&[])) {
                        Ok(Some(headers)) => {
                            let req = H3Request::from_headers(headers);
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
                            let pool = pool.clone();
                            request_tasks.push(tokio::spawn(async move {
                                let bytes = match h3_to_h1_roundtrip(&req, backend, &pool).await {
                                    Ok(b) => b,
                                    Err(e) => {
                                        tracing::warn!(error = %e, "H3→H1 roundtrip failed");
                                        Vec::new()
                                    }
                                };
                                (sid, bytes)
                            }));
                        }
                        Ok(None) => {}
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

/// Round-robin-ish: pick the first backend for now. 3b.3c-2 does not
/// plumb a real balancer state through the router; the balancer crate
/// will own that when the router moves into the main binary path.
fn select_backend(backends: &Arc<Vec<SocketAddr>>) -> Option<SocketAddr> {
    backends.first().copied()
}
