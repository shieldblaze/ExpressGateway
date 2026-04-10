//! Main HTTP/2 proxy handler with stream multiplexing.
//!
//! Each incoming HTTP/2 stream is independently routed through the L7 load
//! balancer, allowing per-stream backend selection. The handler supports
//! configurable maximum concurrent streams.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tracing::debug;

use expressgateway_core::lb::{HttpRequest, HttpResponse, LoadBalancer};

use crate::body::BodyConfig;
use crate::body_limits::BodyLimitsConfig;
use crate::errors::HttpError;
use crate::flood_protection::ControlFrameRateLimiter;
use crate::headers;
use crate::uri;

/// Configuration for the HTTP/2 proxy handler.
#[derive(Debug, Clone)]
pub struct H2ProxyConfig {
    /// Proxy identifier for `Via` header.
    pub proxy_name: String,
    /// Maximum concurrent streams per connection.
    pub max_concurrent_streams: u32,
    /// Initial window size (bytes).
    pub initial_window_size: u32,
    /// Connection-level window size (bytes).
    pub connection_window_size: u32,
    /// Maximum frame size (bytes).
    pub max_frame_size: u32,
    /// Maximum header list size (bytes).
    pub max_header_list_size: u32,
    /// Request body configuration.
    pub body_config: BodyConfig,
    /// Body limits configuration.
    pub body_limits: BodyLimitsConfig,
    /// Backend connection timeout.
    pub connect_timeout: Duration,
    /// Backend response timeout.
    pub response_timeout: Duration,
}

impl Default for H2ProxyConfig {
    fn default() -> Self {
        Self {
            proxy_name: "expressgateway".to_string(),
            max_concurrent_streams: 100,
            initial_window_size: 65_535,
            connection_window_size: 1_048_576, // 1 MB
            max_frame_size: 16_384,
            max_header_list_size: 32 * 1024,
            body_config: BodyConfig::default(),
            body_limits: BodyLimitsConfig::default(),
            connect_timeout: Duration::from_secs(5),
            response_timeout: Duration::from_secs(30),
        }
    }
}

/// HTTP/2 proxy handler for processing multiplexed streams.
///
/// Each stream is independently routed to a backend via the load balancer.
/// The handler tracks active streams and enforces concurrency limits.
pub struct H2ProxyHandler {
    /// Configuration for this handler.
    config: H2ProxyConfig,
    /// L7 load balancer for backend selection.
    lb: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>>,
    /// Currently active stream count.
    active_streams: AtomicU32,
    /// Control frame flood protection.
    flood_limiter: parking_lot::Mutex<ControlFrameRateLimiter>,
}

impl H2ProxyHandler {
    /// Create a new HTTP/2 proxy handler.
    pub fn new(
        config: H2ProxyConfig,
        lb: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>>,
    ) -> Self {
        Self {
            config,
            lb,
            active_streams: AtomicU32::new(0),
            flood_limiter: parking_lot::Mutex::new(ControlFrameRateLimiter::new()),
        }
    }

    /// Process an incoming HTTP/2 stream request.
    ///
    /// Validates the request, selects a backend via the load balancer, and
    /// returns the processed request ready for forwarding.
    pub fn process_stream(
        &self,
        method: &str,
        path: &str,
        authority: &str,
        req_headers: &http::header::HeaderMap,
        client_addr: SocketAddr,
        is_tls: bool,
    ) -> Result<H2ProcessedStream, HttpError> {
        // Check concurrent streams limit.
        let current = self.active_streams.load(Ordering::Acquire);
        if current >= self.config.max_concurrent_streams {
            return Err(HttpError::service_unavailable(
                "maximum concurrent streams reached",
            ));
        }

        // Normalize URI.
        let normalized = uri::normalize(path).map_err(|e| HttpError::bad_request(e.to_string()))?;

        // Build load-balancer request.
        let lb_req = HttpRequest {
            client_addr,
            host: if authority.is_empty() {
                None
            } else {
                Some(authority.to_string())
            },
            path: normalized.path.clone(),
            headers: req_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
                .collect(),
        };

        // Select backend.
        let lb_resp = self
            .lb
            .select(&lb_req)
            .map_err(|e| HttpError::from_core_error(&e))?;

        // Build forwarded headers.
        let mut fwd_headers = req_headers.clone();
        headers::strip_hop_by_hop(&mut fwd_headers);
        headers::inject_proxy_headers(
            &mut fwd_headers,
            client_addr.ip(),
            is_tls,
            "2",
            &self.config.proxy_name,
        );

        debug!(
            backend = %lb_resp.node.address(),
            method = %method,
            path = %normalized.path,
            "forwarding H2 stream"
        );

        Ok(H2ProcessedStream {
            backend_addr: lb_resp.node.address(),
            method: method.to_string(),
            path: normalized.path,
            query: normalized.query,
            authority: authority.to_string(),
            headers: fwd_headers,
        })
    }

