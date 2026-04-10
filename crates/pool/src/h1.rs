//! HTTP/1.1 connection pool with LIFO ordering for warm connection reuse.
//!
//! Uses `DashMap` for per-node sharding and `VecDeque` for the per-node pool.
//! LIFO ordering keeps recently-used connections warm in CPU cache and is more
//! likely to return connections that the OS has not yet marked for TCP keep-alive
//! probes.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::evictor::Evictable;

/// Statistics for the H1 connection pool.
#[derive(Debug, Clone, Copy)]
pub struct PoolStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

/// Configuration for the HTTP/1.1 connection pool.
#[derive(Debug, Clone)]
pub struct H1PoolConfig {
    /// Maximum idle connections per node (default 64).
    pub max_per_node: usize,
    /// Evict connections idle longer than this (default 60s).
    pub idle_timeout: Duration,
    /// Evict connections older than this (default 5 minutes).
    pub max_age: Duration,
}

impl Default for H1PoolConfig {
    fn default() -> Self {
        Self {
            max_per_node: 64,
            idle_timeout: Duration::from_secs(60),
            max_age: Duration::from_secs(300),
        }
    }
}

/// A pooled HTTP/1.1 connection.
#[derive(Debug)]
pub struct PooledConnection {
    /// Unique connection identifier.
    pub id: u64,
    /// When the connection was created.
    created_at: Instant,
    /// When the connection was last used (returned to pool or acquired).
    last_used: Instant,
}

impl PooledConnection {
    /// Create a new pooled connection with the given id.
    #[inline]
    pub fn new(id: u64) -> Self {
        let now = Instant::now();
        Self {
            id,
            created_at: now,
            last_used: now,
        }
    }

