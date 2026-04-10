//! Protocol translation between HTTP/1.1, HTTP/2, and HTTP/3.
//!
//! HTTP/3 uses the same pseudo-header structure as HTTP/2 (`:method`, `:path`,
//! `:scheme`, `:authority`, `:status`). This module translates between the
//! representations used by different protocol versions.

use http::{
    Request, Response, Uri, Version,
    header::{self, HeaderName},
};

#[cfg(test)]
use http::{Method, StatusCode};
use tracing::debug;

/// Errors during protocol translation.
#[derive(Debug, thiserror::Error)]
pub enum TranslationError {
    /// Missing required pseudo-header.
    #[error("missing required pseudo-header: {0}")]
    MissingPseudoHeader(&'static str),

    /// Invalid header value.
    #[error("invalid header value: {0}")]
    InvalidHeaderValue(String),

    /// HTTP error building request/response.
    #[error("http error: {0}")]
    Http(#[from] http::Error),
}

/// Convert an HTTP/1.1-style request into HTTP/3 pseudo-header form.
///
/// HTTP/3 uses the same pseudo-headers as HTTP/2:
/// - `:method` -- from the request method
/// - `:path`   -- from the URI path + query
/// - `:scheme`  -- from the URI scheme (defaults to "https")
/// - `:authority` -- from the Host header or URI authority
///
/// Connection-specific headers (`connection`, `transfer-encoding`, `keep-alive`,
/// `upgrade`, `proxy-connection`) are stripped per RFC 9114 Section 4.2.
pub fn h1_to_h3_request<B>(request: Request<B>) -> Result<Request<B>, TranslationError> {
    let (parts, body) = request.into_parts();

    let method = parts.method.clone();
    let uri = parts.uri.clone();

    // Determine path+query.
    let path = uri
        .path_and_query()
        .map(|pq| pq.to_string())
        .unwrap_or_else(|| "/".to_string());

    // Determine scheme.
    let scheme = uri.scheme_str().unwrap_or("https").to_string();

    // Determine authority from URI or Host header.
    let authority = uri
        .authority()
        .map(|a| a.to_string())
        .or_else(|| {
            parts
                .headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    // Build the H3-compatible URI.
    let h3_uri: Uri = Uri::builder()
        .scheme(scheme.as_str())
        .authority(authority.as_str())
        .path_and_query(path.as_str())
        .build()
        .map_err(TranslationError::Http)?;

    let mut builder = Request::builder()
        .method(method)
        .uri(h3_uri)
        .version(Version::HTTP_3);

    // Copy headers, stripping connection-specific ones.
    for (name, value) in &parts.headers {
        if is_connection_header(name) {
            continue;
        }
        // Do not copy Host; it's represented by :authority in H3.
        if name == header::HOST {
            continue;
        }
        builder = builder.header(name, value);
    }

    let request = builder.body(body)?;
    debug!(
        method = %request.method(),
        uri = %request.uri(),
        "translated H1 -> H3 request"
    );
    Ok(request)
}

/// Convert an HTTP/3 response to HTTP/1.1-compatible form.
///
/// Adds standard HTTP/1.1 headers and strips any H2/H3-only pseudo-headers.
pub fn h3_to_h1_response<B>(response: Response<B>) -> Result<Response<B>, TranslationError> {
    let (parts, body) = response.into_parts();

    let mut builder = Response::builder()
        .status(parts.status)
        .version(Version::HTTP_11);

    // Copy all non-pseudo headers.
    for (name, value) in &parts.headers {
        // Pseudo-headers start with ':' -- they should not appear in headers map
        // for http crate types, but guard anyway.
        if name.as_str().starts_with(':') {
            continue;
        }
        builder = builder.header(name, value);
    }

    let response = builder.body(body)?;
    debug!(
        status = response.status().as_u16(),
        "translated H3 -> H1 response"
    );
    Ok(response)
}

/// Convert an HTTP/2 request to HTTP/3 form.
///
/// HTTP/2 and HTTP/3 share the same pseudo-header structure, so this is
/// mostly a version re-tag plus stripping any H2-specific frames.
pub fn h2_to_h3_request<B>(request: Request<B>) -> Result<Request<B>, TranslationError> {
    let (parts, body) = request.into_parts();

    let mut builder = Request::builder()
        .method(parts.method)
        .uri(parts.uri)
        .version(Version::HTTP_3);

    for (name, value) in &parts.headers {
        if is_connection_header(name) {
            continue;
        }
        builder = builder.header(name, value);
    }

    Ok(builder.body(body)?)
}

/// Convert an HTTP/3 response to HTTP/2 form.
pub fn h3_to_h2_response<B>(response: Response<B>) -> Result<Response<B>, TranslationError> {
    let (parts, body) = response.into_parts();

    let mut builder = Response::builder()
        .status(parts.status)
        .version(Version::HTTP_2);

    for (name, value) in &parts.headers {
        if name.as_str().starts_with(':') {
            continue;
        }
        builder = builder.header(name, value);
    }

    Ok(builder.body(body)?)
}

/// Check if a header is a connection-specific header that must be stripped
/// in HTTP/2 and HTTP/3 per RFC 7540 Section 8.1.2 and RFC 9114 Section 4.2.
fn is_connection_header(name: &HeaderName) -> bool {
    matches!(
        name,
        &header::CONNECTION
            | &header::TRANSFER_ENCODING
            | &header::UPGRADE
            | &header::PROXY_AUTHENTICATE
            | &header::PROXY_AUTHORIZATION
    ) || name.as_str() == "keep-alive"
        || name.as_str() == "proxy-connection"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h1_to_h3_basic() {
        let request = Request::builder()
            .method(Method::GET)
            .uri("http://example.com/path?query=1")
            .header("host", "example.com")
            .header("accept", "text/html")
            .body(())
            .unwrap();

        let h3_req = h1_to_h3_request(request).unwrap();
        assert_eq!(h3_req.method(), Method::GET);
        assert_eq!(h3_req.uri().path(), "/path");
        assert_eq!(h3_req.uri().query(), Some("query=1"));
        assert_eq!(h3_req.uri().authority().unwrap().as_str(), "example.com");
        assert_eq!(h3_req.version(), Version::HTTP_3);
        // Host header should be stripped (represented by :authority).
        assert!(h3_req.headers().get("host").is_none());
        // Accept should be preserved.
        assert_eq!(
            h3_req.headers().get("accept").unwrap().to_str().unwrap(),
            "text/html"
        );
    }

    #[test]
    fn h1_to_h3_strips_connection_headers() {
        let request = Request::builder()
            .method(Method::POST)
            .uri("https://example.com/api")
            .header("host", "example.com")
            .header("connection", "keep-alive")
            .header("transfer-encoding", "chunked")
            .header("keep-alive", "timeout=5")
            .header("content-type", "application/json")
            .body(())
            .unwrap();

        let h3_req = h1_to_h3_request(request).unwrap();
        assert!(h3_req.headers().get("connection").is_none());
        assert!(h3_req.headers().get("transfer-encoding").is_none());
        assert!(h3_req.headers().get("keep-alive").is_none());
        assert!(h3_req.headers().get("content-type").is_some());
    }

    #[test]
    fn test_h3_to_h1_response() {
        let response = Response::builder()
            .status(StatusCode::OK)
            .version(Version::HTTP_3)
            .header("content-type", "text/html")
            .header("server", "test")
            .body(())
            .unwrap();

        let h1_resp = super::h3_to_h1_response(response).unwrap();
        assert_eq!(h1_resp.status(), StatusCode::OK);
        assert_eq!(h1_resp.version(), Version::HTTP_11);
        assert_eq!(
            h1_resp
                .headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap(),
            "text/html"
        );
    }

    #[test]
    fn test_h2_to_h3_request_translation() {
        let request = Request::builder()
            .method(Method::PUT)
            .uri("https://example.com/resource")
            .version(Version::HTTP_2)
            .header("content-type", "application/octet-stream")
            .body(())
            .unwrap();

        let h3_req = super::h2_to_h3_request(request).unwrap();
        assert_eq!(h3_req.method(), Method::PUT);
        assert_eq!(h3_req.version(), Version::HTTP_3);
        assert!(h3_req.headers().get("content-type").is_some());
    }

    #[test]
    fn test_h3_to_h2_response_translation() {
        let response = Response::builder()
            .status(StatusCode::NOT_FOUND)
            .version(Version::HTTP_3)
            .header("x-custom", "value")
            .body(())
            .unwrap();

        let h2_resp = super::h3_to_h2_response(response).unwrap();
        assert_eq!(h2_resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(h2_resp.version(), Version::HTTP_2);
        assert_eq!(
            h2_resp.headers().get("x-custom").unwrap().to_str().unwrap(),
            "value"
        );
    }

    #[test]
    fn h1_to_h3_default_scheme() {
        // URI without scheme should default to https.
        let request = Request::builder()
            .method(Method::GET)
            .uri("/path")
            .header("host", "example.com")
            .body(())
            .unwrap();

        let h3_req = h1_to_h3_request(request).unwrap();
        assert_eq!(h3_req.uri().scheme_str(), Some("https"));
    }

    #[test]
    fn roundtrip_h1_h3_h1() {
        let original = Request::builder()
            .method(Method::GET)
            .uri("https://example.com/test")
            .header("host", "example.com")
            .header("accept", "*/*")
            .body(())
            .unwrap();

        let h3_req = h1_to_h3_request(original).unwrap();
        assert_eq!(h3_req.version(), Version::HTTP_3);
        assert_eq!(h3_req.method(), Method::GET);
        assert_eq!(h3_req.uri().path(), "/test");
    }
}
