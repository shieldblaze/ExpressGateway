//! Least response time (EWMA) HTTP load balancer.
//!
//! Tracks per-node response time using an exponentially weighted moving
//! average (EWMA). Lock-free EWMA updates are performed via CAS loops on
//! `AtomicU64`, storing `f64` bits directly.
//!
//! Three-tier cold-start handling:
//! 1. All nodes cold (< 10 samples): round-robin to gather data.
//! 2. Some nodes cold: prefer cold nodes so they gather samples.
//! 3. All nodes warm (>= 10 samples): select node with lowest EWMA.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use expressgateway_core::error::{Error, Result};
use expressgateway_core::lb::{HttpRequest, HttpResponse, LoadBalancer};
use expressgateway_core::node::Node;
use parking_lot::RwLock;

/// Number of samples before EWMA kicks in.
const COLD_THRESHOLD: u64 = 10;

/// Per-node EWMA state, stored in a DashMap for O(1) lookups.
struct NodeEwma {
    /// EWMA value stored as f64 bits in an AtomicU64 for lock-free CAS updates.
    ewma_bits: AtomicU64,
    /// Number of recorded samples.
    sample_count: AtomicU64,
}

impl NodeEwma {
    fn new() -> Self {
        Self {
            ewma_bits: AtomicU64::new(0.0f64.to_bits()),
            sample_count: AtomicU64::new(0),
        }
    }

    fn ewma(&self) -> f64 {
        f64::from_bits(self.ewma_bits.load(Ordering::Acquire))
    }

    fn samples(&self) -> u64 {
        self.sample_count.load(Ordering::Acquire)
    }

    fn is_warm(&self) -> bool {
        self.samples() >= COLD_THRESHOLD
    }

    /// Update EWMA with a new sample using a CAS loop.
    fn update(&self, value_ms: f64, alpha: f64) {
        self.sample_count.fetch_add(1, Ordering::AcqRel);

        loop {
            let old_bits = self.ewma_bits.load(Ordering::Acquire);
            let old_val = f64::from_bits(old_bits);

            let new_val = if old_val == 0.0 {
                value_ms
            } else {
                alpha * value_ms + (1.0 - alpha) * old_val
            };

            let new_bits = new_val.to_bits();
            match self.ewma_bits.compare_exchange_weak(
                old_bits,
                new_bits,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }
    }
}

/// EWMA-based HTTP load balancer that selects the node with the lowest
/// estimated response time.
///
/// # Cold-start behaviour
///
/// Nodes with fewer than 10 recorded samples are considered "cold". When all
/// nodes are cold, round-robin is used. When some are cold, cold nodes are
/// preferred so they can gather samples. Once all are warm, the node with the
/// lowest EWMA is selected.
pub struct EwmaBalancer {
    nodes: RwLock<Vec<Arc<dyn Node>>>,
    ewma_state: DashMap<String, Arc<NodeEwma>>,
    /// EWMA smoothing factor (0.0 - 1.0).
    alpha: f64,
    /// Round-robin index for cold-start phase.
    rr_index: AtomicUsize,
}

impl EwmaBalancer {
    /// Create a new EWMA balancer with the default alpha of 0.5.
    pub fn new() -> Self {
        Self::with_alpha(0.5)
    }

    /// Create a new EWMA balancer with a custom alpha parameter.
    pub fn with_alpha(alpha: f64) -> Self {
        Self {
            nodes: RwLock::new(Vec::new()),
            ewma_state: DashMap::new(),
            alpha,
            rr_index: AtomicUsize::new(0),
        }
    }

    /// Record a response time observation for a specific node.
    ///
    /// This should be called after each backend response is received.
    pub fn record_response_time(&self, node_id: &str, duration: Duration) {
        let ms = duration.as_secs_f64() * 1000.0;
        if let Some(state) = self.ewma_state.get(node_id) {
            state.update(ms, self.alpha);
        }
    }

    /// Get the current EWMA value for a node (in milliseconds), if tracked.
    pub fn get_ewma(&self, node_id: &str) -> Option<f64> {
        self.ewma_state.get(node_id).map(|s| s.ewma())
    }

    /// Get the sample count for a node.
    pub fn get_sample_count(&self, node_id: &str) -> Option<u64> {
        self.ewma_state.get(node_id).map(|s| s.samples())
    }
}

impl Default for EwmaBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer<HttpRequest, HttpResponse> for EwmaBalancer {
    fn select(&self, _request: &HttpRequest) -> Result<HttpResponse> {
        let nodes = self.nodes.read();
        let online: Vec<_> = nodes.iter().filter(|n| n.is_online()).cloned().collect();
        if online.is_empty() {
            return Err(Error::NoHealthyBackend);
        }

        // Classify nodes as cold or warm.
        let mut cold: Vec<&Arc<dyn Node>> = Vec::new();
        let mut warm: Vec<(&Arc<dyn Node>, f64)> = Vec::new();

        for node in &online {
            match self.ewma_state.get(node.id()) {
                Some(state) if state.is_warm() => {
                    warm.push((node, state.ewma()));
                }
                _ => {
                    cold.push(node);
                }
            }
        }

        let selected = if warm.is_empty() {
            // All cold: round-robin.
            let idx = self.rr_index.fetch_add(1, Ordering::Relaxed) % online.len();
            online[idx].clone()
        } else if !cold.is_empty() {
            // Some cold: prefer cold nodes to gather samples.
            let idx = self.rr_index.fetch_add(1, Ordering::Relaxed) % cold.len();
            cold[idx].clone()
        } else {
            // All warm: pick lowest EWMA.
            warm.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            warm[0].0.clone()
        };

        Ok(HttpResponse {
            node: selected,
            headers_to_add: Vec::new(),
        })
    }

