//! Weighted round-robin L4 load balancer (NGINX-style smooth).
//!
//! Produces a smooth, interleaved distribution of requests according to node
//! weights. For example, weights {5,1,1} yield the sequence A,A,B,A,C,A,A
//! rather than the clumpy A,A,A,A,A,B,C.
//!
//! The smooth WRR algorithm requires mutable state per selection, so a
//! `parking_lot::Mutex` is used. The critical section is O(n) with no
//! allocation and no syscalls, keeping lock hold time minimal.

use std::sync::Arc;

use expressgateway_core::error::{Error, Result};
use expressgateway_core::lb::{L4Request, L4Response, LoadBalancer};
use expressgateway_core::node::Node;
use parking_lot::Mutex;

/// Per-node state for the smooth weighted round-robin algorithm.
struct WeightedNode {
    node: Arc<dyn Node>,
    current_weight: i64,
    effective_weight: i64,
}

/// NGINX-style smooth weighted round-robin L4 load balancer.
///
/// Each selection step adds `effective_weight` to `current_weight`, picks the
/// node with the highest `current_weight`, and then subtracts `total_weight`
/// from the winner. This produces an interleaved distribution.
pub struct WeightedRoundRobinBalancer {
    state: Mutex<Vec<WeightedNode>>,
}

impl WeightedRoundRobinBalancer {
    /// Create a new weighted round-robin balancer with no nodes.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(Vec::new()),
        }
    }

    /// Update the effective weight for a specific node at runtime.
    pub fn set_weight(&self, node_id: &str, weight: i64) {
        let mut state = self.state.lock();
        if let Some(wn) = state.iter_mut().find(|wn| wn.node.id() == node_id) {
            wn.effective_weight = weight;
        }
    }
}

impl Default for WeightedRoundRobinBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer<L4Request, L4Response> for WeightedRoundRobinBalancer {
    fn select(&self, _request: &L4Request) -> Result<L4Response> {
        let mut state = self.state.lock();

        // Collect indices of online nodes with non-zero effective weight.
        let online_indices: Vec<usize> = state
            .iter()
            .enumerate()
            .filter(|(_, wn)| wn.node.is_online() && wn.effective_weight > 0)
            .map(|(i, _)| i)
            .collect();

        if online_indices.is_empty() {
            return Err(Error::NoHealthyBackend);
        }

        // Compute total weight of online nodes.
        let total_weight: i64 = online_indices
            .iter()
            .map(|&i| state[i].effective_weight)
            .sum();

        // Increase current_weight by effective_weight for each online node.
        for &i in &online_indices {
            state[i].current_weight += state[i].effective_weight;
        }

        // Select the node with the highest current_weight among online nodes.
        // Safe: online_indices is non-empty (checked above).
        let best_idx = *online_indices
            .iter()
            .max_by_key(|&&i| state[i].current_weight)
            .expect("online_indices is non-empty");

        // Subtract total_weight from the selected node.
        state[best_idx].current_weight -= total_weight;

        Ok(L4Response {
            node: state[best_idx].node.clone(),
        })
    }

    fn add_node(&self, node: Arc<dyn Node>) {
        let weight = node.weight() as i64;
        self.state.lock().push(WeightedNode {
            node,
            current_weight: 0,
            effective_weight: weight,
        });
    }

    fn remove_node(&self, node_id: &str) {
        self.state.lock().retain(|wn| wn.node.id() != node_id);
    }

    fn online_nodes(&self) -> Vec<Arc<dyn Node>> {
        self.state
            .lock()
            .iter()
            .filter(|wn| wn.node.is_online())
            .map(|wn| wn.node.clone())
            .collect()
    }

    fn all_nodes(&self) -> Vec<Arc<dyn Node>> {
        self.state.lock().iter().map(|wn| wn.node.clone()).collect()
    }

    fn get_node(&self, node_id: &str) -> Option<Arc<dyn Node>> {
        self.state
            .lock()
            .iter()
            .find(|wn| wn.node.id() == node_id)
            .map(|wn| wn.node.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::node::NodeImpl;

    fn make_node(id: &str, port: u16, weight: u32) -> Arc<dyn Node> {
        NodeImpl::new_arc(id.to_string(), ([127, 0, 0, 1], port).into(), weight, None)
    }

    #[test]
    fn smooth_distribution() {
        let lb = WeightedRoundRobinBalancer::new();
        lb.add_node(make_node("A", 8001, 5));
        lb.add_node(make_node("B", 8002, 1));
        lb.add_node(make_node("C", 8003, 1));

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };

        let mut ids: Vec<String> = Vec::new();
        for _ in 0..7 {
            let resp = lb.select(&req).unwrap();
            ids.push(resp.node.id().to_string());
        }

        // Should produce smooth interleaved: A appears 5 times, B once, C once.
        let a_count = ids.iter().filter(|id| *id == "A").count();
        let b_count = ids.iter().filter(|id| *id == "B").count();
        let c_count = ids.iter().filter(|id| *id == "C").count();
        assert_eq!(a_count, 5);
        assert_eq!(b_count, 1);
        assert_eq!(c_count, 1);

        // Verify interleaving: A should not appear in 5 consecutive positions.
        let consecutive_a = ids.windows(5).any(|w| w.iter().all(|id| id == "A"));
        assert!(
            !consecutive_a,
            "Distribution should be smooth, not clumpy: {:?}",
            ids
        );
    }

    #[test]
    fn equal_weights_round_robin() {
        let lb = WeightedRoundRobinBalancer::new();
        lb.add_node(make_node("A", 8001, 1));
        lb.add_node(make_node("B", 8002, 1));
        lb.add_node(make_node("C", 8003, 1));

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };

        let mut ids: Vec<String> = Vec::new();
        for _ in 0..6 {
            let resp = lb.select(&req).unwrap();
            ids.push(resp.node.id().to_string());
        }

        // Equal weights should give each node 2 out of 6 selections.
        let a = ids.iter().filter(|id| *id == "A").count();
        let b = ids.iter().filter(|id| *id == "B").count();
        let c = ids.iter().filter(|id| *id == "C").count();
        assert_eq!(a, 2);
        assert_eq!(b, 2);
        assert_eq!(c, 2);
    }

    #[test]
    fn runtime_weight_update() {
        let lb = WeightedRoundRobinBalancer::new();
        lb.add_node(make_node("A", 8001, 1));
        lb.add_node(make_node("B", 8002, 1));

        lb.set_weight("A", 3);

        let req = L4Request {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
        };

        let mut a_count = 0;
        for _ in 0..4 {
            let resp = lb.select(&req).unwrap();
            if resp.node.id() == "A" {
                a_count += 1;
            }
        }
        assert_eq!(a_count, 3);
    }
}
