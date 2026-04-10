//! Node management endpoints.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::node_registry::{NodeEntry, NodeRegistryState};

/// Response for a single node.
#[derive(Debug, Serialize, Deserialize)]
pub struct NodeResponse {
    pub id: String,
    pub address: String,
    pub state: NodeRegistryState,
    pub missed_heartbeats: u32,
    pub connected_at: String,
}

impl From<&NodeEntry> for NodeResponse {
    fn from(entry: &NodeEntry) -> Self {
        Self {
            id: entry.id.clone(),
            address: entry.address.clone(),
            state: entry.state,
            missed_heartbeats: entry.missed_heartbeats,
            connected_at: entry.connected_at.to_rfc3339(),
        }
    }
}

/// Build node routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/nodes", get(list_nodes))
        .route("/nodes/{id}", get(get_node))
        .route("/nodes/{id}/drain", post(drain_node))
        .route("/nodes/{id}/undrain", post(undrain_node))
        .route("/nodes/{id}", delete(delete_node))
}

/// GET /nodes - List all registered nodes.
async fn list_nodes(State(state): State<AppState>) -> Json<Vec<NodeResponse>> {
    let nodes: Vec<NodeResponse> = state
        .store
        .nodes
        .iter()
        .map(|r| NodeResponse::from(r.value()))
        .collect();
    Json(nodes)
}

/// GET /nodes/:id - Get a specific node.
async fn get_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<NodeResponse>, StatusCode> {
    state
        .store
        .nodes
        .get(&id)
        .map(|r| Json(NodeResponse::from(r.value())))
        .ok_or(StatusCode::NOT_FOUND)
}

/// POST /nodes/:id/drain - Drain a node.
async fn drain_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<NodeResponse>, StatusCode> {
    let mut entry = state
        .store
        .nodes
        .get_mut(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    if entry.drain() {
        Ok(Json(NodeResponse::from(entry.value())))
    } else {
        Err(StatusCode::CONFLICT)
    }
}

/// POST /nodes/:id/undrain - Undrain a node.
async fn undrain_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<NodeResponse>, StatusCode> {
    let mut entry = state
        .store
        .nodes
        .get_mut(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    if entry.undrain() {
        Ok(Json(NodeResponse::from(entry.value())))
    } else {
        Err(StatusCode::CONFLICT)
    }
}

/// DELETE /nodes/:id - Remove a node.
async fn delete_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state
        .store
        .nodes
        .remove(&id)
        .map(|_| StatusCode::NO_CONTENT)
        .ok_or(StatusCode::NOT_FOUND)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest;
    use crate::store::ControlPlaneStore;
    use axum::body::Body;
    use axum::http::Request;
    use http::StatusCode;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_app() -> (Router, Arc<ControlPlaneStore>) {
        let store = ControlPlaneStore::new_shared();
        let app = rest::router(store.clone());
        (app, store)
    }

    #[tokio::test]
    async fn list_nodes_empty() {
        let (app, _store) = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/nodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let nodes: Vec<NodeResponse> = serde_json::from_slice(&body).unwrap();
        assert!(nodes.is_empty());
    }

    #[tokio::test]
    async fn list_nodes_with_entries() {
        let (app, store) = test_app();
        store.nodes.insert(
            "n1".into(),
            NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None),
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/nodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let nodes: Vec<NodeResponse> = serde_json::from_slice(&body).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "n1");
    }

    #[tokio::test]
    async fn get_node_found() {
        let (app, store) = test_app();
        store.nodes.insert(
            "n1".into(),
            NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None),
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/nodes/n1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_node_not_found() {
        let (app, _store) = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/nodes/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn drain_and_undrain_node() {
        let (app, store) = test_app();
        store.nodes.insert(
            "n1".into(),
            NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None),
        );

        // Drain
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/nodes/n1/drain")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let node: NodeResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(node.state, NodeRegistryState::Draining);

        // Undrain
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/nodes/n1/undrain")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let node: NodeResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(node.state, NodeRegistryState::Connected);
    }

    #[tokio::test]
    async fn delete_node_success() {
        let (app, store) = test_app();
        store.nodes.insert(
            "n1".into(),
            NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None),
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/controlplane/nodes/n1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(store.nodes.is_empty());
    }

    #[tokio::test]
    async fn delete_node_not_found() {
        let (app, _store) = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/controlplane/nodes/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
