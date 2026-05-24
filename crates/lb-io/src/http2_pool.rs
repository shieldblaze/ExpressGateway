//! HTTP/2 upstream connection pool for backend `protocol = "h2"`.
//!
//! Mirrors the shape of [`crate::pool::TcpPool`] and
//! [`crate::quic_pool::QuicUpstreamPool`]: a per-peer cache keyed by
//! `SocketAddr`, but the cached entity is a hyper 1.x H2
//! [`hyper::client::conn::http2::SendRequest`] handle rather than a raw
//! socket. H2 multiplexes streams over a single TCP connection, so the
//! pool's job is to amortise the TCP+H2 handshake across many requests
//! and to redial transparently when the underlying connection dies.
//!
//! ## Lifecycle
//!
//! [`Http2Pool::send_request`] looks up the cached `SendRequest` for
//! `addr`. If present and `is_ready()`, the request is forwarded as a
//! new H2 stream. If absent or closed, the pool dials a fresh TCP
//! connection (via the supplied [`crate::pool::TcpPool`]) and runs
//! [`hyper::client::conn::http2::handshake`].
//!
//! No second-attempt retry is performed: if `send_request` returns an
//! error the caller surfaces a 502. This keeps the request-body
//! lifecycle simple — the pool never has to clone or replay a body it
//! does not own.
//!
//! ## Bounds & defaults
//!
//! Per-listener caps (PROTO-001):
//! * `max_concurrent_streams = 256`.
//! * `initial_stream_window = 65_535` — RFC 7540 default.
//! * `keep_alive_interval = 30 s` — H2 PING liveness probe cadence.
//! * `keep_alive_timeout = 10 s` — H2 PING-ACK deadline.
//!
//! These are exposed on [`Http2PoolConfig`]; defaults match Pingora.
//!
//! ## What this module does NOT do
//!
//! * TLS termination on the upstream side. PROTO-001's e2e tests run
//!   plaintext H2 backends; production H2 backends behind TLS will need
//!   ALPN-aware dial machinery, which is OUT-OF-SCOPE for v1.
//! * Connection-eviction on age — we trust hyper's keep-alive PING to
//!   detect dead peers.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http2::{Builder, SendRequest};
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use parking_lot::Mutex;
use tokio::task::JoinHandle;

use crate::pool::TcpPool;

/// S6 / H3→H2 R8 (I0.5, lead-approved Option A): the request-body type
/// the H2 upstream pool accepts.
///
/// Widened from `BoxBody<Bytes, hyper::Error>` to a boxed
/// `std::error::Error` so a *streaming, bounded-incremental* request
/// body can signal a mid-body abort (H3 client RESET / premature
/// channel close) with a CONSTRUCTIBLE error — `hyper::Error` has no
/// public constructor, so an erroring streaming request body could not
/// be expressed against the old alias (request-smuggling parity: a
/// truncated request must RST_STREAM the upstream, never be presented
/// as complete). This is a **type-only widening with NO behavioural
/// change**: hyper's `SendRequest` already accepts any body whose
/// `Error: Into<Box<dyn Error + Send + Sync>>`, and `hyper::Error`
/// itself satisfies that bound, so every pre-existing caller adapts
/// with a single `.map_err(Into::into)` (or unchanged when its body is
/// `Infallible`/already-boxed).
pub type H2ReqBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

/// Default H2 max concurrent streams per upstream connection.
pub const DEFAULT_H2_MAX_CONCURRENT_STREAMS: u32 = 256;
/// Default H2 initial stream window (RFC 7540 §6.5.2 initial value).
pub const DEFAULT_H2_INITIAL_STREAM_WINDOW: u32 = 65_535;
/// Default H2 keep-alive interval.
pub const DEFAULT_H2_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
/// Default H2 keep-alive timeout.
pub const DEFAULT_H2_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(10);
/// Default upstream H2 send timeout.
pub const DEFAULT_H2_SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration for an [`Http2Pool`]. Defaults match the values
/// documented on the module — Pingora-aligned.
#[derive(Debug, Clone, Copy)]
pub struct Http2PoolConfig {
    /// Maximum concurrent streams the pool will open on a single H2
    /// connection. Surfaced as the hyper builder's
    /// `max_concurrent_reset_streams` (the closest knob hyper exposes).
    pub max_concurrent_streams: u32,
    /// Initial stream window in bytes.
    pub initial_stream_window: u32,
    /// PING keep-alive interval; `Duration::ZERO` disables.
    pub keep_alive_interval: Duration,
    /// PING-ACK timeout.
    pub keep_alive_timeout: Duration,
    /// Header-roundtrip timeout per `send_request`.
    pub send_timeout: Duration,
}

