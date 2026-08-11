//! L7 proxy engine with protocol bridging and frame pipeline.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![allow(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod authority;
pub mod grpc_proxy;
pub mod h1_proxy;
pub mod h1_to_h1;
pub mod h1_to_h2;
pub mod h1_to_h3;
pub mod h2_proxy;
pub mod h2_security;
pub mod h2_to_h1;
pub mod h2_to_h2;
pub mod h2_to_h3;
pub mod h3_to_h1;
pub mod h3_to_h2;
pub mod h3_to_h3;
pub mod security_hooks;
pub mod sni_authority;
pub mod stripped_request;
/// ROUND8-OPS-06 / REL-2-07: L7 wire-in for W3C trace-context propagation.
pub mod trace_ctx;
pub mod upstream;
pub mod ws_proxy;

use h1_to_h1::H1ToH1Bridge;
use h1_to_h2::H1ToH2Bridge;
use h1_to_h3::H1ToH3Bridge;
use h2_to_h1::H2ToH1Bridge;
use h2_to_h2::H2ToH2Bridge;
use h2_to_h3::H2ToH3Bridge;
use h3_to_h1::H3ToH1Bridge;
use h3_to_h2::H3ToH2Bridge;
use h3_to_h3::H3ToH3Bridge;

use bytes::Bytes;

/// Header-flood cap, enforced by every bridge in both directions.
pub const MAX_HEADERS: usize = 256;

/// HTTP protocol version for bridging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// HTTP/1.1
    Http1,
    /// HTTP/2
    Http2,
    /// HTTP/3 (QUIC-based)
    Http3,
}

/// Protocol-neutral HTTP request IR that every bridge converts into.
#[derive(Debug, Clone)]
pub struct BridgeRequest {
    /// HTTP method (e.g., "GET", "POST").
    pub method: String,
    /// Request URI / path.
    pub uri: String,
    /// Header list; may contain `:`-prefixed pseudo-headers for H2/H3.
    pub headers: Vec<(String, String)>,
    /// Request body bytes (ref-counted, zero-copy clone).
    pub body: Bytes,
    /// URI scheme; `None` is interpreted as `"https"` when minting `:scheme`.
    pub scheme: Option<String>,
    /// PROTO-2-12 — trailer fields (RFC 9110 §6.6), threaded through so the
    /// destination protocol can re-emit them.
    pub trailers: Vec<(String, String)>,
}

impl Default for BridgeRequest {
    fn default() -> Self {
        Self {
            method: "GET".to_owned(),
            uri: "/".to_owned(),
            headers: Vec::new(),
            body: Bytes::new(),
            scheme: None,
            trailers: Vec::new(),
        }
    }
}

/// Protocol-neutral HTTP response representation for bridging.
#[derive(Debug, Clone)]
pub struct BridgeResponse {
    /// HTTP status code (e.g., 200, 404).
    pub status: u16,
    /// Response header list.
    pub headers: Vec<(String, String)>,
    /// Response body bytes (ref-counted, zero-copy clone).
    pub body: Bytes,
    /// PROTO-2-12 — trailer fields; see [`BridgeRequest::trailers`].
    pub trailers: Vec<(String, String)>,
}

impl Default for BridgeResponse {
    fn default() -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: Bytes::new(),
            trailers: Vec::new(),
        }
    }
}

/// Errors that can occur during L7 protocol bridging.
#[derive(Debug, thiserror::Error)]
pub enum L7Error {
    /// A generic bridge processing error.
    #[error("bridge error: {0}")]
    BridgeError(String),

    /// A required pseudo-header was missing from the request.
    #[error("missing required pseudo-header: {0}")]
    MissingPseudoHeader(String),

    /// The requested source/destination protocol combination is not supported.
    #[error("unsupported bridge: {src:?} -> {dst:?}")]
    UnsupportedBridge {
        /// Source protocol.
        src: Protocol,
        /// Destination protocol.
        dst: Protocol,
    },

    /// The request or response contains more headers than [`MAX_HEADERS`].
    #[error("too many headers: {count} exceeds limit {max}")]
    TooManyHeaders {
        /// Actual header count.
        count: usize,
        /// Configured maximum.
        max: usize,
    },
}

