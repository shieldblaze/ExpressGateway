//! Protocol translation between HTTP/1.1 and HTTP/2.
//!
//! **H1 -> H2**: Request line + headers -> pseudo-headers + HEADERS frame;
//! chunked body -> DATA frames; `Host` -> `:authority`.
//!
//! **H2 -> H1**: Pseudo-headers -> request line; DATA frames -> chunked body;
//! trailer HEADERS -> chunked trailers.

use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Request, Uri, Version};

/// Pseudo-header names used in HTTP/2 (RFC 9113 Section 8.3).
pub const PSEUDO_METHOD: &str = ":method";
pub const PSEUDO_SCHEME: &str = ":scheme";
pub const PSEUDO_AUTHORITY: &str = ":authority";
pub const PSEUDO_PATH: &str = ":path";
pub const PSEUDO_STATUS: &str = ":status";

/// Errors during protocol translation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TranslationError {
    /// Missing required pseudo-header.
    #[error("missing pseudo-header: {name}")]
    MissingPseudoHeader { name: String },

    /// Invalid header value.
    #[error("invalid header value: {detail}")]
    InvalidHeader { detail: String },

    /// Invalid URI.
    #[error("invalid URI: {detail}")]
    InvalidUri { detail: String },
}

/// Convert an HTTP/1.1 request into HTTP/2 pseudo-header representation.
///
/// - Extracts method, path, scheme (from `is_tls`), and authority (from `Host` header).
/// - Removes the `Host` header (replaced by `:authority`).
/// - Strips hop-by-hop headers that are not valid in HTTP/2.
pub fn h1_to_h2_headers(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    is_tls: bool,
) -> Result<H2PseudoHeaders, TranslationError> {
    let scheme = if is_tls { "https" } else { "http" };

    // Authority: prefer URI authority, fall back to Host header.
    let authority = uri
        .authority()
        .map(|a| a.to_string())
        .or_else(|| {
            headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    // Path: use URI path+query, default to "/".
    let path = if uri.path().is_empty() {
        "/".to_string()
    } else {
        match uri.query() {
            Some(q) => format!("{}?{q}", uri.path()),
            None => uri.path().to_string(),
        }
    };

    // Build clean headers without hop-by-hop and without Host.
    // Per RFC 9113 §8.2.2, connection-specific headers are forbidden in HTTP/2
    // except `te: trailers` which is the only allowed TE value.
    let mut clean_headers = HeaderMap::new();
    for (name, value) in headers.iter() {
        // Skip Host (promoted to :authority) and connection-specific headers.
        if name == header::HOST
            || name == header::CONNECTION
            || name == header::TRANSFER_ENCODING
            || name == HeaderName::from_static("keep-alive")
            || name == header::UPGRADE
            || name == HeaderName::from_static("proxy-connection")
        {
            continue;
        }
        // TE header: only `trailers` is allowed in HTTP/2 (RFC 9113 §8.2.2).
        if name == header::TE {
            if let Ok(v) = value.to_str()
                && v.eq_ignore_ascii_case("trailers")
            {
                clean_headers.append(name.clone(), value.clone());
            }
            // Skip any non-"trailers" TE values.
            continue;
        }
        clean_headers.append(name.clone(), value.clone());
    }

    Ok(H2PseudoHeaders {
        method: method.to_string(),
        scheme: scheme.to_string(),
        authority,
        path,
        headers: clean_headers,
    })
}

/// Pseudo-headers and regular headers for an HTTP/2 request.
#[derive(Debug, Clone)]
pub struct H2PseudoHeaders {
    /// `:method` pseudo-header value.
    pub method: String,
    /// `:scheme` pseudo-header value.
    pub scheme: String,
    /// `:authority` pseudo-header value.
    pub authority: String,
    /// `:path` pseudo-header value.
    pub path: String,
    /// Regular headers (no pseudo-headers, no hop-by-hop).
    pub headers: HeaderMap,
}

/// Convert HTTP/2 pseudo-headers back to an HTTP/1.1 request.
///
/// - `:method` -> request method
/// - `:path` -> request URI
/// - `:authority` -> `Host` header
/// - `:scheme` is consumed but not mapped to a header
///
/// Per RFC 9113 §8.2.2, connection-specific header fields are forbidden in
/// HTTP/2. This function rejects them during H2->H1 downgrade to prevent
/// smuggling through protocol translation.
pub fn h2_to_h1_request<B: Default>(
    method: &str,
    path: &str,
    authority: &str,
    headers: &HeaderMap,
) -> Result<Request<B>, TranslationError> {
    let method: Method = method
        .parse()
        .map_err(|_| TranslationError::InvalidHeader {
            detail: format!("invalid method: {method}"),
        })?;

    let uri: Uri = path.parse().map_err(|_| TranslationError::InvalidUri {
        detail: format!("invalid path: {path}"),
    })?;

    // RFC 9113 §8.2.2: Reject forbidden connection-specific headers that
    // should never appear in HTTP/2 frames. Their presence indicates a
    // malformed or smuggling-attempt H2 request.
    for (name, _) in headers.iter() {
        if name == header::CONNECTION
            || name == header::TRANSFER_ENCODING
            || name == header::UPGRADE
            || name == HeaderName::from_static("keep-alive")
            || name == HeaderName::from_static("proxy-connection")
        {
            return Err(TranslationError::InvalidHeader {
                detail: format!(
                    "forbidden connection-specific header in HTTP/2: {} (RFC 9113 §8.2.2)",
                    name
                ),
            });
        }
    }

    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .version(Version::HTTP_11);

    // Set Host header from :authority.
    if !authority.is_empty() {
        builder = builder.header(header::HOST, authority);
    }

    // Copy regular headers (already validated above).
    if let Some(h) = builder.headers_mut() {
        for (name, value) in headers.iter() {
            h.append(name.clone(), value.clone());
        }
    }

    builder
        .body(B::default())
        .map_err(|e| TranslationError::InvalidHeader {
            detail: e.to_string(),
        })
}

/// Determine the `Transfer-Encoding` strategy for H2-to-H1 body translation.
///
/// When the content length is known, returns `Some(length)`.
/// When the content length is unknown (streaming), the H1 response should use
/// chunked transfer encoding (return `None`).
pub fn h2_to_h1_body_encoding(content_length: Option<u64>) -> H1BodyEncoding {
    match content_length {
        Some(len) => H1BodyEncoding::ContentLength(len),
        None => H1BodyEncoding::Chunked,
    }
}

/// How to encode a body when translating from H2 to H1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H1BodyEncoding {
    /// Use `Content-Length` header with the known size.
    ContentLength(u64),
    /// Use `Transfer-Encoding: chunked`.
    Chunked,
}

/// Extract the `:authority` from a set of HTTP/2 pseudo-headers (for CONNECT).
///
/// Per RFC 9113 Section 8.5, CONNECT requests only have `:method` and `:authority`.
pub fn extract_connect_authority(headers: &HeaderMap) -> Option<String> {
    headers
        .get(PSEUDO_AUTHORITY)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Build the `Via` header value for protocol translation.
///
/// Format: `<received-protocol> <proxy-name>`.
pub fn via_header_value(version: Version, proxy_name: &str) -> HeaderValue {
    let proto = match version {
        Version::HTTP_11 => "1.1",
        Version::HTTP_2 => "2",
        _ => "1.0",
    };
    HeaderValue::from_str(&format!("{proto} {proxy_name}"))
        .unwrap_or_else(|_| HeaderValue::from_static("1.1 expressgateway"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h1_to_h2_basic_conversion() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));

        let uri: Uri = "/foo/bar?q=1".parse().unwrap();
        let result = h1_to_h2_headers(&Method::GET, &uri, &headers, true).unwrap();

        assert_eq!(result.method, "GET");
        assert_eq!(result.scheme, "https");
        assert_eq!(result.authority, "example.com");
        assert_eq!(result.path, "/foo/bar?q=1");
        // Host should not be in regular headers.
        assert!(!result.headers.contains_key(header::HOST));
        // Content-Type should be preserved.
        assert!(result.headers.contains_key(header::CONTENT_TYPE));
    }

    #[test]
    fn h1_to_h2_strips_connection_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(
            header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );

        let uri: Uri = "/".parse().unwrap();
        let result = h1_to_h2_headers(&Method::GET, &uri, &headers, false).unwrap();

        assert!(!result.headers.contains_key(header::CONNECTION));
        assert!(!result.headers.contains_key(header::TRANSFER_ENCODING));
    }

    #[test]
    fn h1_to_h2_preserves_te_trailers() {
        // RFC 9113 §8.2.2: `te: trailers` is the only TE value allowed in H2.
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        headers.insert(header::TE, HeaderValue::from_static("trailers"));

        let uri: Uri = "/".parse().unwrap();
        let result = h1_to_h2_headers(&Method::POST, &uri, &headers, false).unwrap();
        assert!(result.headers.contains_key(header::TE));
        assert_eq!(
            result.headers.get(header::TE).unwrap().to_str().unwrap(),
            "trailers"
        );
    }

    #[test]
    fn h1_to_h2_strips_te_non_trailers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        headers.insert(header::TE, HeaderValue::from_static("gzip"));

        let uri: Uri = "/".parse().unwrap();
        let result = h1_to_h2_headers(&Method::POST, &uri, &headers, false).unwrap();
        assert!(!result.headers.contains_key(header::TE));
    }

    #[test]
    fn h1_to_h2_http_scheme() {
        let headers = HeaderMap::new();
        let uri: Uri = "/".parse().unwrap();
        let result = h1_to_h2_headers(&Method::GET, &uri, &headers, false).unwrap();
        assert_eq!(result.scheme, "http");
    }

    #[test]
    fn h2_to_h1_basic_conversion() {
        let headers = HeaderMap::new();
        let req: Request<()> = h2_to_h1_request("GET", "/foo", "example.com", &headers).unwrap();

        assert_eq!(req.method(), Method::GET);
        assert_eq!(req.uri().path(), "/foo");
        assert_eq!(req.version(), Version::HTTP_11);
        assert_eq!(
            req.headers().get(header::HOST).unwrap().to_str().unwrap(),
            "example.com"
        );
    }

    #[test]
    fn h2_to_h1_empty_authority() {
        let headers = HeaderMap::new();
        let req: Request<()> = h2_to_h1_request("POST", "/bar", "", &headers).unwrap();
        assert!(!req.headers().contains_key(header::HOST));
    }

    #[test]
    fn h2_to_h1_invalid_method() {
        let headers = HeaderMap::new();
        let result: Result<Request<()>, _> =
            h2_to_h1_request("NOT A METHOD", "/", "example.com", &headers);
        assert!(result.is_err());
    }

    #[test]
    fn body_encoding_content_length() {
        assert_eq!(
            h2_to_h1_body_encoding(Some(42)),
            H1BodyEncoding::ContentLength(42)
        );
    }

    #[test]
    fn body_encoding_chunked() {
        assert_eq!(h2_to_h1_body_encoding(None), H1BodyEncoding::Chunked);
    }

    #[test]
    fn via_header_http11() {
        let val = via_header_value(Version::HTTP_11, "eg");
        assert_eq!(val.to_str().unwrap(), "1.1 eg");
    }

    #[test]
    fn via_header_http2() {
        let val = via_header_value(Version::HTTP_2, "eg");
        assert_eq!(val.to_str().unwrap(), "2 eg");
    }

    #[test]
    fn h1_to_h2_default_path() {
        let headers = HeaderMap::new();
        let uri: Uri = "/".parse().unwrap();
        let result = h1_to_h2_headers(&Method::GET, &uri, &headers, false).unwrap();
        assert_eq!(result.path, "/");
    }

    #[test]
    fn h2_to_h1_rejects_connection_header() {
        // RFC 9113 §8.2.2: Connection-specific headers are forbidden in H2.
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        let result: Result<Request<()>, _> =
            h2_to_h1_request("GET", "/", "example.com", &headers);
        assert!(result.is_err());
    }

    #[test]
    fn h2_to_h1_rejects_transfer_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
        let result: Result<Request<()>, _> =
            h2_to_h1_request("GET", "/", "example.com", &headers);
        assert!(result.is_err());
    }

    #[test]
    fn h2_to_h1_rejects_upgrade_header() {
        let mut headers = HeaderMap::new();
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        let result: Result<Request<()>, _> =
            h2_to_h1_request("GET", "/", "example.com", &headers);
        assert!(result.is_err());
    }

    #[test]
    fn h2_to_h1_rejects_keep_alive_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("keep-alive"),
            HeaderValue::from_static("timeout=5"),
        );
        let result: Result<Request<()>, _> =
            h2_to_h1_request("GET", "/", "example.com", &headers);
        assert!(result.is_err());
    }
}
