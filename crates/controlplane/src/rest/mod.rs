//! REST API for the control plane (axum-based).
//!
//! All routes are mounted under `/api/v1/controlplane/`.

pub mod clusters;
pub mod config;
pub mod health_checks;
pub mod listeners;
pub mod nodes;
pub mod routes;
pub mod status;
pub mod tls_certs;

use axum::Router;
use std::sync::Arc;

use crate::store::ControlPlaneStore;

/// Shared application state for all REST handlers.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<ControlPlaneStore>,
}

/// Build the complete control plane REST router.
///
/// All routes are nested under `/api/v1/controlplane/`.
pub fn router(store: Arc<ControlPlaneStore>) -> Router {
    let state = AppState { store };

    let api = Router::new()
        .merge(nodes::routes())
        .merge(clusters::routes())
        .merge(routes::routes())
        .merge(listeners::routes())
        .merge(health_checks::routes())
        .merge(tls_certs::routes())
        .merge(status::routes())
        .merge(config::routes())
        .with_state(state);

    Router::new().nest("/api/v1/controlplane", api)
}