    /// Create a pooled connection with explicit timestamps (for testing).
    pub fn with_times(id: u64, created_at: Instant, last_used: Instant) -> Self {
        Self {
            id,
            created_at,
            last_used,
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

    /// Touch the connection to update last-used time.
    #[inline]
    fn touch(&mut self) {
        self.last_used = Instant::now();
    }
}

/// Per-node pool of HTTP/1.1 connections.
struct NodePool {
    connections: VecDeque<PooledConnection>,
    max_size: usize,
}

impl NodePool {
    #[inline]
    fn new(max_size: usize) -> Self {
        Self {
            connections: VecDeque::with_capacity(max_size.min(64)),
            max_size,
        }
    }
}

/// HTTP/1.1 connection pool using LIFO ordering (most recently used first).
///
/// LIFO ordering keeps recently-used connections warm in CPU cache and is more
/// likely to return connections that the OS has not yet marked for TCP keep-alive
/// probes.
pub struct H1Pool {
    pools: DashMap<String, NodePool>,
    config: H1PoolConfig,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

impl H1Pool {
    /// Create a new pool with the given configuration.
    pub fn new(config: H1PoolConfig) -> Self {
        Self {
            pools: DashMap::new(),
            config,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// Acquire a connection from the pool for the given node.
    ///
    /// Returns `None` if no usable connections are available (pool miss).
    /// Uses LIFO ordering: pops from the back of the deque to get the most
    /// recently returned connection.
    pub fn acquire(&self, node_id: &str) -> Option<PooledConnection> {
        let mut pool = match self.pools.get_mut(node_id) {
            Some(p) => p,
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        // LIFO: pop from back (most recently returned)
        while let Some(mut conn) = pool.connections.pop_back() {
            if conn.idle_time() > self.config.idle_timeout || conn.age() > self.config.max_age {
                self.evictions.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            conn.touch();
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Some(conn);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Return a connection to the pool.
    ///
    /// If the per-node pool is full, the connection is silently dropped.
    pub fn release(&self, node_id: &str, mut conn: PooledConnection) {
        conn.touch();
        let mut pool = self
            .pools
            .entry(node_id.to_owned())
            .or_insert_with(|| NodePool::new(self.config.max_per_node));

        if pool.connections.len() >= pool.max_size {
            return;
        }
        // LIFO: push to back so it will be the next acquired.
        pool.connections.push_back(conn);
    }

    /// Evict idle and expired connections from all node pools.
    ///
    /// Returns the number of connections evicted.
    pub fn evict_idle(&self) -> usize {
        let mut total = 0usize;
        for mut entry in self.pools.iter_mut() {
            let pool = entry.value_mut();
            let before = pool.connections.len();
            pool.connections.retain(|conn| {
                conn.idle_time() <= self.config.idle_timeout && conn.age() <= self.config.max_age
            });
            let evicted = before - pool.connections.len();
            total += evicted;
        }
        self.evictions.fetch_add(total as u64, Ordering::Relaxed);
        total
    }

    /// Drain all connections for a specific node (backend removal).
    ///
    /// Returns the number of connections drained.
    pub fn drain_node(&self, node_id: &str) -> usize {
        match self.pools.remove(node_id) {
            Some((_, pool)) => pool.connections.len(),
            None => 0,
        }
    }

    /// Get current pool statistics.
    #[inline]
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    /// Number of pooled connections for a given node.
    #[inline]
    pub fn pooled_count(&self, node_id: &str) -> usize {
        self.pools
            .get(node_id)
            .map(|p| p.connections.len())
            .unwrap_or(0)
    }

    /// Total number of pooled connections across all nodes.
    pub fn total_pooled(&self) -> usize {
        self.pools.iter().map(|e| e.value().connections.len()).sum()
    }
}

impl Evictable for H1Pool {
    fn evict_expired(&self) -> usize {
        self.evict_idle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifo_ordering() {
        let config = H1PoolConfig::default();
        let pool = H1Pool::new(config);

        pool.release("node-a", PooledConnection::new(1));
        pool.release("node-a", PooledConnection::new(2));
        pool.release("node-a", PooledConnection::new(3));

        let c = pool.acquire("node-a").expect("should get conn 3");
        assert_eq!(c.id, 3);
        let c = pool.acquire("node-a").expect("should get conn 2");
        assert_eq!(c.id, 2);
        let c = pool.acquire("node-a").expect("should get conn 1");
        assert_eq!(c.id, 1);
        assert!(pool.acquire("node-a").is_none());
    }

    #[test]
    fn hit_miss_counters() {
        let pool = H1Pool::new(H1PoolConfig::default());

        assert!(pool.acquire("node-a").is_none());
        let stats = pool.stats();
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 0);

        pool.release("node-a", PooledConnection::new(1));
        let _ = pool.acquire("node-a").expect("should hit");
        let stats = pool.stats();
        assert_eq!(stats.hits, 1);
    }

    #[test]
    fn max_per_node_limit() {
        let config = H1PoolConfig {
            max_per_node: 2,
            ..Default::default()
        };
        let pool = H1Pool::new(config);

        pool.release("node-a", PooledConnection::new(1));
        pool.release("node-a", PooledConnection::new(2));
        pool.release("node-a", PooledConnection::new(3));

        assert_eq!(pool.pooled_count("node-a"), 2);
    }

    #[test]
    fn idle_eviction() {
        let config = H1PoolConfig {
            max_per_node: 64,
            idle_timeout: Duration::from_millis(0),
            max_age: Duration::from_secs(300),
        };
        let pool = H1Pool::new(config);

        pool.release("node-a", PooledConnection::new(1));
        pool.release("node-a", PooledConnection::new(2));

        let evicted = pool.evict_idle();
        assert_eq!(evicted, 2);
        assert_eq!(pool.pooled_count("node-a"), 0);
    }

    #[test]
    fn max_age_eviction() {
        let config = H1PoolConfig {
            max_per_node: 64,
            idle_timeout: Duration::from_secs(60),
            max_age: Duration::from_millis(0),
        };
        let pool = H1Pool::new(config);

        pool.release("node-a", PooledConnection::new(1));

        let evicted = pool.evict_idle();
        assert_eq!(evicted, 1);
    }

    #[test]
    fn acquire_skips_expired_connections() {
        let config = H1PoolConfig {
            max_per_node: 64,
            idle_timeout: Duration::from_millis(0),
            max_age: Duration::from_secs(300),
        };
        let pool = H1Pool::new(config);

        pool.release("node-a", PooledConnection::new(1));

        assert!(pool.acquire("node-a").is_none());
        let stats = pool.stats();
        assert!(stats.evictions >= 1);
    }

    #[test]
    fn per_node_isolation() {
        let pool = H1Pool::new(H1PoolConfig::default());

        pool.release("node-a", PooledConnection::new(1));
        pool.release("node-b", PooledConnection::new(2));

        assert!(pool.acquire("node-c").is_none());
        let a = pool.acquire("node-a").expect("node-a should have conn");
        assert_eq!(a.id, 1);
        let b = pool.acquire("node-b").expect("node-b should have conn");
        assert_eq!(b.id, 2);
    }

    #[test]
    fn total_pooled_across_nodes() {
        let pool = H1Pool::new(H1PoolConfig::default());

        pool.release("node-a", PooledConnection::new(1));
        pool.release("node-a", PooledConnection::new(2));
        pool.release("node-b", PooledConnection::new(3));

        assert_eq!(pool.total_pooled(), 3);
    }

    #[test]
    fn drain_node_removes_all() {
        let pool = H1Pool::new(H1PoolConfig::default());

        pool.release("node-a", PooledConnection::new(1));
        pool.release("node-a", PooledConnection::new(2));
        pool.release("node-b", PooledConnection::new(3));

        let drained = pool.drain_node("node-a");
        assert_eq!(drained, 2);
        assert_eq!(pool.pooled_count("node-a"), 0);
        assert_eq!(pool.pooled_count("node-b"), 1);
    }

    #[test]
    fn drain_nonexistent_node() {
        let pool = H1Pool::new(H1PoolConfig::default());
        assert_eq!(pool.drain_node("nonexistent"), 0);
    }
}
