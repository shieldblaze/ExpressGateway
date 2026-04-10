//! Status endpoints.

use axum::{Json, Router, extract::State, routing::get};
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::node_registry::NodeRegistryState;

/// Overall control plane status.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub status: String,
    pub total_nodes: usize,
    pub connected_nodes: usize,
    pub unhealthy_nodes: usize,
    pub disconnected_nodes: usize,
    pub draining_nodes: usize,
    pub total_clusters: usize,
    pub total_routes: usize,
    pub total_listeners: usize,
    pub total_health_checks: usize,
    pub total_tls_certs: usize,
    pub config_version: Option<u64>,
}

/// Summary status for a single node.
#[derive(Debug, Serialize, Deserialize)]
pub struct NodeStatusResponse {
    pub id: String,
    pub address: String,
    pub state: NodeRegistryState,
    pub missed_heartbeats: u32,
}

/// Build status routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/status", get(get_status))
        .route("/status/nodes", get(get_node_statuses))
}

/// GET /status - Overall control plane status.
async fn get_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let store = &state.store;

    let mut connected = 0usize;
    let mut unhealthy = 0usize;
    let mut disconnected = 0usize;
    let mut draining = 0usize;

    for entry in store.nodes.iter() {
        match entry.value().state {
            NodeRegistryState::Connected => connected += 1,
            NodeRegistryState::Unhealthy => unhealthy += 1,
            NodeRegistryState::Disconnected => disconnected += 1,
            NodeRegistryState::Draining => draining += 1,
        }
    }

    let config_version = store.config_versions.current().map(|v| v.version);

    Json(StatusResponse {
        status: "ok".into(),
        total_nodes: store.nodes.len(),
        connected_nodes: connected,
        unhealthy_nodes: unhealthy,
        disconnected_nodes: disconnected,
        draining_nodes: draining,
        total_clusters: store.clusters.len(),
        total_routes: store.routes.len(),
        total_listeners: store.listeners.len(),
        total_health_checks: store.health_checks.len(),
        total_tls_certs: store.tls_certs.len(),
        config_version,
    })
}

/// GET /status/nodes - Per-node status summary.
async fn get_node_statuses(State(state): State<AppState>) -> Json<Vec<NodeStatusResponse>> {
    let nodes: Vec<NodeStatusResponse> = state
        .store
        .nodes
        .iter()
        .map(|entry| NodeStatusResponse {
            id: entry.value().id.clone(),
            address: entry.value().address.clone(),
            state: entry.value().state,
            missed_heartbeats: entry.value().missed_heartbeats,
        })
        .collect();
    Json(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_registry::NodeEntry;
    use crate::rest;
    use crate::store::ControlPlaneStore;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_app() -> (Router, Arc<ControlPlaneStore>) {
        let store = ControlPlaneStore::new_shared();
        let app = rest::router(store.clone());
        (app, store)
    }

    #[tokio::test]
    async fn status_empty() {
        let (app, _store) = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: StatusResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(status.status, "ok");
        assert_eq!(status.total_nodes, 0);
        assert!(status.config_version.is_none());
    }

    #[tokio::test]
    async fn status_with_nodes() {
        let (app, store) = test_app();

        store.nodes.insert(
            "n1".into(),
            NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None),
        );
        let mut n2 = NodeEntry::new("n2".into(), "10.0.0.2:8080".into(), None);
        n2.state = NodeRegistryState::Unhealthy;
        store.nodes.insert("n2".into(), n2);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: StatusResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(status.total_nodes, 2);
        assert_eq!(status.connected_nodes, 1);
        assert_eq!(status.unhealthy_nodes, 1);
    }

    #[tokio::test]
    async fn node_statuses() {
        let (app, store) = test_app();

        store.nodes.insert(
            "n1".into(),
            NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None),
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/status/nodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let nodes: Vec<NodeStatusResponse> = serde_json::from_slice(&body).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "n1");
    }
}
