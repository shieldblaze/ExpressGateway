//! QUIC connection pool with connection ID routing and stream multiplexing.
//!
//! Similar to HTTP/2 pooling but adds QUIC-specific features:
//! - Connection ID (DCID/SCID) routing for packet-level demuxing
//! - Stream multiplexing with CAS-based acquire/release
//! - Age-based eviction respecting active streams

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::evictor::Evictable;

/// Statistics for the QUIC connection pool.
#[derive(Debug, Clone, Copy)]
pub struct QuicPoolStats {
    pub streams_acquired: u64,
    pub streams_exhausted: u64,
    pub evictions: u64,
}

/// Configuration for the QUIC connection pool.
#[derive(Debug, Clone)]
pub struct QuicPoolConfig {
    /// Maximum connections per node (default 4).
    pub max_per_node: usize,
    /// Maximum concurrent streams per connection (default 100).
    pub max_concurrent_streams: u32,
    /// Maximum connection age before eviction (default 5 minutes).
    pub max_age: Duration,
}

impl Default for QuicPoolConfig {
    fn default() -> Self {
        Self {
            max_per_node: 4,
            max_concurrent_streams: 100,
            max_age: Duration::from_secs(300),
        }
    }
}

/// Maximum QUIC Connection ID length per RFC 9000 section 17.2.
const MAX_CID_LEN: usize = 20;

/// Stack-allocated QUIC Connection ID (max 20 bytes per RFC 9000).
/// Avoids heap allocation for every pooled connection.
#[derive(Clone)]
pub struct ConnectionId {
    buf: [u8; MAX_CID_LEN],
    len: u8,
}

impl ConnectionId {
    /// Create a new ConnectionId from a byte slice.
    /// Panics if `bytes.len() > 20`.
    pub fn from_slice(bytes: &[u8]) -> Self {
        assert!(
            bytes.len() <= MAX_CID_LEN,
            "QUIC Connection ID exceeds 20-byte maximum (got {})",
            bytes.len()
        );
        let mut buf = [0u8; MAX_CID_LEN];
        buf[..bytes.len()].copy_from_slice(bytes);
        Self {
            buf,
            len: bytes.len() as u8,
        }
    }

    /// Return the connection ID as a byte slice.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl PartialEq<[u8]> for ConnectionId {
    fn eq(&self, other: &[u8]) -> bool {
        self.as_bytes() == other
    }
}

impl std::fmt::Debug for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CID({:02x?})", self.as_bytes())
    }
}

/// A pooled QUIC connection with per-flow state (DCID/SCID).
pub struct QuicPooledConnection {
    /// Unique connection identifier.
    pub id: u64,
    /// Destination Connection ID (stack-allocated, max 20 bytes).
    pub dcid: ConnectionId,
    /// Source Connection ID (stack-allocated, max 20 bytes).
    pub scid: ConnectionId,
    /// Number of active streams.
    active_streams: AtomicU32,
    /// Maximum concurrent streams.
    max_concurrent_streams: u32,
    /// When the connection was created.
    created_at: Instant,
}

impl std::fmt::Debug for QuicPooledConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicPooledConnection")
            .field("id", &self.id)
            .field("dcid", &self.dcid)
            .field("scid", &self.scid)
            .field(
                "active_streams",
                &self.active_streams.load(Ordering::Relaxed),
            )
            .finish()
    }
}

