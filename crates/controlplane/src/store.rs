//! In-memory store for control plane state.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::config_version::ConfigVersionStore;
use crate::node_registry::NodeEntry;

/// A cluster entry in the control plane store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterEntry {
    pub id: String,
    pub name: String,
    pub lb_strategy: String,
    pub max_connections_per_node: Option<u64>,
    pub drain_timeout_s: Option<u64>,
    pub nodes: Vec<ClusterNodeRef>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A reference to a node within a cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNodeRef {
    pub address: String,
    pub weight: Option<u32>,
    pub max_connections: Option<u64>,
}

/// A route entry in the control plane store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteEntry {
    pub id: String,
    pub host: Option<String>,
    pub path: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub cluster: String,
    pub priority: u32,
    pub lb_strategy: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A listener entry in the control plane store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerEntry {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub bind: String,
    pub tls_profile: Option<String>,
    pub http_versions: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A health check entry in the control plane store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckEntry {
    pub id: String,
    pub cluster_id: String,
    pub interval_s: u64,
    pub timeout_ms: u64,
    pub rise: u32,
    pub fall: u32,
    pub http_method: Option<String>,
    pub http_path: Option<String>,
    pub expected_status: Option<Vec<u16>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A TLS certificate entry in the control plane store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsCertEntry {
    pub id: String,
    pub hostname: String,
    /// PEM-encoded certificate chain (not the private key).
    pub cert_pem: String,
    /// Whether the private key has been uploaded (we don't serialize the key).
    pub has_private_key: bool,
    pub ocsp_stapling: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Central in-memory store for all control plane state.
pub struct ControlPlaneStore {
    pub nodes: DashMap<String, NodeEntry>,
    pub clusters: DashMap<String, ClusterEntry>,
    pub routes: DashMap<String, RouteEntry>,
    pub listeners: DashMap<String, ListenerEntry>,
    pub health_checks: DashMap<String, HealthCheckEntry>,
    pub tls_certs: DashMap<String, TlsCertEntry>,
    pub config_versions: ConfigVersionStore,
}

impl ControlPlaneStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            nodes: DashMap::new(),
            clusters: DashMap::new(),
            routes: DashMap::new(),
            listeners: DashMap::new(),
            health_checks: DashMap::new(),
            tls_certs: DashMap::new(),
            config_versions: ConfigVersionStore::new(),
        }
    }

    /// Create a shared store wrapped in Arc.
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Get a snapshot of the entire store as JSON (for config versioning).
    pub fn snapshot(&self) -> serde_json::Value {
        let clusters: Vec<ClusterEntry> = self.clusters.iter().map(|r| r.value().clone()).collect();
        let routes: Vec<RouteEntry> = self.routes.iter().map(|r| r.value().clone()).collect();
        let listeners: Vec<ListenerEntry> =
            self.listeners.iter().map(|r| r.value().clone()).collect();
        let health_checks: Vec<HealthCheckEntry> = self
            .health_checks
            .iter()
            .map(|r| r.value().clone())
            .collect();
        let tls_certs: Vec<TlsCertEntry> =
            self.tls_certs.iter().map(|r| r.value().clone()).collect();

        serde_json::json!({
            "clusters": clusters,
            "routes": routes,
            "listeners": listeners,
            "health_checks": health_checks,
            "tls_certs": tls_certs,
        })
    }
}

impl Default for ControlPlaneStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_registry::NodeRegistryState;

    #[test]
    fn store_creation() {
        let store = ControlPlaneStore::new();
        assert!(store.nodes.is_empty());
        assert!(store.clusters.is_empty());
        assert!(store.routes.is_empty());
        assert!(store.listeners.is_empty());
        assert!(store.health_checks.is_empty());
        assert!(store.tls_certs.is_empty());
    }

    #[test]
    fn store_node_crud() {
        let store = ControlPlaneStore::new();
        let node = NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None);
        store.nodes.insert("n1".into(), node);

        assert_eq!(store.nodes.len(), 1);
        let entry = store.nodes.get("n1").unwrap();
        assert_eq!(entry.state, NodeRegistryState::Connected);
    }

    #[test]
    fn store_snapshot() {
        let store = ControlPlaneStore::new();
        let now = Utc::now();
        store.clusters.insert(
            "c1".into(),
            ClusterEntry {
                id: "c1".into(),
                name: "test-cluster".into(),
                lb_strategy: "round_robin".into(),
                max_connections_per_node: Some(1000),
                drain_timeout_s: Some(30),
                nodes: vec![],
                created_at: now,
                updated_at: now,
            },
        );

        let snap = store.snapshot();
        let clusters = snap["clusters"].as_array().unwrap();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0]["name"], "test-cluster");
    }

    #[test]
    fn shared_store() {
        let store = ControlPlaneStore::new_shared();
        store.nodes.insert(
            "n1".into(),
            NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None),
        );
        assert_eq!(store.nodes.len(), 1);
    }
}
