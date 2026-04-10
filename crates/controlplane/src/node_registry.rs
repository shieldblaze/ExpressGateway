//! Node registry with lifecycle state machine and heartbeat tracking.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Node lifecycle states in the control plane registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRegistryState {
    /// Node is connected and healthy.
    Connected,
    /// Node has missed heartbeats and is considered unhealthy.
    Unhealthy,
    /// Node is disconnected from the control plane.
    Disconnected,
    /// Node is draining (no new connections, finishing existing).
    Draining,
}

impl std::fmt::Display for NodeRegistryState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected => write!(f, "connected"),
            Self::Unhealthy => write!(f, "unhealthy"),
            Self::Disconnected => write!(f, "disconnected"),
            Self::Draining => write!(f, "draining"),
        }
    }
}

/// A node entry in the control plane registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeEntry {
    /// Unique node identifier.
    pub id: String,
    /// Network address of the node (e.g., "10.0.0.1:8080").
    pub address: String,
    /// Current lifecycle state.
    pub state: NodeRegistryState,
    /// Session token assigned to this node (if registered).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    /// Last heartbeat timestamp (for internal tracking, not serialized).
    #[serde(skip)]
    pub last_heartbeat: Option<Instant>,
    /// Number of consecutive missed heartbeats.
    pub missed_heartbeats: u32,
    /// Timestamp when the node first connected.
    pub connected_at: DateTime<Utc>,
}

impl NodeEntry {
    /// Create a new node entry in the Connected state.
    pub fn new(id: String, address: String, session_token: Option<String>) -> Self {
        Self {
            id,
            address,
            state: NodeRegistryState::Connected,
            session_token,
            last_heartbeat: Some(Instant::now()),
            missed_heartbeats: 0,
            connected_at: Utc::now(),
        }
    }

    /// Record a heartbeat, resetting counters and potentially restoring state.
    pub fn record_heartbeat(&mut self) {
        self.last_heartbeat = Some(Instant::now());
        self.missed_heartbeats = 0;
        if self.state == NodeRegistryState::Unhealthy {
            self.state = NodeRegistryState::Connected;
        }
    }

    /// Record a missed heartbeat and transition state according to thresholds.
    ///
    /// - After `miss_threshold` missed heartbeats: Connected -> Unhealthy
    /// - After `disconnect_threshold` missed heartbeats: Unhealthy -> Disconnected
    pub fn record_missed_heartbeat(&mut self, miss_threshold: u32, disconnect_threshold: u32) {
        self.missed_heartbeats += 1;
        match self.state {
            NodeRegistryState::Connected if self.missed_heartbeats >= miss_threshold => {
                self.state = NodeRegistryState::Unhealthy;
            }
            NodeRegistryState::Unhealthy if self.missed_heartbeats >= disconnect_threshold => {
                self.state = NodeRegistryState::Disconnected;
            }
            _ => {}
        }
    }

    /// Begin draining the node. Only valid from Connected state.
    /// Returns true if the transition was successful.
    pub fn drain(&mut self) -> bool {
        if self.state == NodeRegistryState::Connected {
            self.state = NodeRegistryState::Draining;
            true
        } else {
            false
        }
    }

    /// Cancel draining and return to Connected state.
    /// Returns true if the transition was successful.
    pub fn undrain(&mut self) -> bool {
        if self.state == NodeRegistryState::Draining {
            self.state = NodeRegistryState::Connected;
            true
        } else {
            false
        }
    }

    /// Disconnect the node. Valid from any state except already Disconnected.
    pub fn disconnect(&mut self) -> bool {
        if self.state != NodeRegistryState::Disconnected {
            self.state = NodeRegistryState::Disconnected;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_node_is_connected() {
        let node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        assert_eq!(node.state, NodeRegistryState::Connected);
        assert_eq!(node.missed_heartbeats, 0);
        assert!(node.last_heartbeat.is_some());
    }

    #[test]
    fn heartbeat_resets_missed_count() {
        let mut node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        node.missed_heartbeats = 2;
        node.state = NodeRegistryState::Unhealthy;
        node.record_heartbeat();
        assert_eq!(node.missed_heartbeats, 0);
        assert_eq!(node.state, NodeRegistryState::Connected);
    }

    #[test]
    fn missed_heartbeats_transition_to_unhealthy() {
        let mut node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        // Miss threshold = 3, disconnect threshold = 6
        for _ in 0..2 {
            node.record_missed_heartbeat(3, 6);
        }
        assert_eq!(node.state, NodeRegistryState::Connected);

        node.record_missed_heartbeat(3, 6);
        assert_eq!(node.state, NodeRegistryState::Unhealthy);
        assert_eq!(node.missed_heartbeats, 3);
    }

    #[test]
    fn missed_heartbeats_transition_to_disconnected() {
        let mut node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        for _ in 0..6 {
            node.record_missed_heartbeat(3, 6);
        }
        assert_eq!(node.state, NodeRegistryState::Disconnected);
        assert_eq!(node.missed_heartbeats, 6);
    }

    #[test]
    fn drain_from_connected() {
        let mut node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        assert!(node.drain());
        assert_eq!(node.state, NodeRegistryState::Draining);
    }

    #[test]
    fn drain_from_unhealthy_fails() {
        let mut node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        node.state = NodeRegistryState::Unhealthy;
        assert!(!node.drain());
        assert_eq!(node.state, NodeRegistryState::Unhealthy);
    }

    #[test]
    fn undrain_from_draining() {
        let mut node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        node.drain();
        assert!(node.undrain());
        assert_eq!(node.state, NodeRegistryState::Connected);
    }

    #[test]
    fn undrain_from_connected_fails() {
        let mut node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        assert!(!node.undrain());
    }

    #[test]
    fn disconnect_from_any_state() {
        let mut node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        assert!(node.disconnect());
        assert_eq!(node.state, NodeRegistryState::Disconnected);
        // Already disconnected
        assert!(!node.disconnect());
    }

    #[test]
    fn heartbeat_restores_unhealthy_to_connected() {
        let mut node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        // Make it unhealthy
        for _ in 0..3 {
            node.record_missed_heartbeat(3, 6);
        }
        assert_eq!(node.state, NodeRegistryState::Unhealthy);

        // A heartbeat should restore it
        node.record_heartbeat();
        assert_eq!(node.state, NodeRegistryState::Connected);
        assert_eq!(node.missed_heartbeats, 0);
    }
}
