//! Route CRUD endpoints.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use super::AppState;
use crate::store::RouteEntry;

/// Request body for creating/updating a route.
#[derive(Debug, Serialize, Deserialize)]
pub struct RouteRequest {
    pub host: Option<String>,
    pub path: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub cluster: String,
    #[serde(default = "default_priority")]
    pub priority: u32,
    pub lb_strategy: Option<String>,
}

fn default_priority() -> u32 {
    100
}

/// Build route routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/routes", post(create_route))
        .route("/routes", get(list_routes))
        .route("/routes/{id}", get(get_route))
        .route("/routes/{id}", put(update_route))
        .route("/routes/{id}", delete(delete_route))
}

/// POST /routes - Create a new route.
async fn create_route(
    State(state): State<AppState>,
    Json(req): Json<RouteRequest>,
) -> (StatusCode, Json<RouteEntry>) {
    let now = Utc::now();
    let id = Uuid::new_v4().to_string();
    let entry = RouteEntry {
        id: id.clone(),
        host: req.host,
        path: req.path,
        headers: req.headers,
        cluster: req.cluster,
        priority: req.priority,
        lb_strategy: req.lb_strategy,
        created_at: now,
        updated_at: now,
    };
    state.store.routes.insert(id, entry.clone());
    (StatusCode::CREATED, Json(entry))
}

/// GET /routes - List all routes.
async fn list_routes(State(state): State<AppState>) -> Json<Vec<RouteEntry>> {
    let mut routes: Vec<RouteEntry> = state
        .store
        .routes
        .iter()
        .map(|r| r.value().clone())
        .collect();
    routes.sort_by_key(|r| r.priority);
    Json(routes)
}

/// GET /routes/:id - Get a specific route.
async fn get_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RouteEntry>, StatusCode> {
    state
        .store
        .routes
        .get(&id)
        .map(|r| Json(r.value().clone()))
        .ok_or(StatusCode::NOT_FOUND)
}

/// PUT /routes/:id - Update a route.
async fn update_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RouteRequest>,
) -> Result<Json<RouteEntry>, StatusCode> {
    let mut entry = state
        .store
        .routes
        .get_mut(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    entry.host = req.host;
    entry.path = req.path;
    entry.headers = req.headers;
    entry.cluster = req.cluster;
    entry.priority = req.priority;
    entry.lb_strategy = req.lb_strategy;
    entry.updated_at = Utc::now();

    Ok(Json(entry.value().clone()))
}

/// DELETE /routes/:id - Delete a route.
async fn delete_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state
        .store
        .routes
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

    fn route_json() -> String {
        serde_json::to_string(&RouteRequest {
            host: Some("example.com".into()),
            path: Some("/api".into()),
            headers: None,
            cluster: "backend-cluster".into(),
            priority: 10,
            lb_strategy: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn create_and_list_routes() {
        let (app, _store) = test_app();

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/routes")
                    .header("content-type", "application/json")
                    .body(Body::from(route_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // List
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/routes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let routes: Vec<RouteEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].host.as_deref(), Some("example.com"));
    }

    #[tokio::test]
    async fn update_route() {
        let (app, _store) = test_app();

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/routes")
                    .header("content-type", "application/json")
                    .body(Body::from(route_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: RouteEntry = serde_json::from_slice(&body).unwrap();

        // Update
        let update = serde_json::to_string(&RouteRequest {
            host: Some("new.example.com".into()),
            path: Some("/v2".into()),
            headers: None,
            cluster: "new-cluster".into(),
            priority: 5,
            lb_strategy: Some("least_connections".into()),
        })
        .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/v1/controlplane/routes/{}", created.id))
                    .header("content-type", "application/json")
                    .body(Body::from(update))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let updated: RouteEntry = serde_json::from_slice(&body).unwrap();
        assert_eq!(updated.host.as_deref(), Some("new.example.com"));
        assert_eq!(updated.priority, 5);
    }

    #[tokio::test]
    async fn delete_route() {
        let (app, store) = test_app();

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/routes")
                    .header("content-type", "application/json")
                    .body(Body::from(route_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: RouteEntry = serde_json::from_slice(&body).unwrap();

        // Delete
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/controlplane/routes/{}", created.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(store.routes.is_empty());
    }
}
