//! Backend node definition and management.
//!
//! [`NodeImpl`] is the concrete, lock-free backend node used on the data path.
//! All counters use atomic operations so a `NodeImpl` can be shared across
//! worker threads behind an `Arc` with zero contention on reads.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam::atomic::AtomicCell;
use serde::{Deserialize, Serialize};

use crate::health::Health;

/// States a node can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeState {
    /// Actively receiving traffic.
    Online,
    /// Marked offline by health check.
    Offline,
    /// Transitional state after restoration from offline (awaiting health check
    /// verification).
    Idle,
    /// Manually taken offline by operator.
    ManualOffline,
}

/// A backend node that can receive traffic.
pub trait Node: Send + Sync {
    /// Unique identifier for this node.
    fn id(&self) -> &str;

    /// Network address of this node.
    fn address(&self) -> SocketAddr;

    /// Current node state.
    fn state(&self) -> NodeState;

    /// Set the node state.
    fn set_state(&self, state: NodeState);

    /// Number of currently active connections.
    fn active_connections(&self) -> u64;

    /// Maximum allowed connections. `None` means unlimited.
    fn max_connections(&self) -> Option<u64>;

    /// Total bytes sent to this node.
    fn bytes_sent(&self) -> u64;

    /// Total bytes received from this node.
    fn bytes_received(&self) -> u64;

    /// Current health assessment.
    fn health(&self) -> Health;

    /// Weight for weighted load balancing.
    fn weight(&self) -> u32;

    /// Increment active connection count. Returns the new count.
    fn inc_connections(&self) -> u64;

    /// Decrement active connection count. Returns the new count.
    /// Saturates at zero -- never underflows.
    fn dec_connections(&self) -> u64;

    /// Add to bytes sent counter.
    fn add_bytes_sent(&self, bytes: u64);

    /// Add to bytes received counter.
    fn add_bytes_received(&self, bytes: u64);

    /// Check if this node can accept a new connection (atomically).
    fn try_acquire_connection(&self) -> bool;

    /// Whether the node is available for traffic.
    #[inline]
    fn is_online(&self) -> bool {
        self.state() == NodeState::Online
    }
}

/// Concrete implementation of [`Node`] with atomic counters.
///
/// All mutable state is behind atomics so no locks are required on the hot
/// path. The struct is `Send + Sync` by construction.
pub struct NodeImpl {
    id: String,
    address: SocketAddr,
    state: AtomicCell<NodeState>,
    active_connections: AtomicU64,
    max_connections: Option<u64>,
    weight: AtomicCell<u32>,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    health: AtomicCell<Health>,
}

impl NodeImpl {
    /// Create a new node with the given configuration.
    pub fn new(id: String, address: SocketAddr, weight: u32, max_connections: Option<u64>) -> Self {
        Self {
            id,
            address,
            state: AtomicCell::new(NodeState::Online),
            active_connections: AtomicU64::new(0),
            max_connections,
            weight: AtomicCell::new(weight),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            health: AtomicCell::new(Health::Unknown),
        }
    }

    /// Create a new node wrapped in `Arc`.
    pub fn new_arc(
        id: String,
        address: SocketAddr,
        weight: u32,
        max_connections: Option<u64>,
    ) -> Arc<Self> {
        Arc::new(Self::new(id, address, weight, max_connections))
    }

    /// Set the health state.
    #[inline]
    pub fn set_health(&self, health: Health) {
        self.health.store(health);
    }

    /// Set the weight.
    #[inline]
    pub fn set_weight(&self, weight: u32) {
        self.weight.store(weight);
    }
}

impl Node for NodeImpl {
    #[inline]
    fn id(&self) -> &str {
        &self.id
    }

    #[inline]
    fn address(&self) -> SocketAddr {
        self.address
    }

    #[inline]
    fn state(&self) -> NodeState {
        self.state.load()
    }

    #[inline]
    fn set_state(&self, state: NodeState) {
        self.state.store(state);
    }

    #[inline]
    fn active_connections(&self) -> u64 {
        self.active_connections.load(Ordering::Relaxed)
    }

    #[inline]
    fn max_connections(&self) -> Option<u64> {
        self.max_connections
    }

    #[inline]
    fn bytes_sent(&self) -> u64 {
        self.bytes_sent.load(Ordering::Relaxed)
    }

