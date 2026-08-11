//! HTTP/2 upstream connection pool for backend `protocol = "h2"`. Caches a hyper `SendRequest`
//! per peer, not a socket, since H2 multiplexes many streams over one connection.
//!
//! NO retry on send failure — the caller surfaces a 502 — so the pool never has to clone or
//! replay a body it does not own. There is also no age-based eviction; dead-peer detection is
//! entirely hyper's keep-alive PING. Upstream TLS is not handled here.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::time::Duration;

use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http2::{Builder, SendRequest};
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use parking_lot::Mutex;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::idle_send::{IdleSendError, idle_bounded_send};
use crate::pool::TcpPool;

/// Request-body type for the H2 upstream pool. The error is a BOXED `std::error::Error`, not
/// `hyper::Error`, because `hyper::Error` has no public constructor — a streaming body could
/// therefore not express a mid-body abort, and a truncated request would reach the backend as
/// COMPLETE instead of resetting the stream.
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

/// Max HPACK header-list size accepted FROM a backend (F-RES-2), matching the client-facing
/// policy so a malicious backend gets no more decode budget than a malicious client.
pub const MAX_HEADER_LIST_SIZE: u32 = 64 * 1024;

/// Configuration for an [`Http2Pool`]; Pingora-aligned defaults.
#[derive(Debug, Clone, Copy)]
pub struct Http2PoolConfig {
    /// Concurrent streams per H2 connection. Applied via hyper's
    /// `max_concurrent_reset_streams`, the closest knob hyper exposes.
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

/// HTTP/2 upstream connection pool; cheap to clone, all clones share one per-peer cache.
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
    /// New pool dialing backends through `tcp_pool`.
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

