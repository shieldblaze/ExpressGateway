//! HTTP server that exposes `/metrics` in Prometheus exposition format and
//! `/healthz` for liveness probes.
//!
//! The metrics server runs on a dedicated Tokio task and must not interfere
//! with the data plane.  Gathering and encoding Prometheus metrics allocates
//! (unavoidable for text encoding), but this happens only on scrape intervals
//! (typically 15-30 s), not per-request.

use axum::{Router, response::IntoResponse, routing::get};
use hyper::StatusCode;
use prometheus::TextEncoder;
use std::net::SocketAddr;
use tokio::net::TcpListener;

use crate::registry::try_global_registry;

/// Handler for `GET /metrics`.
async fn metrics_handler() -> impl IntoResponse {
    let registry = match try_global_registry() {
        Some(r) => r,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "metrics registry not yet initialised",
            )
                .into_response();
        }
    };

    let encoder = TextEncoder::new();
    let metric_families = registry.registry.gather();

    match encoder.encode_to_string(&metric_families) {
        Ok(body) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(err) => {
            tracing::error!("failed to encode metrics: {}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, "metrics encoding failed").into_response()
        }
    }
}

/// Liveness probe endpoint.
async fn healthz_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// Build an [`axum::Router`] with the `/metrics` and `/healthz` endpoints.
pub fn metrics_router() -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/healthz", get(healthz_handler))
}

/// Start the metrics HTTP server on the given address.
///
/// Returns when `shutdown` resolves, allowing the caller to coordinate
/// graceful shutdown.
pub async fn serve_metrics(
    addr: SocketAddr,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let app = metrics_router();
    tracing::info!("metrics server listening on {}", addr);
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    tracing::info!("metrics server stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_metrics_endpoint() {
        // Ensure the global registry is initialised.
        let m = crate::registry::register_all();
        m.connections_total
            .with_label_values(&["default", "http"])
            .inc();

        let app = metrics_router();

        let req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();

        assert!(
            text.contains("expressgateway_connections_total"),
            "expected namespaced prometheus metrics in response body, got: {}",
            &text[..text.len().min(500)]
        );
    }

    #[tokio::test]
    async fn test_healthz_endpoint() {
        let app = metrics_router();

        let req = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"ok");
    }
}
