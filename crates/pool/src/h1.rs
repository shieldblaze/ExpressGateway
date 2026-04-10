//! HTTP/1.1 connection pool with LIFO ordering for warm connection reuse.

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
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Time since last use.
    pub fn idle_time(&self) -> Duration {
        self.last_used.elapsed()
    }

    /// Touch the connection to update last-used time.
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
    fn new(max_size: usize) -> Self {
        Self {
            connections: VecDeque::with_capacity(max_size),
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
            // Skip expired connections.
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
            // Pool is full; drop the connection.
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

    /// Get current pool statistics.
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    /// Number of pooled connections for a given node.
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

        // Release connections 1, 2, 3 in order.
        pool.release("node-a", PooledConnection::new(1));
        pool.release("node-a", PooledConnection::new(2));
        pool.release("node-a", PooledConnection::new(3));

        // LIFO: should get 3, 2, 1 back.
        let c = pool.acquire("node-a").unwrap();
        assert_eq!(c.id, 3);
        let c = pool.acquire("node-a").unwrap();
        assert_eq!(c.id, 2);
        let c = pool.acquire("node-a").unwrap();
        assert_eq!(c.id, 1);

        // Pool is empty now.
        assert!(pool.acquire("node-a").is_none());
    }

    #[test]
    fn hit_miss_counters() {
        let pool = H1Pool::new(H1PoolConfig::default());

        // Miss on empty pool.
        assert!(pool.acquire("node-a").is_none());
        let stats = pool.stats();
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 0);

        pool.release("node-a", PooledConnection::new(1));
        let _ = pool.acquire("node-a").unwrap();
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
        pool.release("node-a", PooledConnection::new(3)); // dropped (over limit)

        assert_eq!(pool.pooled_count("node-a"), 2);
    }

    #[test]
    fn idle_eviction() {
        let config = H1PoolConfig {
            max_per_node: 64,
            idle_timeout: Duration::from_millis(0), // immediate expiry
            max_age: Duration::from_secs(300),
        };
        let pool = H1Pool::new(config);

        pool.release("node-a", PooledConnection::new(1));
        pool.release("node-a", PooledConnection::new(2));

        // By the time we evict, connections have non-zero idle time (> 0ms).
        let evicted = pool.evict_idle();
        assert_eq!(evicted, 2);
        assert_eq!(pool.pooled_count("node-a"), 0);
    }

    #[test]
    fn max_age_eviction() {
        let config = H1PoolConfig {
            max_per_node: 64,
            idle_timeout: Duration::from_secs(60),
            max_age: Duration::from_millis(0), // immediate age-out
        };
        let pool = H1Pool::new(config);

        pool.release("node-a", PooledConnection::new(1));

        // Connection is already older than 0ms.
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

        // Connection idle_time > 0ms threshold, so acquire should miss.
        assert!(pool.acquire("node-a").is_none());
        let stats = pool.stats();
        // The expired connection counts as an eviction during acquire.
        assert!(stats.evictions >= 1);
    }

    #[test]
    fn per_node_isolation() {
        let pool = H1Pool::new(H1PoolConfig::default());

        pool.release("node-a", PooledConnection::new(1));
        pool.release("node-b", PooledConnection::new(2));

        assert!(pool.acquire("node-c").is_none());
        let a = pool.acquire("node-a").unwrap();
        assert_eq!(a.id, 1);
        let b = pool.acquire("node-b").unwrap();
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
}
