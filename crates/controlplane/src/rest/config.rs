//! Configuration management endpoints.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::config_version::ConfigVersion;

/// Request body for rolling back to a previous config version.
#[derive(Debug, Serialize, Deserialize)]
pub struct RollbackRequest {
    pub target_version: u64,
}

/// Build config routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/config/versions", get(list_versions))
        .route("/config/current", get(current_version))
        .route("/config/rollback", post(rollback))
}

/// GET /config/versions - List all config versions.
async fn list_versions(State(state): State<AppState>) -> Json<Vec<ConfigVersion>> {
    Json(state.store.config_versions.list())
}

/// GET /config/current - Get the current config version.
async fn current_version(State(state): State<AppState>) -> Result<Json<ConfigVersion>, StatusCode> {
    state
        .store
        .config_versions
        .current()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// POST /config/rollback - Rollback to a specific config version.
async fn rollback(
    State(state): State<AppState>,
    Json(req): Json<RollbackRequest>,
) -> Result<Json<ConfigVersion>, StatusCode> {
    state
        .store
        .config_versions
        .rollback(req.target_version)
        .map(Json)
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

    #[tokio::test]
    async fn list_versions_empty() {
        let (app, _store) = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/config/versions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let versions: Vec<ConfigVersion> = serde_json::from_slice(&body).unwrap();
        assert!(versions.is_empty());
    }

    #[tokio::test]
    async fn current_version_not_found() {
        let (app, _store) = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/config/current")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn current_version_exists() {
        let (app, store) = test_app();
        store
            .config_versions
            .push("initial".into(), serde_json::json!({"key": "value"}));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/config/current")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let version: ConfigVersion = serde_json::from_slice(&body).unwrap();
        assert_eq!(version.version, 1);
    }

    #[tokio::test]
    async fn rollback_success() {
        let (app, store) = test_app();
        store
            .config_versions
            .push("v1".into(), serde_json::json!({"state": "original"}));
        store
            .config_versions
            .push("v2".into(), serde_json::json!({"state": "modified"}));

        let req_body = serde_json::to_string(&RollbackRequest { target_version: 1 }).unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/config/rollback")
                    .header("content-type", "application/json")
                    .body(Body::from(req_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let version: ConfigVersion = serde_json::from_slice(&body).unwrap();
        assert_eq!(version.version, 3); // new version created by rollback
        assert_eq!(version.snapshot, serde_json::json!({"state": "original"}));
    }

    #[tokio::test]
    async fn rollback_nonexistent() {
        let (app, _store) = test_app();
        let req_body = serde_json::to_string(&RollbackRequest {
            target_version: 999,
        })
        .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/config/rollback")
                    .header("content-type", "application/json")
                    .body(Body::from(req_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
