//! Random L4 load balancer.
//!
//! Selects a random online backend node using a thread-local RNG.
//! Uses `ArcSwap` for lock-free reads on the hot path.

use std::sync::Arc;

use arc_swap::ArcSwap;
use expressgateway_core::error::{Error, Result};
use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use expressgateway_core::node::Node;
use rand::Rng;

/// Random L4 load balancer.
///
/// Each selection picks a uniformly random online node. The node list is
/// read lock-free via `ArcSwap` and the RNG is thread-local (no
/// synchronisation).
pub struct RandomBalancer {
    nodes: ArcSwap<Vec<Arc<dyn Node>>>,
}

impl RandomBalancer {
    /// Create a new random balancer with no nodes.
    pub fn new() -> Self {
        Self {
            nodes: ArcSwap::new(Arc::new(Vec::new())),
        }
    }
}

impl Default for RandomBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer<L4Request, L4Response> for RandomBalancer {
    fn select(&self, _request: &L4Request) -> Result<L4Response> {
        let nodes = self.nodes.load();

        // Count online nodes and pick a random one without allocating.
        let online_count = nodes.iter().filter(|n| n.is_online()).count();
        if online_count == 0 {
            return Err(Error::NoHealthyBackend);
        }

        let idx = rand::thread_rng().gen_range(0..online_count);
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
    use expressgateway_core::node::NodeImpl;
    use std::collections::HashSet;

    fn make_node(id: &str, port: u16) -> Arc<dyn Node> {
        NodeImpl::new_arc(id.to_string(), ([127, 0, 0, 1], port).into(), 1, None)
    }

    #[test]
    fn selects_from_online_nodes() {
        let lb = RandomBalancer::new();
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));
        lb.add_node(make_node("n3", 8003));

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };

        // After many iterations, all nodes should be selected at least once.
        let mut seen = HashSet::new();
        for _ in 0..100 {
            let resp = lb.select(&req).unwrap();
            seen.insert(resp.node.id().to_string());
        }
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn single_node_always_selected() {
        let lb = RandomBalancer::new();
        lb.add_node(make_node("only", 8001));

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        for _ in 0..10 {
            let resp = lb.select(&req).unwrap();
            assert_eq!(resp.node.id(), "only");
        }
    }

    #[test]
    fn empty_returns_error() {
        let lb = RandomBalancer::new();
        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        assert!(lb.select(&req).is_err());
    }
}
