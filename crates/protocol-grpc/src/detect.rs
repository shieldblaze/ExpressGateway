//! gRPC request detection.
//!
//! A request is considered native gRPC if its `Content-Type` header starts with
//! `application/grpc` but NOT `application/grpc-web`.
//!
//! gRPC-Web requests have `Content-Type` starting with `application/grpc-web`.

use http::HeaderMap;

/// Returns `true` when the request headers indicate a native gRPC call
/// (not gRPC-Web).
///
/// Per the spec, `content-type` must be `application/grpc` optionally followed
/// by `+<subtype>` (e.g. `application/grpc+proto`, `application/grpc+json`).
pub fn is_grpc(headers: &HeaderMap) -> bool {
    match extract_content_type(headers) {
        Some(ct) => is_native_grpc_content_type(ct),
        None => false,
    }
}

/// Returns `true` when the request headers indicate a gRPC-Web call.
///
/// gRPC-Web content types: `application/grpc-web`, `application/grpc-web+proto`,
/// `application/grpc-web-text`, `application/grpc-web-text+proto`.
pub fn is_grpc_web(headers: &HeaderMap) -> bool {
    match extract_content_type(headers) {
        Some(ct) => ct.starts_with("application/grpc-web"),
        None => false,
    }
}

/// Returns `true` for either native gRPC or gRPC-Web.
pub fn is_any_grpc(headers: &HeaderMap) -> bool {
    match extract_content_type(headers) {
        Some(ct) => ct.starts_with("application/grpc"),
        None => false,
    }
}

/// Returns `true` if the gRPC-Web content type uses text encoding
/// (base64-encoded bodies).
pub fn is_grpc_web_text(headers: &HeaderMap) -> bool {
    match extract_content_type(headers) {
        Some(ct) => ct.starts_with("application/grpc-web-text"),
        None => false,
    }
}

/// Extract content-type header value as a string slice.
fn extract_content_type(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
}

/// Check if a content-type is native gRPC (not gRPC-Web).
///
/// `application/grpc` followed by nothing, `+proto`, `+json`, etc. but NOT
/// `application/grpc-web*`.
fn is_native_grpc_content_type(ct: &str) -> bool {
    if !ct.starts_with("application/grpc") {
        return false;
    }
    // Exclude grpc-web variants.
    let rest = &ct["application/grpc".len()..];
    // Native gRPC: empty, or starts with '+' (subtype), or ';' (params).
    rest.is_empty() || rest.starts_with('+') || rest.starts_with(';')
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::{CONTENT_TYPE, HeaderMap, HeaderValue};

    #[test]
    fn detects_grpc_content_type() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
        assert!(is_grpc(&headers));
    }

    #[test]
    fn detects_grpc_proto_subtype() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/grpc+proto"),
        );
        assert!(is_grpc(&headers));
    }

    #[test]
    fn detects_grpc_json_subtype() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/grpc+json"),
        );
        assert!(is_grpc(&headers));
    }

    #[test]
    fn rejects_plain_http() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert!(!is_grpc(&headers));
    }

    #[test]
    fn rejects_missing_content_type() {
        let headers = HeaderMap::new();
        assert!(!is_grpc(&headers));
    }

    #[test]
    fn rejects_partial_match() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grp"));
        assert!(!is_grpc(&headers));
    }

    #[test]
    fn grpc_web_not_detected_as_native() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/grpc-web"),
        );
        assert!(!is_grpc(&headers));
        assert!(is_grpc_web(&headers));
    }

    #[test]
    fn grpc_web_text_detected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/grpc-web-text+proto"),
        );
        assert!(!is_grpc(&headers));
        assert!(is_grpc_web(&headers));
        assert!(is_grpc_web_text(&headers));
    }

    #[test]
    fn is_any_grpc_catches_both() {
        let mut h1 = HeaderMap::new();
        h1.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
        assert!(is_any_grpc(&h1));

        let mut h2 = HeaderMap::new();
        h2.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/grpc-web"),
        );
        assert!(is_any_grpc(&h2));

        let mut h3 = HeaderMap::new();
        h3.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert!(!is_any_grpc(&h3));
    }

    #[test]
    fn grpc_with_params() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/grpc;charset=utf-8"),
        );
        assert!(is_grpc(&headers));
    }
}
