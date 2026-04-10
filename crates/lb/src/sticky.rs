//! Cookie-based sticky session support.
//!
//! Wraps an inner HTTP load balancer and adds session affinity via a
//! `Set-Cookie` header. The cookie value is a SHA-256 hash of the node ID
//! to avoid leaking internal identifiers.

use std::collections::HashMap;
use std::sync::Arc;

use expressgateway_core::error::Result;
use expressgateway_core::lb::{HttpRequest, HttpResponse, LoadBalancer};
use expressgateway_core::node::Node;
use parking_lot::RwLock;
use sha2::{Digest, Sha256};

/// Cookie name used for sticky session routing.
pub const COOKIE_NAME: &str = "X-SBZ-EGW-RouteID";

/// Compute the SHA-256 hex digest of a node ID.
fn hash_node_id(node_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(node_id.as_bytes());
    let result = hasher.finalize();
    // Hex-encode the digest.
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Cookie-based sticky session HTTP load balancer.
///
/// On each request, checks for an existing route cookie. If the cookie is
/// present and the corresponding node is online, that node is returned.
/// Otherwise, the inner load balancer is consulted and a `Set-Cookie` header
/// is injected into the response.
///
/// Cookie attributes: `Domain` (from `Host` header), `Path=/`,
/// `HttpOnly=true`, `SameSite=Strict`, `Secure` when TLS.
pub struct StickySessionBalancer {
    inner: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>>,
    /// Maps hashed node ID -> node, for O(1) cookie lookups.
    hashed_id_to_node: RwLock<HashMap<String, Arc<dyn Node>>>,
    /// Whether to set the Secure cookie attribute.
    tls: bool,
}

impl StickySessionBalancer {
    /// Create a new sticky session balancer wrapping the given inner balancer.
    ///
    /// If `tls` is true, the `Secure` cookie attribute will be set.
    pub fn new(inner: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>>, tls: bool) -> Self {
        Self {
            inner,
            hashed_id_to_node: RwLock::new(HashMap::new()),
            tls,
        }
    }

    /// Build the Set-Cookie header value for a given node.
    fn build_cookie(&self, node: &dyn Node, host: Option<&str>) -> String {
        let hashed = hash_node_id(node.id());
        let mut cookie = format!(
            "{}={}; Path=/; HttpOnly; SameSite=Strict",
            COOKIE_NAME, hashed
        );
        if let Some(domain) = host {
            // Strip port if present.
            let domain = domain.split(':').next().unwrap_or(domain);
            cookie.push_str(&format!("; Domain={}", domain));
        }
        if self.tls {
            cookie.push_str("; Secure");
        }
        cookie
    }

    /// Extract the route cookie value from request headers.
    fn extract_cookie(request: &HttpRequest) -> Option<String> {
        for (name, value) in &request.headers {
            if name.to_lowercase() == "cookie" {
                // Parse cookie header: "name1=val1; name2=val2; ..."
                for part in value.split(';') {
                    let part = part.trim();
                    if let Some(val) = part.strip_prefix(COOKIE_NAME)
                        && let Some(val) = val.strip_prefix('=')
                    {
                        return Some(val.to_string());
                    }
                }
            }
        }
        None
    }
}

impl LoadBalancer<HttpRequest, HttpResponse> for StickySessionBalancer {
    fn select(&self, request: &HttpRequest) -> Result<HttpResponse> {
        // Check for existing cookie.
        if let Some(cookie_val) = Self::extract_cookie(request) {
            let map = self.hashed_id_to_node.read();
            if let Some(node) = map.get(&cookie_val)
                && node.is_online()
            {
                return Ok(HttpResponse {
                    node: node.clone(),
                    headers_to_add: Vec::new(),
                });
            }
        }

        // No valid cookie or node offline; delegate to inner balancer.
        let mut resp = self.inner.select(request)?;

        // Register the node in our hash map.
        let hashed = hash_node_id(resp.node.id());
        {
            let mut map = self.hashed_id_to_node.write();
            map.insert(hashed, resp.node.clone());
        }

        // Inject Set-Cookie header.
        let cookie = self.build_cookie(resp.node.as_ref(), request.host.as_deref());
        resp.headers_to_add.push(("Set-Cookie".to_string(), cookie));

        Ok(resp)
    }

    fn add_node(&self, node: Arc<dyn Node>) {
        let hashed = hash_node_id(node.id());
        self.hashed_id_to_node.write().insert(hashed, node.clone());
        self.inner.add_node(node);
    }

    fn remove_node(&self, node_id: &str) {
        let hashed = hash_node_id(node_id);
        self.hashed_id_to_node.write().remove(&hashed);
        self.inner.remove_node(node_id);
    }

    fn online_nodes(&self) -> Vec<Arc<dyn Node>> {
        self.inner.online_nodes()
    }

    fn all_nodes(&self) -> Vec<Arc<dyn Node>> {
        self.inner.all_nodes()
    }

    fn get_node(&self, node_id: &str) -> Option<Arc<dyn Node>> {
        self.inner.get_node(node_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l7::HttpRoundRobinBalancer;
    use expressgateway_core::node::{NodeImpl, NodeState};

    fn make_node(id: &str, port: u16) -> Arc<NodeImpl> {
        NodeImpl::new_arc(id.to_string(), ([127, 0, 0, 1], port).into(), 1, None)
    }

    fn make_request(host: Option<&str>, cookie: Option<&str>) -> HttpRequest {
        let mut headers = Vec::new();
        if let Some(c) = cookie {
            headers.push(("Cookie".to_string(), format!("{}={}", COOKIE_NAME, c)));
        }
        HttpRequest {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
            host: host.map(String::from),
            path: "/".into(),
            headers,
        }
    }

    #[test]
    fn injects_set_cookie_on_first_request() {
        let inner = Arc::new(HttpRoundRobinBalancer::new());
        let lb = StickySessionBalancer::new(inner, false);
        lb.add_node(make_node("n1", 8001));

        let req = make_request(Some("example.com"), None);
        let resp = lb.select(&req).unwrap();

        assert_eq!(resp.node.id(), "n1");
        assert_eq!(resp.headers_to_add.len(), 1);

        let (name, value) = &resp.headers_to_add[0];
        assert_eq!(name, "Set-Cookie");
        assert!(value.contains(COOKIE_NAME));
        assert!(value.contains("HttpOnly"));
        assert!(value.contains("SameSite=Strict"));
        assert!(value.contains("Domain=example.com"));
        assert!(!value.contains("Secure"));
    }

    #[test]
    fn secure_flag_when_tls() {
        let inner = Arc::new(HttpRoundRobinBalancer::new());
        let lb = StickySessionBalancer::new(inner, true);
        lb.add_node(make_node("n1", 8001));

        let req = make_request(Some("example.com"), None);
        let resp = lb.select(&req).unwrap();

        let (_, value) = &resp.headers_to_add[0];
        assert!(value.contains("Secure"));
    }

    #[test]
    fn sticky_routing_with_cookie() {
        let inner = Arc::new(HttpRoundRobinBalancer::new());
        let lb = StickySessionBalancer::new(inner, false);
        let n1 = make_node("n1", 8001);
        let n2 = make_node("n2", 8002);
        lb.add_node(n1);
        lb.add_node(n2);

        // First request: get a node and its cookie.
        let req = make_request(Some("example.com"), None);
        let resp = lb.select(&req).unwrap();
        let first_node = resp.node.id().to_string();

        // Extract cookie value from Set-Cookie header.
        let cookie_val = hash_node_id(&first_node);

        // Subsequent requests with the cookie should go to the same node.
        for _ in 0..10 {
            let req = make_request(Some("example.com"), Some(&cookie_val));
            let resp = lb.select(&req).unwrap();
            assert_eq!(resp.node.id(), first_node);
            // No new Set-Cookie header when using existing cookie.
            assert!(resp.headers_to_add.is_empty());
        }
    }

    #[test]
    fn falls_back_when_node_offline() {
        let inner = Arc::new(HttpRoundRobinBalancer::new());
        let lb = StickySessionBalancer::new(inner, false);
        let n1 = make_node("n1", 8001);
        let n2 = make_node("n2", 8002);
        lb.add_node(n1.clone());
        lb.add_node(n2.clone());

        // Get cookie for n1.
        let cookie_val = hash_node_id("n1");

        // Take n1 offline.
        n1.set_state(NodeState::Offline);

        // Should fall back to inner balancer (round-robin picks n2).
        let req = make_request(Some("example.com"), Some(&cookie_val));
        let resp = lb.select(&req).unwrap();
        assert_eq!(resp.node.id(), "n2");
        // Should inject a new cookie.
        assert!(!resp.headers_to_add.is_empty());
    }

    #[test]
    fn cookie_value_is_sha256_of_node_id() {
        let hash = hash_node_id("test-node-1");
        // SHA-256 hex digest should be 64 characters.
        assert_eq!(hash.len(), 64);
        // Should be deterministic.
        assert_eq!(hash, hash_node_id("test-node-1"));
        // Different IDs produce different hashes.
        assert_ne!(hash, hash_node_id("test-node-2"));
    }
}
