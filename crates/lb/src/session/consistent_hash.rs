//! Consistent-hash session persistence.
//!
//! Stateless session persistence that hashes the client source IP (without
//! port) onto a ring of backend nodes. No per-session storage is needed;
//! the ring deterministically maps clients to backends.
//!
//! Uses `ArcSwap` for lock-free reads on the hot path.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::net::IpAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use expressgateway_core::node::Node;
use expressgateway_core::session::SessionPersistence;
use parking_lot::Mutex;

/// Number of virtual nodes per real backend node.
const VIRTUAL_NODES: usize = 150;

/// Hash a byte string with Murmur3-x64-128.
fn murmur3_hash(data: &[u8]) -> u128 {
    murmur3::murmur3_x64_128(&mut Cursor::new(data), 0).unwrap_or(0)
}

/// Snapshot of ring + node list, swapped atomically.
struct RingSnapshot {
    ring: BTreeMap<u128, String>,
    nodes: Vec<Arc<dyn Node>>,
}

/// Consistent-hash session persistence.
///
/// This is a stateless implementation: it does not store any per-session data.
/// Instead, it hashes the client IP to a position on a ring and returns the
/// corresponding backend node ID. The ring is rebuilt when nodes are added or
/// removed.
pub struct ConsistentHashPersistence {
    snapshot: ArcSwap<RingSnapshot>,
    mutation_lock: Mutex<()>,
}

impl ConsistentHashPersistence {
    /// Create a new consistent-hash persistence with an empty ring.
    pub fn new() -> Self {
        Self {
            snapshot: ArcSwap::new(Arc::new(RingSnapshot {
                ring: BTreeMap::new(),
                nodes: Vec::new(),
            })),
            mutation_lock: Mutex::new(()),
        }
    }

    /// Add a backend node to the ring.
    pub fn add_node(&self, node: Arc<dyn Node>) {
        let _guard = self.mutation_lock.lock();
        let old = self.snapshot.load();
        let mut ring = old.ring.clone();
        let mut nodes = old.nodes.clone();

        let node_id = node.id().to_string();
        for i in 0..VIRTUAL_NODES {
            let vnode_key = format!("{}-vnode-{}", node_id, i);
            let hash = murmur3_hash(vnode_key.as_bytes());
            ring.insert(hash, node_id.clone());
        }
        nodes.push(node);

        self.snapshot.store(Arc::new(RingSnapshot { ring, nodes }));
    }

    /// Remove a backend node from the ring.
    pub fn remove_node(&self, node_id: &str) {
        let _guard = self.mutation_lock.lock();
        let old = self.snapshot.load();
        let mut ring = old.ring.clone();
        let mut nodes = old.nodes.clone();

        for i in 0..VIRTUAL_NODES {
            let vnode_key = format!("{}-vnode-{}", node_id, i);
            let hash = murmur3_hash(vnode_key.as_bytes());
            ring.remove(&hash);
        }
        nodes.retain(|n| n.id() != node_id);

        self.snapshot.store(Arc::new(RingSnapshot { ring, nodes }));
    }
}

impl Default for ConsistentHashPersistence {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionPersistence<IpAddr, String> for ConsistentHashPersistence {
    fn get(&self, key: &IpAddr) -> Option<String> {
        let snap = self.snapshot.load();
        if snap.ring.is_empty() {
            return None;
        }

        // Hash client IP (without port).
        let ip_str = key.to_string();
        let hash = murmur3_hash(ip_str.as_bytes());

        // Walk the ring clockwise, checking for online nodes.
        let candidates = snap.ring.range(hash..).chain(snap.ring.range(..hash));

        for (_, node_id) in candidates {
            if let Some(node) = snap.nodes.iter().find(|n| n.id() == node_id)
                && node.is_online()
            {
                return Some(node_id.clone());
            }
        }

        None
    }

    fn put(&self, _key: IpAddr, _value: String) {
        // Stateless: no per-session storage needed.
        // The ring deterministically maps clients to backends.
    }

    fn remove(&self, _key: &IpAddr) {
        // Stateless: nothing to remove.
    }

    fn len(&self) -> usize {
        // Return the number of nodes on the ring, not sessions.
        let snap = self.snapshot.load();
        snap.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::node::{NodeImpl, NodeState};

    fn make_node(id: &str, port: u16) -> Arc<NodeImpl> {
        NodeImpl::new_arc(id.to_string(), ([127, 0, 0, 1], port).into(), 1, None)
    }

    #[test]
    fn stable_mapping() {
        let store = ConsistentHashPersistence::new();
        store.add_node(make_node("n1", 8001));
        store.add_node(make_node("n2", 8002));
        store.add_node(make_node("n3", 8003));

        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let first = store.get(&ip).unwrap();

        for _ in 0..20 {
            assert_eq!(store.get(&ip).unwrap(), first);
        }
    }

    #[test]
    fn empty_ring_returns_none() {
        let store = ConsistentHashPersistence::new();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(store.get(&ip), None);
    }

    #[test]
    fn fallback_when_offline() {
        let store = ConsistentHashPersistence::new();
        let n1 = make_node("n1", 8001);
        let n2 = make_node("n2", 8002);
        store.add_node(n1.clone());
        store.add_node(n2.clone());

        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let original = store.get(&ip).unwrap();

        // Take the original node offline.
        if original == "n1" {
            n1.set_state(NodeState::Offline);
        } else {
            n2.set_state(NodeState::Offline);
        }

        let fallback = store.get(&ip).unwrap();
        assert_ne!(fallback, original);
    }

    #[test]
    fn put_and_remove_are_noop() {
        let store = ConsistentHashPersistence::new();
        store.add_node(make_node("n1", 8001));

        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        // put is a no-op.
        store.put(ip, "something".into());
        // remove is a no-op.
        store.remove(&ip);

        // Still works via the ring.
        assert!(store.get(&ip).is_some());
    }

    #[test]
    fn remove_node_from_ring() {
        let store = ConsistentHashPersistence::new();
        store.add_node(make_node("n1", 8001));
        store.add_node(make_node("n2", 8002));

        store.remove_node("n1");
        assert_eq!(store.len(), 1);

        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let result = store.get(&ip).unwrap();
        assert_eq!(result, "n2");
    }
}
