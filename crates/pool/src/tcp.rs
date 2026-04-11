//! TCP (L4) connection pool with FIFO ordering for even connection aging.
//!
//! FIFO ordering ensures connections age evenly and no single connection sits
//! idle indefinitely while newer ones are used.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::evictor::Evictable;

/// Configuration for the TCP connection pool.
#[derive(Debug, Clone)]
pub struct TcpPoolConfig {
    /// Maximum idle connections per backend (default 8).
    pub max_per_backend: usize,
    /// Maximum total pooled connections across all backends (default 256).
    pub max_total: usize,
    /// Evict connections idle longer than this (default 60s).
    pub idle_timeout: Duration,
}

impl Default for TcpPoolConfig {
    fn default() -> Self {
        Self {
            max_per_backend: 8,
            max_total: 256,
            idle_timeout: Duration::from_secs(60),
        }
    }
}

/// A pooled TCP connection.
#[derive(Debug)]
pub struct TcpPooledConnection {
    /// Unique connection identifier.
    pub id: u64,
    /// When the connection was created.
    created_at: Instant,
    /// When the connection was last used.
    last_used: Instant,
    /// Whether this connection is still usable (simulates validation).
    usable: bool,
}

impl TcpPooledConnection {
    /// Create a new pooled TCP connection.
    #[inline]
    pub fn new(id: u64) -> Self {
        let now = Instant::now();
        Self {
            id,
            created_at: now,
            last_used: now,
            usable: true,
        }
    }

    /// Age of the connection since creation.
    #[inline]
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Time since last use.
    #[inline]
    pub fn idle_time(&self) -> Duration {
        self.last_used.elapsed()
    }

    /// Whether this connection is usable.
    #[inline]
    pub fn is_usable(&self) -> bool {
        self.usable
    }

    /// Mark this connection as dead/unusable.
    #[inline]
    pub fn mark_dead(&mut self) {
        self.usable = false;
    }

    /// Touch the connection to update last-used time.
    #[inline]
    fn touch(&mut self) {
        self.last_used = Instant::now();
    }
}

/// TCP connection pool using FIFO ordering (oldest connection first).
pub struct TcpPool {
    pools: DashMap<String, VecDeque<TcpPooledConnection>>,
    config: TcpPoolConfig,
    total_pooled: AtomicUsize,
}

impl TcpPool {
    /// Create a new TCP pool with the given configuration.
    pub fn new(config: TcpPoolConfig) -> Self {
        Self {
            pools: DashMap::new(),
            config,
            total_pooled: AtomicUsize::new(0),
        }
    }

    /// Acquire a connection from the pool for the given backend.
    ///
    /// Returns `None` if no usable connections are available. Dead connections
    /// are silently discarded during acquisition.
    /// Uses FIFO ordering: pops from the front (oldest first).
    pub fn acquire(&self, backend_id: &str) -> Option<TcpPooledConnection> {
        let mut pool = self.pools.get_mut(backend_id)?;
        while let Some(mut conn) = pool.pop_front() {
            self.total_pooled.fetch_sub(1, Ordering::Relaxed);
            if !conn.is_usable() {
                continue;
            }
            if conn.idle_time() > self.config.idle_timeout {
                continue;
            }
            conn.touch();
            return Some(conn);
        }
        None
    }

