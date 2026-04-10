//! Consistent-hash HTTP load balancer.
//!
//! Maps HTTP requests onto a hash ring using a configurable key source:
//! either a specific HTTP header value or the client IP address. Falls back
//! to the client IP when the requested header is absent.
//!
//! Uses `ArcSwap` for lock-free reads of the ring and node list on the
//! hot path. Mutations are serialised with a `Mutex`.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use arc_swap::ArcSwap;
use expressgateway_core::error::{Error, Result};
use expressgateway_core::lb::{HttpRequest, HttpResponse, LoadBalancer};
use expressgateway_core::node::Node;
use parking_lot::Mutex;

/// Number of virtual nodes per real backend node on the hash ring.
const VIRTUAL_NODES: usize = 150;

/// Source of the hash key for consistent hashing.
#[derive(Debug, Clone)]
pub enum HashKeySource {
    /// Use the value of the named HTTP header.
    Header(String),
    /// Use the client IP address.
    ClientIp,
}

/// Snapshot of ring + node list, swapped atomically.
struct RingSnapshot {
    ring: BTreeMap<u128, String>,
    nodes: Vec<Arc<dyn Node>>,
}

/// Consistent-hash HTTP load balancer.
///
/// Uses a Murmur3-128 hash ring with 150 virtual nodes per backend. The hash
/// key is derived from either an HTTP header or the client IP.
pub struct HttpConsistentHashBalancer {
    snapshot: ArcSwap<RingSnapshot>,
    mutation_lock: Mutex<()>,
    key_source: HashKeySource,
}

impl HttpConsistentHashBalancer {
    /// Create a new consistent-hash HTTP balancer.
    ///
    /// The `key_source` determines what request data is hashed for ring lookup.
    pub fn new(key_source: HashKeySource) -> Self {
        Self {
            snapshot: ArcSwap::new(Arc::new(RingSnapshot {
                ring: BTreeMap::new(),
                nodes: Vec::new(),
            })),
            mutation_lock: Mutex::new(()),
            key_source,
        }
    }

    /// Create a balancer that hashes on client IP.
    pub fn with_client_ip() -> Self {
        Self::new(HashKeySource::ClientIp)
    }

    /// Create a balancer that hashes on a specific HTTP header, falling back
    /// to client IP when the header is absent.
    pub fn with_header(header_name: impl Into<String>) -> Self {
        Self::new(HashKeySource::Header(header_name.into()))
    }
}

/// Hash a byte string with Murmur3-x64-128.
fn murmur3_hash(data: &[u8]) -> u128 {
    murmur3::murmur3_x64_128(&mut Cursor::new(data), 0).unwrap_or(0)
}

impl LoadBalancer<HttpRequest, HttpResponse> for HttpConsistentHashBalancer {
    fn select(&self, request: &HttpRequest) -> Result<HttpResponse> {
        let snap = self.snapshot.load();

        if snap.ring.is_empty() || snap.nodes.is_empty() {
            return Err(Error::NoHealthyBackend);
        }

        // Determine the hash key.
        let key = match &self.key_source {
            HashKeySource::Header(name) => {
                // Look for the header (case-insensitive).
                let lower = name.to_lowercase();
                request
                    .headers
                    .iter()
                    .find(|(k, _)| k.to_lowercase() == lower)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| request.client_addr.ip().to_string())
            }
            HashKeySource::ClientIp => request.client_addr.ip().to_string(),
        };

        let hash = murmur3_hash(key.as_bytes());

        // Walk the ring clockwise.
        let candidates = snap.ring.range(hash..).chain(snap.ring.range(..hash));

        for (_ring_hash, node_id) in candidates {
            if let Some(node) = snap.nodes.iter().find(|n| n.id() == node_id)
                && node.is_online()
            {
                return Ok(HttpResponse {
                    node: node.clone(),
                    headers_to_add: Vec::new(),
                });
            }
        }

