//! TCP connection pool with per-peer LRU and Pingora-style liveness probe.
//!
//! The pool stores idle `std::net::TcpStream` handles in blocking mode,
//! keyed by the remote [`SocketAddr`]. Converting back to a
//! [`tokio::net::TcpStream`] happens on acquire via `TcpStream::from_std`,
//! matching the pattern already used in `crates/lb/src/main.rs` for direct
//! dials. Idle connections live in `std` form because a tokio stream
//! cannot cleanly unregister itself from the reactor and park as a
//! blocking-idle handle.
//!
//! Bounds:
//!
//! * [`PoolConfig::per_peer_max`] — maximum idle connections per peer.
//! * [`PoolConfig::total_max`]    — maximum idle connections across all
//!   peers.
//!
//! Both are enforced on insertion (Drop of [`PooledTcp`]); the oldest
//! entry is evicted when the per-peer cap is reached.
//!
//! Lifetime knobs:
//!
//! * [`PoolConfig::idle_timeout`] — connections idle longer than this are
//!   discarded at acquire time.
//! * [`PoolConfig::max_age`]      — connections older than this (since
//!   dial) are discarded at acquire time.
//!
//! Liveness probe (Pingora EC-01): before handing a pooled socket back to
//! a caller, the pool switches it to non-blocking mode and attempts a
//! one-byte read. `WouldBlock` is healthy (the peer has nothing to say),
//! `Ok(0)` means the peer half-closed, any other error means the socket
//! is unusable. Healthy sockets are kept in non-blocking mode and handed
//! directly to [`tokio::net::TcpStream::from_std`].
//!
//! Reaping is acquire-driven: no background task is spawned. This keeps
//! the pool usable from non-tokio contexts (unit tests, benchmarks) and
//! matches the simpler of the two options called out in the design
//! guidance. A scheduled background sweeper is a straightforward
//! follow-up and is deliberately out of scope for Pillar 2.

use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, TcpStream as StdTcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::net::TcpStream;

use crate::Runtime;
use crate::sockopts::BackendSockOpts;

/// Default upper bound on idle connections per peer.
pub const DEFAULT_PER_PEER_MAX: usize = 8;
/// Default upper bound on idle connections pool-wide.
pub const DEFAULT_TOTAL_MAX: usize = 256;
/// Default idle timeout for a pooled connection (seconds).
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 60;
/// Default maximum age of a pooled connection since dial (seconds).
pub const DEFAULT_MAX_AGE_SECS: u64 = 5 * 60;

/// Configuration for [`TcpPool`]. Defaults match PROMPT.md §21 TCP L4.
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    /// Maximum idle connections cached per peer.
    pub per_peer_max: usize,
    /// Maximum idle connections cached across all peers.
    pub total_max: usize,
    /// Idle connections older than this at acquire time are discarded.
    pub idle_timeout: Duration,
    /// Connections older than this (since original dial) are discarded.
    pub max_age: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            per_peer_max: DEFAULT_PER_PEER_MAX,
            total_max: DEFAULT_TOTAL_MAX,
            idle_timeout: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
            max_age: Duration::from_secs(DEFAULT_MAX_AGE_SECS),
        }
    }
}

/// An idle connection parked in the pool.
struct IdleConn {
    stream: StdTcpStream,
    created_at: Instant,
    last_used: Instant,
}

/// Interior mutable state shared between every [`TcpPool`] clone.
struct TcpPoolInner {
    config: PoolConfig,
    connect_opts: BackendSockOpts,
    per_peer: DashMap<SocketAddr, Mutex<VecDeque<IdleConn>>>,
    total: AtomicUsize,
    runtime: Runtime,
}

/// Pool handle. Cheap to clone; shares state with every other clone.
#[derive(Clone)]
pub struct TcpPool {
    inner: Arc<TcpPoolInner>,
}

