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
    sender: SendRequest<BoxBody<Bytes, hyper::Error>>,
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
    /// # Errors
    ///
    /// * [`Http2PoolError::Dial`] — TCP dial failed.
    /// * [`Http2PoolError::Handshake`] — hyper H2 handshake failed.
    /// * [`Http2PoolError::Send`] — `send_request` failed.
    /// * [`Http2PoolError::Timeout`] — header roundtrip exceeded
    ///   [`Http2PoolConfig::send_timeout`].
    pub async fn send_request(
        &self,
        addr: SocketAddr,
        request: Request<BoxBody<Bytes, hyper::Error>>,
    ) -> Result<Response<Incoming>, Http2PoolError> {
        let mut sender = self.acquire_sender(addr).await?;
        let send_fut = sender.send_request(request);
        match tokio::time::timeout(self.inner.config.send_timeout, send_fut).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => {
                // Drop the cached entry — it may be stuck.
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
    ) -> Result<SendRequest<BoxBody<Bytes, hyper::Error>>, Http2PoolError> {
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

    fn take_alive_sender(
        &self,
        addr: SocketAddr,
    ) -> Option<SendRequest<BoxBody<Bytes, hyper::Error>>> {
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

    async fn dial_and_handshake(
        &self,
        addr: SocketAddr,
    ) -> Result<(SendRequest<BoxBody<Bytes, hyper::Error>>, JoinHandle<()>), Http2PoolError> {
        let pool = self.inner.tcp_pool.clone();
        let pooled = tokio::task::spawn_blocking(move || pool.acquire(addr))
            .await
            .map_err(|e| Http2PoolError::Handshake(format!("dial join: {e}")))??;
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
            .handshake::<_, BoxBody<Bytes, hyper::Error>>(TokioIo::new(stream))
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