    fn add_node(&self, node: Arc<dyn Node>) {
        let id = node.id().to_string();
        self.ewma_state.insert(id, Arc::new(NodeEwma::new()));
        self.nodes.write().push(node);
    }

    fn remove_node(&self, node_id: &str) {
        self.ewma_state.remove(node_id);
        self.nodes.write().retain(|n| n.id() != node_id);
    }

    fn online_nodes(&self) -> Vec<Arc<dyn Node>> {
        self.nodes
            .read()
            .iter()
            .filter(|n| n.is_online())
            .cloned()
            .collect()
    }

    fn all_nodes(&self) -> Vec<Arc<dyn Node>> {
        self.nodes.read().clone()
    }

    fn get_node(&self, node_id: &str) -> Option<Arc<dyn Node>> {
        self.nodes
            .read()
            .iter()
            .find(|n| n.id() == node_id)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::node::NodeImpl;

    fn make_node(id: &str, port: u16) -> Arc<dyn Node> {
        NodeImpl::new_arc(id.to_string(), ([127, 0, 0, 1], port).into(), 1, None)
    }

    fn make_request() -> HttpRequest {
        HttpRequest {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
            host: Some("example.com".into()),
            path: "/".into(),
            headers: Vec::new(),
        }
    }

    #[test]
    fn cold_start_round_robin() {
        let lb = EwmaBalancer::new();
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));
        lb.add_node(make_node("n3", 8003));

        // No samples recorded yet; should round-robin.
        let mut ids: Vec<String> = Vec::new();
        for _ in 0..6 {
            let resp = lb.select(&make_request()).unwrap();
            ids.push(resp.node.id().to_string());
        }
        // Each node should appear twice in 6 selections.
        assert_eq!(ids.iter().filter(|id| *id == "n1").count(), 2);
        assert_eq!(ids.iter().filter(|id| *id == "n2").count(), 2);
        assert_eq!(ids.iter().filter(|id| *id == "n3").count(), 2);
    }

    #[test]
    fn warm_nodes_prefer_lowest_ewma() {
        let lb = EwmaBalancer::new();
        lb.add_node(make_node("fast", 8001));
        lb.add_node(make_node("slow", 8002));

        // Warm up both nodes.
        for _ in 0..COLD_THRESHOLD {
            lb.record_response_time("fast", Duration::from_millis(10));
            lb.record_response_time("slow", Duration::from_millis(100));
        }

        // Should consistently pick the faster node.
        for _ in 0..10 {
            let resp = lb.select(&make_request()).unwrap();
            assert_eq!(resp.node.id(), "fast");
        }
    }

    #[test]
    fn prefers_cold_nodes_when_mixed() {
        let lb = EwmaBalancer::new();
        lb.add_node(make_node("warm", 8001));
        lb.add_node(make_node("cold", 8002));

        // Warm up only one node.
        for _ in 0..COLD_THRESHOLD {
            lb.record_response_time("warm", Duration::from_millis(50));
        }

        // Should prefer the cold node.
        for _ in 0..5 {
            let resp = lb.select(&make_request()).unwrap();
            assert_eq!(resp.node.id(), "cold");
        }
    }

    #[test]
    fn ewma_converges() {
        let lb = EwmaBalancer::new();
        lb.add_node(make_node("n1", 8001));

        // Record increasing response times.
        for i in 1..=20 {
            lb.record_response_time("n1", Duration::from_millis(i * 10));
        }

        let ewma = lb.get_ewma("n1").unwrap();
        // EWMA with alpha=0.5 should be between the first and last values.
        assert!(ewma > 10.0, "EWMA should be > 10ms: {}", ewma);
        assert!(ewma < 200.0, "EWMA should be < 200ms: {}", ewma);
    }

    #[test]
    fn sample_count_tracks() {
        let lb = EwmaBalancer::new();
        lb.add_node(make_node("n1", 8001));

        assert_eq!(lb.get_sample_count("n1"), Some(0));
        lb.record_response_time("n1", Duration::from_millis(10));
        assert_eq!(lb.get_sample_count("n1"), Some(1));
        for _ in 0..9 {
            lb.record_response_time("n1", Duration::from_millis(10));
        }
        assert_eq!(lb.get_sample_count("n1"), Some(10));
    }

    #[test]
    fn custom_alpha() {
        let lb = EwmaBalancer::with_alpha(0.9);
        lb.add_node(make_node("n1", 8001));

        // High alpha means new values dominate.
        lb.record_response_time("n1", Duration::from_millis(100));
        lb.record_response_time("n1", Duration::from_millis(10));

        let ewma = lb.get_ewma("n1").unwrap();
        // With alpha=0.9, EWMA ~ 0.9*10 + 0.1*100 = 19.
        assert!(
            ewma < 25.0,
            "High alpha should weight recent values: {}",
            ewma
        );
    }
}