    /// Forward `request` to `addr`, dialing fresh when the cached connection is missing or dead.
    ///
    /// **ROUND8-L7-10 — H2 cousin of the H1 take-and-discard pattern.** Every `Send`-class error
    /// and every timeout evicts the whole cached `PeerEntry`. The breadth is deliberate: hyper
    /// surfaces all H2 framing faults (PROTOCOL_ERROR, FRAME_SIZE_ERROR, mid-body STREAM_CLOSED,
    /// body-length over/under-read) as `SendRequest` errors, and one corrupted stream on a
    /// multiplexed connection can corrupt every other stream on the same peer. Pingora shipped
    /// this upstream-smuggling bug class in 0.6.0 and again in 0.8.0.
    ///
    /// # Errors
    ///
    /// [`Http2PoolError`]. `Send` and `Timeout` evict the cached entry before returning.
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
                self.evict(addr);
                Err(Http2PoolError::Send(e.to_string()))
            }
            Err(_) => {
                self.evict(addr);
                Err(Http2PoolError::Timeout)
            }
        }
    }

    /// [`Self::send_request`] under [`crate::idle_send::idle_bounded_send`]'s two-phase deadline
    /// instead of a fixed wall-clock, so a slow-but-progressing upload is not 504-truncated.
    ///
    /// The caller OWNS and drives `last_progress` and `upload_complete`; the pool only reads them.
    /// Both [`IdleSendError`] variants collapse onto [`Http2PoolError::Timeout`] to keep the enum
    /// stable — the firing phase survives only in the warn log. ROUND8-L7-10 eviction applies to
    /// both error arms.
    ///
    /// # Errors
    ///
    /// [`Http2PoolError`]. `Send` and `Timeout` evict the cached entry.
    // Over clippy's seven-arg limit, but each argument is load-bearing for the deadline contract
    // and none has a sensible per-pool default.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_request_idle(
        &self,
        addr: SocketAddr,
        request: Request<H2ReqBody>,
        last_progress: Arc<AtomicU64>,
        upload_complete: Arc<AtomicBool>,
        epoch: Instant,
        idle: Duration,
        head_timeout: Duration,
    ) -> Result<Response<Incoming>, Http2PoolError> {
        let mut sender = self.acquire_sender(addr).await?;
        let send_fut = sender.send_request(request);
        match idle_bounded_send(
            send_fut,
            last_progress,
            upload_complete,
            epoch,
            idle,
            head_timeout,
        )
        .await
        {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => {
                self.evict(addr);
                Err(Http2PoolError::Send(e.to_string()))
            }
            Err(idle_err) => {
                // The phase survives only here — the returned variant is the same either way.
                let phase = match idle_err {
                    IdleSendError::IdleTimeout(_) => "idle",
                    IdleSendError::HeadTimeout(_) => "head",
                };
                tracing::warn!(
                    phase, %addr, error = %idle_err,
                    "h2 idle/head deadline fired",
                );
                self.evict(addr);
                Err(Http2PoolError::Timeout)
            }
        }
    }

    /// Sender for `addr`, dialing fresh when the cached entry is missing or dead.
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

    /// Tear down the cached connection to `addr`, resetting every stream on it (F-MD-4).
    ///
    /// Call this when an inbound request was truncated mid-body. Injecting a body error into
    /// hyper's `SendStream` does NOT work here: on a multiplexed connection hyper may END_STREAM
    /// the upstream — presenting the truncated body as COMPLETE — before it ever polls the
    /// injected error. Dropping the `PeerEntry` aborts the driver task, which closes the
    /// connection and deterministically resets the in-flight streams. Connection-scoped teardown
    /// is the deliberate trade: an L7 abort is rare, a smuggled-complete request is not
    /// recoverable.
    pub fn reset_peer(&self, addr: SocketAddr) {
        let _evicted = self.inner.peers.lock().remove(&addr);
    }

    async fn dial_and_handshake(
        &self,
        addr: SocketAddr,
    ) -> Result<(SendRequest<H2ReqBody>, JoinHandle<()>), Http2PoolError> {
        let pooled = self.inner.tcp_pool.acquire_async(addr).await?;
        let stream = pooled
            .take_stream()
            .ok_or_else(|| Http2PoolError::Handshake("pooled stream missing".to_owned()))?;

        let mut builder = Builder::new(TokioExecutor::new());
        builder
            .initial_stream_window_size(self.inner.config.initial_stream_window)
            .max_concurrent_reset_streams(self.inner.config.max_concurrent_streams as usize)
            // Explicit, not left to the h2 crate's undocumented implicit default (F-RES-2).
            .max_header_list_size(MAX_HEADER_LIST_SIZE)
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

/// Collect a response body into one [`Bytes`], bounded against an unbounded upstream response.
///
/// # Errors
///
/// `InvalidData` past `max_body`, or a wrapped hyper body error.
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

    /// Only checks that `send_request_idle` keeps `send_request`'s dial-failure path; deadline
    /// behaviour itself is covered in `idle_send::tests`.
    #[tokio::test]
    async fn send_request_idle_dial_fail_smoke() {
        use http_body_util::Empty;
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

        // Virtually always refused on Linux dev hosts.
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let req: Request<H2ReqBody> = Request::builder()
            .uri("/")
            .body(
                Empty::<Bytes>::new()
                    .map_err(|never: std::convert::Infallible| {
                        let e: Box<dyn std::error::Error + Send + Sync> = Box::new(never);
                        e
                    })
                    .boxed(),
            )
            .unwrap();

        let res = pool
            .send_request_idle(
                addr,
                req,
                Arc::new(AtomicU64::new(0)),
                Arc::new(AtomicBool::new(false)),
                Instant::now(),
                Duration::from_millis(50),
                Duration::from_secs(1),
            )
            .await;

        assert!(
            matches!(res, Err(Http2PoolError::Dial(_))),
            "expected Dial error, got {res:?}",
        );
    }

    /// In-process H2 backend for the arms below. The acceptor is never shut down explicitly —
    /// the `#[tokio::test]` runtime teardown is the only stop signal.
    async fn spawn_h2_backend<F, R>(handler: F) -> SocketAddr
    where
        F: Fn(Request<Incoming>) -> R + Send + Sync + 'static,
        R: std::future::Future<
                Output = Result<Response<http_body_util::Full<Bytes>>, std::io::Error>,
            > + Send
            + 'static,
    {
        use hyper::server::conn::http2;
        use hyper::service::service_fn;
        use std::sync::Arc as StdArc;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = StdArc::new(handler);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let h = handler.clone();
                tokio::spawn(async move {
                    let _ = http2::Builder::new(TokioExecutor::new())
                        .serve_connection(
                            TokioIo::new(stream),
                            service_fn(move |req| {
                                let h = h.clone();
                                async move { h(req).await }
                            }),
                        )
                        .await;
                });
            }
        });
        addr
    }

    fn tcp_pool_for_test() -> TcpPool {
        TcpPool::new(
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
        )
    }

    fn empty_request() -> Request<H2ReqBody> {
        use http_body_util::Empty;
        Request::builder()
            .uri("/")
            .body(
                Empty::<Bytes>::new()
                    .map_err(|never: std::convert::Infallible| {
                        let e: Box<dyn std::error::Error + Send + Sync> = Box::new(never);
                        e
                    })
                    .boxed(),
            )
            .unwrap()
    }

    /// Success arm: acquire_sender → handshake → `idle_bounded_send` → 200.
    #[tokio::test]
    async fn send_request_idle_success_arm() {
        use http_body_util::Full;
        let addr = spawn_h2_backend(|_req| async move {
            Ok::<_, std::io::Error>(
                Response::builder()
                    .status(200)
                    .body(Full::<Bytes>::from("ok"))
                    .unwrap(),
            )
        })
        .await;
        let pool = Http2Pool::new(Http2PoolConfig::default(), tcp_pool_for_test());
        let upload_complete = Arc::new(AtomicBool::new(true)); // empty body — complete immediately.
        let res = pool
            .send_request_idle(
                addr,
                empty_request(),
                Arc::new(AtomicU64::new(0)),
                upload_complete,
                Instant::now(),
                Duration::from_secs(2),
                Duration::from_secs(5),
            )
            .await;
        let resp = res.expect("send_request_idle should succeed");
        assert_eq!(resp.status(), 200);
        assert!(pool.peer_count() >= 1, "pool should cache the peer");
    }

    /// Phase B head timeout must fire AND evict, for ROUND8-L7-10 parity with `send_request`.
    #[tokio::test]
    async fn send_request_idle_head_timeout_arm() {
        let addr = spawn_h2_backend(|_req| async move {
            std::future::pending::<Result<Response<http_body_util::Full<Bytes>>, std::io::Error>>()
                .await
        })
        .await;
        let pool = Http2Pool::new(Http2PoolConfig::default(), tcp_pool_for_test());
        let upload_complete = Arc::new(AtomicBool::new(true));
        let res = pool
            .send_request_idle(
                addr,
                empty_request(),
                Arc::new(AtomicU64::new(0)),
                upload_complete,
                Instant::now(),
                Duration::from_secs(60),
                Duration::from_millis(100),
            )
            .await;
        assert!(
            matches!(res, Err(Http2PoolError::Timeout)),
            "expected Timeout, got {res:?}",
        );
        assert_eq!(
            pool.peer_count(),
            0,
            "pool must evict on timeout (ROUND8-L7-10 parity)",
        );
    }

    /// Phase A idle timeout must collapse onto `Timeout` and evict.
    #[tokio::test]
    async fn send_request_idle_idle_timeout_arm() {
        let addr = spawn_h2_backend(|_req| async move {
            std::future::pending::<Result<Response<http_body_util::Full<Bytes>>, std::io::Error>>()
                .await
        })
        .await;
        let pool = Http2Pool::new(Http2PoolConfig::default(), tcp_pool_for_test());
        let upload_complete = Arc::new(AtomicBool::new(false));
        let res = pool
            .send_request_idle(
                addr,
                empty_request(),
                Arc::new(AtomicU64::new(0)),
                upload_complete,
                Instant::now(),
                Duration::from_millis(100),
                Duration::from_secs(60),
            )
            .await;
        assert!(
            matches!(res, Err(Http2PoolError::Timeout)),
            "expected Timeout (idle collapsed), got {res:?}",
        );
        assert_eq!(pool.peer_count(), 0, "pool must evict on idle timeout");
    }
}