impl Default for Http2PoolConfig {
    fn default() -> Self {
        Self {
            max_concurrent_streams: DEFAULT_H2_MAX_CONCURRENT_STREAMS,
            initial_stream_window: DEFAULT_H2_INITIAL_STREAM_WINDOW,
            keep_alive_interval: DEFAULT_H2_KEEP_ALIVE_INTERVAL,
            keep_alive_timeout: DEFAULT_H2_KEEP_ALIVE_TIMEOUT,
            send_timeout: DEFAULT_H2_SEND_TIMEOUT,
        }
    }
}

/// Per-peer cached entry: a `SendRequest` handle plus the driver task.
struct PeerEntry {
    sender: SendRequest<H2ReqBody>,
    driver: JoinHandle<()>,
}

impl PeerEntry {
    fn is_alive(&self) -> bool {
        !self.sender.is_closed() && !self.driver.is_finished()
    }
}

impl Drop for PeerEntry {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

/// Errors surfaced by [`Http2Pool::send_request`].
#[derive(Debug, thiserror::Error)]
pub enum Http2PoolError {
    /// Upstream TCP dial failed.
    #[error("upstream dial failed: {0}")]
    Dial(#[from] io::Error),
    /// hyper H2 handshake failed against the dialed peer.
    #[error("h2 handshake failed: {0}")]
    Handshake(String),
    /// `send_request` returned a hyper-level error.
    #[error("h2 send_request failed: {0}")]
    Send(String),
    /// Header roundtrip exceeded the configured timeout.
    #[error("h2 send_request timed out")]
    Timeout,
}

/// HTTP/2 upstream connection pool.
///
/// Cheap to clone via [`Arc`]; every clone shares the same per-peer
/// cache.
#[derive(Clone)]
pub struct Http2Pool {
    inner: Arc<Http2PoolInner>,
}

struct Http2PoolInner {
    config: Http2PoolConfig,
    tcp_pool: TcpPool,
    peers: Mutex<HashMap<SocketAddr, PeerEntry>>,
}

impl std::fmt::Debug for Http2Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.inner.peers.lock().len();
        f.debug_struct("Http2Pool")
            .field("config", &self.inner.config)
            .field("peers", &count)
            .finish()
    }
}

