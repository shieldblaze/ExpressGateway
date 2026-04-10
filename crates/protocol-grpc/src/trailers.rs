//! gRPC trailer forwarding.
//!
//! gRPC sends status information in HTTP/2 trailing headers. This module
//! provides utilities to detect and preserve the required trailer fields.
//!
//! Per the gRPC spec, the trailers MUST contain `grpc-status` and MAY contain
//! `grpc-message` and `grpc-status-details-bin`.

use http::header::HeaderName;
use http::HeaderMap;

/// Header names that MUST be forwarded in gRPC trailers.
///
/// Pre-parsed as `HeaderName` to avoid repeated parsing on the hot path.
static GRPC_STATUS: HeaderName = HeaderName::from_static("grpc-status");
static GRPC_MESSAGE: HeaderName = HeaderName::from_static("grpc-message");
static GRPC_STATUS_DETAILS_BIN: HeaderName = HeaderName::from_static("grpc-status-details-bin");

/// The canonical trailer key names as string slices (for documentation/logging).
pub const GRPC_TRAILER_KEYS: &[&str] = &["grpc-status", "grpc-message", "grpc-status-details-bin"];

/// Pre-parsed trailer header names for zero-allocation lookups.
static TRAILER_NAMES: &[&HeaderName] = &[&GRPC_STATUS, &GRPC_MESSAGE, &GRPC_STATUS_DETAILS_BIN];

/// Returns `true` if the header map contains at least `grpc-status`, which is
/// the minimum requirement for a valid gRPC trailer frame.
#[inline]
pub fn is_grpc_trailers(headers: &HeaderMap) -> bool {
    headers.contains_key(&GRPC_STATUS)
}

/// Extracts the gRPC trailer fields from `source` into a new [`HeaderMap`].
///
/// Only the three canonical trailer keys (`grpc-status`, `grpc-message`,
/// `grpc-status-details-bin`) are copied. All other headers are dropped.
pub fn extract_grpc_trailers(source: &HeaderMap) -> HeaderMap {
    let mut trailers = HeaderMap::with_capacity(GRPC_TRAILER_KEYS.len());
    for &name in TRAILER_NAMES {
        if let Some(value) = source.get(name) {
            trailers.insert(name.clone(), value.clone());
        }
    }
    trailers
}

/// Merges gRPC trailer fields from `trailers` into `target`, overwriting any
/// existing values for the canonical keys.
pub fn merge_grpc_trailers(target: &mut HeaderMap, trailers: &HeaderMap) {
    for &name in TRAILER_NAMES {
        if let Some(value) = trailers.get(name) {
            target.insert(name.clone(), value.clone());
        }
    }
}

/// Parse the `grpc-status` code from a trailer map.
///
/// Returns `None` if the header is absent or not a valid integer.
pub fn parse_grpc_status(trailers: &HeaderMap) -> Option<u32> {
    trailers
        .get(&GRPC_STATUS)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u32>().ok())
}

/// Parse the `grpc-message` from a trailer map.
///
/// Returns `None` if the header is absent.
pub fn parse_grpc_message(trailers: &HeaderMap) -> Option<&str> {
    trailers
        .get(&GRPC_MESSAGE)
        .and_then(|v| v.to_str().ok())
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

    #[test]
    fn parse_status_code() {
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", HeaderValue::from_static("14"));
        assert_eq!(parse_grpc_status(&trailers), Some(14));
    }

    #[test]
    fn parse_status_code_missing() {
        let trailers = HeaderMap::new();
        assert_eq!(parse_grpc_status(&trailers), None);
    }

    #[test]
    fn parse_message() {
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-message", HeaderValue::from_static("not found"));
        assert_eq!(parse_grpc_message(&trailers), Some("not found"));
    }

    #[test]
    fn parse_message_missing() {
        let trailers = HeaderMap::new();
        assert_eq!(parse_grpc_message(&trailers), None);
    }
}
