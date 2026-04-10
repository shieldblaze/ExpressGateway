//! Least-connection L4 load balancer.
//!
//! Selects the online backend node with the fewest active connections via a
//! simple O(n) scan. Uses `ArcSwap` for lock-free reads on the hot path.

use std::sync::Arc;

use arc_swap::ArcSwap;
use expressgateway_core::error::{Error, Result};
use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use expressgateway_core::node::Node;

/// Least-connection L4 load balancer.
///
/// On each selection, scans all online nodes and picks the one with the
/// lowest `active_connections()` count. The scan is lock-free (reads
/// through `ArcSwap`) and allocation-free.
pub struct LeastConnectionBalancer {
    nodes: ArcSwap<Vec<Arc<dyn Node>>>,
}

impl LeastConnectionBalancer {
    /// Create a new least-connection balancer with no nodes.
    pub fn new() -> Self {
        Self {
            nodes: ArcSwap::new(Arc::new(Vec::new())),
        }
    }
}

impl Default for LeastConnectionBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer<L4Request, L4Response> for LeastConnectionBalancer {
    fn select(&self, _request: &L4Request) -> Result<L4Response> {
        let nodes = self.nodes.load();
        let best = nodes
            .iter()
            .filter(|n| n.is_online())
            .min_by_key(|n| n.active_connections())
            .cloned();

        match best {
            Some(node) => Ok(L4Response { node }),
            None => Err(Error::NoHealthyBackend),
        }
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

    fn make_node(id: &str, port: u16) -> Arc<NodeImpl> {
        NodeImpl::new_arc(id.to_string(), ([127, 0, 0, 1], port).into(), 1, None)
    }

    #[test]
    fn picks_lowest_connections() {
        let lb = LeastConnectionBalancer::new();
        let n1 = make_node("n1", 8001);
        let n2 = make_node("n2", 8002);
        let n3 = make_node("n3", 8003);

        // Give n1 5 connections, n2 2 connections, n3 0 connections.
        for _ in 0..5 {
            n1.inc_connections();
        }
        for _ in 0..2 {
            n2.inc_connections();
        }

        lb.add_node(n1);
        lb.add_node(n2);
        lb.add_node(n3);

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        let resp = lb.select(&req).unwrap();
        assert_eq!(resp.node.id(), "n3");
    }

    #[test]
    fn updates_after_connections_change() {
        let lb = LeastConnectionBalancer::new();
        let n1 = make_node("n1", 8001);
        let n2 = make_node("n2", 8002);

        n1.inc_connections();
        lb.add_node(n1.clone() as Arc<dyn Node>);
        lb.add_node(n2.clone() as Arc<dyn Node>);

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };

        // n2 has fewer connections.
        let resp = lb.select(&req).unwrap();
        assert_eq!(resp.node.id(), "n2");

        // Now give n2 more connections.
        n2.inc_connections();
        n2.inc_connections();
        n1.dec_connections();

        let resp = lb.select(&req).unwrap();
        assert_eq!(resp.node.id(), "n1");
    }

    #[test]
    fn no_online_nodes() {
        let lb = LeastConnectionBalancer::new();
        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        assert!(lb.select(&req).is_err());
    }
}
