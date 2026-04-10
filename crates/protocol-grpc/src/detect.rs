//! gRPC request detection.
//!
//! A request is considered gRPC if its `Content-Type` header starts with
//! `application/grpc`.

use http::HeaderMap;

/// Returns `true` when the request headers indicate a gRPC call.
pub fn is_grpc(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.starts_with("application/grpc"))
        .unwrap_or(false)
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
}
