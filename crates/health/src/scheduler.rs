//! Health check scheduler -- runs periodic health checks against backend nodes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use expressgateway_core::node::{Node, NodeState};
use expressgateway_core::{Health, HealthCheckConfig, HealthChecker, HealthState};
use parking_lot::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::backoff::ExponentialBackoff;
use crate::tracker::HealthTracker;

/// Per-node health check state managed by the scheduler.
struct NodeHealthState {
    tracker: HealthTracker,
    backoff: ExponentialBackoff,
}

/// Background health check scheduler that periodically checks all registered nodes.
pub struct HealthCheckScheduler {
    /// Health checker implementation (TCP, UDP, HTTP, etc.).
    checker: Arc<dyn HealthChecker>,
    /// Configuration for health check timing and thresholds.
    config: HealthCheckConfig,
    /// Per-node health tracking state.
    node_states: Arc<RwLock<HashMap<String, NodeHealthState>>>,
    /// Handle to the background task.
    task_handle: RwLock<Option<JoinHandle<()>>>,
}

impl HealthCheckScheduler {
    /// Create a new health check scheduler.
    pub fn new(checker: Arc<dyn HealthChecker>, config: HealthCheckConfig) -> Self {
        Self {
            checker,
            config,
            node_states: Arc::new(RwLock::new(HashMap::new())),
            task_handle: RwLock::new(None),
        }
    }

    /// Start the background health check loop for the given nodes.
    pub fn start(&self, nodes: Vec<Arc<dyn Node>>) {
        let checker = self.checker.clone();
        let config = self.config.clone();
        let node_states = self.node_states.clone();

        // Initialize per-node state.
        {
            let mut states = node_states.write();
            for node in &nodes {
                states
                    .entry(node.id().to_string())
                    .or_insert_with(|| NodeHealthState {
                        tracker: HealthTracker::new(config.samples, config.rise, config.fall),
                        backoff: ExponentialBackoff::default(),
                    });
            }
        }

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(config.interval);

            loop {
                interval.tick().await;

                for node in &nodes {
                    if node.state() == NodeState::ManualOffline {
                        debug!(node_id = node.id(), "skipping manually offlined node");
                        continue;
                    }

                    // Check backoff delay.
                    let delay = {
                        let states = node_states.read();
                        states
                            .get(node.id())
                            .map(|s| s.backoff.next_delay())
                            .unwrap_or(Duration::ZERO)
                    };

                    if delay > Duration::ZERO {
                        debug!(
                            node_id = node.id(),
                            delay_ms = delay.as_millis(),
                            "backing off health check"
                        );
                        tokio::time::sleep(delay).await;
                    }

                    let result = checker.check(node.address()).await;
                    let success = result.state == HealthState::Healthy;

                    debug!(
                        node_id = node.id(),
                        healthy = success,
                        latency_ms = result.latency.as_millis(),
                        "health check completed"
                    );

                    // Update tracker and backoff.
                    let mut states = node_states.write();
                    if let Some(node_state) = states.get_mut(node.id()) {
                        node_state.tracker.record(success);

                        if success {
                            node_state.backoff.record_success();
                        } else {
                            node_state.backoff.record_failure();
                        }

                        // Update node state based on tracker.
                        let health = node_state.tracker.health();
                        update_node_state(node.as_ref(), &node_state.tracker, health);
                    }
                }
            }
        });

        *self.task_handle.write() = Some(handle);
    }

    /// Stop the background health check loop.
    pub fn stop(&self) {
        if let Some(handle) = self.task_handle.write().take() {
            handle.abort();
            info!("health check scheduler stopped");
        }
    }

    /// Get the current health for a specific node.
    pub fn node_health(&self, node_id: &str) -> Health {
        self.node_states
            .read()
            .get(node_id)
            .map(|s| s.tracker.health())
            .unwrap_or(Health::Unknown)
    }
}

