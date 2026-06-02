//! SESSION 27 / WS-over-H3 (RFC 9220) Stage B — bounded
//! `AsyncRead + AsyncWrite` tunnel adapter over a single H3 bidirectional
//! stream.
//!
//! ## Why this exists
//!
//! The WebSocket frame relay is single-sourced (R12) as
//! `lb_l7::ws_proxy::WsProxy::proxy_frames`, which is generic over
//! `AsyncRead + AsyncWrite` IO. The H3 datapath lives here in `lb-quic`,
//! and `lb-quic` cannot depend on `lb-l7` (that would form a cycle —
//! `lb-l7` already depends on `lb-quic`). This module is the protocol-
//! agnostic SEAM: it turns the bytes flowing on one H3 bidi stream into a
//! pair of bounded channels exposed as an `AsyncRead + AsyncWrite`
//! ([`H3WsTunnel`]) on one side and a plain channel endpoint pair
//! ([`H3TunnelEndpoints`]) on the actor side. A higher layer that depends
//! on BOTH crates (the `lb` binary) can then run `proxy_frames` over the
//! `H3WsTunnel` while the per-connection actor pumps the H3 stream into /
//! out of the endpoints.
//!
//! ## Bounded by construction (R8)
//!
//! Both directions ride a **bounded** `tokio::sync::mpsc` channel of
//! depth [`H3_WS_TUNNEL_DEPTH`] carrying chunks of at most
//! [`H3_WS_TUNNEL_CHUNK_MAX`] bytes. The total in-flight bytes retained
//! per direction is therefore `<= depth * chunk_max` (≈ 64 KiB),
//! INDEPENDENT of how much the peer sends:
//!
//! * **Write side** ([`AsyncWrite`] on `H3WsTunnel`): backed by a
//!   [`tokio_util::sync::PollSender`]. When the actor stops draining the
//!   `from_writer` receiver, `poll_reserve` returns `Pending` and the
//!   writer (i.e. `proxy_frames`) PARKS — true end-to-end backpressure,
//!   not buffering. (This is the property the WS-over-H2 path lacked,
//!   CF-S27-2.)
//! * **Read side** ([`AsyncRead`] on `H3WsTunnel`): a stalled reader
//!   leaves messages queued in the bounded channel; once the channel is
//!   full the actor's `to_reader.try_send` fails and the actor stops
//!   pulling from `quiche` (QUIC flow control is then not extended), so
//!   the client is paced at the window.
//!
//! ## Close + reset mapping (RFC 9220)
//!
//! * Orderly close (peer FIN on the stream) → the actor drops its
//!   `to_reader` sender → the tunnel reader observes channel-closed →
//!   `AsyncRead` returns EOF (`Ok(())` with nothing filled).
//! * Stream RESET (`H3_REQUEST_CANCELLED` per RFC 9220) → the actor sends
//!   [`TunnelInbound::Reset`] → the reader surfaces
//!   `io::ErrorKind::ConnectionReset`, which `proxy_frames`/tungstenite
//!   treats as an abnormal drop (distinct from a clean WS Close — the
//!   F-MD-4-adjacent mapping).
//! * Tunnel writer shutdown (`proxy_frames` finished) → `poll_shutdown`
//!   drops the `PollSender`, closing `from_writer` so the actor FINs the
//!   H3 stream.
//!
//! This module is **standalone**: it has no `quiche` dependency and is
//! unit-testable in isolation (see the tests below). Wiring it into
//! `conn_actor` is a later stage.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::PollSender;

/// Per-direction channel depth. With [`H3_WS_TUNNEL_CHUNK_MAX`] this caps
/// the in-flight bytes retained in one direction to
/// `H3_WS_TUNNEL_DEPTH * H3_WS_TUNNEL_CHUNK_MAX` (≈ 64 KiB), matching the
/// request-body channel discipline (`conn_actor::H3_BODY_CHANNEL_DEPTH`).
pub const H3_WS_TUNNEL_DEPTH: usize = 8;

