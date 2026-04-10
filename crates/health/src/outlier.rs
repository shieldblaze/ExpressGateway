//! Outlier detection (passive health checking) based on actual traffic patterns.
//!
//! Monitors success/failure from real traffic per node and ejects nodes
//! that exceed failure thresholds, then restores them after an ejection period.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use expressgateway_core::node::{Node, NodeState};

/// Configuration for outlier detection.
#[derive(Debug, Clone)]
pub struct OutlierConfig {
    /// Number of consecutive failures before ejection.
    pub consecutive_failure_threshold: u32,
    /// Base ejection duration; may increase with repeated ejections.
    pub ejection_duration: Duration,
    /// Maximum percentage of nodes that can be ejected at once (0.0 to 1.0).
    pub max_ejection_percent: f64,
}

impl Default for OutlierConfig {
    fn default() -> Self {
        Self {
            consecutive_failure_threshold: 5,
            ejection_duration: Duration::from_secs(30),
            max_ejection_percent: 0.5,
        }
    }
}

/// Per-node tracking data for outlier detection.
struct NodeOutlierState {
    /// Consecutive failure count (atomic for thread safety).
    consecutive_failures: AtomicU64,
    /// Time of ejection in nanos since epoch (0 = not ejected).
    ejection_time: AtomicU64,
    /// Number of times this node has been ejected (for escalating ejection duration).
    ejection_count: AtomicU64,
}

impl NodeOutlierState {
    fn new() -> Self {
        Self {
            consecutive_failures: AtomicU64::new(0),
            ejection_time: AtomicU64::new(0),
            ejection_count: AtomicU64::new(0),
        }
    }

    fn is_ejected(&self) -> bool {
        self.ejection_time.load(Ordering::Acquire) > 0
    }
}

/// Outlier detector that monitors real traffic and ejects unhealthy nodes.
pub struct OutlierDetector {
    states: DashMap<String, NodeOutlierState>,
    config: OutlierConfig,
}

impl OutlierDetector {
    /// Create a new outlier detector with the given configuration.
    pub fn new(config: OutlierConfig) -> Self {
        Self {
            states: DashMap::new(),
            config,
        }
    }

    /// Record a successful request to a node.
    pub fn record_success(&self, node_id: &str) {
        let state = self
            .states
            .entry(node_id.to_string())
            .or_insert_with(NodeOutlierState::new);
        state.consecutive_failures.store(0, Ordering::Release);
    }

    /// Record a failed request to a node.
    pub fn record_failure(&self, node_id: &str) {
        let state = self
            .states
            .entry(node_id.to_string())
            .or_insert_with(NodeOutlierState::new);
        state.consecutive_failures.fetch_add(1, Ordering::AcqRel);
    }

