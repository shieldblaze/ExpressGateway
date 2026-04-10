//! HTTP/2 connection pool with shared connections and stream multiplexing.
//!
//! Unlike HTTP/1.1, multiple requests share the same underlying connection via
//! HTTP/2 streams. A new connection is only created when all existing connections
//! for a node are at their concurrent stream limit.
//!
//! Stream acquisition uses CAS for lock-free concurrency.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::evictor::Evictable;

/// Configuration for the HTTP/2 connection pool.
#[derive(Debug, Clone)]
pub struct H2PoolConfig {
    /// Maximum connections per node (default 4).
    pub max_per_node: usize,
    /// Maximum concurrent streams per connection (default 100).
    pub max_concurrent_streams: u32,
    /// Maximum connection age before eviction (default 5 minutes).
    pub max_age: Duration,
}

impl Default for H2PoolConfig {
    fn default() -> Self {
        Self {
            max_per_node: 4,
            max_concurrent_streams: 100,
            max_age: Duration::from_secs(300),
        }
    }
}

/// A pooled HTTP/2 connection supporting multiple concurrent streams.
pub struct H2PooledConnection {
    /// Unique connection identifier.
    pub id: u64,
    /// Number of active streams on this connection.
    active_streams: AtomicU32,
    /// Maximum concurrent streams allowed.
    max_concurrent_streams: u32,
    /// When the connection was created.
    created_at: Instant,
}

impl std::fmt::Debug for H2PooledConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H2PooledConnection")
            .field("id", &self.id)
            .field(
                "active_streams",
                &self.active_streams.load(Ordering::Relaxed),
            )
            .field("max_concurrent_streams", &self.max_concurrent_streams)
            .finish()
    }
}

impl H2PooledConnection {
    /// Create a new HTTP/2 pooled connection.
    #[inline]
    pub fn new(id: u64, max_concurrent_streams: u32) -> Self {
        Self {
            id,
            active_streams: AtomicU32::new(0),
            max_concurrent_streams,
            created_at: Instant::now(),
        }
    }

    /// Whether this connection has capacity for another stream.
    #[inline]
    pub fn has_capacity(&self) -> bool {
        self.active_streams.load(Ordering::Acquire) < self.max_concurrent_streams
    }

    /// Try to acquire a stream on this connection.
    ///
    /// Returns `true` if a stream was successfully acquired via CAS.
    /// Returns `false` if the connection is at capacity.
    #[inline]
    pub fn acquire_stream(&self) -> bool {
        loop {
            let current = self.active_streams.load(Ordering::Acquire);
            if current >= self.max_concurrent_streams {
                return false;
            }
            match self.active_streams.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(_) => continue,
            }
        }
    }

    /// Release a stream on this connection.
    #[inline]
    pub fn release_stream(&self) {
        let prev = self.active_streams.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0, "release_stream called with 0 active streams");
    }

    /// Number of active streams.
    #[inline]
    pub fn active_streams(&self) -> u32 {
        self.active_streams.load(Ordering::Relaxed)
    }

    /// Age of this connection since creation.
    #[inline]
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }
}

/// HTTP/2 connection pool supporting stream multiplexing.
pub struct H2Pool {
    pools: DashMap<String, Vec<H2PooledConnection>>,
    config: H2PoolConfig,
}

impl H2Pool {
    /// Create a new HTTP/2 pool with the given configuration.
    pub fn new(config: H2PoolConfig) -> Self {
        Self {
            pools: DashMap::new(),
            config,
        }
    }

    /// Try to acquire a stream on an existing connection for the given node.
    ///
    /// Returns the connection id if a stream was successfully acquired.
    /// Returns `None` if no connection has capacity (caller should create a new one).
    pub fn acquire_stream(&self, node_id: &str) -> Option<u64> {
        let pool = self.pools.get(node_id)?;
        for conn in pool.iter() {
            if conn.acquire_stream() {
                return Some(conn.id);
            }
        }
        None
    }

    /// Release a stream on a connection for the given node.
    pub fn release_stream(&self, node_id: &str, conn_id: u64) {
        if let Some(pool) = self.pools.get(node_id)
            && let Some(conn) = pool.iter().find(|c| c.id == conn_id)
        {
            conn.release_stream();
        }
    }

    /// Add a new connection for the given node.
    ///
    /// Returns `false` if the per-node limit has been reached.
    pub fn add_connection(&self, node_id: &str, conn: H2PooledConnection) -> bool {
        let mut pool = self.pools.entry(node_id.to_owned()).or_default();

        if pool.len() >= self.config.max_per_node {
            return false;
        }
        pool.push(conn);
        true
    }

    /// Number of connections for a given node.
    #[inline]
    pub fn connection_count(&self, node_id: &str) -> usize {
        self.pools.get(node_id).map(|p| p.len()).unwrap_or(0)
    }

    /// Whether a new connection can be created for the node.
    #[inline]
    pub fn can_add_connection(&self, node_id: &str) -> bool {
        self.connection_count(node_id) < self.config.max_per_node
    }

