//! Prometheus metrics endpoint.

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};

use super::AppState;
use crate::node_registry::NodeRegistryState;

/// Build metrics routes.
pub fn routes() -> Router<AppState> {
    Router::new().route("/metrics", get(prometheus_metrics))
}

/// GET /metrics - Prometheus metrics in text exposition format.
async fn prometheus_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let store = &state.store;

    let mut connected = 0u64;
    let mut unhealthy = 0u64;
    let mut disconnected = 0u64;
    let mut draining = 0u64;

    for entry in store.nodes.iter() {
        match entry.value().state {
            NodeRegistryState::Connected => connected += 1,
            NodeRegistryState::Unhealthy => unhealthy += 1,
            NodeRegistryState::Disconnected => disconnected += 1,
            NodeRegistryState::Draining => draining += 1,
        }
    }

    let total_nodes = store.nodes.len() as u64;
    let total_clusters = store.clusters.len() as u64;
    let total_routes = store.routes.len() as u64;
    let total_listeners = store.listeners.len() as u64;
    let total_health_checks = store.health_checks.len() as u64;
    let total_tls_certs = store.tls_certs.len() as u64;
    let config_version = store
        .config_versions
        .current()
        .map(|v| v.version)
        .unwrap_or(0);

    // Build Prometheus text exposition format.
    let body = format!(
        "\
# HELP expressgateway_controlplane_nodes_total Total number of registered nodes.
# TYPE expressgateway_controlplane_nodes_total gauge
expressgateway_controlplane_nodes_total {total_nodes}
# HELP expressgateway_controlplane_nodes_connected Number of connected nodes.
# TYPE expressgateway_controlplane_nodes_connected gauge
expressgateway_controlplane_nodes_connected {connected}
# HELP expressgateway_controlplane_nodes_unhealthy Number of unhealthy nodes.
# TYPE expressgateway_controlplane_nodes_unhealthy gauge
expressgateway_controlplane_nodes_unhealthy {unhealthy}
# HELP expressgateway_controlplane_nodes_disconnected Number of disconnected nodes.
# TYPE expressgateway_controlplane_nodes_disconnected gauge
expressgateway_controlplane_nodes_disconnected {disconnected}
# HELP expressgateway_controlplane_nodes_draining Number of draining nodes.
# TYPE expressgateway_controlplane_nodes_draining gauge
expressgateway_controlplane_nodes_draining {draining}
# HELP expressgateway_controlplane_clusters_total Total number of clusters.
# TYPE expressgateway_controlplane_clusters_total gauge
expressgateway_controlplane_clusters_total {total_clusters}
# HELP expressgateway_controlplane_routes_total Total number of routes.
# TYPE expressgateway_controlplane_routes_total gauge
expressgateway_controlplane_routes_total {total_routes}
# HELP expressgateway_controlplane_listeners_total Total number of listeners.
# TYPE expressgateway_controlplane_listeners_total gauge
expressgateway_controlplane_listeners_total {total_listeners}
# HELP expressgateway_controlplane_health_checks_total Total number of health checks.
# TYPE expressgateway_controlplane_health_checks_total gauge
expressgateway_controlplane_health_checks_total {total_health_checks}
# HELP expressgateway_controlplane_tls_certs_total Total number of TLS certificates.
# TYPE expressgateway_controlplane_tls_certs_total gauge
expressgateway_controlplane_tls_certs_total {total_tls_certs}
# HELP expressgateway_controlplane_config_version Current configuration version.
# TYPE expressgateway_controlplane_config_version gauge
expressgateway_controlplane_config_version {config_version}
"
    );

    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

#[cfg(test)]
mod tests {
    use crate::node_registry::NodeEntry;
    use crate::rest;
    use crate::store::ControlPlaneStore;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    #[tokio::test]
    async fn metrics_empty_store() {
        let store = ControlPlaneStore::new_shared();
        let app = rest::router(store);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);

        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("text/plain"));

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("expressgateway_controlplane_nodes_total 0"));
        assert!(text.contains("expressgateway_controlplane_config_version 0"));
    }

    #[tokio::test]
    async fn metrics_with_nodes() {
        let store = ControlPlaneStore::new_shared();
        store.nodes.insert(
            "n1".into(),
            NodeEntry::new("n1".into(), "10.0.0.1:8080".into(), None),
        );
        store.nodes.insert(
            "n2".into(),
            NodeEntry::new("n2".into(), "10.0.0.2:8080".into(), None),
        );

        let app = rest::router(Arc::clone(&store));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("expressgateway_controlplane_nodes_total 2"));
        assert!(text.contains("expressgateway_controlplane_nodes_connected 2"));
    }
}
