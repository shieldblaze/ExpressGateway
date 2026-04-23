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
#![warn(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod h1_proxy;
pub mod h1_to_h1;
pub mod h1_to_h2;
pub mod h1_to_h3;
pub mod h2_to_h1;
pub mod h2_to_h2;
pub mod h2_to_h3;
pub mod h3_to_h1;
pub mod h3_to_h2;
pub mod h3_to_h3;

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

/// Maximum number of headers allowed through any bridge.
///
/// Protects against header-flooding attacks. Configurable at the type level;
/// individual bridges enforce this in both `bridge_request` and
/// `bridge_response`.
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

/// Protocol-neutral HTTP request representation for bridging.
///
/// This is the intermediate representation that all protocol bridges work with.
/// Each bridge converts from its source protocol into this form, potentially
/// transforming headers and metadata to match the destination protocol.
#[derive(Debug, Clone)]
pub struct BridgeRequest {
    /// HTTP method (e.g., "GET", "POST").
    pub method: String,
    /// Request URI / path.
    pub uri: String,
    /// Header list. May contain pseudo-headers (prefixed with `:`) for HTTP/2
    /// and HTTP/3 representations.
    pub headers: Vec<(String, String)>,
    /// Request body bytes (ref-counted, zero-copy clone).
    pub body: Bytes,
    /// URI scheme for the request (e.g., "https", "http").
    ///
    /// Used by H1-to-H2 and H1-to-H3 bridges to populate the `:scheme`
    /// pseudo-header.  Defaults to `None`, which bridges interpret as
    /// `"https"`.
    pub scheme: Option<String>,
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
///
/// Used by every bridge implementation in both request and response paths.
pub(crate) const fn check_header_count(count: usize) -> Result<(), L7Error> {
    if count > MAX_HEADERS {
        return Err(L7Error::TooManyHeaders {
            count,
            max: MAX_HEADERS,
        });
    }
    Ok(())
}

/// Trait for protocol bridges that convert between source and destination protocols.
///
/// Implementations handle the header and metadata transformations required when
/// proxying between different HTTP protocol versions.
pub trait Bridge: Send + Sync {
    /// Transform a request from the source protocol representation to the
    /// destination protocol representation.
    ///
    /// # Errors
    ///
    /// Returns [`L7Error`] if the request cannot be bridged (e.g., missing
    /// required pseudo-headers when converting from HTTP/2 to HTTP/1.1).
    fn bridge_request(&self, req: &BridgeRequest) -> Result<BridgeRequest, L7Error>;

    /// Transform a response from the destination protocol representation back
    /// to the source protocol representation.
    ///
    /// # Errors
    ///
    /// Returns [`L7Error`] if the response cannot be bridged.
    fn bridge_response(&self, resp: &BridgeResponse) -> Result<BridgeResponse, L7Error>;

    /// The protocol this bridge accepts as input.
    fn source_protocol(&self) -> Protocol;

    /// The protocol this bridge produces as output.
    fn dest_protocol(&self) -> Protocol;
}

/// Create a bridge for the given source and destination protocol combination.
///
/// Returns a boxed [`Bridge`] implementation that handles the header
/// transformations required for the specified protocol pair.
///
/// All 9 combinations of HTTP/1.1, HTTP/2, and HTTP/3 are supported.
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
