//! WebSocket-over-HTTP/2 support (RFC 8441).
//!
//! HTTP/2 supports WebSocket connections via the Extended CONNECT method.
//! Multiple WebSocket connections can be multiplexed over a single HTTP/2
//! connection, each mapped to a separate H2 stream.
//!
//! Note: The `:protocol` pseudo-header is represented in the `http` crate's
//! [`Request`] extensions (since pseudo-headers are not regular headers).
//! We store it as a [`H2Protocol`] extension on the request.

use http::{Method, Request};

/// The `:protocol` pseudo-header value for WebSocket over HTTP/2.
pub const H2_WS_PROTOCOL: &str = "websocket";

/// Extension type representing the HTTP/2 `:protocol` pseudo-header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H2Protocol(pub String);

/// Check if an HTTP/2 request is an Extended CONNECT for WebSocket.
///
/// RFC 8441 requires:
/// - Method is CONNECT
/// - `:protocol` pseudo-header is `websocket`
///
/// The `:protocol` pseudo-header must be stored as a [`H2Protocol`]
/// extension on the request.
pub fn is_h2_websocket_upgrade<T>(req: &Request<T>) -> bool {
    req.method() == Method::CONNECT
        && req
            .extensions()
            .get::<H2Protocol>()
            .map(|p| p.0.eq_ignore_ascii_case(H2_WS_PROTOCOL))
            .unwrap_or(false)
}

/// Metadata for an HTTP/2 WebSocket stream.
#[derive(Debug, Clone)]
pub struct H2WebSocketStream {
    /// The HTTP/2 stream ID.
    pub stream_id: u32,
    /// The target path (from `:path` pseudo-header).
    pub path: String,
    /// Optional subprotocol from `Sec-WebSocket-Protocol`.
    pub subprotocol: Option<String>,
}

impl H2WebSocketStream {
    /// Extract H2 WebSocket stream metadata from a request.
    pub fn from_request<T>(req: &Request<T>, stream_id: u32) -> Option<Self> {
        if !is_h2_websocket_upgrade(req) {
            return None;
        }

        let path = req.uri().path().to_string();
        let subprotocol = req
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        Some(Self {
            stream_id,
            path,
            subprotocol,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect_request_with_protocol(protocol: &str) -> Request<()> {
        let mut req = Request::builder()
            .method(Method::CONNECT)
            .uri("https://example.com/ws")
            .body(())
            .unwrap();
        req.extensions_mut()
            .insert(H2Protocol(protocol.to_string()));
        req
    }

    #[test]
    fn detects_h2_websocket() {
        let req = connect_request_with_protocol("websocket");
        assert!(is_h2_websocket_upgrade(&req));
    }

    #[test]
    fn rejects_non_connect() {
        let mut req = Request::builder()
            .method(Method::GET)
            .uri("https://example.com/ws")
            .body(())
            .unwrap();
        req.extensions_mut()
            .insert(H2Protocol("websocket".to_string()));
        assert!(!is_h2_websocket_upgrade(&req));
    }

    #[test]
    fn rejects_missing_protocol() {
        let req = Request::builder()
            .method(Method::CONNECT)
            .uri("https://example.com/ws")
            .body(())
            .unwrap();
        assert!(!is_h2_websocket_upgrade(&req));
    }

    #[test]
    fn rejects_wrong_protocol() {
        let req = connect_request_with_protocol("h2c");
        assert!(!is_h2_websocket_upgrade(&req));
    }

    #[test]
    fn stream_metadata_extraction() {
        let mut req = Request::builder()
            .method(Method::CONNECT)
            .uri("https://example.com/chat")
            .header("sec-websocket-protocol", "graphql-ws")
            .body(())
            .unwrap();
        req.extensions_mut()
            .insert(H2Protocol("websocket".to_string()));

        let stream = H2WebSocketStream::from_request(&req, 3).unwrap();
        assert_eq!(stream.stream_id, 3);
        assert_eq!(stream.path, "/chat");
        assert_eq!(stream.subprotocol.as_deref(), Some("graphql-ws"));
    }

    #[test]
    fn case_insensitive_protocol() {
        let req = connect_request_with_protocol("WebSocket");
        assert!(is_h2_websocket_upgrade(&req));
    }
}