impl std::fmt::Debug for TcpPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpPool")
            .field("config", &self.inner.config)
            .field("total_idle", &self.inner.total.load(Ordering::Relaxed))
            .field("peers", &self.inner.per_peer.len())
            .finish()
    }
}

impl TcpPool {
    /// Construct a new pool with the supplied configuration and backend
    /// socket options. Fresh dials inherit `connect_opts` via
    /// [`Runtime::connect`].
    #[must_use]
    pub fn new(config: PoolConfig, connect_opts: BackendSockOpts, runtime: Runtime) -> Self {
        Self {
            inner: Arc::new(TcpPoolInner {
                config,
                connect_opts,
                per_peer: DashMap::new(),
                total: AtomicUsize::new(0),
                runtime,
            }),
        }
    }

    /// Number of idle connections currently parked across every peer.
    ///
    /// Primarily useful for tests and metrics.
    #[must_use]
    pub fn idle_count(&self) -> usize {
        self.inner.total.load(Ordering::Relaxed)
    }

    /// Idle connections parked for `addr`, if any.
    #[must_use]
    pub fn idle_count_for(&self, addr: SocketAddr) -> usize {
        self.inner.per_peer.get(&addr).map_or(0, |q| q.lock().len())
    }

    /// Acquire a connection to `addr`, reusing a pooled idle entry when
    /// possible.
    ///
    /// The returned [`PooledTcp`] wraps a live
    /// [`tokio::net::TcpStream`]. On drop the wrapper returns the socket
    /// to the pool unless the caller marked it non-reusable via
    /// [`PooledTcp::set_reusable`].
    ///
    /// Blocking `connect(2)` for a fresh dial runs inline on the calling
    /// task, mirroring `Runtime::connect`. Callers that do not want to
    /// stall a tokio worker should wrap this call in
    /// `tokio::task::spawn_blocking` exactly as they do today for the
    /// direct-dial path.
    ///
    /// # Errors
    /// Propagates any `io::Error` from the fallback `connect(2)` or from
    /// `set_nonblocking` / `TcpStream::from_std`.
    pub fn acquire(&self, addr: SocketAddr) -> io::Result<PooledTcp> {
        while let Some(idle) = self.pop_idle(addr) {
            match self.validate_and_upgrade(idle, addr) {
                Ok(pooled) => return Ok(pooled),
                Err(ValidationOutcome::Discard) => continue,
                Err(ValidationOutcome::Fatal(err)) => return Err(err),
            }
        }
        self.dial_new(addr)
    }

    /// Pop the oldest idle entry (FIFO), decrementing the total counter.
    fn pop_idle(&self, addr: SocketAddr) -> Option<IdleConn> {
        let idle = {
            let entry = self.inner.per_peer.get(&addr)?;
            entry.lock().pop_front()
        };
        if idle.is_some() {
            self.inner.total.fetch_sub(1, Ordering::Relaxed);
        }
        idle
    }

    /// Check age / idle timeout / liveness; on success return the wrapped
    /// tokio stream. On transient failure the caller should try the next
    /// idle entry; on fatal failure the caller must surface the error.
    fn validate_and_upgrade(
        &self,
        idle: IdleConn,
        addr: SocketAddr,
    ) -> Result<PooledTcp, ValidationOutcome> {
        let now = Instant::now();
        if now.duration_since(idle.created_at) > self.inner.config.max_age {
            return Err(ValidationOutcome::Discard);
        }
        if now.duration_since(idle.last_used) > self.inner.config.idle_timeout {
            return Err(ValidationOutcome::Discard);
        }
        let stream = idle.stream;
        if !probe_alive(&stream) {
            return Err(ValidationOutcome::Discard);
        }
        // probe_alive leaves the socket in non-blocking mode — exactly
        // what tokio::net::TcpStream::from_std requires.
        match TcpStream::from_std(stream) {
            Ok(tokio_stream) => Ok(PooledTcp::new(
                tokio_stream,
                addr,
                idle.created_at,
                self.inner.clone(),
            )),
            Err(err) => Err(ValidationOutcome::Fatal(err)),
        }
    }

