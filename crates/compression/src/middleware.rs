//! Compression decision logic for HTTP responses.
//!
//! Determines whether a response should be compressed and, if so, which
//! algorithm to use based on request and response headers plus configurable
//! thresholds.

use http::HeaderMap;
use tracing::trace;

use crate::algorithm::CompressionAlgorithm;
use crate::mime::is_compressible;
use crate::negotiation::negotiate;

/// Default minimum body size (in bytes) below which compression is skipped.
pub const DEFAULT_MIN_BODY_SIZE: u64 = 256;

/// Configuration for compression decisions.
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Minimum body size in bytes to trigger compression.
    pub min_body_size: u64,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            min_body_size: DEFAULT_MIN_BODY_SIZE,
        }
    }
}

/// Decide whether to compress a response and which algorithm to use.
///
/// Returns `None` when compression should be skipped. Reasons for skipping:
///
/// - The response already has a `Content-Encoding` header.
/// - `Content-Length` is present and below the configured minimum body size.
/// - The `Content-Type` is not in the compressible set.
/// - No acceptable encoding could be negotiated from `Accept-Encoding`.
pub fn should_compress(
    accept_encoding: &str,
    response_headers: &HeaderMap,
    config: &CompressionConfig,
) -> Option<CompressionAlgorithm> {
    // 1. Skip if already encoded.
    if response_headers.contains_key(http::header::CONTENT_ENCODING) {
        trace!("skipping compression: Content-Encoding already present");
        return None;
    }

    // 2. Skip if body is too small.
    if let Some(len) = response_headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        && len < config.min_body_size
    {
        trace!(
            len,
            min = config.min_body_size,
            "skipping compression: body too small"
        );
        return None;
    }

    // 3. Skip if content type is not compressible.
    if let Some(ct) = response_headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        if !is_compressible(ct) {
            trace!(
                content_type = ct,
                "skipping compression: not a compressible type"
            );
            return None;
        }
    } else {
        // No Content-Type header -- err on the side of not compressing.
        trace!("skipping compression: no Content-Type header");
        return None;
    }

    // 4. Negotiate the best encoding.
    negotiate(accept_encoding)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use http::HeaderMap;
    use http::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};

    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for &(k, v) in pairs {
            map.insert(
                http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        map
    }

    #[test]
    fn happy_path_gzip() {
        let h = headers(&[
            (CONTENT_TYPE.as_str(), "text/html"),
            (CONTENT_LENGTH.as_str(), "1024"),
        ]);
        let result = should_compress("gzip", &h, &CompressionConfig::default());
        assert_eq!(result, Some(CompressionAlgorithm::Gzip));
    }

    #[test]
    fn skip_already_encoded() {
        let h = headers(&[
            (CONTENT_TYPE.as_str(), "text/html"),
            (CONTENT_LENGTH.as_str(), "1024"),
            (CONTENT_ENCODING.as_str(), "br"),
        ]);
        let result = should_compress("gzip", &h, &CompressionConfig::default());
        assert_eq!(result, None);
    }

    #[test]
    fn skip_body_too_small() {
        let h = headers(&[
            (CONTENT_TYPE.as_str(), "text/html"),
            (CONTENT_LENGTH.as_str(), "100"),
        ]);
        let result = should_compress("gzip", &h, &CompressionConfig::default());
        assert_eq!(result, None);
    }

    #[test]
    fn skip_not_compressible_type() {
        let h = headers(&[
            (CONTENT_TYPE.as_str(), "image/png"),
            (CONTENT_LENGTH.as_str(), "50000"),
        ]);
        let result = should_compress("gzip", &h, &CompressionConfig::default());
        assert_eq!(result, None);
    }

    #[test]
    fn skip_no_content_type() {
        let h = headers(&[(CONTENT_LENGTH.as_str(), "1024")]);
        let result = should_compress("gzip", &h, &CompressionConfig::default());
        assert_eq!(result, None);
    }

    #[test]
    fn skip_no_acceptable_encoding() {
        let h = headers(&[
            (CONTENT_TYPE.as_str(), "application/json"),
            (CONTENT_LENGTH.as_str(), "1024"),
        ]);
        let result = should_compress("identity", &h, &CompressionConfig::default());
        assert_eq!(result, None);
    }

    #[test]
    fn negotiate_brotli_preferred() {
        let h = headers(&[
            (CONTENT_TYPE.as_str(), "application/json"),
            (CONTENT_LENGTH.as_str(), "4096"),
        ]);
        let result = should_compress("br;q=1.0, gzip;q=0.8", &h, &CompressionConfig::default());
        assert_eq!(result, Some(CompressionAlgorithm::Brotli));
    }

    #[test]
    fn custom_min_body_size() {
        let h = headers(&[
            (CONTENT_TYPE.as_str(), "text/css"),
            (CONTENT_LENGTH.as_str(), "100"),
        ]);
        let config = CompressionConfig { min_body_size: 50 };
        let result = should_compress("gzip", &h, &config);
        assert_eq!(result, Some(CompressionAlgorithm::Gzip));
    }

    #[test]
    fn no_content_length_still_compresses() {
        // When Content-Length is absent, we cannot check the size threshold, so
        // we proceed with compression.
        let h = headers(&[(CONTENT_TYPE.as_str(), "text/html")]);
        let result = should_compress("gzip", &h, &CompressionConfig::default());
        assert_eq!(result, Some(CompressionAlgorithm::Gzip));
    }

    #[test]
    fn content_type_with_params() {
        let h = headers(&[
            (CONTENT_TYPE.as_str(), "text/html; charset=utf-8"),
            (CONTENT_LENGTH.as_str(), "2048"),
        ]);
        let result = should_compress("deflate, gzip", &h, &CompressionConfig::default());
        assert_eq!(result, Some(CompressionAlgorithm::Gzip));
    }
}