/// Largest chunk the actor pushes toward the reader per channel message.
/// Mirrors `h3_bridge::H3_BODY_CHUNK_MAX` (8 KiB) so the WS tunnel reuses
/// the proven bound.
pub const H3_WS_TUNNEL_CHUNK_MAX: usize = 8 * 1024;

/// Actor → tunnel-reader message. Carries either a chunk of stream bytes
/// or an out-of-band stream-RESET signal.
#[derive(Debug, Clone)]
pub enum TunnelInbound {
    /// A chunk read off the H3 stream (≤ [`H3_WS_TUNNEL_CHUNK_MAX`]).
    Data(Bytes),
    /// The H3 stream was RESET by the peer (`H3_REQUEST_CANCELLED`,
    /// RFC 9220). Surfaced to the reader as `ConnectionReset` — an
    /// abnormal drop, NOT a clean EOF.
    Reset,
}

/// The actor-side endpoints of a tunnel. The per-connection actor:
/// * pushes bytes it reads off the H3 stream into `to_reader`
///   (`try_send(TunnelInbound::Data(..))`), or `TunnelInbound::Reset` on a
///   stream reset, and drops `to_reader` on FIN to signal EOF;
/// * drains `from_writer` and writes each `Bytes` onto the H3 stream
///   (`stream_send`), FINning when `from_writer` closes.
pub struct H3TunnelEndpoints {
    /// Sender into the tunnel's read side. Bounded — `try_send` fails when
    /// full, which the actor uses as the QUIC-flow-control backpressure
    /// signal (stop pulling from `quiche`).
    pub to_reader: mpsc::Sender<TunnelInbound>,
    /// Receiver of bytes the tunnel writer produced. The actor sends these
    /// on the H3 stream; channel-closed ⇒ writer finished ⇒ FIN the stream.
    pub from_writer: mpsc::Receiver<Bytes>,
}

/// SESSION 28 / WS-over-H3 (RFC 9220) Stage C — the validated extended
/// CONNECT target the actor hands the injected relay launcher.
///
/// Built from the request the actor already validated
/// ([`crate::h3_bridge::validate_request_pseudo_headers`] guarantees
/// `:method=CONNECT` + `:protocol` + `:scheme` + `:path` + `:authority`
/// under `ws_enabled`). The launcher (in the `lb` binary) uses `path` +
/// `subprotocols` to drive the upstream RFC 6455 client handshake.
#[derive(Debug, Clone)]
pub struct WsConnectRequest {
    /// `:authority` of the extended CONNECT (the WS target host).
    pub authority: String,
    /// `:path` of the extended CONNECT — the WS resource path, forwarded
    /// verbatim onto the upstream RFC 6455 `GET` request line.
    pub path: String,
    /// The client's `sec-websocket-protocol` offer (a comma-separated
    /// list), if present. Forwarded to the upstream so a real subprotocol
    /// negotiation happens on the H1 leg; the upstream's selection is
    /// echoed back in the `200` (RFC 8441 §5 / RFC 6455 §1.3).
    pub subprotocols: Option<String>,
}

/// SESSION 28 / WS-over-H3 Stage C — the launcher's readiness verdict,
/// gating the H3 response the actor sends. This is the H3 analog of the
/// WS-H1 GHSA fix / WS-H2 F-S27-1: the upstream RFC 6455 handshake
/// completes (or fails) **before** any client-visible `2xx`, so a client
/// is never committed to WS framing toward a backend that never agreed.
#[derive(Debug)]
pub enum WsUpstreamOutcome {
    /// The upstream handshake completed. The actor sends `200` (extended
    /// CONNECT success, RFC 9220) and begins tunnel-mode relay. `headers`
    /// carries any response field to echo — e.g. the upstream-selected
    /// `sec-websocket-protocol`.
    Ready {
        /// Extra response header fields to emit alongside `:status 200`.
        headers: Vec<(String, String)>,
    },
    /// The upstream dial/handshake failed. The actor sends `status`
    /// (`502` refused/unreachable, `504` dial/handshake timeout) and
    /// tears the stream down. No tunnel byte ever flows.
    Failed {
        /// HTTP status the actor returns to the H3 client.
        status: u16,
    },
}