    /// Fresh dial + setsockopt via [`Runtime::connect`].
    fn dial_new(&self, addr: SocketAddr) -> io::Result<PooledTcp> {
        let stream = self.inner.runtime.connect(addr, &self.inner.connect_opts)?;
        let created_at = Instant::now();
        stream.set_nonblocking(true)?;
        let tokio_stream = TcpStream::from_std(stream)?;
        Ok(PooledTcp::new(
            tokio_stream,
            addr,
            created_at,
            self.inner.clone(),
        ))
    }
}

/// Reason a pooled connection was rejected by [`TcpPool::validate_and_upgrade`].
enum ValidationOutcome {
    /// Discard this entry and try the next one (or dial fresh).
    Discard,
    /// Pool operation failed in a way that should surface to the caller.
    Fatal(io::Error),
}

/// A checked-out connection. Drops back into the pool unless marked
/// non-reusable.
///
/// Internally the tokio [`TcpStream`] is stored as `Option<TcpStream>`
/// so [`Drop`] can steal it via [`Option::take`] before handing the
/// underlying `std::net::TcpStream` back to the pool. All accessor
/// methods maintain the invariant `stream.is_some()` up until either
/// `take_stream` or `drop` runs exactly once.
pub struct PooledTcp {
    stream: Option<TcpStream>,
    addr: SocketAddr,
    created_at: Instant,
    reusable: bool,
    pool: Option<Arc<TcpPoolInner>>,
}

impl std::fmt::Debug for PooledTcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledTcp")
            .field("addr", &self.addr)
            .field("created_at", &self.created_at)
            .field("reusable", &self.reusable)
            .field("stream_taken", &self.stream.is_none())
            .field("pool_attached", &self.pool.is_some())
            .finish()
    }
}

impl PooledTcp {
    const fn new(
        stream: TcpStream,
        addr: SocketAddr,
        created_at: Instant,
        pool: Arc<TcpPoolInner>,
    ) -> Self {
        Self {
            stream: Some(stream),
            addr,
            created_at,
            reusable: true,
            pool: Some(pool),
        }
    }

    /// Mutable access to the underlying tokio stream.
    ///
    /// Returns [`None`] only after [`PooledTcp::take_stream`] has been
    /// called, which consumes the wrapper; in practice callers never
    /// observe [`None`] because `take_stream` takes `self` by value.
    pub fn stream_mut(&mut self) -> Option<&mut TcpStream> {
        self.stream.as_mut()
    }

    /// Immutable access to the underlying tokio stream.
    #[must_use]
    pub const fn stream(&self) -> Option<&TcpStream> {
        self.stream.as_ref()
    }

    /// Remote address this connection is bound to.
    #[must_use]
    pub const fn peer_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Duration since the underlying socket was dialed.
    #[must_use]
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Mark this connection as reusable (the default) or not.
    ///
    /// Callers that observed an I/O error on the stream should call
    /// `set_reusable(false)` before dropping so the pool does not park a
    /// broken socket.
    pub const fn set_reusable(&mut self, reusable: bool) {
        self.reusable = reusable;
    }

    /// Current reusable flag.
    #[must_use]
    pub const fn is_reusable(&self) -> bool {
        self.reusable
    }

    /// Detach the stream from the pool. After this the pool will not try
    /// to recycle it.
    ///
    /// Returns [`None`] only in the theoretical case where the wrapper
    /// was already emptied, which is unreachable under normal use.
    pub fn take_stream(mut self) -> Option<TcpStream> {
        // Preventing Drop from re-entering the pool.
        self.pool = None;
        self.stream.take()
    }

