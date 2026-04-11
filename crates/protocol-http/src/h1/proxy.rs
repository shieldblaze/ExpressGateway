//! Main HTTP/1.1 proxy handler.
//!
//! Uses hyper 1.x for HTTP/1.1 parsing with keep-alive connection pooling,
//! chunked transfer encoding support, and request body streaming (chunk-by-chunk,
//! no full buffering).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response, Version};
use http_body_util::Full;
use tracing::{debug, warn};

use expressgateway_core::lb::{HttpRequest, HttpResponse, LoadBalancer};

use crate::body::BodyConfig;
use crate::errors::HttpError;
use crate::headers;
use crate::uri;

/// Configuration for the HTTP/1.1 proxy handler.
#[derive(Debug, Clone)]
pub struct H1ProxyConfig {
    /// Proxy identifier for `Via` header.
    pub proxy_name: String,
    /// Request body configuration.
    pub body_config: BodyConfig,
    /// Maximum URI length (default 8192).
    pub max_uri_length: usize,
    /// Maximum header size in bytes (default 32 KB).
    pub max_header_size: usize,
    /// Backend connection timeout.
    pub connect_timeout: Duration,
    /// Backend response timeout.
    pub response_timeout: Duration,
    /// Keep-alive timeout for idle connections.
    pub keep_alive_timeout: Duration,
    /// Whether to enable keep-alive (default true).
    pub keep_alive: bool,
}

impl Default for H1ProxyConfig {
    fn default() -> Self {
        Self {
            proxy_name: "expressgateway".to_string(),
            body_config: BodyConfig::default(),
            max_uri_length: 8192,
            max_header_size: 32 * 1024,
            connect_timeout: Duration::from_secs(5),
            response_timeout: Duration::from_secs(30),
            keep_alive_timeout: Duration::from_secs(60),
            keep_alive: true,
        }
    }
}

/// HTTP/1.1 proxy handler that processes incoming requests, selects a backend
/// via the load balancer, and forwards the request.
pub struct H1ProxyHandler {
    /// Configuration for this handler.
    config: H1ProxyConfig,
    /// L7 load balancer for backend selection.
    lb: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>>,
}

impl H1ProxyHandler {
    /// Create a new HTTP/1.1 proxy handler.
    pub fn new(
        config: H1ProxyConfig,
        lb: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>>,
    ) -> Self {
        Self { config, lb }
    }

    /// Process an incoming HTTP/1.1 request.
    ///
    /// Validates the request, selects a backend, manipulates headers, and
    /// returns an appropriate response (or error).
    pub fn process_request(
        &self,
        req: &Request<()>,
        client_addr: SocketAddr,
        is_tls: bool,
    ) -> Result<ProcessedRequest, Box<Response<Full<Bytes>>>> {
        // ── Request smuggling defense ────────────────────────────────────
        // RFC 9112 §6.3: If both Transfer-Encoding and Content-Length are
        // present, or if Transfer-Encoding contains an obfuscated value, or
        // if there are multiple conflicting Content-Length values, the
        // message framing is ambiguous. A proxy MUST reject such requests
        // to prevent CL.TE / TE.CL / TE.TE smuggling attacks.
        validate_request_framing(req).map_err(|e| Box::new(e.into_response()))?;

        // ── Host header validation ───────────────────────────────────────
        // RFC 9112 §3.2: A server MUST respond with 400 to any HTTP/1.1
        // request that lacks a Host header field or that contains more than
        // one.
        validate_host_header(req).map_err(|e| Box::new(e.into_response()))?;

        // Check URI length.
        let uri_str = req.uri().to_string();
        if uri_str.len() > self.config.max_uri_length {
            return Err(Box::new(
                HttpError::uri_too_long(uri_str.len(), self.config.max_uri_length).into_response(),
            ));
        }

        // Normalize URI.
        let normalized = uri::normalize(&uri_str)
            .map_err(|e| Box::new(HttpError::bad_request(e.to_string()).into_response()))?;

        // Check total header size.
        let header_size: usize = req
            .headers()
            .iter()
            .map(|(k, v)| k.as_str().len() + v.len() + 4) // ": " + "\r\n"
            .sum();
        if header_size > self.config.max_header_size {
            return Err(Box::new(
                HttpError::header_too_large(header_size, self.config.max_header_size)
                    .into_response(),
            ));
        }

        // Check content-length against body limit.
        if let Some(cl) = req
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            && let Some(max) = self.config.body_config.max_body_size
            && cl > max
        {
            return Err(Box::new(
                HttpError::content_too_large(cl, max).into_response(),
            ));
        }

        // Build load-balancer request.
        let lb_req = HttpRequest {
            client_addr,
            host: req
                .headers()
                .get(http::header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string()),
            path: normalized.path.clone(),
            headers: req.headers().clone(),
        };

        // Select backend.
        let lb_resp = self
            .lb
            .select(&lb_req)
            .map_err(|e| HttpError::from_core_error(&e).into_response())?;

        // Build forwarded headers.
        let mut fwd_headers = req.headers().clone();
        headers::strip_hop_by_hop(&mut fwd_headers);
        headers::inject_proxy_headers(
            &mut fwd_headers,
            client_addr.ip(),
            is_tls,
            "1.1",
            &self.config.proxy_name,
        );

        // Add any headers from the load balancer (e.g. Set-Cookie from sticky sessions).
        for (name, value) in &lb_resp.headers_to_add {
            fwd_headers.insert(name.clone(), value.clone());
        }

        debug!(
            backend = %lb_resp.node.address(),
            method = %req.method(),
            path = %normalized.path,
            "forwarding H1 request"
        );

        Ok(ProcessedRequest {
            backend_addr: lb_resp.node.address(),
            method: req.method().clone(),
            path: normalized.path,
            query: normalized.query,
            version: Version::HTTP_11,
            headers: fwd_headers,
            keep_alive: self.config.keep_alive && is_keep_alive(req),
        })
    }