/// SESSION 28 / WS-over-H3 Stage C — what the injected launcher returns:
/// a readiness signal + the relay task handle.
///
/// The actor polls `ready` each tick (non-blocking `try_recv`); on
/// [`WsUpstreamOutcome::Ready`] it sends the `200` and activates the
/// tunnel pump; on [`WsUpstreamOutcome::Failed`] it sends the error and
/// tears down. `task` is the spawned relay (dial + handshake +
/// `proxy_frames`); the actor aborts it on teardown so a torn-down tunnel
/// never leaks its relay task.
pub struct WsRelayHandle {
    /// Upstream-handshake readiness — resolves once, before the `200`.
    pub ready: oneshot::Receiver<WsUpstreamOutcome>,
    /// The relay task (dial + upstream handshake + `proxy_frames` over the
    /// tunnel). Aborted by the actor on teardown.
    pub task: JoinHandle<()>,
}

/// SESSION 28 / WS-over-H3 Stage C — the dependency-inversion seam.
///
/// `lb-quic` cannot import `lb_l7::ws_proxy::proxy_frames` (the
/// `lb-l7 → lb-quic` dependency would form a cycle), so the relay is
/// **injected** as this closure from the `lb` binary (which sees both
/// crates), MIRRORING the existing
/// `config_factory: Arc<dyn Fn()->Result<quiche::Config,_>>` threaded
/// through the same `QuicListenerParams → RouterParams → ActorParams`
/// chain. The closure dials the H1 backend, completes the upstream
/// RFC 6455 handshake **before** signalling readiness, then runs the
/// single-sourced `proxy_frames` over the [`H3WsTunnel`] (R12 — the relay
/// is NOT duplicated in `lb-quic`).
pub type WsRelayLauncher = Arc<dyn Fn(H3WsTunnel, WsConnectRequest) -> WsRelayHandle + Send + Sync>;

/// The `proxy_frames`-side handle: a bounded `AsyncRead + AsyncWrite` over
/// one H3 bidi stream. Cheap to construct; not `Clone` (a tunnel is owned
/// by exactly one relay task).
pub struct H3WsTunnel {
    /// Bounded write path. `PollSender` exposes `poll_reserve` so
    /// `poll_write` PARKS under backpressure instead of buffering.
    writer: PollSender<Bytes>,
    /// Bounded read path: chunks (or a Reset) pushed by the actor.
    reader: mpsc::Receiver<TunnelInbound>,
    /// Unconsumed tail of the last `Data` chunk when the caller's read
    /// buffer was smaller than the chunk. Drained before the next recv.
    leftover: Bytes,
    /// Set once the read side has observed EOF or a Reset — subsequent
    /// reads return the same terminal result (0-fill EOF) deterministically.
    read_done: bool,
    /// Set once `poll_shutdown` closed the writer; further writes error.
    write_closed: bool,
}

impl std::fmt::Debug for H3WsTunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H3WsTunnel")
            .field("leftover_len", &self.leftover.len())
            .field("read_done", &self.read_done)
            .field("write_closed", &self.write_closed)
            .finish()
    }
}

impl H3WsTunnel {
    /// Build a tunnel + its matching actor-side endpoints. Both directions
    /// are bounded to [`H3_WS_TUNNEL_DEPTH`]. Returns
    /// `(tunnel, endpoints)`: `tunnel` is handed to the WS relay
    /// (`proxy_frames`), `endpoints` to the per-connection H3 actor.
    #[must_use]
    pub fn new() -> (Self, H3TunnelEndpoints) {
        // actor --to_reader--> tunnel.reader  (inbound: H3 stream → relay)
        let (to_reader, reader) = mpsc::channel::<TunnelInbound>(H3_WS_TUNNEL_DEPTH);
        // tunnel.writer --from_writer--> actor (outbound: relay → H3 stream)
        let (writer_tx, from_writer) = mpsc::channel::<Bytes>(H3_WS_TUNNEL_DEPTH);
        let tunnel = Self {
            writer: PollSender::new(writer_tx),
            reader,
            leftover: Bytes::new(),
            read_done: false,
            write_closed: false,
        };
        let endpoints = H3TunnelEndpoints {
            to_reader,
            from_writer,
        };
        (tunnel, endpoints)
    }

