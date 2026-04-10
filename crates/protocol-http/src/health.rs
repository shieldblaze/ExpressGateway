//! Health and readiness endpoints for the HTTP proxy.
//!
//! - `GET /health` returns 200 if the proxy is not draining, 503 otherwise.
//! - `GET /ready` returns 200 if at least one backend node is online, 503 otherwise.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use expressgateway_core::node::Node;
use http::{Response, StatusCode};
use http_body_util::Full;

/// State shared with the health endpoint handlers.
pub struct HealthState {
    /// Whether the proxy is currently draining (graceful shutdown).
    draining: AtomicBool,
    /// Backend nodes to check for readiness.
    nodes: parking_lot::RwLock<Vec<Arc<dyn Node>>>,
}

impl HealthState {
    /// Create a new health state (not draining, no nodes).
    pub fn new() -> Self {
        Self {
            draining: AtomicBool::new(false),
            nodes: parking_lot::RwLock::new(Vec::new()),
        }
    }

    /// Mark the proxy as draining.
    pub fn set_draining(&self, draining: bool) {
        self.draining.store(draining, Ordering::Release);
    }

    /// Whether the proxy is draining.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    /// Set the backend nodes.
    pub fn set_nodes(&self, nodes: Vec<Arc<dyn Node>>) {
        *self.nodes.write() = nodes;
    }

    /// Add a backend node.
    pub fn add_node(&self, node: Arc<dyn Node>) {
        self.nodes.write().push(node);
    }

    /// Check if at least one backend node is online.
    pub fn has_online_backend(&self) -> bool {
        self.nodes.read().iter().any(|n| n.is_online())
    }
}

impl Default for HealthState {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle a `GET /health` request.
///
/// Returns 200 with `{"status":"healthy"}` if the proxy is not draining,
/// or 503 with `{"status":"draining"}` if the proxy is shutting down.
pub fn handle_health(state: &HealthState) -> Response<Full<Bytes>> {
    if state.is_draining() {
        json_response(StatusCode::SERVICE_UNAVAILABLE, r#"{"status":"draining"}"#)
    } else {
        json_response(StatusCode::OK, r#"{"status":"healthy"}"#)
    }
}

/// Handle a `GET /ready` request.
///
/// Returns 200 with `{"status":"ready"}` if at least one backend is online,
/// or 503 with `{"status":"not_ready"}` if no backends are available.
pub fn handle_ready(state: &HealthState) -> Response<Full<Bytes>> {
    if state.has_online_backend() {
        json_response(StatusCode::OK, r#"{"status":"ready"}"#)
    } else {
        json_response(StatusCode::SERVICE_UNAVAILABLE, r#"{"status":"not_ready"}"#)
    }
}

/// Build a JSON response with the given status and body.
///
/// Infallible in practice: status is always valid and header names are static.
fn json_response(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("content-length", body.len().to_string())
        .body(Full::new(Bytes::from(body.to_owned())))
        .unwrap_or_else(|_| {
            // Unreachable, but never panic on the data path.
            Response::new(Full::new(Bytes::from_static(
                b"{\"status\":\"error\"}",
            )))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::node::NodeImpl;

    fn make_node(id: &str, port: u16) -> Arc<dyn Node> {
        NodeImpl::new_arc(id.to_string(), ([127, 0, 0, 1], port).into(), 1, None)
    }

    #[test]
    fn health_ok_when_not_draining() {
        let state = HealthState::new();
        let resp = handle_health(&state);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn health_503_when_draining() {
        let state = HealthState::new();
        state.set_draining(true);
        let resp = handle_health(&state);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn ready_ok_with_online_backend() {
        let state = HealthState::new();
        state.add_node(make_node("n1", 8001));
        let resp = handle_ready(&state);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn ready_503_with_no_backends() {
        let state = HealthState::new();
        let resp = handle_ready(&state);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn ready_503_when_all_backends_offline() {
        let state = HealthState::new();
        let node = NodeImpl::new_arc("n1".to_string(), ([127, 0, 0, 1], 8001).into(), 1, None);
        node.set_state(expressgateway_core::node::NodeState::Offline);
        state.add_node(node);
        let resp = handle_ready(&state);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn ready_ok_if_at_least_one_online() {
        let state = HealthState::new();
        let offline = NodeImpl::new_arc("n1".to_string(), ([127, 0, 0, 1], 8001).into(), 1, None);
        offline.set_state(expressgateway_core::node::NodeState::Offline);
        state.add_node(offline);
        state.add_node(make_node("n2", 8002));
        let resp = handle_ready(&state);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn health_response_has_json_content_type() {
        let state = HealthState::new();
        let resp = handle_health(&state);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json"
        );
    }
}
