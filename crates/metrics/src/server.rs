//! HTTP server that exposes a `/metrics` endpoint in Prometheus exposition
//! format.

use axum::{Router, response::IntoResponse, routing::get};
use hyper::StatusCode;
use prometheus::TextEncoder;
use std::net::SocketAddr;
use tokio::net::TcpListener;

use crate::registry::global_registry;

/// Handler for `GET /metrics`.
async fn metrics_handler() -> impl IntoResponse {
    let registry = global_registry();
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

/// Build an [`axum::Router`] with the `/metrics` endpoint.
pub fn metrics_router() -> Router {
    Router::new().route("/metrics", get(metrics_handler))
}

/// Start the metrics HTTP server on the given address.
///
/// This function blocks until the server shuts down.
pub async fn serve_metrics(addr: SocketAddr) -> std::io::Result<()> {
    let app = metrics_router();
    tracing::info!("metrics server listening on {}", addr);
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
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
        // Ensure the global registry is initialised and record some data so
        // that the Prometheus encoder actually produces output.
        let m = crate::registry::register_all();
        m.connections_total.with_label_values(&["http"]).inc();

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

        // The response should contain the metric we recorded.
        assert!(
            text.contains("connections_total"),
            "expected prometheus metrics in response body, got: {}",
            &text[..text.len().min(500)]
        );
    }
}
