//! Health check CRUD endpoints.

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
use crate::store::HealthCheckEntry;

/// Request body for creating/updating a health check.
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthCheckRequest {
    pub cluster_id: String,
    #[serde(default = "default_interval")]
    pub interval_s: u64,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_rise")]
    pub rise: u32,
    #[serde(default = "default_fall")]
    pub fall: u32,
    pub http_method: Option<String>,
    pub http_path: Option<String>,
    pub expected_status: Option<Vec<u16>>,
}

fn default_interval() -> u64 {
    10
}
fn default_timeout() -> u64 {
    5000
}
fn default_rise() -> u32 {
    2
}
fn default_fall() -> u32 {
    3
}

/// Build health check routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health-checks", post(create_health_check))
        .route("/health-checks", get(list_health_checks))
        .route("/health-checks/{id}", put(update_health_check))
        .route("/health-checks/{id}", delete(delete_health_check))
}

/// POST /health-checks - Create a new health check.
async fn create_health_check(
    State(state): State<AppState>,
    Json(req): Json<HealthCheckRequest>,
) -> (StatusCode, Json<HealthCheckEntry>) {
    let now = Utc::now();
    let id = Uuid::new_v4().to_string();
    let entry = HealthCheckEntry {
        id: id.clone(),
        cluster_id: req.cluster_id,
        interval_s: req.interval_s,
        timeout_ms: req.timeout_ms,
        rise: req.rise,
        fall: req.fall,
        http_method: req.http_method,
        http_path: req.http_path,
        expected_status: req.expected_status,
        created_at: now,
        updated_at: now,
    };
    state.store.health_checks.insert(id, entry.clone());
    (StatusCode::CREATED, Json(entry))
}

/// GET /health-checks - List all health checks.
async fn list_health_checks(State(state): State<AppState>) -> Json<Vec<HealthCheckEntry>> {
    let checks: Vec<HealthCheckEntry> = state
        .store
        .health_checks
        .iter()
        .map(|r| r.value().clone())
        .collect();
    Json(checks)
}

/// PUT /health-checks/:id - Update a health check.
async fn update_health_check(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<HealthCheckRequest>,
) -> Result<Json<HealthCheckEntry>, StatusCode> {
    let mut entry = state
        .store
        .health_checks
        .get_mut(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    entry.cluster_id = req.cluster_id;
    entry.interval_s = req.interval_s;
    entry.timeout_ms = req.timeout_ms;
    entry.rise = req.rise;
    entry.fall = req.fall;
    entry.http_method = req.http_method;
    entry.http_path = req.http_path;
    entry.expected_status = req.expected_status;
    entry.updated_at = Utc::now();

    Ok(Json(entry.value().clone()))
}

/// DELETE /health-checks/:id - Delete a health check.
async fn delete_health_check(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state
        .store
        .health_checks
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

    fn hc_json() -> String {
        serde_json::to_string(&HealthCheckRequest {
            cluster_id: "cluster-1".into(),
            interval_s: 10,
            timeout_ms: 5000,
            rise: 2,
            fall: 3,
            http_method: Some("GET".into()),
            http_path: Some("/healthz".into()),
            expected_status: Some(vec![200]),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn create_and_list_health_checks() {
        let (app, _store) = test_app();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/health-checks")
                    .header("content-type", "application/json")
                    .body(Body::from(hc_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/health-checks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let checks: Vec<HealthCheckEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].cluster_id, "cluster-1");
    }

    #[tokio::test]
    async fn update_health_check() {
        let (app, _store) = test_app();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/health-checks")
                    .header("content-type", "application/json")
                    .body(Body::from(hc_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: HealthCheckEntry = serde_json::from_slice(&body).unwrap();

        let update = serde_json::to_string(&HealthCheckRequest {
            cluster_id: "cluster-2".into(),
            interval_s: 30,
            timeout_ms: 10000,
            rise: 3,
            fall: 5,
            http_method: Some("HEAD".into()),
            http_path: Some("/ready".into()),
            expected_status: Some(vec![200, 204]),
        })
        .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/v1/controlplane/health-checks/{}", created.id))
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
        let updated: HealthCheckEntry = serde_json::from_slice(&body).unwrap();
        assert_eq!(updated.cluster_id, "cluster-2");
        assert_eq!(updated.interval_s, 30);
    }

    #[tokio::test]
    async fn delete_health_check() {
        let (app, store) = test_app();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/health-checks")
                    .header("content-type", "application/json")
                    .body(Body::from(hc_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: HealthCheckEntry = serde_json::from_slice(&body).unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/controlplane/health-checks/{}", created.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(store.health_checks.is_empty());
    }
}
