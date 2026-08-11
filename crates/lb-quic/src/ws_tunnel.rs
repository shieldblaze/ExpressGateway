//! WS-over-H3 (RFC 9220) — bounded `AsyncRead + AsyncWrite` tunnel adapter over one H3 bidi
//! stream. The WebSocket frame relay is single-sourced as `lb_l7::ws_proxy::proxy_frames`, but
//! `lb-quic` cannot depend on `lb-l7` (that would cycle), so this module is the protocol-agnostic
//! SEAM: one H3 bidi stream becomes a pair of bounded channels the `lb` binary can run
//! `proxy_frames` over. No `quiche` dependency.
//!
//! **Bounded by construction (R8).** Both directions ride a bounded channel of depth
//! [`H3_WS_TUNNEL_DEPTH`] carrying chunks of at most [`H3_WS_TUNNEL_CHUNK_MAX`]. On the write side
//! a [`tokio_util::sync::PollSender`] makes `poll_reserve` return `Pending` when the actor stops
//! draining, so the writer PARKS rather than buffers — the property the WS-over-H2 path lacked
//! (CF-S27-2). On the read side a full channel makes the actor's `try_send` fail, so it stops
//! pulling from quiche and QUIC flow control paces the client.
//!
//! **Close vs reset (RFC 9220).** An orderly FIN drops the actor's `to_reader` ⇒ the reader sees
//! channel-closed ⇒ EOF. A stream RESET sends [`TunnelInbound::Reset`] ⇒ the reader surfaces
//! `ConnectionReset`, which tungstenite treats as an abnormal drop — deliberately distinct from a
//! clean WS Close.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::PollSender;

/// Per-direction channel depth; with [`H3_WS_TUNNEL_CHUNK_MAX`] this is the R8 in-flight bound.
pub const H3_WS_TUNNEL_DEPTH: usize = 8;

/// Largest chunk the actor pushes toward the reader; mirrors `h3_bridge::H3_BODY_CHUNK_MAX`.
pub const H3_WS_TUNNEL_CHUNK_MAX: usize = 8 * 1024;

/// Actor → tunnel-reader message: a chunk of stream bytes, or a reset signal.
#[derive(Debug, Clone)]
pub enum TunnelInbound {
    /// A chunk read off the H3 stream (≤ [`H3_WS_TUNNEL_CHUNK_MAX`]).
    Data(Bytes),
    /// The H3 stream was RESET by the peer ⇒ the reader surfaces `ConnectionReset`, never EOF.
    Reset,
}

/// The actor-side endpoints: the actor pushes bytes read off the H3 stream into `to_reader`
/// (dropping it on FIN to signal EOF) and drains `from_writer` onto the stream, FINning on close.
pub struct H3TunnelEndpoints {
    /// Sender into the tunnel's read side. Bounded — a failing `try_send` is the actor's
    /// QUIC-flow-control backpressure signal.
    pub to_reader: mpsc::Sender<TunnelInbound>,
    /// Receiver of bytes the tunnel writer produced, for `stream_send`.
    pub from_writer: mpsc::Receiver<Bytes>,
}

/// The validated extended CONNECT target handed to the relay launcher;
/// [`crate::h3_bridge::validate_request_pseudo_headers`] has already guaranteed the pseudo set.
#[derive(Debug, Clone)]
pub struct WsConnectRequest {
    /// `:authority` of the extended CONNECT (the WS target host).
    pub authority: String,
    /// `:path` of the extended CONNECT — the WS resource path.
    pub path: String,
    /// The client's `sec-websocket-protocol` offer, forwarded upstream.
    pub subprotocols: Option<String>,
}

/// The launcher's readiness verdict, gating the H3 response. The H3 analog of the WS-H1 GHSA fix
/// / WS-H2 F-S27-1: the upstream RFC 6455 handshake completes (or fails) **before** any
/// client-visible `2xx`, so a client is never committed to WS framing toward a backend that never
/// agreed.
#[derive(Debug)]
pub enum WsUpstreamOutcome {
    /// The upstream handshake completed — the actor may send the `200`.
    Ready {
        /// Extra response header fields to emit alongside `:status 200`.
        headers: Vec<(String, String)>,
    },
    /// The upstream dial/handshake failed — the actor returns `status`.
    Failed {
        /// HTTP status the actor returns to the H3 client.
        status: u16,
    },
}

/// What the injected launcher returns: a readiness signal plus the relay task handle. The actor
/// polls `ready` each tick and aborts `task` on teardown, so a torn-down tunnel never leaks it.
pub struct WsRelayHandle {
    /// Upstream-handshake readiness — resolves once, before the `200`.
    pub ready: oneshot::Receiver<WsUpstreamOutcome>,
    /// The relay task (dial + upstream handshake + `proxy_frames`).
    pub task: JoinHandle<()>,
}

