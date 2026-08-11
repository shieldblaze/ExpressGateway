//! TCP connection pool with per-peer LRU and a Pingora-style liveness probe.
//!
//! Idle sockets are parked as `std::net::TcpStream`, not tokio streams: a tokio stream cannot
//! cleanly unregister from the reactor to sleep.
//!
//! Reaping is acquire-driven with NO background task, which keeps the pool usable outside a tokio
//! runtime — nothing expires while the pool sits untouched.

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
/// Default connect-timeout for a fresh async dial; mirrors `runtime.connect_timeout_ms`.
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;

/// Configuration for [`TcpPool`].
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
    /// Dial deadline for [`TcpPool::acquire_async`]. IGNORED by the blocking [`TcpPool::acquire`]
    /// path, which inherits the kernel's minute-plus `connect(2)` default.
    pub connect_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            per_peer_max: DEFAULT_PER_PEER_MAX,
            total_max: DEFAULT_TOTAL_MAX,
            idle_timeout: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
            max_age: Duration::from_secs(DEFAULT_MAX_AGE_SECS),
            connect_timeout: Duration::from_millis(DEFAULT_CONNECT_TIMEOUT_MS),
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
    /// New pool; fresh dials inherit `connect_opts`.
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

    /// Idle connections parked across every peer.
    #[must_use]
    pub fn idle_count(&self) -> usize {
        self.inner.total.load(Ordering::Relaxed)
    }

    /// Idle connections parked for `addr`, if any.
    #[must_use]
    pub fn idle_count_for(&self, addr: SocketAddr) -> usize {
        self.inner.per_peer.get(&addr).map_or(0, |q| q.lock().len())
    }

    /// Acquire a connection to `addr`, reusing a pooled entry when one validates.
    ///
    /// NOT for production callers (CODE-2-09) — the fresh-dial `connect(2)` blocks inline on the
    /// calling task. Use [`TcpPool::acquire_async`]; this stays for non-tokio embedders and tests.
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

    /// [`TcpPool::acquire`] with a cancellable async dial under [`PoolConfig::connect_timeout`].
    /// The production path — reuse, expiry and [`probe_alive`] are identical; only the dial differs.
    pub async fn acquire_async(&self, addr: SocketAddr) -> io::Result<PooledTcp> {
        while let Some(idle) = self.pop_idle(addr) {
            match self.validate_and_upgrade(idle, addr) {
                Ok(pooled) => return Ok(pooled),
                Err(ValidationOutcome::Discard) => continue,
                Err(ValidationOutcome::Fatal(err)) => return Err(err),
            }
        }
        self.dial_new_async(addr).await
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

    /// Check age, idle timeout and liveness. `Discard` means try the next entry; `Fatal` must
    /// surface to the caller.
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
        // `probe_alive` deliberately leaves the socket non-blocking, which is what `from_std`
        // requires — do not "restore" blocking mode here.
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

    /// Fresh async dial under the connect deadline, then post-connect sockopts.
    async fn dial_new_async(&self, addr: SocketAddr) -> io::Result<PooledTcp> {
        let connect_fut = TcpStream::connect(addr);
        let stream =
            match tokio::time::timeout(self.inner.config.connect_timeout, connect_fut).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return Err(e),
                Err(_elapsed) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "TcpStream::connect to {addr} exceeded {} ms",
                            self.inner.config.connect_timeout.as_millis()
                        ),
                    ));
                }
            };
        crate::sockopts::apply_connected_tokio(&stream, &self.inner.connect_opts)?;
        let created_at = Instant::now();
        Ok(PooledTcp::new(stream, addr, created_at, self.inner.clone()))
    }
}

/// Reason a pooled connection was rejected by [`TcpPool::validate_and_upgrade`].
enum ValidationOutcome {
    /// Discard this entry and try the next one (or dial fresh).
    Discard,
    /// Pool operation failed in a way that should surface to the caller.
    Fatal(io::Error),
}