    /// Copy as much of `leftover` as fits into `buf`, retaining the rest.
    /// Returns `true` if any bytes were copied.
    fn drain_leftover(&mut self, buf: &mut ReadBuf<'_>) -> bool {
        if self.leftover.is_empty() || buf.remaining() == 0 {
            return false;
        }
        let n = self.leftover.len().min(buf.remaining());
        // `split_to` returns the leading `n` bytes and retains the tail in
        // `self.leftover` (cheap — Bytes is refcounted). Avoids a panicking
        // slice index.
        let head = self.leftover.split_to(n);
        buf.put_slice(&head);
        true
    }
}

impl Default for H3WsTunnel {
    fn default() -> Self {
        Self::new().0
    }
}

impl AsyncRead for H3WsTunnel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // Terminal EOF is sticky: once closed, every read is a clean EOF.
        if self.read_done {
            return Poll::Ready(Ok(()));
        }
        // 1) Serve any retained tail first.
        if self.drain_leftover(buf) {
            return Poll::Ready(Ok(()));
        }
        // 2) Pull the next inbound message.
        match self.reader.poll_recv(cx) {
            Poll::Ready(Some(TunnelInbound::Data(bytes))) => {
                self.leftover = bytes;
                // drain_leftover handles the empty-chunk case (no bytes
                // copied, returns Pending-equivalent by falling through to
                // a 0-fill Ready, which the caller re-polls). Guard against
                // a zero-length chunk producing a spurious EOF: only return
                // here when we actually placed bytes OR the chunk was
                // genuinely empty (treated as "no data this turn").
                if self.drain_leftover(buf) {
                    Poll::Ready(Ok(()))
                } else {
                    // Empty Data chunk: nothing to deliver. Re-arm by
                    // waking ourselves so the caller polls again rather
                    // than mistaking this for EOF.
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
            Poll::Ready(Some(TunnelInbound::Reset)) => {
                self.read_done = true;
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "h3 ws tunnel: stream reset (H3_REQUEST_CANCELLED)",
                )))
            }
            Poll::Ready(None) => {
                // Actor dropped `to_reader` ⇒ orderly stream end ⇒ EOF.
                self.read_done = true;
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for H3WsTunnel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.write_closed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "h3 ws tunnel: write after shutdown",
            )));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // Reserve a slot first — this is the backpressure point (R8): when
        // the actor is not draining `from_writer`, `poll_reserve` returns
        // Pending and the writer parks. Only after a slot is granted do we
        // commit a bounded chunk.
        match self.writer.poll_reserve(cx) {
            Poll::Ready(Ok(())) => {
                let n = data.len().min(H3_WS_TUNNEL_CHUNK_MAX);
                let chunk = Bytes::copy_from_slice(data.get(..n).unwrap_or(data));
                match self.writer.send_item(chunk) {
                    Ok(()) => Poll::Ready(Ok(n)),
                    Err(_) => {
                        // Receiver (actor) gone — the H3 stream is no longer
                        // consuming our writes.
                        self.write_closed = true;
                        Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "h3 ws tunnel: peer (actor) closed the write path",
                        )))
                    }
                }
            }
            Poll::Ready(Err(_)) => {
                self.write_closed = true;
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "h3 ws tunnel: write path closed",
                )))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Each `poll_write` already hands the chunk to the bounded channel;
        // there is no intermediate buffer to flush.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Close the write half: drop the PollSender so the actor's
        // `from_writer` channel closes and it FINs the H3 stream.
        self.write_closed = true;
        self.writer.close();
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // ── READ side ────────────────────────────────────────────────────────

    /// A chunk pushed by the actor is read back byte-identically, including
    /// non-UTF-8 bytes, and across a buffer smaller than the chunk
    /// (leftover retention).
    #[tokio::test]
    async fn read_chunk_byte_identical_with_small_buffer() {
        let (mut tunnel, ep) = H3WsTunnel::new();
        let payload = Bytes::from_static(&[0xFF, 0x00, 0x80, b'a', b'b', b'c', 0x7f]);
        ep.to_reader
            .send(TunnelInbound::Data(payload.clone()))
            .await
            .unwrap();
        drop(ep.to_reader); // signal EOF after the one chunk

        let mut got = Vec::new();
        // Read with a 2-byte buffer to force multiple poll_read passes that
        // exercise the leftover path.
        let mut small = [0u8; 2];
        loop {
            let n = tunnel.read(&mut small).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(small.get(..n).unwrap_or_default());
        }
        assert_eq!(got, payload.to_vec(), "read bytes must be byte-identical");
    }

    /// Dropping `to_reader` (orderly stream end) surfaces a clean EOF
    /// (read returns 0), and EOF is sticky.
    #[tokio::test]
    async fn dropped_sender_is_clean_eof() {
        let (mut tunnel, ep) = H3WsTunnel::new();
        drop(ep.to_reader);
        let mut buf = [0u8; 16];
        assert_eq!(tunnel.read(&mut buf).await.unwrap(), 0, "EOF expected");
        // Sticky: a second read is still a clean 0.
        assert_eq!(
            tunnel.read(&mut buf).await.unwrap(),
            0,
            "EOF must be sticky"
        );
    }

    /// A `Reset` message surfaces as `ConnectionReset` (abnormal drop),
    /// NOT a clean EOF — the F-MD-4-adjacent close-vs-reset mapping.
    #[tokio::test]
    async fn reset_maps_to_connection_reset_error() {
        let (mut tunnel, ep) = H3WsTunnel::new();
        ep.to_reader.send(TunnelInbound::Reset).await.unwrap();
        let mut buf = [0u8; 16];
        let err = tunnel
            .read(&mut buf)
            .await
            .expect_err("reset must surface as an io error, not EOF");
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
    }

    /// Data delivered BEFORE a reset is still read out, and the reset is
    /// observed on the following read (ordering preserved through the
    /// bounded channel).
    #[tokio::test]
    async fn data_then_reset_preserves_order() {
        let (mut tunnel, ep) = H3WsTunnel::new();
        ep.to_reader
            .send(TunnelInbound::Data(Bytes::from_static(b"hello")))
            .await
            .unwrap();
        ep.to_reader.send(TunnelInbound::Reset).await.unwrap();
        let mut buf = [0u8; 16];
        let n = tunnel.read(&mut buf).await.unwrap();
        assert_eq!(buf.get(..n).unwrap_or_default(), b"hello");
        let err = tunnel.read(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
    }

    // ── WRITE side ───────────────────────────────────────────────────────

    /// Bytes written through the tunnel arrive on `from_writer`
    /// byte-identically, chunked at most to `H3_WS_TUNNEL_CHUNK_MAX`.
    #[tokio::test]
    async fn write_arrives_on_endpoint_chunked() {
        let (mut tunnel, mut ep) = H3WsTunnel::new();
        // A payload larger than one chunk to prove the per-write cap.
        let big = vec![0xABu8; H3_WS_TUNNEL_CHUNK_MAX + 100];
        // Drain concurrently so the bounded channel does not wedge the test.
        let drainer = tokio::spawn(async move {
            let mut acc = Vec::new();
            while let Some(chunk) = ep.from_writer.recv().await {
                assert!(
                    chunk.len() <= H3_WS_TUNNEL_CHUNK_MAX,
                    "each channel message must be <= the chunk cap"
                );
                acc.extend_from_slice(&chunk);
            }
            acc
        });
        tunnel.write_all(&big).await.unwrap();
        tunnel.shutdown().await.unwrap();
        let got = drainer.await.unwrap();
        assert_eq!(got, big, "written bytes must round-trip byte-identical");
    }

    /// R8 — the load-bearing backpressure proof: with the actor NOT
    /// draining `from_writer`, a writer that exceeds the bounded channel
    /// capacity PARKS (does not complete, does not buffer unboundedly).
    /// Once draining resumes the write completes (liveness).
    #[tokio::test]
    async fn write_parks_under_backpressure_then_resumes() {
        let (mut tunnel, mut ep) = H3WsTunnel::new();
        // Each write_all of one chunk-max occupies exactly one channel slot.
        // Fill depth + a margin so the writer must block on a full channel.
        let chunk = vec![0x5au8; H3_WS_TUNNEL_CHUNK_MAX];
        let writes = H3_WS_TUNNEL_DEPTH + 4;

        let writer = tokio::spawn(async move {
            for _ in 0..writes {
                tunnel.write_all(&chunk).await.unwrap();
            }
            tunnel.shutdown().await.unwrap();
            tunnel
        });

        // Give the writer time to fill the channel and PARK. It must NOT
        // finish while we withhold the receiver: the bounded channel holds
        // at most `depth` items, so `writes > depth` cannot all be sent.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !writer.is_finished(),
            "writer must PARK under backpressure (bounded channel, no drain)"
        );

        // Resume draining → the parked writer makes progress and completes
        // (liveness), and the total bytes are exactly what was written.
        let mut total = 0usize;
        while let Some(c) = ep.from_writer.recv().await {
            total += c.len();
        }
        let _tunnel = tokio::time::timeout(Duration::from_secs(5), writer)
            .await
            .expect("writer must complete once draining resumes")
            .unwrap();
        assert_eq!(
            total,
            writes * H3_WS_TUNNEL_CHUNK_MAX,
            "all written bytes must arrive once backpressure is released"
        );
    }

    /// After `poll_shutdown`, the actor's `from_writer` closes (recv ⇒
    /// None) so the actor knows to FIN the H3 stream, and further writes
    /// error rather than silently succeeding.
    #[tokio::test]
    async fn shutdown_closes_endpoint_and_blocks_further_writes() {
        let (mut tunnel, mut ep) = H3WsTunnel::new();
        tunnel.write_all(b"last").await.unwrap();
        tunnel.shutdown().await.unwrap();
        // The single message is delivered, then the channel is closed.
        assert_eq!(ep.from_writer.recv().await.as_deref(), Some(&b"last"[..]));
        assert!(
            ep.from_writer.recv().await.is_none(),
            "from_writer must close after shutdown so the actor FINs"
        );
        let err = tunnel.write(b"more").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

    /// A write whose peer (actor) has dropped `from_writer` errors with
    /// BrokenPipe rather than hanging — the relay learns the H3 stream is
    /// gone.
    #[tokio::test]
    async fn write_after_actor_gone_is_broken_pipe() {
        let (mut tunnel, ep) = H3WsTunnel::new();
        drop(ep.from_writer);
        // The first write may need a poll to observe the closed receiver.
        let res = tunnel.write_all(b"orphaned").await;
        assert!(
            res.is_err_and(|e| e.kind() == io::ErrorKind::BrokenPipe),
            "write to a closed actor endpoint must be BrokenPipe"
        );
    }

    /// An empty `Data` chunk does not spuriously signal EOF; a subsequent
    /// real chunk is still delivered.
    #[tokio::test]
    async fn empty_data_chunk_is_not_eof() {
        let (mut tunnel, ep) = H3WsTunnel::new();
        ep.to_reader
            .send(TunnelInbound::Data(Bytes::new()))
            .await
            .unwrap();
        ep.to_reader
            .send(TunnelInbound::Data(Bytes::from_static(b"after-empty")))
            .await
            .unwrap();
        drop(ep.to_reader);
        let mut got = Vec::new();
        let mut buf = [0u8; 32];
        loop {
            let n = tunnel.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(buf.get(..n).unwrap_or_default());
        }
        assert_eq!(
            got, b"after-empty",
            "empty chunk must not truncate the stream"
        );
    }
}