    fn return_to_pool(&mut self) {
        let Some(pool) = self.pool.take() else {
            return;
        };
        let Some(stream) = self.stream.take() else {
            return;
        };
        if !self.reusable {
            return;
        }

        let now = Instant::now();
        if now.duration_since(self.created_at) > pool.config.max_age {
            return;
        }

        // Convert tokio -> std so the idle socket is not registered with
        // the reactor while it sleeps.
        let std_stream = match stream.into_std() {
            Ok(s) => s,
            Err(err) => {
                tracing::debug!(addr = %self.addr, ?err, "into_std failed; discarding conn");
                return;
            }
        };
        if let Err(err) = std_stream.set_nonblocking(false) {
            tracing::debug!(addr = %self.addr, ?err, "set_nonblocking(false) failed; discarding conn");
            return;
        }

        if pool.total.load(Ordering::Relaxed) >= pool.config.total_max {
            return;
        }

        let idle = IdleConn {
            stream: std_stream,
            created_at: self.created_at,
            last_used: now,
        };

        let queue_ref = pool
            .per_peer
            .entry(self.addr)
            .or_insert_with(|| Mutex::new(VecDeque::new()));
        let mut queue = queue_ref.lock();
        while queue.len() >= pool.config.per_peer_max {
            if queue.pop_front().is_some() {
                pool.total.fetch_sub(1, Ordering::Relaxed);
            } else {
                break;
            }
        }
        queue.push_back(idle);
        pool.total.fetch_add(1, Ordering::Relaxed);
        drop(queue);
        drop(queue_ref);
    }
}

impl Drop for PooledTcp {
    fn drop(&mut self) {
        self.return_to_pool();
    }
}