/// A checked-out connection; re-parks into the pool on drop unless marked non-reusable. The
/// stream is an `Option` only so `Drop` can steal it — it is `Some` until exactly one of
/// `take_stream` or `drop` runs.
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

    /// Mutable access to the stream; never `None` in practice, since `take_stream` consumes self.
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

    /// Mark this connection non-reusable so a broken socket is not parked.
    ///
    /// **ROUND8-L7-10 — API contract for future H1 upstream reuse.** No production caller today
    /// (H1 upstreams `take_stream` and are single-use), but DO NOT DELETE: Pingora paid for the
    /// body-length-mismatch upstream-smuggling bug twice (0.6.0 and 0.8.0), and this call on any
    /// over/under-read before drop is the fix. Deleted as dead, it gets silently reinvented wrong.
    pub const fn set_reusable(&mut self, reusable: bool) {
        self.reusable = reusable;
    }

    /// Current reusable flag.
    #[must_use]
    pub const fn is_reusable(&self) -> bool {
        self.reusable
    }

    /// Detach the stream so the pool never recycles it.
    pub fn take_stream(mut self) -> Option<TcpStream> {
        // Stops Drop from re-entering the pool.
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

        // Back to std so the idle socket is not registered with the reactor while it sleeps.
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

/// Liveness probe (Pingora EC-01). `WouldBlock` is HEALTHY — the peer simply has nothing to say.
/// `Ok(0)` is a half-close and `Ok(n)` is protocol desync; both are unusable. Leaves the stream
/// non-blocking for [`tokio::net::TcpStream::from_std`].
fn probe_alive(stream: &StdTcpStream) -> bool {
    use std::io::Read;

    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let mut buf = [0u8; 1];
    // `impl Read for &TcpStream` reads through a shared reference; the binding exists only to
    // satisfy the trait's `&mut self`.
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

        // Reuse must yield the same local port.
        let mut c2 = pool.acquire(addr).unwrap();
        let local_second = c2.stream_mut().unwrap().local_addr().unwrap();
        assert_eq!(local_first, local_second);
        assert_eq!(pool.idle_count(), 0);
    }

    #[tokio::test]
    async fn probe_discards_peer_closed_connection() {
        let (_l, addr, _stop) = echo_listener();
        let pool = pool_with(PoolConfig::default());

        {
            let mut c = pool.acquire(addr).unwrap();
            let s = c.stream_mut().unwrap();
            s.write_all(b"x").await.unwrap();
            let mut buf = [0u8; 1];
            s.read_exact(&mut buf).await.unwrap();
        }
        assert_eq!(pool.idle_count(), 1);

        // Simulate the peer half-closing while the socket sat idle.
        shutdown_first_idle(&pool, addr);

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
            max_age: Duration::from_millis(50),
            ..PoolConfig::default()
        };
        let pool = pool_with(cfg);

        {
            let _c = pool.acquire(addr).unwrap();
        }
        assert_eq!(pool.idle_count(), 1);

        tokio::time::sleep(Duration::from_millis(120)).await;

        let _c2 = pool.acquire(addr).unwrap();
        // Expired entry discarded on acquire, fresh one dialed.
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
            ..PoolConfig::default()
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

    /// Hand-rolled proptest (avoids a new workspace dep): no op sequence may breach either cap.
    #[tokio::test]
    async fn size_invariant_holds_under_random_ops() {
        use rand::RngExt;
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
            ..PoolConfig::default()
        };
        let pool = pool_with(cfg);

        let mut rng = rand::rngs::StdRng::seed_from_u64(0xDEAD_BEEF);

        let mut held: Vec<PooledTcp> = Vec::new();
        for _ in 0..400 {
            let op: u8 = rng.random_range(0..3);
            match op {
                0 | 1 => {
                    let i = rng.random_range(0..peers.len());
                    if let Some(peer) = peers.get(i) {
                        if let Ok(c) = pool.acquire(*peer) {
                            held.push(c);
                        }
                    }
                }
                _ => {
                    if !held.is_empty() {
                        let idx = rng.random_range(0..held.len());
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

    /// `acquire_async` parks back into the per-peer queue exactly like the blocking path.
    #[tokio::test]
    async fn acquire_async_dials_then_parks() {
        let (_l, addr, _stop) = echo_listener();
        let pool = pool_with(PoolConfig::default());

        {
            let mut c = pool.acquire_async(addr).await.unwrap();
            let s = c.stream_mut().unwrap();
            s.write_all(b"hi").await.unwrap();
            let mut buf = [0u8; 2];
            s.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"hi");
        }
        assert_eq!(pool.idle_count_for(addr), 1);

        let _c2 = pool.acquire_async(addr).await.unwrap();
        assert_eq!(pool.idle_count(), 0);
    }

    /// The async dial must honour `connect_timeout` against a black-holed address.
    #[tokio::test]
    async fn acquire_async_timeout_fires() {
        let cfg = PoolConfig {
            connect_timeout: Duration::from_millis(150),
            ..PoolConfig::default()
        };
        let pool = pool_with(cfg);
        // RFC 5737 TEST-NET-1 — packets are dropped, never refused.
        let unreachable: SocketAddr = "192.0.2.1:1".parse().unwrap();

        let start = std::time::Instant::now();
        let res = pool.acquire_async(unreachable).await;
        let elapsed = start.elapsed();

        assert!(res.is_err(), "expected timeout error, got {res:?}");
        let err = res.unwrap_err();
        // Either our TimedOut or an early routing error is fine; what matters is that the call
        // never falls through to the kernel's minute-plus default.
        assert!(
            elapsed < Duration::from_secs(2),
            "async dial took {elapsed:?}, expected <2s (timeout {err:?})"
        );
    }

    /// Source-level guard: the dial path must never route through the blocking dispatcher again.
    #[test]
    fn no_spawn_blocking_in_pool_dial_path() {
        let pool_src = include_str!("pool.rs");
        let needle = ["tokio::task::", "spawn_blocking"].concat();
        let mut bad = Vec::new();
        for (lineno, line) in pool_src.lines().enumerate() {
            if !line.contains(&needle) {
                continue;
            }
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue;
            }
            bad.push(format!("pool.rs:{}: {}", lineno + 1, line));
        }
        assert!(
            bad.is_empty(),
            "TcpPool dial path must not use the deprecated blocking dispatcher; offenders:\n{}",
            bad.join("\n")
        );
    }
}