    #[inline]
    fn bytes_received(&self) -> u64 {
        self.bytes_received.load(Ordering::Relaxed)
    }

    #[inline]
    fn health(&self) -> Health {
        self.health.load()
    }

    #[inline]
    fn weight(&self) -> u32 {
        self.weight.load()
    }

    #[inline]
    fn inc_connections(&self) -> u64 {
        self.active_connections.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Decrement active connections, saturating at zero.
    ///
    /// Uses a CAS loop to prevent underflow when called more times than
    /// `inc_connections`.
    #[inline]
    fn dec_connections(&self) -> u64 {
        loop {
            let current = self.active_connections.load(Ordering::Acquire);
            if current == 0 {
                return 0;
            }
            if self
                .active_connections
                .compare_exchange_weak(
                    current,
                    current - 1,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return current - 1;
            }
        }
    }

    #[inline]
    fn add_bytes_sent(&self, bytes: u64) {
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    }

    #[inline]
    fn add_bytes_received(&self, bytes: u64) {
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
    }

    fn try_acquire_connection(&self) -> bool {
        if let Some(max) = self.max_connections {
            // CAS loop: atomically check capacity and increment.
            loop {
                let current = self.active_connections.load(Ordering::Acquire);
                if current >= max {
                    return false;
                }
                if self
                    .active_connections
                    .compare_exchange_weak(
                        current,
                        current + 1,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return true;
                }
            }
        } else {
            self.active_connections.fetch_add(1, Ordering::Relaxed);
            true
        }
    }
}

impl std::fmt::Debug for NodeImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeImpl")
            .field("id", &self.id)
            .field("address", &self.address)
            .field("state", &self.state.load())
            .field(
                "active_connections",
                &self.active_connections.load(Ordering::Relaxed),
            )
            .field("max_connections", &self.max_connections)
            .field("weight", &self.weight.load())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_state_transitions() {
        let node = NodeImpl::new(
            "test-1".into(),
            "127.0.0.1:8080".parse().unwrap(),
            1,
            Some(10),
        );
        assert_eq!(node.state(), NodeState::Online);
        assert!(node.is_online());

        node.set_state(NodeState::Offline);
        assert_eq!(node.state(), NodeState::Offline);
        assert!(!node.is_online());
    }

    #[test]
    fn test_connection_counting() {
        let node = NodeImpl::new("test-1".into(), "127.0.0.1:8080".parse().unwrap(), 1, None);
        assert_eq!(node.active_connections(), 0);

        assert_eq!(node.inc_connections(), 1);
        assert_eq!(node.inc_connections(), 2);
        assert_eq!(node.active_connections(), 2);

        assert_eq!(node.dec_connections(), 1);
        assert_eq!(node.active_connections(), 1);
    }

    #[test]
    fn test_dec_connections_saturates_at_zero() {
        let node = NodeImpl::new("test-1".into(), "127.0.0.1:8080".parse().unwrap(), 1, None);
        assert_eq!(node.active_connections(), 0);
        // Must not underflow.
        assert_eq!(node.dec_connections(), 0);
        assert_eq!(node.dec_connections(), 0);
        assert_eq!(node.active_connections(), 0);
    }

    #[test]
    fn test_try_acquire_connection_with_limit() {
        let node = NodeImpl::new(
            "test-1".into(),
            "127.0.0.1:8080".parse().unwrap(),
            1,
            Some(2),
        );

        assert!(node.try_acquire_connection());
        assert!(node.try_acquire_connection());
        // At limit now
        assert!(!node.try_acquire_connection());

        node.dec_connections();
        assert!(node.try_acquire_connection());
    }

    #[test]
    fn test_try_acquire_connection_unlimited() {
        let node = NodeImpl::new("test-1".into(), "127.0.0.1:8080".parse().unwrap(), 1, None);

        for _ in 0..1000 {
            assert!(node.try_acquire_connection());
        }
        assert_eq!(node.active_connections(), 1000);
    }

    #[test]
    fn test_bytes_tracking() {
        let node = NodeImpl::new("test-1".into(), "127.0.0.1:8080".parse().unwrap(), 1, None);
        node.add_bytes_sent(1024);
        node.add_bytes_sent(2048);
        assert_eq!(node.bytes_sent(), 3072);

        node.add_bytes_received(512);
        assert_eq!(node.bytes_received(), 512);
    }
}