/// Non-blocking read-zero liveness probe.
///
/// Switches `stream` to non-blocking mode and attempts a one-byte read.
/// Returns `true` when the kernel reports `WouldBlock`, meaning the peer
/// has not half-closed and has no data queued. Returns `false` on
/// `Ok(0)` (peer closed), `Ok(n)` (unexpected bytes pending — protocol
/// desync, drop to be safe), or any other error.
///
/// On success the stream is left in non-blocking mode so
/// [`tokio::net::TcpStream::from_std`] can adopt it directly.
fn probe_alive(stream: &StdTcpStream) -> bool {
    use std::io::Read;

    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let mut buf = [0u8; 1];
    // `read` on a non-blocking socket returns WouldBlock if the peer has
    // sent no data and has not closed. Any other result is bad news.
    // `impl Read for &TcpStream` lets us read through a shared reference;
    // we take `&mut &TcpStream` to satisfy the trait's `&mut self`.
    let mut reader: &StdTcpStream = stream;
    matches!(reader.read(&mut buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;
    use std::thread;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::IoBackend;

    fn echo_listener() -> (TcpListener, SocketAddr, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let listener_clone = listener.try_clone().unwrap();
        thread::spawn(move || {
            listener_clone.set_nonblocking(false).unwrap();
            for stream in listener_clone.incoming() {
                if stop2.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(mut s) = stream else { return };
                thread::spawn(move || {
                    let mut buf = [0u8; 1024];
                    loop {
                        match s.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => {
                                if s.write_all(&buf[..n]).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                });
            }
        });
        (listener, addr, stop)
    }

    fn shutdown_first_idle(pool: &TcpPool, addr: SocketAddr) {
        let entry = pool.inner.per_peer.get(&addr).unwrap();
        let guard = entry.lock();
        let idle = guard.front().unwrap();
        idle.stream.shutdown(std::net::Shutdown::Both).unwrap();
        drop(guard);
        drop(entry);
    }

    fn pool_with(cfg: PoolConfig) -> TcpPool {
        TcpPool::new(
            cfg,
            BackendSockOpts {
                nodelay: true,
                keepalive: true,
                ..Default::default()
            },
            Runtime::with_backend(IoBackend::Epoll),
        )
    }

    #[test]
    fn defaults_match_prompt_section_21() {
        let cfg = PoolConfig::default();
        assert_eq!(cfg.per_peer_max, 8);
        assert_eq!(cfg.total_max, 256);
        assert_eq!(cfg.idle_timeout, Duration::from_secs(60));
        assert_eq!(cfg.max_age, Duration::from_secs(300));
    }

    #[tokio::test]
    async fn acquire_then_release_returns_same_socket() {
        let (_l, addr, _stop) = echo_listener();
        let pool = pool_with(PoolConfig::default());

        // Dial, exchange bytes, release.
        let local_first;
        {
            let mut c = pool.acquire(addr).unwrap();
            let s = c.stream_mut().unwrap();
            local_first = s.local_addr().unwrap();
            s.write_all(b"hi").await.unwrap();
            let mut buf = [0u8; 2];
            s.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"hi");
        }
        assert_eq!(pool.idle_count(), 1);
        assert_eq!(pool.idle_count_for(addr), 1);

        // Reuse: local port should match.
        let mut c2 = pool.acquire(addr).unwrap();
        let local_second = c2.stream_mut().unwrap().local_addr().unwrap();
        assert_eq!(local_first, local_second);
        assert_eq!(pool.idle_count(), 0);
    }

    #[tokio::test]
    async fn probe_discards_peer_closed_connection() {
        let (_l, addr, _stop) = echo_listener();
        let pool = pool_with(PoolConfig::default());

        // Dial and release.
        {
            let mut c = pool.acquire(addr).unwrap();
            let s = c.stream_mut().unwrap();
            s.write_all(b"x").await.unwrap();
            let mut buf = [0u8; 1];
            s.read_exact(&mut buf).await.unwrap();
        }
        assert_eq!(pool.idle_count(), 1);

        // Reach into the idle queue and shut down the stream to simulate
        // the peer half-closing while we were idle.
        shutdown_first_idle(&pool, addr);

        // Acquire: probe should fire, drop the stale entry, dial fresh.
        let mut c2 = pool.acquire(addr).unwrap();
        let s = c2.stream_mut().unwrap();
        s.write_all(b"y").await.unwrap();
        let mut buf = [0u8; 1];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"y");
        assert_eq!(pool.idle_count(), 0);
        drop(c2);
        assert_eq!(pool.idle_count(), 1);
    }

    #[tokio::test]
    async fn per_peer_max_enforced() {
        let (_l, addr, _stop) = echo_listener();
        let cfg = PoolConfig {
            per_peer_max: 2,
            total_max: 16,
            ..PoolConfig::default()
        };
        let pool = pool_with(cfg);

        // Open 4 concurrently, then drop them all — only 2 should remain.
        let c1 = pool.acquire(addr).unwrap();
        let c2 = pool.acquire(addr).unwrap();
        let c3 = pool.acquire(addr).unwrap();
        let c4 = pool.acquire(addr).unwrap();
        drop(c1);
        drop(c2);
        drop(c3);
        drop(c4);
        assert_eq!(pool.idle_count_for(addr), 2);
        assert!(pool.idle_count() <= 2);
    }

    #[tokio::test]
    async fn total_max_enforced() {
        // Two peers, total_max=3, per_peer_max=8. After releasing 5 we
        // must never exceed 3 idle in the pool.
        let (_l1, addr1, _s1) = echo_listener();
        let (_l2, addr2, _s2) = echo_listener();
        let cfg = PoolConfig {
            per_peer_max: 8,
            total_max: 3,
            ..PoolConfig::default()
        };
        let pool = pool_with(cfg);

        let a1 = pool.acquire(addr1).unwrap();
        let a2 = pool.acquire(addr1).unwrap();
        let a3 = pool.acquire(addr2).unwrap();
        let a4 = pool.acquire(addr2).unwrap();
        let a5 = pool.acquire(addr1).unwrap();

        drop(a1);
        drop(a2);
        drop(a3);
        drop(a4);
        drop(a5);

        assert!(
            pool.idle_count() <= 3,
            "pool idle={} exceeds total_max=3",
            pool.idle_count()
        );
    }

    #[tokio::test]
    async fn max_age_expiry_discards_on_acquire() {
        let (_l, addr, _stop) = echo_listener();
        let cfg = PoolConfig {
            per_peer_max: 4,
            total_max: 16,
            idle_timeout: Duration::from_secs(60),
            // aggressively short
            max_age: Duration::from_millis(50),
        };
        let pool = pool_with(cfg);

        {
            let _c = pool.acquire(addr).unwrap();
        }
        assert_eq!(pool.idle_count(), 1);

        // Sleep past max_age.
        tokio::time::sleep(Duration::from_millis(120)).await;

        let _c2 = pool.acquire(addr).unwrap();
        // The expired entry was discarded on acquire; a fresh one dialed.
        assert_eq!(pool.idle_count(), 0);
    }

    #[tokio::test]
    async fn idle_timeout_discards_on_acquire() {
        let (_l, addr, _stop) = echo_listener();
        let cfg = PoolConfig {
            per_peer_max: 4,
            total_max: 16,
            idle_timeout: Duration::from_millis(30),
            max_age: Duration::from_secs(60),
        };
        let pool = pool_with(cfg);

        {
            let _c = pool.acquire(addr).unwrap();
        }
        assert_eq!(pool.idle_count(), 1);
        tokio::time::sleep(Duration::from_millis(80)).await;
        let _c2 = pool.acquire(addr).unwrap();
        assert_eq!(pool.idle_count(), 0);
    }

    #[tokio::test]
    async fn non_reusable_is_not_parked() {
        let (_l, addr, _stop) = echo_listener();
        let pool = pool_with(PoolConfig::default());
        {
            let mut c = pool.acquire(addr).unwrap();
            c.set_reusable(false);
        }
        assert_eq!(pool.idle_count(), 0);
    }

    #[tokio::test]
    async fn take_stream_detaches_from_pool() {
        let (_l, addr, _stop) = echo_listener();
        let pool = pool_with(PoolConfig::default());
        let c = pool.acquire(addr).unwrap();
        let raw = c.take_stream();
        assert!(raw.is_some());
        assert_eq!(pool.idle_count(), 0);
    }

    /// Randomized size-invariant check: after any sequence of
    /// acquire/release operations the total idle count must never exceed
    /// `total_max`, and per-peer counts never exceed `per_peer_max`.
    /// Hand-rolled proptest to avoid pulling in a new workspace dep.
    #[tokio::test]
    async fn size_invariant_holds_under_random_ops() {
        use rand::Rng;
        use rand::SeedableRng;

        let (_l1, addr1, _s1) = echo_listener();
        let (_l2, addr2, _s2) = echo_listener();
        let (_l3, addr3, _s3) = echo_listener();
        let peers = [addr1, addr2, addr3];

        let cfg = PoolConfig {
            per_peer_max: 3,
            total_max: 5,
            idle_timeout: Duration::from_secs(60),
            max_age: Duration::from_secs(60),
        };
        let pool = pool_with(cfg);

        let mut rng = rand::rngs::StdRng::seed_from_u64(0xDEAD_BEEF);

        let mut held: Vec<PooledTcp> = Vec::new();
        for _ in 0..400 {
            let op: u8 = rng.gen_range(0..3);
            match op {
                0 | 1 => {
                    let i = rng.gen_range(0..peers.len());
                    if let Some(peer) = peers.get(i) {
                        if let Ok(c) = pool.acquire(*peer) {
                            held.push(c);
                        }
                    }
                }
                _ => {
                    if !held.is_empty() {
                        let idx = rng.gen_range(0..held.len());
                        let _ = held.swap_remove(idx);
                    }
                }
            }

            assert!(
                pool.idle_count() <= 5,
                "idle_count {} exceeds total_max",
                pool.idle_count()
            );
            for a in &peers {
                assert!(
                    pool.idle_count_for(*a) <= 3,
                    "idle_count_for {a} exceeds per_peer_max"
                );
            }
        }

        drop(held);
        assert!(pool.idle_count() <= 5);
    }
}