        Err(Error::NoHealthyBackend)
    }

    fn add_node(&self, node: Arc<dyn Node>) {
        let _guard = self.mutation_lock.lock();
        let old = self.snapshot.load();
        let mut ring = old.ring.clone();
        let mut nodes = old.nodes.clone();

        let node_id = node.id().to_string();
        for i in 0..VIRTUAL_NODES {
            let vnode_key = format!("{}-vnode-{}", node_id, i);
            let hash = murmur3_hash(vnode_key.as_bytes());
            ring.insert(hash, node_id.clone());
        }
        nodes.push(node);

        self.snapshot.store(Arc::new(RingSnapshot { ring, nodes }));
    }

    fn remove_node(&self, node_id: &str) {
        let _guard = self.mutation_lock.lock();
        let old = self.snapshot.load();
        let mut ring = old.ring.clone();
        let mut nodes = old.nodes.clone();

        for i in 0..VIRTUAL_NODES {
            let vnode_key = format!("{}-vnode-{}", node_id, i);
            let hash = murmur3_hash(vnode_key.as_bytes());
            ring.remove(&hash);
        }
        nodes.retain(|n| n.id() != node_id);

        self.snapshot.store(Arc::new(RingSnapshot { ring, nodes }));
    }

    fn online_nodes(&self) -> Vec<Arc<dyn Node>> {
        let snap = self.snapshot.load();
        snap.nodes
            .iter()
            .filter(|n| n.is_online())
            .cloned()
            .collect()
    }

    fn all_nodes(&self) -> Vec<Arc<dyn Node>> {
        let snap = self.snapshot.load();
        snap.nodes.clone()
    }

    fn get_node(&self, node_id: &str) -> Option<Arc<dyn Node>> {
        let snap = self.snapshot.load();
        snap.nodes.iter().find(|n| n.id() == node_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::node::NodeImpl;

    fn make_node(id: &str, port: u16) -> Arc<dyn Node> {
        NodeImpl::new_arc(id.to_string(), ([127, 0, 0, 1], port).into(), 1, None)
    }

    fn make_request_with_header(header: &str, value: &str) -> HttpRequest {
        HttpRequest {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
            host: Some("example.com".into()),
            path: "/".into(),
            headers: vec![(header.to_string(), value.to_string())],
        }
    }

    #[test]
    fn stable_mapping_by_ip() {
        let lb = HttpConsistentHashBalancer::with_client_ip();
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));

        let req = HttpRequest {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
            host: None,
            path: "/".into(),
            headers: Vec::new(),
        };

        let first = lb.select(&req).unwrap().node.id().to_string();
        for _ in 0..10 {
            assert_eq!(lb.select(&req).unwrap().node.id(), first);
        }
    }

    #[test]
    fn hash_by_header() {
        let lb = HttpConsistentHashBalancer::with_header("X-User-Id");
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));

        let req_a = make_request_with_header("X-User-Id", "user-123");
        let req_b = make_request_with_header("X-User-Id", "user-456");

        let node_a = lb.select(&req_a).unwrap().node.id().to_string();
        let node_b = lb.select(&req_b).unwrap().node.id().to_string();

        // Same user always maps to same node.
        for _ in 0..10 {
            assert_eq!(lb.select(&req_a).unwrap().node.id(), node_a);
            assert_eq!(lb.select(&req_b).unwrap().node.id(), node_b);
        }
    }

    #[test]
    fn falls_back_to_client_ip_when_header_absent() {
        let lb = HttpConsistentHashBalancer::with_header("X-Missing");
        lb.add_node(make_node("n1", 8001));
        lb.add_node(make_node("n2", 8002));

        let req = HttpRequest {
            client_addr: "10.0.0.1:5000".parse().unwrap(),
            host: None,
            path: "/".into(),
            headers: Vec::new(), // No X-Missing header.
        };

        // Should not error; falls back to client IP.
        let resp = lb.select(&req).unwrap();
        assert!(!resp.node.id().is_empty());
    }
}