    /// Return a connection to the pool.
    ///
    /// The connection is silently dropped if:
    /// - The per-backend limit is reached
    /// - The global total limit is reached
    /// - The connection is not usable
    pub fn release(&self, backend_id: &str, mut conn: TcpPooledConnection) {
        if !conn.is_usable() {
            return;
        }

        // Reserve a slot in the global total via CAS to prevent exceeding max_total
        // under contention.
        loop {
            let current_total = self.total_pooled.load(Ordering::Acquire);
            if current_total >= self.config.max_total {
                return;
            }
            match self.total_pooled.compare_exchange_weak(
                current_total,
                current_total + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }

        conn.touch();

        // Fast path: backend already exists, no key allocation needed.
        if let Some(mut pool) = self.pools.get_mut(backend_id) {
            if pool.len() >= self.config.max_per_backend {
                self.total_pooled.fetch_sub(1, Ordering::Relaxed);
                return;
            }
            pool.push_back(conn);
            return;
        }

        // Slow path: first connection for this backend, allocate key.
        let mut pool = self
            .pools
            .entry(backend_id.to_owned())
            .or_insert_with(|| VecDeque::with_capacity(self.config.max_per_backend.min(32)));

        if pool.len() >= self.config.max_per_backend {
            self.total_pooled.fetch_sub(1, Ordering::Relaxed);
            return;
        }

        pool.push_back(conn);
    }

    /// Evict idle connections from all backend pools.
    ///
    /// Returns the number of connections evicted.
    pub fn evict_idle(&self) -> usize {
        let mut total = 0usize;
        for mut entry in self.pools.iter_mut() {
            let pool = entry.value_mut();
            let before = pool.len();
            pool.retain(|conn| conn.idle_time() <= self.config.idle_timeout && conn.is_usable());
            let evicted = before - pool.len();
            total += evicted;
        }
        self.total_pooled.fetch_sub(total, Ordering::Relaxed);
        total
    }

    /// Drain all connections for a specific backend (backend removal).
    pub fn drain_backend(&self, backend_id: &str) -> usize {
        match self.pools.remove(backend_id) {
            Some((_, pool)) => {
                let count = pool.len();
                self.total_pooled.fetch_sub(count, Ordering::Relaxed);
                count
            }
            None => 0,
        }
    }

    /// Number of pooled connections for a given backend.
    #[inline]
    pub fn pooled_count(&self, backend_id: &str) -> usize {
        self.pools.get(backend_id).map(|p| p.len()).unwrap_or(0)
    }

    /// Total number of pooled connections across all backends.
    #[inline]
    pub fn total_pooled(&self) -> usize {
        self.total_pooled.load(Ordering::Relaxed)
    }
}

impl Evictable for TcpPool {
    fn evict_expired(&self) -> usize {
        self.evict_idle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_ordering() {
        let pool = TcpPool::new(TcpPoolConfig::default());

        pool.release("backend-a", TcpPooledConnection::new(1));
        pool.release("backend-a", TcpPooledConnection::new(2));
        pool.release("backend-a", TcpPooledConnection::new(3));

        let c = pool.acquire("backend-a").expect("conn 1");
        assert_eq!(c.id, 1);
        let c = pool.acquire("backend-a").expect("conn 2");
        assert_eq!(c.id, 2);
        let c = pool.acquire("backend-a").expect("conn 3");
        assert_eq!(c.id, 3);
        assert!(pool.acquire("backend-a").is_none());
    }

    #[test]
    fn dead_connections_discarded() {
        let pool = TcpPool::new(TcpPoolConfig::default());

        let mut dead = TcpPooledConnection::new(1);
        dead.mark_dead();
        pool.release("backend-a", dead);
        assert_eq!(pool.pooled_count("backend-a"), 0);

        let conn = TcpPooledConnection::new(2);
        pool.release("backend-a", conn);
        assert_eq!(pool.pooled_count("backend-a"), 1);

        let mut c = pool.acquire("backend-a").expect("should have conn");
        c.mark_dead();
        pool.release("backend-a", c);
        assert_eq!(pool.pooled_count("backend-a"), 0);
    }

    #[test]
    fn max_per_backend_limit() {
        let config = TcpPoolConfig {
            max_per_backend: 2,
            max_total: 256,
            idle_timeout: Duration::from_secs(60),
        };
        let pool = TcpPool::new(config);

        pool.release("backend-a", TcpPooledConnection::new(1));
        pool.release("backend-a", TcpPooledConnection::new(2));
        pool.release("backend-a", TcpPooledConnection::new(3));

        assert_eq!(pool.pooled_count("backend-a"), 2);
    }

    #[test]
    fn max_total_limit() {
        let config = TcpPoolConfig {
            max_per_backend: 8,
            max_total: 2,
            idle_timeout: Duration::from_secs(60),
        };
        let pool = TcpPool::new(config);

        pool.release("backend-a", TcpPooledConnection::new(1));
        pool.release("backend-b", TcpPooledConnection::new(2));
        pool.release("backend-c", TcpPooledConnection::new(3));

        assert_eq!(pool.total_pooled(), 2);
    }

    #[test]
    fn idle_eviction() {
        let config = TcpPoolConfig {
            max_per_backend: 8,
            max_total: 256,
            idle_timeout: Duration::from_millis(0),
        };
        let pool = TcpPool::new(config);

        pool.release("backend-a", TcpPooledConnection::new(1));
        pool.release("backend-a", TcpPooledConnection::new(2));

        let evicted = pool.evict_idle();
        assert_eq!(evicted, 2);
        assert_eq!(pool.total_pooled(), 0);
    }

    #[test]
    fn acquire_skips_expired() {
        let config = TcpPoolConfig {
            max_per_backend: 8,
            max_total: 256,
            idle_timeout: Duration::from_millis(0),
        };
        let pool = TcpPool::new(config);

        pool.release("backend-a", TcpPooledConnection::new(1));

        assert!(pool.acquire("backend-a").is_none());
    }

    #[test]
    fn per_backend_isolation() {
        let pool = TcpPool::new(TcpPoolConfig::default());

        pool.release("backend-a", TcpPooledConnection::new(1));
        pool.release("backend-b", TcpPooledConnection::new(2));

        assert!(pool.acquire("backend-c").is_none());
        let a = pool.acquire("backend-a").expect("backend-a conn");
        assert_eq!(a.id, 1);
    }

    #[test]
    fn drain_backend_removes_all() {
        let pool = TcpPool::new(TcpPoolConfig::default());

        pool.release("backend-a", TcpPooledConnection::new(1));
        pool.release("backend-a", TcpPooledConnection::new(2));
        pool.release("backend-b", TcpPooledConnection::new(3));

        let drained = pool.drain_backend("backend-a");
        assert_eq!(drained, 2);
        assert_eq!(pool.pooled_count("backend-a"), 0);
        assert_eq!(pool.pooled_count("backend-b"), 1);
    }
}