/// The dependency-inversion seam. `lb-quic` cannot import `lb_l7::ws_proxy::proxy_frames` (the
/// `lb-l7 → lb-quic` edge would cycle), so the relay is **injected** as this closure from the `lb`
/// binary, mirroring `config_factory` through the same `QuicListenerParams → RouterParams →
/// ActorParams` chain. The closure completes the upstream handshake BEFORE signalling readiness.
pub type WsRelayLauncher = Arc<dyn Fn(H3WsTunnel, WsConnectRequest) -> WsRelayHandle + Send + Sync>;

/// The `proxy_frames`-side handle. Not `Clone` — a tunnel is owned by exactly one relay task.
pub struct H3WsTunnel {
    /// Bounded write path; `PollSender::poll_reserve` makes `poll_write` PARK under backpressure.
    writer: PollSender<Bytes>,
    /// Bounded read path: chunks (or a Reset) pushed by the actor.
    reader: mpsc::Receiver<TunnelInbound>,
    /// Unconsumed tail of the last `Data` chunk; drained before the next recv.
    leftover: Bytes,
    /// Sticky terminal: once EOF or a Reset is observed every later read returns the same.
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
    /// Build a tunnel + its matching actor-side endpoints, both directions bounded.
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
    fn drain_leftover(&mut self, buf: &mut ReadBuf<'_>) -> bool {
        if self.leftover.is_empty() || buf.remaining() == 0 {
            return false;
        }
        let n = self.leftover.len().min(buf.remaining());
        // `split_to` retains the tail in `self.leftover` (cheap — `Bytes` is refcounted) and
        // avoids a panicking slice index.
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
        if self.drain_leftover(buf) {
            return Poll::Ready(Ok(()));
        }
        match self.reader.poll_recv(cx) {
            Poll::Ready(Some(TunnelInbound::Data(bytes))) => {
                self.leftover = bytes;
                // Guard against a zero-length chunk producing a spurious EOF: only return here
                // when bytes were actually placed.
                if self.drain_leftover(buf) {
                    Poll::Ready(Ok(()))
                } else {
                    // Empty Data chunk: re-arm by waking ourselves so the caller polls again
                    // rather than mistaking this for EOF.
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
        // Reserve a slot FIRST — the backpressure point (R8): when the actor is not draining
        // `from_writer`, `poll_reserve` returns Pending and the writer parks.
        match self.writer.poll_reserve(cx) {
            Poll::Ready(Ok(())) => {
                let n = data.len().min(H3_WS_TUNNEL_CHUNK_MAX);
                let chunk = Bytes::copy_from_slice(data.get(..n).unwrap_or(data));
                match self.writer.send_item(chunk) {
                    Ok(()) => Poll::Ready(Ok(n)),
                    Err(_) => {
                        // Receiver (actor) gone — the H3 stream is finished.
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
        // `poll_write` already hands each chunk to the bounded channel; nothing to flush.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Dropping the `PollSender` closes `from_writer`, the actor's cue to FIN the H3 stream.
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

    /// A chunk pushed by the actor reads back byte-identically, including across a small buffer.
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
        // A 2-byte buffer forces multiple poll_read passes through `leftover`.
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

    /// Dropping `to_reader` (orderly stream end) surfaces a clean, sticky EOF.
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

    /// A `Reset` surfaces as `ConnectionReset`, never a clean EOF.
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

    /// Data delivered BEFORE a reset is still read out, and only then does the reset surface.
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

    /// Bytes written through the tunnel arrive on `from_writer`, chunked.
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

    /// R8 — the load-bearing backpressure proof: with the actor NOT draining `from_writer`, a
    /// writer exceeding the channel capacity PARKS. Resuming the drain completes it (liveness).
    #[tokio::test]
    async fn write_parks_under_backpressure_then_resumes() {
        let (mut tunnel, mut ep) = H3WsTunnel::new();
        // Each write_all of one chunk-max occupies exactly one channel slot.
        let chunk = vec![0x5au8; H3_WS_TUNNEL_CHUNK_MAX];
        let writes = H3_WS_TUNNEL_DEPTH + 4;

        let writer = tokio::spawn(async move {
            for _ in 0..writes {
                tunnel.write_all(&chunk).await.unwrap();
            }
            tunnel.shutdown().await.unwrap();
            tunnel
        });

        // Give the writer time to fill the channel and PARK; it must NOT complete here.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !writer.is_finished(),
            "writer must PARK under backpressure (bounded channel, no drain)"
        );

        // Resume draining ⇒ the parked writer makes progress and completes.
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

    /// After `poll_shutdown` the actor's `from_writer` closes — its signal to FIN the H3 stream.
    #[tokio::test]
    async fn shutdown_closes_endpoint_and_blocks_further_writes() {
        let (mut tunnel, mut ep) = H3WsTunnel::new();
        tunnel.write_all(b"last").await.unwrap();
        tunnel.shutdown().await.unwrap();
        assert_eq!(ep.from_writer.recv().await.as_deref(), Some(&b"last"[..]));
        assert!(
            ep.from_writer.recv().await.is_none(),
            "from_writer must close after shutdown so the actor FINs"
        );
        let err = tunnel.write(b"more").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

    /// A write whose peer dropped `from_writer` errors rather than hanging.
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

    /// An empty `Data` chunk does not spuriously signal EOF.
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
