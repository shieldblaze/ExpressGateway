//! gRPC trailer forwarding.
//!
//! gRPC sends status information in HTTP/2 trailing headers. This module
//! provides utilities to detect and preserve the required trailer fields.

use http::HeaderMap;

/// Header names that MUST be forwarded in gRPC trailers.
pub const GRPC_TRAILER_KEYS: &[&str] = &["grpc-status", "grpc-message", "grpc-status-details-bin"];

/// Returns `true` if the header map contains at least `grpc-status`, which is
/// the minimum requirement for a valid gRPC trailer frame.
pub fn is_grpc_trailers(headers: &HeaderMap) -> bool {
    headers.contains_key("grpc-status")
}

/// Extracts the gRPC trailer fields from `source` into a new [`HeaderMap`].
///
/// Only the three canonical trailer keys (`grpc-status`, `grpc-message`,
/// `grpc-status-details-bin`) are copied. All other headers are dropped.
pub fn extract_grpc_trailers(source: &HeaderMap) -> HeaderMap {
    let mut trailers = HeaderMap::with_capacity(GRPC_TRAILER_KEYS.len());
    for &key in GRPC_TRAILER_KEYS {
        if let Some(value) = source.get(key) {
            trailers.insert(
                http::header::HeaderName::from_bytes(key.as_bytes()).unwrap(),
                value.clone(),
            );
        }
    }
    trailers
}

/// Merges gRPC trailer fields from `trailers` into `target`, overwriting any
/// existing values for the canonical keys.
pub fn merge_grpc_trailers(target: &mut HeaderMap, trailers: &HeaderMap) {
    for &key in GRPC_TRAILER_KEYS {
        if let Some(value) = trailers.get(key) {
            target.insert(
                http::header::HeaderName::from_bytes(key.as_bytes()).unwrap(),
                value.clone(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn detects_grpc_trailers() {
        let mut headers = HeaderMap::new();
        headers.insert("grpc-status", HeaderValue::from_static("0"));
        assert!(is_grpc_trailers(&headers));
    }

    #[test]
    fn rejects_non_grpc_trailers() {
        let headers = HeaderMap::new();
        assert!(!is_grpc_trailers(&headers));
    }

    #[test]
    fn extracts_all_trailer_keys() {
        let mut headers = HeaderMap::new();
        headers.insert("grpc-status", HeaderValue::from_static("0"));
        headers.insert("grpc-message", HeaderValue::from_static("OK"));
        headers.insert(
            "grpc-status-details-bin",
            HeaderValue::from_static("base64data"),
        );
        headers.insert("x-custom", HeaderValue::from_static("ignored"));

        let trailers = extract_grpc_trailers(&headers);
        assert_eq!(trailers.len(), 3);
        assert_eq!(trailers.get("grpc-status").unwrap(), "0");
        assert_eq!(trailers.get("grpc-message").unwrap(), "OK");
        assert!(trailers.get("x-custom").is_none());
    }

    #[test]
    fn merge_overwrites() {
        let mut target = HeaderMap::new();
        target.insert("grpc-status", HeaderValue::from_static("13"));

        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", HeaderValue::from_static("0"));

        merge_grpc_trailers(&mut target, &trailers);
        assert_eq!(target.get("grpc-status").unwrap(), "0");
    }
}