    /// Get consecutive failure count for a node.
    pub fn consecutive_failures(&self, node_id: &str) -> u64 {
        self.states
            .get(node_id)
            .map(|s| s.consecutive_failures.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Evaluate all nodes and perform two-phase ejection/restoration.
    ///
    /// Phase 1: Check for restoration (ejection time has elapsed).
    /// Phase 2: Check for ejection (failure threshold exceeded).
    ///
    /// Respects the max ejection percentage cap.
    pub fn evaluate(&self, nodes: &[Arc<dyn Node>]) {
        if nodes.is_empty() {
            return;
        }

        let now_nanos = nanos_since_epoch();
        let total_nodes = nodes.len();

        // Phase 1: Restore nodes whose ejection time has elapsed.
        for node in nodes {
            if let Some(state) = self.states.get(node.id())
                && state.is_ejected()
            {
                let ejection_time = state.ejection_time.load(Ordering::Acquire);
                let ejection_count = state.ejection_count.load(Ordering::Acquire);
                let ejection_duration = self
                    .config
                    .ejection_duration
                    .saturating_mul(ejection_count.max(1) as u32);

                let elapsed = Duration::from_nanos(now_nanos.saturating_sub(ejection_time));
                if elapsed >= ejection_duration {
                    // Restore: set to Idle for re-verification.
                    state.ejection_time.store(0, Ordering::Release);
                    state.consecutive_failures.store(0, Ordering::Release);
                    node.set_state(NodeState::Idle);
                }
            }
        }

        // Phase 2: Check for new ejections, respecting max ejection percentage.
        let current_ejected = self.states.iter().filter(|s| s.is_ejected()).count();
        let max_ejectable = ((total_nodes as f64 * self.config.max_ejection_percent).floor()
            as usize)
            .saturating_sub(current_ejected);

        let mut new_ejections = 0usize;
        for node in nodes {
            if new_ejections >= max_ejectable {
                break;
            }

            if node.state() == NodeState::Offline || node.state() == NodeState::ManualOffline {
                continue;
            }

            if let Some(state) = self.states.get(node.id()) {
                if state.is_ejected() {
                    continue;
                }

                let failures = state.consecutive_failures.load(Ordering::Acquire);
                if failures >= self.config.consecutive_failure_threshold as u64 {
                    // Eject the node.
                    state.ejection_time.store(now_nanos, Ordering::Release);
                    state.ejection_count.fetch_add(1, Ordering::AcqRel);
                    node.set_state(NodeState::Offline);
                    new_ejections += 1;
                }
            }
        }
    }

    /// Check if a specific node is currently ejected.
    pub fn is_ejected(&self, node_id: &str) -> bool {
        self.states
            .get(node_id)
            .map(|s| s.is_ejected())
            .unwrap_or(false)
    }

    /// Number of tracked nodes.
    pub fn tracked_count(&self) -> usize {
        self.states.len()
    }
}

impl Default for OutlierDetector {
    fn default() -> Self {
        Self::new(OutlierConfig::default())
    }
}

fn nanos_since_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::NodeImpl;

    fn make_node(id: &str) -> Arc<NodeImpl> {
        NodeImpl::new_arc(id.to_string(), "127.0.0.1:8080".parse().unwrap(), 1, None)
    }

    #[test]
    fn test_record_success_resets_failures() {
        let detector = OutlierDetector::default();
        detector.record_failure("n1");
        detector.record_failure("n1");
        assert_eq!(detector.consecutive_failures("n1"), 2);

        detector.record_success("n1");
        assert_eq!(detector.consecutive_failures("n1"), 0);
    }

    #[test]
    fn test_ejection_on_threshold() {
        let config = OutlierConfig {
            consecutive_failure_threshold: 3,
            ejection_duration: Duration::from_secs(30),
            max_ejection_percent: 1.0,
        };
        let detector = OutlierDetector::new(config);
        let node = make_node("n1");

        for _ in 0..3 {
            detector.record_failure("n1");
        }

        let nodes: Vec<Arc<dyn Node>> = vec![node.clone()];
        detector.evaluate(&nodes);

        assert!(detector.is_ejected("n1"));
        assert_eq!(node.state(), NodeState::Offline);
    }

    #[test]
    fn test_no_ejection_below_threshold() {
        let config = OutlierConfig {
            consecutive_failure_threshold: 5,
            ejection_duration: Duration::from_secs(30),
            max_ejection_percent: 1.0,
        };
        let detector = OutlierDetector::new(config);
        let node = make_node("n1");

        for _ in 0..4 {
            detector.record_failure("n1");
        }

        let nodes: Vec<Arc<dyn Node>> = vec![node.clone()];
        detector.evaluate(&nodes);

        assert!(!detector.is_ejected("n1"));
        assert_eq!(node.state(), NodeState::Online);
    }

    #[test]
    fn test_max_ejection_percentage_cap() {
        let config = OutlierConfig {
            consecutive_failure_threshold: 1,
            ejection_duration: Duration::from_secs(300),
            max_ejection_percent: 0.5, // At most 50%
        };
        let detector = OutlierDetector::new(config);

        let nodes_impl: Vec<Arc<NodeImpl>> = (0..4).map(|i| make_node(&format!("n{i}"))).collect();
        for n in &nodes_impl {
            detector.record_failure(n.id());
        }

        let nodes: Vec<Arc<dyn Node>> = nodes_impl
            .iter()
            .map(|n| n.clone() as Arc<dyn Node>)
            .collect();
        detector.evaluate(&nodes);

        let ejected_count = nodes_impl
            .iter()
            .filter(|n| n.state() == NodeState::Offline)
            .count();
        // Should eject at most 50% = 2 out of 4.
        assert!(
            ejected_count <= 2,
            "ejected {ejected_count} out of 4, expected at most 2"
        );
    }

    #[test]
    fn test_restoration_after_ejection_duration() {
        let config = OutlierConfig {
            consecutive_failure_threshold: 1,
            ejection_duration: Duration::from_millis(10),
            max_ejection_percent: 1.0,
        };
        let detector = OutlierDetector::new(config);
        let node = make_node("n1");

        detector.record_failure("n1");
        let nodes: Vec<Arc<dyn Node>> = vec![node.clone()];
        detector.evaluate(&nodes);
        assert_eq!(node.state(), NodeState::Offline);

        // Wait for ejection duration.
        std::thread::sleep(Duration::from_millis(20));

        detector.evaluate(&nodes);
        assert_eq!(node.state(), NodeState::Idle);
        assert!(!detector.is_ejected("n1"));
    }
}