impl Http2Pool {
    /// Build a new pool that dials backends through the supplied
    /// [`TcpPool`].
    #[must_use]
    pub fn new(config: Http2PoolConfig, tcp_pool: TcpPool) -> Self {
        Self {
            inner: Arc::new(Http2PoolInner {
                config,
                tcp_pool,
                peers: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Number of peers with an open H2 connection in the cache.
    #[must_use]
    pub fn peer_count(&self) -> usize {
        self.inner.peers.lock().len()
    }

    /// Forward `request` to `addr` over a multiplexed H2 connection.
    ///
    /// On a missing or dead cached connection the pool dials fresh and
    /// stores the new sender for subsequent calls.
    ///
    /// **ROUND8-L7-10 — H2 cousin of the H1 take-and-discard pattern.**
    /// This method evicts the cached `PeerEntry` for `addr` on every
    /// `Send`-class hyper error (`Ok(Err(e))` branch below) and on
    /// every header-roundtrip timeout (`Err(_)` branch). That covers
    /// the full set of H2 stream framing errors a misbehaving upstream
    /// can emit — `PROTOCOL_ERROR`, `FRAME_SIZE_ERROR`, mid-body
    /// `STREAM_CLOSED`, body-length over-read / under-read — because
    /// hyper surfaces all of them as `SendRequest` errors. Eviction
    /// here is **deliberately broad**: the H2 reuse path multiplexes
    /// many streams on one connection, so a single corrupted stream
    /// could otherwise corrupt every concurrent stream on the same
    /// peer. See Pingora 0.6.0 / 0.8.0 CHANGELOG for the upstream-
    /// smuggling bug class this guards against.
    ///
    /// # Errors
    ///
    /// * [`Http2PoolError::Dial`] — TCP dial failed.
    /// * [`Http2PoolError::Handshake`] — hyper H2 handshake failed.
    /// * [`Http2PoolError::Send`] — `send_request` failed; cached
    ///   entry for this address is evicted before returning.
    /// * [`Http2PoolError::Timeout`] — header roundtrip exceeded
    ///   [`Http2PoolConfig::send_timeout`]; cached entry is evicted.
    pub async fn send_request(
        &self,
        addr: SocketAddr,
        request: Request<H2ReqBody>,
    ) -> Result<Response<Incoming>, Http2PoolError> {
        let mut sender = self.acquire_sender(addr).await?;
        let send_fut = sender.send_request(request);
        match tokio::time::timeout(self.inner.config.send_timeout, send_fut).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => {
                // ROUND8-L7-10: evict on every Send-class error so a
                // single stream-framing fault (PROTOCOL_ERROR, FRAME_
                // SIZE_ERROR, mid-body STREAM_CLOSED, body-length
                // mismatch) cannot corrupt subsequent multiplexed
                // streams on the same upstream peer.
                self.evict(addr);
                Err(Http2PoolError::Send(e.to_string()))
            }
            Err(_) => {
                self.evict(addr);
                Err(Http2PoolError::Timeout)
            }
        }
    }

    /// Get a sender for `addr`, dialing fresh when the cached entry is
    /// missing or dead.
    async fn acquire_sender(
        &self,
        addr: SocketAddr,
    ) -> Result<SendRequest<H2ReqBody>, Http2PoolError> {
        if let Some(sender) = self.take_alive_sender(addr) {
            return Ok(sender);
        }
        let (sender, driver) = self.dial_and_handshake(addr).await?;
        let entry = PeerEntry {
            sender: sender.clone(),
            driver,
        };
        self.replace_entry(addr, entry);
        Ok(sender)
    }

    fn take_alive_sender(&self, addr: SocketAddr) -> Option<SendRequest<H2ReqBody>> {
        let mut peers = self.inner.peers.lock();
        match peers.get(&addr) {
            Some(entry) if entry.is_alive() => Some(entry.sender.clone()),
            Some(_) => {
                peers.remove(&addr);
                None
            }
            None => None,
        }
    }

    fn replace_entry(&self, addr: SocketAddr, entry: PeerEntry) {
        let mut peers = self.inner.peers.lock();
        peers.insert(addr, entry);
    }

    fn evict(&self, addr: SocketAddr) {
        let mut peers = self.inner.peers.lock();
        peers.remove(&addr);
    }

    /// F-MD-4 (S10 H2→H2 request-smuggling fix) — forcibly tear down the
    /// cached H2 connection to `addr`, RESETTING every stream currently
    /// multiplexed on it.
    ///
    /// This is the H2 analog of the H1 path's `conn_handle.abort()`
    /// backstop. When an L7 streaming-request pump determines that the
    /// inbound request was truncated mid-body (client RST_STREAM without
    /// END_STREAM, over-cap, or a forbidden trailer), it MUST guarantee
    /// the upstream stream is reset so the backend never observes the
    /// truncated request as COMPLETE. Injecting a body error into hyper's
    /// `SendStream` is racy on a multiplexed connection: hyper may
    /// gracefully finalize (END_STREAM) the upstream stream — emitting the
    /// truncated body as complete — before it polls the injected error,
    /// especially when the downstream cancellation drops the request body
    /// at a frame boundary. Dropping the cached `PeerEntry` aborts its
    /// driver task ([`PeerEntry::drop`] → `driver.abort()`), which closes
    /// the underlying connection and DETERMINISTICALLY resets the
    /// in-flight upstream stream(s).
    ///
    /// Like [`Self::send_request`]'s ROUND8-L7-10 eviction this is
    /// deliberately connection-scoped: an L7 abort is a rare,
    /// security-relevant event, and resetting the peer connection (the
    /// same broad teardown the error-eviction path already performs) is
    /// the safe choice over risking a smuggled-complete request.
    pub fn reset_peer(&self, addr: SocketAddr) {
        // Dropping the removed `PeerEntry` runs its `Drop` impl which
        // `driver.abort()`s the connection task → connection close → all
        // streams on it reset.
        let _evicted = self.inner.peers.lock().remove(&addr);
    }

    async fn dial_and_handshake(
        &self,
        addr: SocketAddr,
    ) -> Result<(SendRequest<H2ReqBody>, JoinHandle<()>), Http2PoolError> {
        // CODE-2-09 follow-on: async dial via `TcpPool::acquire_async`,
        // eliminating the previous `spawn_blocking(pool.acquire)` site
        // that shared the global blocking pool with `dns::resolve`.
        let pooled = self.inner.tcp_pool.acquire_async(addr).await?;
        let stream = pooled
            .take_stream()
            .ok_or_else(|| Http2PoolError::Handshake("pooled stream missing".to_owned()))?;

        let mut builder = Builder::new(TokioExecutor::new());
        builder
            .initial_stream_window_size(self.inner.config.initial_stream_window)
            .max_concurrent_reset_streams(self.inner.config.max_concurrent_streams as usize)
            .timer(TokioTimer::new());
        if !self.inner.config.keep_alive_interval.is_zero() {
            builder
                .keep_alive_interval(self.inner.config.keep_alive_interval)
                .keep_alive_timeout(self.inner.config.keep_alive_timeout)
                .keep_alive_while_idle(true);
        }
        let (sender, conn) = builder
            .handshake::<_, H2ReqBody>(TokioIo::new(stream))
            .await
            .map_err(|e| Http2PoolError::Handshake(e.to_string()))?;

        let driver = tokio::spawn(async move {
            let _ = conn.await;
        });
        Ok((sender, driver))
    }
}

/// Convenience helper: collect the body of a [`Response<Incoming>`] into
/// a single [`Bytes`]. Bound by `max_body` to defend against unbounded
/// upstream responses.
///
/// # Errors
///
/// Returns an [`io::Error`] of kind `InvalidData` if the body exceeds
/// `max_body` bytes, or wraps any underlying hyper body error.
pub async fn collect_body_bounded(body: Incoming, max_body: usize) -> io::Result<Bytes> {
    let collected = body
        .collect()
        .await
        .map_err(|e| io::Error::other(format!("body collect: {e}")))?;
    let bytes = collected.to_bytes();
    if bytes.len() > max_body {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("body too large: {} > {}", bytes.len(), max_body),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_documented_values() {
        let cfg = Http2PoolConfig::default();
        assert_eq!(cfg.max_concurrent_streams, 256);
        assert_eq!(cfg.initial_stream_window, 65_535);
        assert_eq!(cfg.keep_alive_interval, Duration::from_secs(30));
        assert_eq!(cfg.keep_alive_timeout, Duration::from_secs(10));
        assert_eq!(cfg.send_timeout, Duration::from_secs(30));
    }

    #[test]
    fn pool_starts_empty() {
        let tcp_pool = TcpPool::new(
            crate::pool::PoolConfig::default(),
            crate::sockopts::BackendSockOpts {
                nodelay: true,
                keepalive: true,
                rcvbuf: None,
                sndbuf: None,
                quickack: false,
                tcp_fastopen_connect: false,
            },
            crate::Runtime::with_backend(crate::IoBackend::Epoll),
        );
        let pool = Http2Pool::new(Http2PoolConfig::default(), tcp_pool);
        assert_eq!(pool.peer_count(), 0);
    }
}