impl QuicPooledConnection {
    /// Create a new QUIC pooled connection.
    pub fn new(id: u64, dcid: &[u8], scid: &[u8], max_concurrent_streams: u32) -> Self {
        Self {
            id,
            dcid: ConnectionId::from_slice(dcid),
            scid: ConnectionId::from_slice(scid),
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

    /// Try to acquire a stream on this connection via CAS.
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
    ///
    /// Uses CAS loop to prevent underflow in release builds. A spurious
    /// double-release is logged and ignored rather than wrapping to `u32::MAX`.
    #[inline]
    pub fn release_stream(&self) {
        loop {
            let current = self.active_streams.load(Ordering::Acquire);
            if current == 0 {
                tracing::error!(conn_id = self.id, "release_stream called with 0 active streams — ignoring");
                return;
            }
            match self.active_streams.compare_exchange_weak(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(_) => continue,
            }
        }
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

/// QUIC connection pool supporting connection ID routing and stream multiplexing.
pub struct QuicPool {
    pools: DashMap<String, Vec<QuicPooledConnection>>,
    config: QuicPoolConfig,
    streams_acquired: AtomicU64,
    streams_exhausted: AtomicU64,
    evictions: AtomicU64,
}

impl QuicPool {
    /// Create a new QUIC pool with the given configuration.
    pub fn new(config: QuicPoolConfig) -> Self {
        Self {
            pools: DashMap::new(),
            config,
            streams_acquired: AtomicU64::new(0),
            streams_exhausted: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// Try to acquire a stream on an existing connection for the given node.
    ///
    /// Returns the connection id if a stream was successfully acquired.
    pub fn acquire_stream(&self, node_id: &str) -> Option<u64> {
        let pool = self.pools.get(node_id)?;
        for conn in pool.iter() {
            if conn.acquire_stream() {
                self.streams_acquired.fetch_add(1, Ordering::Relaxed);
                return Some(conn.id);
            }
        }
        self.streams_exhausted.fetch_add(1, Ordering::Relaxed);
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

    /// Look up a connection by its Destination Connection ID.
    ///
    /// This enables QUIC's connection ID routing: incoming packets are routed
    /// to the correct connection based on their DCID.
    pub fn find_by_dcid(&self, node_id: &str, dcid: &[u8]) -> Option<u64> {
        let pool = self.pools.get(node_id)?;
        pool.iter()
            .find(|c| c.dcid == *dcid)
            .map(|c| c.id)
    }

    /// Add a new connection for the given node.
    ///
    /// Returns `false` if the per-node limit has been reached.
    pub fn add_connection(&self, node_id: &str, conn: QuicPooledConnection) -> bool {
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
        self.evictions.fetch_add(total as u64, Ordering::Relaxed);
        total
    }

    /// Get current pool statistics.
    #[inline]
    pub fn stats(&self) -> QuicPoolStats {
        QuicPoolStats {
            streams_acquired: self.streams_acquired.load(Ordering::Relaxed),
            streams_exhausted: self.streams_exhausted.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }
}

impl Evictable for QuicPool {
    fn evict_expired(&self) -> usize {
        self.evict_aged()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_capacity() {
        let conn = QuicPooledConnection::new(1, &[1, 2, 3], &[4, 5, 6], 2);
        assert!(conn.has_capacity());

        assert!(conn.acquire_stream());
        assert!(conn.has_capacity());

        assert!(conn.acquire_stream());
        assert!(!conn.has_capacity());
        assert!(!conn.acquire_stream());

        conn.release_stream();
        assert!(conn.has_capacity());
    }

    #[test]
    fn dcid_routing() {
        let config = QuicPoolConfig::default();
        let pool = QuicPool::new(config);

        let dcid1 = [0x01, 0x02, 0x03];
        let dcid2 = [0x04, 0x05, 0x06];

        pool.add_connection(
            "node-a",
            QuicPooledConnection::new(1, &dcid1, &[0xAA], 100),
        );
        pool.add_connection(
            "node-a",
            QuicPooledConnection::new(2, &dcid2, &[0xBB], 100),
        );

        assert_eq!(pool.find_by_dcid("node-a", &dcid1), Some(1));
        assert_eq!(pool.find_by_dcid("node-a", &dcid2), Some(2));
        assert_eq!(pool.find_by_dcid("node-a", &[0xFF]), None);
    }

    #[test]
    fn max_connections_per_node() {
        let config = QuicPoolConfig {
            max_per_node: 2,
            max_concurrent_streams: 100,
            max_age: Duration::from_secs(300),
        };
        let pool = QuicPool::new(config);

        assert!(pool.add_connection("node-a", QuicPooledConnection::new(1, &[], &[], 100)));
        assert!(pool.add_connection("node-a", QuicPooledConnection::new(2, &[], &[], 100)));
        assert!(!pool.add_connection("node-a", QuicPooledConnection::new(3, &[], &[], 100)));

        assert_eq!(pool.connection_count("node-a"), 2);
    }

    #[test]
    fn evict_aged_idle_connections() {
        let config = QuicPoolConfig {
            max_per_node: 4,
            max_concurrent_streams: 100,
            max_age: Duration::from_millis(0),
        };
        let pool = QuicPool::new(config);

        pool.add_connection("node-a", QuicPooledConnection::new(1, &[], &[], 100));
        pool.add_connection("node-a", QuicPooledConnection::new(2, &[], &[], 100));

        let evicted = pool.evict_aged();
        assert_eq!(evicted, 2);
        assert_eq!(pool.connection_count("node-a"), 0);
    }

    #[test]
    fn evict_skips_active_connections() {
        let config = QuicPoolConfig {
            max_per_node: 4,
            max_concurrent_streams: 100,
            max_age: Duration::from_millis(0),
        };
        let pool = QuicPool::new(config);

        pool.add_connection("node-a", QuicPooledConnection::new(1, &[], &[], 100));
        pool.add_connection("node-a", QuicPooledConnection::new(2, &[], &[], 100));

        pool.acquire_stream("node-a");

        let evicted = pool.evict_aged();
        assert_eq!(evicted, 1);
        assert_eq!(pool.connection_count("node-a"), 1);
    }

    #[test]
    fn drain_node_removes_all() {
        let pool = QuicPool::new(QuicPoolConfig::default());

        pool.add_connection("node-a", QuicPooledConnection::new(1, &[], &[], 100));
        pool.add_connection("node-a", QuicPooledConnection::new(2, &[], &[], 100));

        let drained = pool.drain_node("node-a");
        assert_eq!(drained, 2);
        assert_eq!(pool.connection_count("node-a"), 0);
    }
}