    /// Drain all connections for a specific node (backend removal).
    pub fn drain_node(&self, node_id: &str) -> usize {
        match self.pools.remove(node_id) {
            Some((_, pool)) => pool.len(),
            None => 0,
        }
    }

    /// Remove connections that have exceeded the max age and have no active streams.
    pub fn evict_aged(&self) -> usize {
        let mut total = 0;
        for mut entry in self.pools.iter_mut() {
            let pool = entry.value_mut();
            let before = pool.len();
            pool.retain(|conn| {
                !(conn.age() > self.config.max_age
                    && conn.active_streams.load(Ordering::Relaxed) == 0)
            });
            total += before - pool.len();
        }
        total
    }
}

impl Evictable for H2Pool {
    fn evict_expired(&self) -> usize {
        self.evict_aged()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_capacity_check() {
        let conn = H2PooledConnection::new(1, 2);
        assert!(conn.has_capacity());

        assert!(conn.acquire_stream());
        assert!(conn.has_capacity());

        assert!(conn.acquire_stream());
        assert!(!conn.has_capacity());

        assert!(!conn.acquire_stream());

        conn.release_stream();
        assert!(conn.has_capacity());
        assert!(conn.acquire_stream());
    }

    #[test]
    fn pool_acquire_returns_connection_with_capacity() {
        let config = H2PoolConfig {
            max_per_node: 4,
            max_concurrent_streams: 2,
            max_age: Duration::from_secs(300),
        };
        let pool = H2Pool::new(config);

        pool.add_connection("node-a", H2PooledConnection::new(1, 2));

        let id1 = pool.acquire_stream("node-a").expect("should acquire");
        assert_eq!(id1, 1);
        let id2 = pool.acquire_stream("node-a").expect("should acquire");
        assert_eq!(id2, 1);

        assert!(pool.acquire_stream("node-a").is_none());

        pool.release_stream("node-a", 1);
        let id3 = pool.acquire_stream("node-a").expect("should acquire after release");
        assert_eq!(id3, 1);
    }

    #[test]
    fn max_connections_per_node() {
        let config = H2PoolConfig {
            max_per_node: 2,
            max_concurrent_streams: 100,
            max_age: Duration::from_secs(300),
        };
        let pool = H2Pool::new(config);

        assert!(pool.add_connection("node-a", H2PooledConnection::new(1, 100)));
        assert!(pool.add_connection("node-a", H2PooledConnection::new(2, 100)));
        assert!(!pool.add_connection("node-a", H2PooledConnection::new(3, 100)));

        assert_eq!(pool.connection_count("node-a"), 2);
    }

    #[test]
    fn multiple_connections_round_robin_capacity() {
        let config = H2PoolConfig {
            max_per_node: 4,
            max_concurrent_streams: 1,
            max_age: Duration::from_secs(300),
        };
        let pool = H2Pool::new(config);

        pool.add_connection("node-a", H2PooledConnection::new(1, 1));
        pool.add_connection("node-a", H2PooledConnection::new(2, 1));

        let id1 = pool.acquire_stream("node-a").expect("conn 1");
        assert_eq!(id1, 1);

        let id2 = pool.acquire_stream("node-a").expect("conn 2");
        assert_eq!(id2, 2);

        assert!(pool.acquire_stream("node-a").is_none());
    }

    #[test]
    fn evict_aged_connections() {
        let config = H2PoolConfig {
            max_per_node: 4,
            max_concurrent_streams: 100,
            max_age: Duration::from_millis(0),
        };
        let pool = H2Pool::new(config);

        pool.add_connection("node-a", H2PooledConnection::new(1, 100));
        pool.add_connection("node-a", H2PooledConnection::new(2, 100));

        let evicted = pool.evict_aged();
        assert_eq!(evicted, 2);
        assert_eq!(pool.connection_count("node-a"), 0);
    }

    #[test]
    fn evict_skips_connections_with_active_streams() {
        let config = H2PoolConfig {
            max_per_node: 4,
            max_concurrent_streams: 100,
            max_age: Duration::from_millis(0),
        };
        let pool = H2Pool::new(config);

        pool.add_connection("node-a", H2PooledConnection::new(1, 100));
        pool.add_connection("node-a", H2PooledConnection::new(2, 100));

        pool.acquire_stream("node-a");

        let evicted = pool.evict_aged();
        assert_eq!(evicted, 1);
        assert_eq!(pool.connection_count("node-a"), 1);
    }

    #[test]
    fn drain_node_removes_all() {
        let pool = H2Pool::new(H2PoolConfig::default());

        pool.add_connection("node-a", H2PooledConnection::new(1, 100));
        pool.add_connection("node-a", H2PooledConnection::new(2, 100));

        let drained = pool.drain_node("node-a");
        assert_eq!(drained, 2);
        assert_eq!(pool.connection_count("node-a"), 0);
    }
}