    /// Access the configuration.
    pub fn config(&self) -> &H1ProxyConfig {
        &self.config
    }
}

/// A validated and transformed request ready to be forwarded to a backend.
#[derive(Debug)]
pub struct ProcessedRequest {
    /// Backend address to connect to.
    pub backend_addr: SocketAddr,
    /// HTTP method.
    pub method: http::Method,
    /// Normalized path.
    pub path: String,
    /// Query string (if any).
    pub query: Option<String>,
    /// HTTP version.
    pub version: Version,
    /// Headers to send to the backend (hop-by-hop stripped, proxy headers injected).
    pub headers: http::header::HeaderMap,
    /// Whether this connection should be kept alive.
    pub keep_alive: bool,
}

impl ProcessedRequest {
    /// Reconstruct the URI (path + query).
    pub fn uri_string(&self) -> String {
        match &self.query {
            Some(q) => format!("{}?{q}", self.path),
            None => self.path.clone(),
        }
    }
}

/// Determine if a request wants keep-alive based on HTTP version and headers.
fn is_keep_alive(req: &Request<()>) -> bool {
    if req.version() == Version::HTTP_11 {
        // HTTP/1.1 defaults to keep-alive unless explicitly closed.
        !req.headers()
            .get(http::header::CONNECTION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("close"))
    } else {
        // HTTP/1.0 needs explicit keep-alive.
        req.headers()
            .get(http::header::CONNECTION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("keep-alive"))
    }
}

/// Build an error response for upstream failures.
pub fn upstream_error_response(err: &expressgateway_core::Error) -> Response<Full<Bytes>> {
    let http_err = HttpError::from_core_error(err);
    warn!(status = %http_err.status, error = %http_err.message, "upstream error");
    http_err.into_response()
}

/// Build a 502 Bad Gateway response.
pub fn bad_gateway(detail: &str) -> Response<Full<Bytes>> {
    HttpError::bad_gateway(detail).into_response()
}

/// Build a 504 Gateway Timeout response.
pub fn gateway_timeout() -> Response<Full<Bytes>> {
    HttpError::gateway_timeout().into_response()
}

