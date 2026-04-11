//! Load balancer trait definition.

use std::sync::Arc;

use crate::error::Result;
use crate::node::Node;

/// Load balancer that selects a backend node for a given request.
///
/// Generic over `Req` (the request type used for selection) and `Resp`
/// (the response type containing the selected node and any additional data).
pub trait LoadBalancer<Req, Resp>: Send + Sync {
    /// Select a backend node for the given request.
    fn select(&self, request: &Req) -> Result<Resp>;

    /// Add a node to the load balancer pool.
    fn add_node(&self, node: Arc<dyn Node>);

    /// Remove a node by ID.
    fn remove_node(&self, node_id: &str);

    /// Get all online nodes.
    fn online_nodes(&self) -> Vec<Arc<dyn Node>>;

    /// Get all nodes (including offline).
    fn all_nodes(&self) -> Vec<Arc<dyn Node>>;

    /// Get a specific node by ID.
    fn get_node(&self, node_id: &str) -> Option<Arc<dyn Node>>;
}

/// L4 load balance request: client socket address.
#[derive(Debug, Clone)]
pub struct L4Request {
    pub client_addr: std::net::SocketAddr,
}

/// L4 load balance response: selected backend node.
#[derive(Debug, Clone)]
pub struct L4Response {
    pub node: Arc<dyn Node>,
}

impl std::fmt::Debug for dyn Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id())
            .field("address", &self.address())
            .field("state", &self.state())
            .finish()
    }
}

/// HTTP load balance request with protocol-specific data.
///
/// Uses `http::HeaderMap` to avoid per-header `String` allocations -- the `http`
/// crate stores header names as enum variants (for known headers) and values as
/// `Bytes`-backed slices, both of which are cheaper than owned `String`s on the
/// hot path.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub client_addr: std::net::SocketAddr,
    pub host: Option<String>,
    pub path: String,
    pub headers: http::HeaderMap,
}

/// HTTP load balance response with optional headers to inject.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub node: Arc<dyn Node>,
    pub headers_to_add: http::HeaderMap,
}
