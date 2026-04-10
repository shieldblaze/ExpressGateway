//! Consistent-hash L4 load balancer.
//!
//! Maps client socket addresses onto a hash ring backed by a `BTreeMap`.
//! Each real node gets 150 virtual nodes placed on the ring using Murmur3-128.
//! Selection walks the ring clockwise to find the next online node, providing
//! minimal disruption when nodes are added or removed.
//!
//! Both the ring and the node list are swapped atomically via `ArcSwap`,
//! giving lock-free reads on the hot path.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use arc_swap::ArcSwap;
use expressgateway_core::error::{Error, Result};
use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use expressgateway_core::node::Node;
use parking_lot::Mutex;

/// Number of virtual nodes per real backend node on the hash ring.
const VIRTUAL_NODES: usize = 150;

/// Snapshot of ring + node list, swapped atomically.
struct RingSnapshot {
    ring: BTreeMap<u128, String>,
    nodes: Vec<Arc<dyn Node>>,
}

/// Consistent-hash L4 load balancer.
///
/// Uses a Murmur3-128 hash ring with 150 virtual nodes per backend. The client
/// socket address is hashed to locate a position on the ring, then the ring is
/// walked clockwise to find the first online node.
pub struct ConsistentHashBalancer {
    /// Current ring snapshot, read lock-free via ArcSwap.
    snapshot: ArcSwap<RingSnapshot>,
    /// Mutex serialises mutations (add/remove) to avoid lost updates.
    mutation_lock: Mutex<()>,
}

impl ConsistentHashBalancer {
    /// Create a new consistent-hash balancer with an empty ring.
    pub fn new() -> Self {
        Self {
            snapshot: ArcSwap::new(Arc::new(RingSnapshot {
                ring: BTreeMap::new(),
                nodes: Vec::new(),
            })),
            mutation_lock: Mutex::new(()),
        }
    }
}

impl Default for ConsistentHashBalancer {
    fn default() -> Self {
        Self::new()
    }
}

/// Hash a byte string with Murmur3-x64-128.
fn murmur3_hash(data: &[u8]) -> u128 {
    murmur3::murmur3_x64_128(&mut Cursor::new(data), 0).unwrap_or(0)
}

impl LoadBalancer<L4Request, L4Response> for ConsistentHashBalancer {
    fn select(&self, request: &L4Request) -> Result<L4Response> {
        let snap = self.snapshot.load();

        if snap.ring.is_empty() || snap.nodes.is_empty() {
            return Err(Error::NoHealthyBackend);
        }

        let key = request.client_addr.to_string();
        let hash = murmur3_hash(key.as_bytes());

        // Walk the ring clockwise from the hash point, wrapping around.
        let candidates = snap.ring.range(hash..).chain(snap.ring.range(..hash));

        for (_ring_hash, node_id) in candidates {
            if let Some(node) = snap.nodes.iter().find(|n| n.id() == node_id)
                && node.is_online()
            {
                return Ok(L4Response { node: node.clone() });
            }
        }

        Err(Error::NoHealthyBackend)
    }

    fn add_node(&self, node: Arc<dyn Node>) {
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

    fn remove_node(&self, node_id: &str) {
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

    fn online_nodes(&self) -> Vec<Arc<dyn Node>> {
        let snap = self.snapshot.load();
        snap.nodes
            .iter()
            .filter(|n| n.is_online())
            .cloned()
            .collect()
    }

    fn all_nodes(&self) -> Vec<Arc<dyn Node>> {
        let snap = self.snapshot.load();
        snap.nodes.clone()
    }

    fn get_node(&self, node_id: &str) -> Option<Arc<dyn Node>> {
        let snap = self.snapshot.load();
        snap.nodes.iter().find(|n| n.id() == node_id).cloned()
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
        let lb = ConsistentHashBalancer::new();
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));
        lb.add_node(make_node("n3", 8003));

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };

        let first = lb.select(&req).unwrap().node.id().to_string();
        // Same request should always map to the same node.
        for _ in 0..20 {
            let resp = lb.select(&req).unwrap();
            assert_eq!(resp.node.id(), first);
        }
    }

    #[test]
    fn different_clients_may_differ() {
        let lb = ConsistentHashBalancer::new();
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));
        lb.add_node(make_node("n3", 8003));

        let mut seen = std::collections::HashSet::new();
        // Try many different client addresses.
        for port in 1000..1100 {
            let req = L4Request {
                client_addr: format!("10.0.0.{}:{}", port % 256, port).parse().unwrap(),
            };
            let resp = lb.select(&req).unwrap();
            seen.insert(resp.node.id().to_string());
        }
        // With 100 different clients and 3 nodes, we should hit at least 2 nodes.
        assert!(
            seen.len() >= 2,
            "Expected distribution across nodes: {:?}",
            seen
        );
    }

    #[test]
    fn fallback_on_offline_node() {
        let lb = ConsistentHashBalancer::new();
        let n1 = make_node("n1", 8001);
        let n2 = make_node("n2", 8002);
        lb.add_node(n1.clone());
        lb.add_node(n2.clone());

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };

        let original = lb.select(&req).unwrap().node.id().to_string();

        // Take the chosen node offline.
        if original == "n1" {
            n1.set_state(NodeState::Offline);
        } else {
            n2.set_state(NodeState::Offline);
        }

        // Should fall back to the other node.
        let resp = lb.select(&req).unwrap();
        assert_ne!(resp.node.id(), original);
    }

    #[test]
    fn empty_ring_returns_error() {
        let lb = ConsistentHashBalancer::new();
        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        assert!(lb.select(&req).is_err());
    }

    #[test]
    fn remove_node_from_ring() {
        let lb = ConsistentHashBalancer::new();
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));

        lb.remove_node("n1");
        assert_eq!(lb.all_nodes().len(), 1);

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        let resp = lb.select(&req).unwrap();
        assert_eq!(resp.node.id(), "n2");
    }
}
