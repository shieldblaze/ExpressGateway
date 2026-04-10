//! Listener CRUD endpoints.

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
use crate::store::ListenerEntry;

/// Request body for creating/updating a listener.
#[derive(Debug, Serialize, Deserialize)]
pub struct ListenerRequest {
    pub name: String,
    pub protocol: String,
    pub bind: String,
    pub tls_profile: Option<String>,
    pub http_versions: Option<Vec<String>>,
}

/// Build listener routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/listeners", post(create_listener))
        .route("/listeners", get(list_listeners))
        .route("/listeners/{id}", put(update_listener))
        .route("/listeners/{id}", delete(delete_listener))
}

/// POST /listeners - Create a new listener.
async fn create_listener(
    State(state): State<AppState>,
    Json(req): Json<ListenerRequest>,
) -> (StatusCode, Json<ListenerEntry>) {
    let now = Utc::now();
    let id = Uuid::new_v4().to_string();
    let entry = ListenerEntry {
        id: id.clone(),
        name: req.name,
        protocol: req.protocol,
        bind: req.bind,
        tls_profile: req.tls_profile,
        http_versions: req.http_versions,
        created_at: now,
        updated_at: now,
    };
    state.store.listeners.insert(id, entry.clone());
    (StatusCode::CREATED, Json(entry))
}

/// GET /listeners - List all listeners.
async fn list_listeners(State(state): State<AppState>) -> Json<Vec<ListenerEntry>> {
    let listeners: Vec<ListenerEntry> = state
        .store
        .listeners
        .iter()
        .map(|r| r.value().clone())
        .collect();
    Json(listeners)
}

/// PUT /listeners/:id - Update a listener.
async fn update_listener(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ListenerRequest>,
) -> Result<Json<ListenerEntry>, StatusCode> {
    let mut entry = state
        .store
        .listeners
        .get_mut(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    entry.name = req.name;
    entry.protocol = req.protocol;
    entry.bind = req.bind;
    entry.tls_profile = req.tls_profile;
    entry.http_versions = req.http_versions;
    entry.updated_at = Utc::now();

    Ok(Json(entry.value().clone()))
}

/// DELETE /listeners/:id - Delete a listener.
async fn delete_listener(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state
        .store
        .listeners
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

    fn listener_json() -> String {
        serde_json::to_string(&ListenerRequest {
            name: "http-frontend".into(),
            protocol: "http".into(),
            bind: "0.0.0.0:8080".into(),
            tls_profile: None,
            http_versions: Some(vec!["h1".into(), "h2".into()]),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn create_and_list_listeners() {
        let (app, _store) = test_app();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/listeners")
                    .header("content-type", "application/json")
                    .body(Body::from(listener_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/listeners")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let listeners: Vec<ListenerEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(listeners.len(), 1);
        assert_eq!(listeners[0].name, "http-frontend");
    }

    #[tokio::test]
    async fn update_listener() {
        let (app, _store) = test_app();

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/listeners")
                    .header("content-type", "application/json")
                    .body(Body::from(listener_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: ListenerEntry = serde_json::from_slice(&body).unwrap();

        // Update
        let update = serde_json::to_string(&ListenerRequest {
            name: "https-frontend".into(),
            protocol: "https".into(),
            bind: "0.0.0.0:8443".into(),
            tls_profile: Some("modern".into()),
            http_versions: Some(vec!["h2".into()]),
        })
        .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/v1/controlplane/listeners/{}", created.id))
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
        let updated: ListenerEntry = serde_json::from_slice(&body).unwrap();
        assert_eq!(updated.name, "https-frontend");
        assert_eq!(updated.protocol, "https");
    }

    #[tokio::test]
    async fn delete_listener() {
        let (app, store) = test_app();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/listeners")
                    .header("content-type", "application/json")
                    .body(Body::from(listener_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: ListenerEntry = serde_json::from_slice(&body).unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/controlplane/listeners/{}", created.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(store.listeners.is_empty());
    }
}
