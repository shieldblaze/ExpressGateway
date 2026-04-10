//! Cluster CRUD endpoints.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use crate::store::{ClusterEntry, ClusterNodeRef};

/// Request body for creating/updating a cluster.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClusterRequest {
    pub name: String,
    pub lb_strategy: String,
    pub max_connections_per_node: Option<u64>,
    pub drain_timeout_s: Option<u64>,
    #[serde(default)]
    pub nodes: Vec<ClusterNodeRef>,
}

/// Build cluster routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clusters", post(create_cluster))
        .route("/clusters", get(list_clusters))
        .route("/clusters/{id}", get(get_cluster))
        .route("/clusters/{id}", put(update_cluster))
        .route("/clusters/{id}", delete(delete_cluster))
}

/// POST /clusters - Create a new cluster.
async fn create_cluster(
    State(state): State<AppState>,
    Json(req): Json<ClusterRequest>,
) -> (StatusCode, Json<ClusterEntry>) {
    let now = Utc::now();
    let id = Uuid::new_v4().to_string();
    let entry = ClusterEntry {
        id: id.clone(),
        name: req.name,
        lb_strategy: req.lb_strategy,
        max_connections_per_node: req.max_connections_per_node,
        drain_timeout_s: req.drain_timeout_s,
        nodes: req.nodes,
        created_at: now,
        updated_at: now,
    };
    state.store.clusters.insert(id, entry.clone());
    (StatusCode::CREATED, Json(entry))
}

/// GET /clusters - List all clusters.
async fn list_clusters(State(state): State<AppState>) -> Json<Vec<ClusterEntry>> {
    let clusters: Vec<ClusterEntry> = state
        .store
        .clusters
        .iter()
        .map(|r| r.value().clone())
        .collect();
    Json(clusters)
}

/// GET /clusters/:id - Get a specific cluster.
async fn get_cluster(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ClusterEntry>, StatusCode> {
    state
        .store
        .clusters
        .get(&id)
        .map(|r| Json(r.value().clone()))
        .ok_or(StatusCode::NOT_FOUND)
}

/// PUT /clusters/:id - Update a cluster.
async fn update_cluster(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ClusterRequest>,
) -> Result<Json<ClusterEntry>, StatusCode> {
    let mut entry = state
        .store
        .clusters
        .get_mut(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    entry.name = req.name;
    entry.lb_strategy = req.lb_strategy;
    entry.max_connections_per_node = req.max_connections_per_node;
    entry.drain_timeout_s = req.drain_timeout_s;
    entry.nodes = req.nodes;
    entry.updated_at = Utc::now();

    Ok(Json(entry.value().clone()))
}

/// DELETE /clusters/:id - Delete a cluster.
async fn delete_cluster(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state
        .store
        .clusters
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
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_app() -> (Router, Arc<ControlPlaneStore>) {
        let store = ControlPlaneStore::new_shared();
        let app = rest::router(store.clone());
        (app, store)
    }

    fn cluster_json() -> String {
        serde_json::to_string(&ClusterRequest {
            name: "test-cluster".into(),
            lb_strategy: "round_robin".into(),
            max_connections_per_node: Some(1000),
            drain_timeout_s: Some(30),
            nodes: vec![ClusterNodeRef {
                address: "10.0.0.1:8080".into(),
                weight: Some(1),
                max_connections: Some(5000),
            }],
        })
        .unwrap()
    }

    #[tokio::test]
    async fn create_and_get_cluster() {
        let (app, _store) = test_app();

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/clusters")
                    .header("content-type", "application/json")
                    .body(Body::from(cluster_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: ClusterEntry = serde_json::from_slice(&body).unwrap();
        assert_eq!(created.name, "test-cluster");
        assert_eq!(created.nodes.len(), 1);

        // Get
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/controlplane/clusters/{}", created.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn list_clusters() {
        let (app, _store) = test_app();

        // Create two clusters
        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/clusters")
                    .header("content-type", "application/json")
                    .body(Body::from(cluster_json()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/clusters")
                    .header("content-type", "application/json")
                    .body(Body::from(cluster_json()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/clusters")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let clusters: Vec<ClusterEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(clusters.len(), 2);
    }

    #[tokio::test]
    async fn update_cluster() {
        let (app, _store) = test_app();

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/clusters")
                    .header("content-type", "application/json")
                    .body(Body::from(cluster_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: ClusterEntry = serde_json::from_slice(&body).unwrap();

        // Update
        let update_json = serde_json::to_string(&ClusterRequest {
            name: "updated-cluster".into(),
            lb_strategy: "least_connections".into(),
            max_connections_per_node: Some(2000),
            drain_timeout_s: Some(60),
            nodes: vec![],
        })
        .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/v1/controlplane/clusters/{}", created.id))
                    .header("content-type", "application/json")
                    .body(Body::from(update_json))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let updated: ClusterEntry = serde_json::from_slice(&body).unwrap();
        assert_eq!(updated.name, "updated-cluster");
        assert_eq!(updated.lb_strategy, "least_connections");
    }

    #[tokio::test]
    async fn delete_cluster() {
        let (app, store) = test_app();

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/clusters")
                    .header("content-type", "application/json")
                    .body(Body::from(cluster_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: ClusterEntry = serde_json::from_slice(&body).unwrap();

        // Delete
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/controlplane/clusters/{}", created.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(store.clusters.is_empty());
    }

    #[tokio::test]
    async fn get_nonexistent_cluster() {
        let (app, _store) = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/clusters/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
