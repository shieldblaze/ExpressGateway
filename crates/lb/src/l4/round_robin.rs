//! Round-robin L4 load balancer.
//!
//! Uses an atomic counter to cycle through online backend nodes in order,
//! providing lock-free selection. Backend list updates use `ArcSwap` for
//! wait-free reads on the hot path.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arc_swap::ArcSwap;
use expressgateway_core::error::{Error, Result};
use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use expressgateway_core::node::Node;

/// Round-robin L4 load balancer.
///
/// Distributes requests evenly across all online backend nodes using a simple
/// rotating counter. Selection is lock-free: the node list is read via
/// `ArcSwap` (a single atomic load) and the index advances via `AtomicUsize`.
pub struct RoundRobinBalancer {
    nodes: ArcSwap<Vec<Arc<dyn Node>>>,
    index: AtomicUsize,
}

impl RoundRobinBalancer {
    /// Create a new round-robin balancer with no nodes.
    pub fn new() -> Self {
        Self {
            nodes: ArcSwap::new(Arc::new(Vec::new())),
            index: AtomicUsize::new(0),
        }
    }
}

impl Default for RoundRobinBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer<L4Request, L4Response> for RoundRobinBalancer {
    fn select(&self, _request: &L4Request) -> Result<L4Response> {
        let nodes = self.nodes.load();

        // Count online nodes and find the one at our index without allocating.
        let online_count = nodes.iter().filter(|n| n.is_online()).count();
        if online_count == 0 {
            return Err(Error::NoHealthyBackend);
        }

        let idx = self.index.fetch_add(1, Ordering::Relaxed) % online_count;
        let node = nodes
            .iter()
            .filter(|n| n.is_online())
            .nth(idx)
            .expect("online_count > 0 guarantees nth(idx) exists");

        Ok(L4Response {
            node: node.clone(),
        })
    }

    fn add_node(&self, node: Arc<dyn Node>) {
        let old = self.nodes.load();
        let mut new_nodes = (**old).clone();
        new_nodes.push(node);
        self.nodes.store(Arc::new(new_nodes));
    }

    fn remove_node(&self, node_id: &str) {
        let old = self.nodes.load();
        let mut new_nodes = (**old).clone();
        new_nodes.retain(|n| n.id() != node_id);
        self.nodes.store(Arc::new(new_nodes));
    }

    fn online_nodes(&self) -> Vec<Arc<dyn Node>> {
        let nodes = self.nodes.load();
        nodes.iter().filter(|n| n.is_online()).cloned().collect()
    }

    fn all_nodes(&self) -> Vec<Arc<dyn Node>> {
        let nodes = self.nodes.load();
        (**nodes).clone()
    }

    fn get_node(&self, node_id: &str) -> Option<Arc<dyn Node>> {
        let nodes = self.nodes.load();
        nodes.iter().find(|n| n.id() == node_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::node::{NodeImpl, NodeState};

    fn make_node(id: &str, port: u16) -> Arc<dyn Node> {
        NodeImpl::new_arc(id.to_string(), ([127, 0, 0, 1], port).into(), 1, None)
    }

    #[test]
    fn cycles_through_nodes() {
        let lb = RoundRobinBalancer::new();
        let n1 = make_node("n1", 8001);
        let n2 = make_node("n2", 8002);
        let n3 = make_node("n3", 8003);
        lb.add_node(n1);
        lb.add_node(n2);
        lb.add_node(n3);

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };

        let mut ids: Vec<String> = Vec::new();
        for _ in 0..9 {
            let resp = lb.select(&req).unwrap();
            ids.push(resp.node.id().to_string());
        }
        // Should cycle: n1 n2 n3 n1 n2 n3 n1 n2 n3
        assert_eq!(
            ids,
            vec!["n1", "n2", "n3", "n1", "n2", "n3", "n1", "n2", "n3"]
        );
    }

    #[test]
    fn skips_offline_nodes() {
        let lb = RoundRobinBalancer::new();
        let n1 = NodeImpl::new_arc("n1".into(), ([127, 0, 0, 1], 8001).into(), 1, None);
        let n2 = NodeImpl::new_arc("n2".into(), ([127, 0, 0, 1], 8002).into(), 1, None);
        n2.set_state(NodeState::Offline);
        lb.add_node(n1);
        lb.add_node(n2 as Arc<dyn Node>);

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        for _ in 0..5 {
            let resp = lb.select(&req).unwrap();
            assert_eq!(resp.node.id(), "n1");
        }
    }

    #[test]
    fn no_nodes_returns_error() {
        let lb = RoundRobinBalancer::new();
        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        assert!(lb.select(&req).is_err());
    }

    #[test]
    fn remove_node_works() {
        let lb = RoundRobinBalancer::new();
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));
        assert_eq!(lb.all_nodes().len(), 2);
        lb.remove_node("n1");
        assert_eq!(lb.all_nodes().len(), 1);
        assert_eq!(lb.all_nodes()[0].id(), "n2");
    }
}
