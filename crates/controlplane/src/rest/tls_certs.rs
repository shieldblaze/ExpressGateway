//! TLS certificate CRUD endpoints.

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
use crate::store::TlsCertEntry;

/// Request body for creating/updating a TLS certificate.
#[derive(Debug, Serialize, Deserialize)]
pub struct TlsCertRequest {
    pub hostname: String,
    pub cert_pem: String,
    #[serde(default)]
    pub has_private_key: bool,
    #[serde(default)]
    pub ocsp_stapling: bool,
}

/// Build TLS cert routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tls-certs", post(create_tls_cert))
        .route("/tls-certs", get(list_tls_certs))
        .route("/tls-certs/{id}", get(get_tls_cert))
        .route("/tls-certs/{id}", put(update_tls_cert))
        .route("/tls-certs/{id}", delete(delete_tls_cert))
}

/// POST /tls-certs - Upload a new TLS certificate.
async fn create_tls_cert(
    State(state): State<AppState>,
    Json(req): Json<TlsCertRequest>,
) -> (StatusCode, Json<TlsCertEntry>) {
    let now = Utc::now();
    let id = Uuid::new_v4().to_string();
    let entry = TlsCertEntry {
        id: id.clone(),
        hostname: req.hostname,
        cert_pem: req.cert_pem,
        has_private_key: req.has_private_key,
        ocsp_stapling: req.ocsp_stapling,
        created_at: now,
        updated_at: now,
    };
    state.store.tls_certs.insert(id, entry.clone());
    (StatusCode::CREATED, Json(entry))
}

/// GET /tls-certs - List all TLS certificates.
async fn list_tls_certs(State(state): State<AppState>) -> Json<Vec<TlsCertEntry>> {
    let certs: Vec<TlsCertEntry> = state
        .store
        .tls_certs
        .iter()
        .map(|r| r.value().clone())
        .collect();
    Json(certs)
}

/// GET /tls-certs/:id - Get a specific TLS certificate.
async fn get_tls_cert(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TlsCertEntry>, StatusCode> {
    state
        .store
        .tls_certs
        .get(&id)
        .map(|r| Json(r.value().clone()))
        .ok_or(StatusCode::NOT_FOUND)
}

/// PUT /tls-certs/:id - Update a TLS certificate.
async fn update_tls_cert(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TlsCertRequest>,
) -> Result<Json<TlsCertEntry>, StatusCode> {
    let mut entry = state
        .store
        .tls_certs
        .get_mut(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    entry.hostname = req.hostname;
    entry.cert_pem = req.cert_pem;
    entry.has_private_key = req.has_private_key;
    entry.ocsp_stapling = req.ocsp_stapling;
    entry.updated_at = Utc::now();

    Ok(Json(entry.value().clone()))
}

/// DELETE /tls-certs/:id - Delete a TLS certificate.
async fn delete_tls_cert(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state
        .store
        .tls_certs
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

    fn cert_json() -> String {
        serde_json::to_string(&TlsCertRequest {
            hostname: "example.com".into(),
            cert_pem: "-----BEGIN CERTIFICATE-----\nMIIB...\n-----END CERTIFICATE-----".into(),
            has_private_key: true,
            ocsp_stapling: false,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn create_and_list_certs() {
        let (app, _store) = test_app();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/tls-certs")
                    .header("content-type", "application/json")
                    .body(Body::from(cert_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/tls-certs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let certs: Vec<TlsCertEntry> = serde_json::from_slice(&body).unwrap();
        assert_eq!(certs.len(), 1);
        assert_eq!(certs[0].hostname, "example.com");
    }

    #[tokio::test]
    async fn update_cert() {
        let (app, _store) = test_app();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/tls-certs")
                    .header("content-type", "application/json")
                    .body(Body::from(cert_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: TlsCertEntry = serde_json::from_slice(&body).unwrap();

        let update = serde_json::to_string(&TlsCertRequest {
            hostname: "new.example.com".into(),
            cert_pem: "-----BEGIN CERTIFICATE-----\nNEW...\n-----END CERTIFICATE-----".into(),
            has_private_key: true,
            ocsp_stapling: true,
        })
        .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/v1/controlplane/tls-certs/{}", created.id))
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
        let updated: TlsCertEntry = serde_json::from_slice(&body).unwrap();
        assert_eq!(updated.hostname, "new.example.com");
        assert!(updated.ocsp_stapling);
    }

    #[tokio::test]
    async fn delete_cert() {
        let (app, store) = test_app();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/controlplane/tls-certs")
                    .header("content-type", "application/json")
                    .body(Body::from(cert_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: TlsCertEntry = serde_json::from_slice(&body).unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/controlplane/tls-certs/{}", created.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(store.tls_certs.is_empty());
    }
}
