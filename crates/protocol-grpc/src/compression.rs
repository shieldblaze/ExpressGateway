//! gRPC-aware compression negotiation.
//!
//! gRPC uses the `grpc-encoding` header to signal the compression algorithm
//! applied to individual messages. This module parses and validates that
//! header.

use http::HeaderMap;

/// Compression algorithms supported by gRPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GrpcEncoding {
    /// No compression.
    Identity,
    /// Gzip compression.
    Gzip,
    /// Deflate compression.
    Deflate,
}

impl GrpcEncoding {
    /// Parse from the `grpc-encoding` header value.
    pub fn from_header(value: &str) -> Option<Self> {
        match value.trim() {
            "identity" => Some(Self::Identity),
            "gzip" => Some(Self::Gzip),
            "deflate" => Some(Self::Deflate),
            _ => None,
        }
    }

    /// Returns the wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
        }
    }
}

impl std::fmt::Display for GrpcEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Extract the `grpc-encoding` from the given header map.
///
/// Returns `None` if the header is absent or has an unsupported value.
pub fn encoding_from_headers(headers: &HeaderMap) -> Option<GrpcEncoding> {
    headers
        .get("grpc-encoding")
        .and_then(|v| v.to_str().ok())
        .and_then(GrpcEncoding::from_header)
}

/// All encodings that this gateway supports for gRPC.
pub fn supported_encodings() -> &'static [GrpcEncoding] {
    &[
        GrpcEncoding::Identity,
        GrpcEncoding::Gzip,
        GrpcEncoding::Deflate,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn parse_identity() {
        assert_eq!(
            GrpcEncoding::from_header("identity"),
            Some(GrpcEncoding::Identity)
        );
    }

    #[test]
    fn parse_gzip() {
        assert_eq!(GrpcEncoding::from_header("gzip"), Some(GrpcEncoding::Gzip));
    }

    #[test]
    fn parse_deflate() {
        assert_eq!(
            GrpcEncoding::from_header("deflate"),
            Some(GrpcEncoding::Deflate)
        );
    }

    #[test]
    fn unsupported_encoding() {
        assert_eq!(GrpcEncoding::from_header("snappy"), None);
    }

    #[test]
    fn from_headers_present() {
        let mut headers = HeaderMap::new();
        headers.insert("grpc-encoding", HeaderValue::from_static("gzip"));
        assert_eq!(encoding_from_headers(&headers), Some(GrpcEncoding::Gzip));
    }

    #[test]
    fn from_headers_absent() {
        let headers = HeaderMap::new();
        assert_eq!(encoding_from_headers(&headers), None);
    }
}
