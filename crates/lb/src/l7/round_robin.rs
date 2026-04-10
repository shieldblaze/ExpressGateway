//! Round-robin HTTP load balancer.
//!
//! Identical algorithm to the L4 round-robin but operates on
//! [`HttpRequest`]/[`HttpResponse`].

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use expressgateway_core::error::{Error, Result};
use expressgateway_core::lb::{HttpRequest, HttpResponse, LoadBalancer};
use expressgateway_core::node::Node;
use parking_lot::RwLock;

/// Round-robin HTTP load balancer.
///
/// Distributes HTTP requests evenly across all online backend nodes using an
/// atomic rotating counter.
pub struct HttpRoundRobinBalancer {
    nodes: RwLock<Vec<Arc<dyn Node>>>,
    index: AtomicUsize,
}

impl HttpRoundRobinBalancer {
    /// Create a new HTTP round-robin balancer with no nodes.
    pub fn new() -> Self {
        Self {
            nodes: RwLock::new(Vec::new()),
            index: AtomicUsize::new(0),
        }
    }
}

impl Default for HttpRoundRobinBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer<HttpRequest, HttpResponse> for HttpRoundRobinBalancer {
    fn select(&self, _request: &HttpRequest) -> Result<HttpResponse> {
        let nodes = self.nodes.read();
        let online: Vec<_> = nodes.iter().filter(|n| n.is_online()).cloned().collect();
        if online.is_empty() {
            return Err(Error::NoHealthyBackend);
        }
        let idx = self.index.fetch_add(1, Ordering::Relaxed) % online.len();
        Ok(HttpResponse {
            node: online[idx].clone(),
            headers_to_add: Vec::new(),
        })
    }

    fn add_node(&self, node: Arc<dyn Node>) {
        self.nodes.write().push(node);
    }

    fn remove_node(&self, node_id: &str) {
        self.nodes.write().retain(|n| n.id() != node_id);
    }

    fn online_nodes(&self) -> Vec<Arc<dyn Node>> {
        self.nodes
            .read()
            .iter()
            .filter(|n| n.is_online())
            .cloned()
            .collect()
    }

    fn all_nodes(&self) -> Vec<Arc<dyn Node>> {
        self.nodes.read().clone()
    }

    fn get_node(&self, node_id: &str) -> Option<Arc<dyn Node>> {
        self.nodes
            .read()
            .iter()
            .find(|n| n.id() == node_id)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::node::NodeImpl;

    fn make_node(id: &str, port: u16) -> Arc<dyn Node> {
        NodeImpl::new_arc(id.to_string(), ([127, 0, 0, 1], port).into(), 1, None)
    }

    fn make_request() -> HttpRequest {
        HttpRequest {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
            host: Some("example.com".into()),
            path: "/".into(),
            headers: Vec::new(),
        }
    }

    #[test]
    fn cycles_through_nodes() {
        let lb = HttpRoundRobinBalancer::new();
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));

        let req = make_request();
        let r1 = lb.select(&req).unwrap();
        let r2 = lb.select(&req).unwrap();
        let r3 = lb.select(&req).unwrap();

        assert_eq!(r1.node.id(), "n1");
        assert_eq!(r2.node.id(), "n2");
        assert_eq!(r3.node.id(), "n1");
    }

    #[test]
    fn response_has_empty_headers() {
        let lb = HttpRoundRobinBalancer::new();
        lb.add_node(make_node("n1", 8001));
        let resp = lb.select(&make_request()).unwrap();
        assert!(resp.headers_to_add.is_empty());
    }
}