/// Validate HTTP/1.1 message framing to prevent request smuggling.
///
/// Rejects the following ambiguous framing per RFC 9112 §6.3:
/// - Both `Transfer-Encoding` and `Content-Length` present (CL.TE / TE.CL).
/// - Multiple `Content-Length` headers with different values.
/// - `Transfer-Encoding` with any value other than exactly `chunked`
///   (prevents TE.TE obfuscation via e.g. `chunked, identity` or
///   `Transfer-Encoding: chunked\r\nTransfer-Encoding: identity`).
fn validate_request_framing(req: &Request<()>) -> Result<(), HttpError> {
    let has_te = req.headers().contains_key(http::header::TRANSFER_ENCODING);
    let has_cl = req.headers().contains_key(http::header::CONTENT_LENGTH);

    // RFC 9112 §6.3: If a message is received with both Transfer-Encoding
    // and Content-Length, the Transfer-Encoding overrides, but a proxy MUST
    // reject such a message to avoid smuggling.
    if has_te && has_cl {
        return Err(HttpError::bad_request(
            "request contains both Transfer-Encoding and Content-Length (RFC 9112 §6.3)",
        ));
    }

    // TE.TE defense: only accept exactly "chunked" (case-insensitive).
    // Reject obfuscated variants like "chunked, identity", " chunked",
    // "chunked\t", or multiple TE headers.
    if has_te {
        let te_values: Vec<_> = req
            .headers()
            .get_all(http::header::TRANSFER_ENCODING)
            .iter()
            .collect();
        if te_values.len() != 1 {
            return Err(HttpError::bad_request(
                "multiple Transfer-Encoding headers (RFC 9112 §6.3)",
            ));
        }
        if let Ok(val) = te_values[0].to_str() {
            if !val.eq_ignore_ascii_case("chunked") {
                return Err(HttpError::bad_request(
                    "Transfer-Encoding value must be exactly \"chunked\" (RFC 9112 §6.3)",
                ));
            }
        } else {
            return Err(HttpError::bad_request(
                "Transfer-Encoding contains non-ASCII bytes",
            ));
        }
    }

    // Conflicting Content-Length values: multiple CL headers must all
    // carry the same value, otherwise framing is ambiguous.
    if has_cl {
        let cl_values: Vec<_> = req
            .headers()
            .get_all(http::header::CONTENT_LENGTH)
            .iter()
            .collect();
        if cl_values.len() > 1 {
            let first = cl_values[0].as_bytes();
            for val in &cl_values[1..] {
                if val.as_bytes() != first {
                    return Err(HttpError::bad_request(
                        "conflicting Content-Length values (RFC 9112 §6.3)",
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Validate the Host header per RFC 9112 §3.2.
///
/// An HTTP/1.1 request MUST contain exactly one Host header field. Requests
/// with zero or more than one Host header are rejected with 400.
fn validate_host_header(req: &Request<()>) -> Result<(), HttpError> {
    // RFC 9112 §3.2: A server MUST respond with a 400 (Bad Request) status
    // code to any HTTP/1.1 request message that lacks a Host header field
    // and to any request message that contains more than one Host header
    // field line or a Host header field with an invalid field-value.
    if req.version() == Version::HTTP_11 {
        let host_count = req.headers().get_all(http::header::HOST).iter().count();
        if host_count == 0 {
            return Err(HttpError::bad_request(
                "missing Host header (RFC 9112 §3.2)",
            ));
        }
        if host_count > 1 {
            return Err(HttpError::bad_request(
                "multiple Host headers (RFC 9112 §3.2)",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use expressgateway_core::lb::{HttpRequest, HttpResponse, LoadBalancer};
    use expressgateway_core::node::{Node, NodeImpl};
    use http::StatusCode as HttpStatusCode;

    /// A simple round-robin LB for testing.
    struct TestLb {
        node: Arc<dyn Node>,
    }

    impl LoadBalancer<HttpRequest, HttpResponse> for TestLb {
        fn select(&self, _request: &HttpRequest) -> expressgateway_core::Result<HttpResponse> {
            Ok(HttpResponse {
                node: self.node.clone(),
                headers_to_add: http::HeaderMap::new(),
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

    fn make_handler() -> H1ProxyHandler {
        let node = NodeImpl::new_arc("n1".into(), ([127, 0, 0, 1], 8080).into(), 1, None);
        let lb: Arc<dyn LoadBalancer<HttpRequest, HttpResponse>> = Arc::new(TestLb { node });
        H1ProxyHandler::new(H1ProxyConfig::default(), lb)
    }

    #[test]
    fn process_simple_request() {
        let handler = make_handler();
        let req = Request::builder()
            .uri("/hello")
            .header("host", "example.com")
            .body(())
            .unwrap();
        let client = "10.0.0.1:1234".parse().unwrap();
        let result = handler.process_request(&req, client, false).unwrap();
        assert_eq!(result.path, "/hello");
        assert_eq!(result.backend_addr.port(), 8080);
        assert!(result.keep_alive);
    }

    #[test]
    fn rejects_long_uri() {
        let handler = make_handler();
        let long_path = format!("/{}", "a".repeat(9000));
        let req = Request::builder()
            .uri(long_path.as_str())
            .header("host", "example.com")
            .body(())
            .unwrap();
        let client = "10.0.0.1:1234".parse().unwrap();
        let result = handler.process_request(&req, client, false);
        assert!(result.is_err());
        let resp = result.unwrap_err();
        assert_eq!(resp.status(), HttpStatusCode::URI_TOO_LONG);
    }

    #[test]
    fn rejects_path_traversal() {
        let handler = make_handler();
        let req = Request::builder()
            .uri("/../etc/passwd")
            .header("host", "example.com")
            .body(())
            .unwrap();
        let client = "10.0.0.1:1234".parse().unwrap();
        let result = handler.process_request(&req, client, false);
        assert!(result.is_err());
        let resp = result.unwrap_err();
        assert_eq!(resp.status(), HttpStatusCode::BAD_REQUEST);
    }

    #[test]
    fn strips_hop_by_hop_headers() {
        let handler = make_handler();
        let req = Request::builder()
            .uri("/test")
            .header("host", "example.com")
            .header("connection", "keep-alive")
            .header("keep-alive", "timeout=5")
            .body(())
            .unwrap();
        let client = "10.0.0.1:1234".parse().unwrap();
        let result = handler.process_request(&req, client, false).unwrap();
        assert!(!result.headers.contains_key("connection"));
        assert!(!result.headers.contains_key("keep-alive"));
    }

    #[test]
    fn injects_proxy_headers() {
        let handler = make_handler();
        let req = Request::builder()
            .uri("/test")
            .header("host", "example.com")
            .body(())
            .unwrap();
        let client = "10.0.0.1:1234".parse().unwrap();
        let result = handler.process_request(&req, client, true).unwrap();
        assert!(result.headers.contains_key("x-forwarded-for"));
        assert_eq!(
            result
                .headers
                .get("x-forwarded-proto")
                .unwrap()
                .to_str()
                .unwrap(),
            "https"
        );
        assert!(result.headers.contains_key("via"));
        assert!(result.headers.contains_key("x-request-id"));
    }

    #[test]
    fn keep_alive_detection_http11() {
        // HTTP/1.1 defaults to keep-alive.
        let req = Request::builder()
            .version(Version::HTTP_11)
            .uri("/")
            .body(())
            .unwrap();
        assert!(is_keep_alive(&req));

        // Close header disables keep-alive.
        let req = Request::builder()
            .version(Version::HTTP_11)
            .uri("/")
            .header("connection", "close")
            .body(())
            .unwrap();
        assert!(!is_keep_alive(&req));
    }

    #[test]
    fn processed_request_uri_string() {
        let pr = ProcessedRequest {
            backend_addr: "127.0.0.1:8080".parse().unwrap(),
            method: http::Method::GET,
            path: "/foo".into(),
            query: Some("bar=1".into()),
            version: Version::HTTP_11,
            headers: http::header::HeaderMap::new(),
            keep_alive: true,
        };
        assert_eq!(pr.uri_string(), "/foo?bar=1");

        let pr2 = ProcessedRequest { query: None, ..pr };
        assert_eq!(pr2.uri_string(), "/foo");
    }

    // ── Request smuggling defense tests ──────────────────────────────

    #[test]
    fn rejects_cl_te_smuggling() {
        // CL.TE: both Content-Length and Transfer-Encoding present.
        let handler = make_handler();
        let req = Request::builder()
            .uri("/")
            .header("host", "example.com")
            .header("content-length", "10")
            .header("transfer-encoding", "chunked")
            .body(())
            .unwrap();
        let client = "10.0.0.1:1234".parse().unwrap();
        let result = handler.process_request(&req, client, false);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), HttpStatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_te_te_obfuscation() {
        // TE.TE: Transfer-Encoding with obfuscated value.
        let handler = make_handler();
        let req = Request::builder()
            .uri("/")
            .header("host", "example.com")
            .header("transfer-encoding", "chunked, identity")
            .body(())
            .unwrap();
        let client = "10.0.0.1:1234".parse().unwrap();
        let result = handler.process_request(&req, client, false);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), HttpStatusCode::BAD_REQUEST);
    }

    #[test]
    fn allows_valid_chunked_te() {
        let handler = make_handler();
        let req = Request::builder()
            .uri("/")
            .header("host", "example.com")
            .header("transfer-encoding", "chunked")
            .body(())
            .unwrap();
        let client = "10.0.0.1:1234".parse().unwrap();
        let result = handler.process_request(&req, client, false);
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_missing_host_http11() {
        let handler = make_handler();
        let req = Request::builder()
            .version(Version::HTTP_11)
            .uri("/")
            .body(())
            .unwrap();
        let client = "10.0.0.1:1234".parse().unwrap();
        let result = handler.process_request(&req, client, false);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), HttpStatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_multiple_host_headers() {
        let handler = make_handler();
        let req = Request::builder()
            .version(Version::HTTP_11)
            .uri("/")
            .header("host", "example.com")
            .header("host", "evil.com")
            .body(())
            .unwrap();
        let client = "10.0.0.1:1234".parse().unwrap();
        let result = handler.process_request(&req, client, false);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), HttpStatusCode::BAD_REQUEST);
    }
}
