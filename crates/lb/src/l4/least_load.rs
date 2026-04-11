//! Least-load L4 load balancer.
//!
//! Selects the online backend node with the lowest load percentage, defined as
//! `active_connections / max_connections`. Ties are broken by absolute
//! connection count. Uses `ArcSwap` for lock-free reads on the hot path.

use std::sync::Arc;

use arc_swap::ArcSwap;
use expressgateway_core::error::{Error, Result};
use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use expressgateway_core::node::Node;
use parking_lot::Mutex;

/// Least-load L4 load balancer.
///
/// Computes load as the ratio of active connections to max connections for each
/// node. Nodes without a connection limit are treated as having 0% load. Ties
/// are broken by the absolute connection count.
pub struct LeastLoadBalancer {
    nodes: ArcSwap<Vec<Arc<dyn Node>>>,
    /// Serialises add/remove to prevent lost updates from concurrent mutations.
    mutation_lock: Mutex<()>,
}

impl LeastLoadBalancer {
    /// Create a new least-load balancer with no nodes.
    pub fn new() -> Self {
        Self {
            nodes: ArcSwap::new(Arc::new(Vec::new())),
            mutation_lock: Mutex::new(()),
        }
    }
}

impl Default for LeastLoadBalancer {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the load percentage (0.0 to 1.0) for a node.
fn load_pct(node: &dyn Node) -> f64 {
    match node.max_connections() {
        Some(max) if max > 0 => node.active_connections() as f64 / max as f64,
        _ => 0.0, // No limit means effectively no load pressure.
    }
}

impl LoadBalancer<L4Request, L4Response> for LeastLoadBalancer {
    fn select(&self, _request: &L4Request) -> Result<L4Response> {
        let nodes = self.nodes.load();
        let best = nodes
            .iter()
            .filter(|n| n.is_online())
            .min_by(|a, b| {
                let la = load_pct(a.as_ref());
                let lb_val = load_pct(b.as_ref());
                la.partial_cmp(&lb_val)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.active_connections().cmp(&b.active_connections()))
            })
            .cloned();

        match best {
            Some(node) => Ok(L4Response { node }),
            None => Err(Error::NoHealthyBackend),
        }
    }

    fn add_node(&self, node: Arc<dyn Node>) {
        let _guard = self.mutation_lock.lock();
        let old = self.nodes.load();
        let mut new_nodes = (**old).clone();
        new_nodes.push(node);
        self.nodes.store(Arc::new(new_nodes));
    }

    fn remove_node(&self, node_id: &str) {
        let _guard = self.mutation_lock.lock();
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

    #[test]
    fn picks_lowest_load_percentage() {
        let lb = LeastLoadBalancer::new();

        // n1: 8/10 = 80% load
        let n1 = NodeImpl::new_arc("n1".into(), ([127, 0, 0, 1], 8001).into(), 1, Some(10));
        for _ in 0..8 {
            n1.inc_connections();
        }

        // n2: 5/100 = 5% load
        let n2 = NodeImpl::new_arc("n2".into(), ([127, 0, 0, 1], 8002).into(), 1, Some(100));
        for _ in 0..5 {
            n2.inc_connections();
        }

        // n3: 3/10 = 30% load
        let n3 = NodeImpl::new_arc("n3".into(), ([127, 0, 0, 1], 8003).into(), 1, Some(10));
        for _ in 0..3 {
            n3.inc_connections();
        }

        lb.add_node(n1);
        lb.add_node(n2);
        lb.add_node(n3);

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        let resp = lb.select(&req).unwrap();
        assert_eq!(resp.node.id(), "n2");
    }

    #[test]
    fn tiebreaker_by_connection_count() {
        let lb = LeastLoadBalancer::new();

        // Both have 0% load (no limit), but different connection counts.
        let n1 = NodeImpl::new_arc("n1".into(), ([127, 0, 0, 1], 8001).into(), 1, None);
        for _ in 0..5 {
            n1.inc_connections();
        }
        let n2 = NodeImpl::new_arc("n2".into(), ([127, 0, 0, 1], 8002).into(), 1, None);
        n2.inc_connections();

        lb.add_node(n1);
        lb.add_node(n2);

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        let resp = lb.select(&req).unwrap();
        assert_eq!(resp.node.id(), "n2");
    }

    #[test]
    fn unlimited_nodes_have_zero_load() {
        let lb = LeastLoadBalancer::new();

        // n1: 50% load (5/10)
        let n1 = NodeImpl::new_arc("n1".into(), ([127, 0, 0, 1], 8001).into(), 1, Some(10));
        for _ in 0..5 {
            n1.inc_connections();
        }

        // n2: unlimited, 10 connections, treated as 0% load.
        let n2 = NodeImpl::new_arc("n2".into(), ([127, 0, 0, 1], 8002).into(), 1, None);
        for _ in 0..10 {
            n2.inc_connections();
        }

        lb.add_node(n1);
        lb.add_node(n2);

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };
        let resp = lb.select(&req).unwrap();
        assert_eq!(resp.node.id(), "n2");
    }
}