impl Drop for HealthCheckScheduler {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Update a node's state based on health tracker results.
fn update_node_state(node: &dyn Node, tracker: &HealthTracker, _health: Health) {
    let current_state = node.state();

    match current_state {
        NodeState::Online => {
            if tracker.should_go_offline() {
                warn!(
                    node_id = node.id(),
                    "node going OFFLINE (fall threshold reached)"
                );
                node.set_state(NodeState::Offline);
            }
        }
        NodeState::Offline => {
            if tracker.should_go_online() {
                info!(
                    node_id = node.id(),
                    "node going ONLINE (rise threshold reached)"
                );
                node.set_state(NodeState::Online);
            }
        }
        NodeState::Idle => {
            if tracker.should_go_online() {
                info!(
                    node_id = node.id(),
                    "node going ONLINE from IDLE (rise threshold reached)"
                );
                node.set_state(NodeState::Online);
            } else if tracker.should_go_offline() {
                warn!(
                    node_id = node.id(),
                    "node going OFFLINE from IDLE (fall threshold reached)"
                );
                node.set_state(NodeState::Offline);
            }
        }
        NodeState::ManualOffline => {
            // Do not override manual decisions.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::{HealthCheckResult, NodeImpl};

    /// A mock health checker that always returns healthy.
    struct AlwaysHealthy;

    #[async_trait::async_trait]
    impl HealthChecker for AlwaysHealthy {
        async fn check(&self, _addr: std::net::SocketAddr) -> HealthCheckResult {
            HealthCheckResult {
                state: HealthState::Healthy,
                latency: Duration::from_millis(1),
                message: None,
            }
        }
    }

    /// A mock health checker that always returns unhealthy.
    struct AlwaysUnhealthy;

    #[async_trait::async_trait]
    impl HealthChecker for AlwaysUnhealthy {
        async fn check(&self, _addr: std::net::SocketAddr) -> HealthCheckResult {
            HealthCheckResult {
                state: HealthState::Unhealthy,
                latency: Duration::from_millis(1),
                message: Some("always fails".to_string()),
            }
        }
    }

    #[test]
    fn test_update_node_state_online_to_offline() {
        let node = NodeImpl::new("n1".into(), "127.0.0.1:8080".parse().unwrap(), 1, None);
        let mut tracker = HealthTracker::new(10, 2, 3);

        // Record enough failures to trigger fall.
        for _ in 0..3 {
            tracker.record(false);
        }

        update_node_state(&node, &tracker, tracker.health());
        assert_eq!(node.state(), NodeState::Offline);
    }

    #[test]
    fn test_update_node_state_offline_to_online() {
        let node = NodeImpl::new("n1".into(), "127.0.0.1:8080".parse().unwrap(), 1, None);
        node.set_state(NodeState::Offline);

        let mut tracker = HealthTracker::new(10, 2, 3);
        for _ in 0..2 {
            tracker.record(true);
        }

        update_node_state(&node, &tracker, tracker.health());
        assert_eq!(node.state(), NodeState::Online);
    }

    #[test]
    fn test_update_node_state_manual_offline_not_overridden() {
        let node = NodeImpl::new("n1".into(), "127.0.0.1:8080".parse().unwrap(), 1, None);
        node.set_state(NodeState::ManualOffline);

        let mut tracker = HealthTracker::new(10, 2, 3);
        for _ in 0..10 {
            tracker.record(true);
        }

        update_node_state(&node, &tracker, tracker.health());
        assert_eq!(node.state(), NodeState::ManualOffline);
    }

    #[tokio::test]
    async fn test_scheduler_start_and_stop() {
        let checker: Arc<dyn HealthChecker> = Arc::new(AlwaysHealthy);
        let config = HealthCheckConfig {
            interval: Duration::from_millis(50),
            timeout: Duration::from_millis(100),
            rise: 2,
            fall: 3,
            samples: 10,
        };

        let scheduler = HealthCheckScheduler::new(checker, config);
        let node = NodeImpl::new_arc("n1".into(), "127.0.0.1:8080".parse().unwrap(), 1, None);

        scheduler.start(vec![node.clone()]);

        // Let a few checks run.
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_ne!(scheduler.node_health("n1"), Health::Unknown);
        scheduler.stop();
    }

    #[tokio::test]
    async fn test_scheduler_with_unhealthy_checker() {
        let checker: Arc<dyn HealthChecker> = Arc::new(AlwaysUnhealthy);
        let config = HealthCheckConfig {
            interval: Duration::from_millis(50),
            timeout: Duration::from_millis(100),
            rise: 2,
            fall: 3,
            samples: 10,
        };

        let scheduler = HealthCheckScheduler::new(checker, config);
        let node = NodeImpl::new_arc("n1".into(), "127.0.0.1:8080".parse().unwrap(), 1, None);

        scheduler.start(vec![node.clone()]);

        // Wait enough for fall threshold (3 failures) accounting for exponential backoff.
        // Failure 1: instant, then 1000ms backoff.
        // Failure 2: after 1s, then 2000ms backoff.
        // Failure 3: after ~3s total -> triggers offline.
        tokio::time::sleep(Duration::from_secs(5)).await;

        assert_eq!(node.state(), NodeState::Offline);
        scheduler.stop();
    }
}
