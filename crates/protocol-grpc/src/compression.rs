//! gRPC-aware compression negotiation.
//!
//! gRPC uses `grpc-encoding` to signal the compression algorithm applied to
//! individual messages, and `grpc-accept-encoding` to advertise which
//! algorithms the sender can decompress.

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
    /// Snappy compression (used by some gRPC implementations).
    Snappy,
    /// Zstandard compression (increasingly common).
    Zstd,
}

impl GrpcEncoding {
    /// Parse from the `grpc-encoding` header value.
    pub fn from_header(value: &str) -> Option<Self> {
        match value.trim() {
            "identity" => Some(Self::Identity),
            "gzip" => Some(Self::Gzip),
            "deflate" => Some(Self::Deflate),
            "snappy" => Some(Self::Snappy),
            "zstd" => Some(Self::Zstd),
            _ => None,
        }
    }

    /// Returns the wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
            Self::Snappy => "snappy",
            Self::Zstd => "zstd",
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

/// Parse the `grpc-accept-encoding` header into a list of accepted encodings.
///
/// The header value is a comma-separated list of encoding names.
/// Unknown encodings are silently ignored.
pub fn accepted_encodings_from_headers(headers: &HeaderMap) -> Vec<GrpcEncoding> {
    headers
        .get("grpc-accept-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(',')
                .filter_map(|s| GrpcEncoding::from_header(s.trim()))
                .collect()
        })
        .unwrap_or_default()
}

/// Build the `grpc-accept-encoding` header value for our supported encodings.
pub fn supported_accept_encoding_value() -> &'static str {
    "identity,gzip,deflate,snappy,zstd"
}

/// All encodings that this gateway supports for gRPC.
pub fn supported_encodings() -> &'static [GrpcEncoding] {
    &[
        GrpcEncoding::Identity,
        GrpcEncoding::Gzip,
        GrpcEncoding::Deflate,
        GrpcEncoding::Snappy,
        GrpcEncoding::Zstd,
    ]
}

/// Select the best encoding from the client's accepted list that we support.
///
/// Returns `Identity` if no common encoding is found (identity is always
/// implicitly accepted per the gRPC spec).
pub fn negotiate_encoding(accepted: &[GrpcEncoding]) -> GrpcEncoding {
    // Prefer in order: zstd > gzip > snappy > deflate > identity
    const PREFERENCE: &[GrpcEncoding] = &[
        GrpcEncoding::Zstd,
        GrpcEncoding::Gzip,
        GrpcEncoding::Snappy,
        GrpcEncoding::Deflate,
    ];

    for &pref in PREFERENCE {
        if accepted.contains(&pref) {
            return pref;
        }
    }

    GrpcEncoding::Identity
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
    fn parse_snappy() {
        assert_eq!(
            GrpcEncoding::from_header("snappy"),
            Some(GrpcEncoding::Snappy)
        );
    }

    #[test]
    fn parse_zstd() {
        assert_eq!(GrpcEncoding::from_header("zstd"), Some(GrpcEncoding::Zstd));
    }

    #[test]
    fn unsupported_encoding() {
        assert_eq!(GrpcEncoding::from_header("brotli"), None);
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

    #[test]
    fn accepted_encodings() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "grpc-accept-encoding",
            HeaderValue::from_static("gzip, identity, deflate"),
        );
        let accepted = accepted_encodings_from_headers(&headers);
        assert_eq!(
            accepted,
            vec![
                GrpcEncoding::Gzip,
                GrpcEncoding::Identity,
                GrpcEncoding::Deflate
            ]
        );
    }

    #[test]
    fn accepted_encodings_ignores_unknown() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "grpc-accept-encoding",
            HeaderValue::from_static("gzip, brotli, zstd"),
        );
        let accepted = accepted_encodings_from_headers(&headers);
        assert_eq!(accepted, vec![GrpcEncoding::Gzip, GrpcEncoding::Zstd]);
    }

    #[test]
    fn negotiate_prefers_zstd() {
        let accepted = vec![GrpcEncoding::Gzip, GrpcEncoding::Zstd];
        assert_eq!(negotiate_encoding(&accepted), GrpcEncoding::Zstd);
    }

    #[test]
    fn negotiate_falls_back_to_identity() {
        let accepted = vec![];
        assert_eq!(negotiate_encoding(&accepted), GrpcEncoding::Identity);
    }
}