/// Check that the header count does not exceed [`MAX_HEADERS`].
pub(crate) const fn check_header_count(count: usize) -> Result<(), L7Error> {
    if count > MAX_HEADERS {
        return Err(L7Error::TooManyHeaders {
            count,
            max: MAX_HEADERS,
        });
    }
    Ok(())
}

/// Trait for protocol bridges: handles the header and metadata transformations
/// required when proxying between different HTTP versions.
pub trait Bridge: Send + Sync {
    /// Transform a request from the source to the destination representation.
    ///
    /// # Errors
    /// [`L7Error`] if it cannot be bridged.
    fn bridge_request(&self, req: &BridgeRequest) -> Result<BridgeRequest, L7Error>;

    /// Transform a response back to the source protocol representation.
    ///
    /// # Errors
    /// [`L7Error`] if it cannot be bridged.
    fn bridge_response(&self, resp: &BridgeResponse) -> Result<BridgeResponse, L7Error>;

    /// The protocol this bridge accepts as input.
    fn source_protocol(&self) -> Protocol;

    /// The protocol this bridge produces as output.
    fn dest_protocol(&self) -> Protocol;
}

/// Create a bridge for a source/destination pair; all 9 combinations exist.
#[must_use]
pub fn create_bridge(source: Protocol, dest: Protocol) -> Box<dyn Bridge> {
    match (source, dest) {
        (Protocol::Http1, Protocol::Http1) => Box::new(H1ToH1Bridge),
        (Protocol::Http1, Protocol::Http2) => Box::new(H1ToH2Bridge),
        (Protocol::Http1, Protocol::Http3) => Box::new(H1ToH3Bridge),
        (Protocol::Http2, Protocol::Http1) => Box::new(H2ToH1Bridge),
        (Protocol::Http2, Protocol::Http2) => Box::new(H2ToH2Bridge),
        (Protocol::Http2, Protocol::Http3) => Box::new(H2ToH3Bridge),
        (Protocol::Http3, Protocol::Http1) => Box::new(H3ToH1Bridge),
        (Protocol::Http3, Protocol::Http2) => Box::new(H3ToH2Bridge),
        (Protocol::Http3, Protocol::Http3) => Box::new(H3ToH3Bridge),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_bridge_combinations_report_correct_protocols() {
        let combos = [
            (Protocol::Http1, Protocol::Http1),
            (Protocol::Http1, Protocol::Http2),
            (Protocol::Http1, Protocol::Http3),
            (Protocol::Http2, Protocol::Http1),
            (Protocol::Http2, Protocol::Http2),
            (Protocol::Http2, Protocol::Http3),
            (Protocol::Http3, Protocol::Http1),
            (Protocol::Http3, Protocol::Http2),
            (Protocol::Http3, Protocol::Http3),
        ];

        for (src, dst) in combos {
            let bridge = create_bridge(src, dst);
            assert_eq!(bridge.source_protocol(), src);
            assert_eq!(bridge.dest_protocol(), dst);
        }
    }

    #[test]
    fn bridge_preserves_body() {
        let body = Bytes::from_static(b"hello world");
        let req = BridgeRequest {
            method: "POST".into(),
            uri: "/test".into(),
            headers: vec![("host".into(), "localhost".into())],
            body: body.clone(),
            scheme: None,
            trailers: Vec::new(),
        };

        let bridge = create_bridge(Protocol::Http1, Protocol::Http2);
        let bridged = bridge.bridge_request(&req).unwrap();
        assert_eq!(bridged.body, body);
    }

    #[test]
    fn bridge_response_preserves_status_and_body() {
        let resp = BridgeResponse {
            status: 404,
            headers: vec![("content-type".into(), "text/plain".into())],
            body: Bytes::from_static(b"not found"),
            trailers: Vec::new(),
        };

        let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
        let bridged = bridge.bridge_response(&resp).unwrap();
        assert_eq!(bridged.status, 404);
        assert_eq!(bridged.body, &b"not found"[..]);
    }

    #[test]
    fn too_many_headers_rejected() {
        let headers: Vec<(String, String)> = (0..=MAX_HEADERS)
            .map(|i| (format!("x-hdr-{i}"), "v".into()))
            .collect();
        let req = BridgeRequest {
            method: "GET".into(),
            uri: "/".into(),
            headers,
            body: Bytes::new(),
            scheme: None,
            trailers: Vec::new(),
        };
        let bridge = create_bridge(Protocol::Http1, Protocol::Http1);
        let err = bridge.bridge_request(&req).unwrap_err();
        assert!(matches!(err, L7Error::TooManyHeaders { .. }));
    }

    #[test]
    fn h1_to_h1_strips_hop_by_hop_headers() {
        let req = BridgeRequest {
            method: "GET".into(),
            uri: "/".into(),
            headers: vec![
                ("host".into(), "example.com".into()),
                ("connection".into(), "keep-alive, x-custom".into()),
                ("keep-alive".into(), "timeout=5".into()),
                ("x-custom".into(), "should-be-removed".into()),
                ("accept".into(), "text/html".into()),
            ],
            body: Bytes::new(),
            scheme: None,
            trailers: Vec::new(),
        };
        let bridge = create_bridge(Protocol::Http1, Protocol::Http1);
        let bridged = bridge.bridge_request(&req).unwrap();
        let names: Vec<&str> = bridged.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(!names.contains(&"connection"));
        assert!(!names.contains(&"keep-alive"));
        assert!(!names.contains(&"x-custom"));
        assert!(names.contains(&"host"));
        assert!(names.contains(&"accept"));
    }

    #[test]
    fn h2_to_h1_missing_authority_errors() {
        let req = BridgeRequest {
            method: "GET".into(),
            uri: "/".into(),
            headers: vec![
                (":method".into(), "GET".into()),
                (":path".into(), "/".into()),
                (":scheme".into(), "https".into()),
                // No :authority
            ],
            body: Bytes::new(),
            scheme: None,
            trailers: Vec::new(),
        };
        let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
        let err = bridge.bridge_request(&req).unwrap_err();
        assert!(matches!(err, L7Error::MissingPseudoHeader(_)));
    }

    #[test]
    fn h2_to_h1_empty_authority_errors() {
        let req = BridgeRequest {
            method: "GET".into(),
            uri: "/".into(),
            headers: vec![
                (":method".into(), "GET".into()),
                (":path".into(), "/".into()),
                (":scheme".into(), "https".into()),
                (":authority".into(), String::new()),
            ],
            body: Bytes::new(),
            scheme: None,
            trailers: Vec::new(),
        };
        let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
        let err = bridge.bridge_request(&req).unwrap_err();
        assert!(matches!(err, L7Error::MissingPseudoHeader(_)));
    }

    #[test]
    fn h3_to_h1_empty_authority_errors() {
        let req = BridgeRequest {
            method: "GET".into(),
            uri: "/".into(),
            headers: vec![
                (":method".into(), "GET".into()),
                (":path".into(), "/".into()),
                (":scheme".into(), "https".into()),
                (":authority".into(), String::new()),
            ],
            body: Bytes::new(),
            scheme: None,
            trailers: Vec::new(),
        };
        let bridge = create_bridge(Protocol::Http3, Protocol::Http1);
        let err = bridge.bridge_request(&req).unwrap_err();
        assert!(matches!(err, L7Error::MissingPseudoHeader(_)));
    }

    #[test]
    fn h1_to_h1_response_strips_te_trailers() {
        let resp = BridgeResponse {
            status: 200,
            headers: vec![
                ("content-type".into(), "text/plain".into()),
                ("te".into(), "trailers".into()),
            ],
            body: Bytes::new(),
            trailers: Vec::new(),
        };
        let bridge = create_bridge(Protocol::Http1, Protocol::Http1);
        let bridged = bridge.bridge_response(&resp).unwrap();
        let names: Vec<&str> = bridged.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            !names.contains(&"te"),
            "TE must not appear in H1-to-H1 response"
        );
        assert!(names.contains(&"content-type"));
    }

    #[test]
    fn h1_to_h2_uses_custom_scheme() {
        let req = BridgeRequest {
            method: "GET".into(),
            uri: "/".into(),
            headers: vec![("host".into(), "example.com".into())],
            body: Bytes::new(),
            scheme: Some("http".into()),
            trailers: Vec::new(),
        };
        let bridge = create_bridge(Protocol::Http1, Protocol::Http2);
        let bridged = bridge.bridge_request(&req).unwrap();
        let scheme = bridged
            .headers
            .iter()
            .find(|(k, _)| k == ":scheme")
            .map(|(_, v)| v.as_str());
        assert_eq!(scheme, Some("http"));
    }
}