    /// Acquire a stream slot. Returns `true` if the stream was acquired.
    pub fn acquire_stream(&self) -> bool {
        loop {
            let current = self.active_streams.load(Ordering::Acquire);
            if current >= self.config.max_concurrent_streams {
                return false;
            }
            if self
                .active_streams
                .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    /// Release a stream slot.
    pub fn release_stream(&self) {
        let prev = self.active_streams.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0, "release_stream called with 0 active streams");
    }

    /// Number of currently active streams.
    pub fn active_streams(&self) -> u32 {
        self.active_streams.load(Ordering::Relaxed)
    }

    /// Check a control frame against the flood protection rate limiter.
    ///
    /// Returns `true` if the frame is allowed, `false` if a GOAWAY should be sent.
    pub fn check_control_frame(
        &self,
        frame_type: crate::flood_protection::ControlFrameType,
    ) -> bool {
        self.flood_limiter.lock().check_frame(frame_type)
    }

    /// Access the configuration.
    pub fn config(&self) -> &H2ProxyConfig {
        &self.config
    }
}

/// A validated HTTP/2 stream ready for forwarding.
#[derive(Debug)]
pub struct H2ProcessedStream {
    /// Backend address to connect to.
    pub backend_addr: SocketAddr,
    /// HTTP method.
    pub method: String,
    /// Normalized path.
    pub path: String,
    /// Query string (if any).
    pub query: Option<String>,
    /// Authority (host).
    pub authority: String,
    /// Headers to send to the backend.
    pub headers: http::header::HeaderMap,
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::lb::{HttpRequest, HttpResponse, LoadBalancer};
    use expressgateway_core::node::{Node, NodeImpl};

    struct TestLb {
        node: Arc<dyn Node>,
    }

    impl LoadBalancer<HttpRequest, HttpResponse> for TestLb {
        fn select(&self, _request: &HttpRequest) -> expressgateway_core::Result<HttpResponse> {
            Ok(HttpResponse {
                node: self.node.clone(),
                headers_to_add: Vec::new(),
            })
        }
        fn add_node(&self, _node: Arc<dyn Node>) {}
        fn remove_node(&self, _node_id: &str) {}
        fn online_nodes(&self) -> Vec<Arc<dyn Node>> {
            vec![self.node.clone()]
        }
        fn all_nodes(&self) -> Vec<Arc<dyn Node>> {
            vec![self.node.clone()]
        }
        fn get_node(&self, _node_id: &str) -> Option<Arc<dyn Node>> {
            Some(self.node.clone())
        }
    }

    fn make_handler() -> H2ProxyHandler {
        let node = NodeImpl::new_arc("n1".into(), ([127, 0, 0, 1], 9090).into(), 1, None);
        let lb: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>> = Arc::new(TestLb { node });
        H2ProxyHandler::new(H2ProxyConfig::default(), lb)
    }

    #[test]
    fn process_stream_basic() {
        let handler = make_handler();
        let headers = http::header::HeaderMap::new();
        let result = handler
            .process_stream(
                "GET",
                "/hello",
                "example.com",
                &headers,
                "10.0.0.1:1234".parse().unwrap(),
                false,
            )
            .unwrap();
        assert_eq!(result.backend_addr.port(), 9090);
        assert_eq!(result.method, "GET");
        assert_eq!(result.path, "/hello");
    }

    #[test]
    fn acquire_release_streams() {
        let handler = make_handler();
        assert_eq!(handler.active_streams(), 0);
        assert!(handler.acquire_stream());
        assert_eq!(handler.active_streams(), 1);
        handler.release_stream();
        assert_eq!(handler.active_streams(), 0);
    }

    #[test]
    fn max_concurrent_streams_enforced() {
        let node = NodeImpl::new_arc("n1".into(), ([127, 0, 0, 1], 9090).into(), 1, None);
        let lb: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>> = Arc::new(TestLb { node });
        let config = H2ProxyConfig {
            max_concurrent_streams: 2,
            ..H2ProxyConfig::default()
        };
        let handler = H2ProxyHandler::new(config, lb);

        assert!(handler.acquire_stream());
        assert!(handler.acquire_stream());
        assert!(!handler.acquire_stream()); // At limit.

        handler.release_stream();
        assert!(handler.acquire_stream()); // Available again.
    }
}
