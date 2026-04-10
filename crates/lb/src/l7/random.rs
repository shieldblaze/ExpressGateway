//! Random HTTP load balancer.
//!
//! Selects a random online backend node using a thread-local RNG.

use std::sync::Arc;

use expressgateway_core::error::{Error, Result};
use expressgateway_core::lb::{HttpRequest, HttpResponse, LoadBalancer};
use expressgateway_core::node::Node;
use parking_lot::RwLock;
use rand::Rng;

/// Random HTTP load balancer.
///
/// Each HTTP request is routed to a uniformly random online node.
pub struct HttpRandomBalancer {
    nodes: RwLock<Vec<Arc<dyn Node>>>,
}

impl HttpRandomBalancer {
    /// Create a new random HTTP balancer with no nodes.
    pub fn new() -> Self {
        Self {
            nodes: RwLock::new(Vec::new()),
        }
    }
}

impl Default for HttpRandomBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer<HttpRequest, HttpResponse> for HttpRandomBalancer {
    fn select(&self, _request: &HttpRequest) -> Result<HttpResponse> {
        let nodes = self.nodes.read();
        let online: Vec<_> = nodes.iter().filter(|n| n.is_online()).cloned().collect();
        if online.is_empty() {
            return Err(Error::NoHealthyBackend);
        }
        let idx = rand::thread_rng().gen_range(0..online.len());
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
    use std::collections::HashSet;

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
    fn distributes_across_nodes() {
        let lb = HttpRandomBalancer::new();
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));
        lb.add_node(make_node("n3", 8003));

        let mut seen = HashSet::new();
        for _ in 0..100 {
            let resp = lb.select(&make_request()).unwrap();
            seen.insert(resp.node.id().to_string());
        }
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn empty_returns_error() {
        let lb = HttpRandomBalancer::new();
        assert!(lb.select(&make_request()).is_err());
    }
}
