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

/// A compact bitset of accepted gRPC encodings. Stack-allocated, zero heap
/// allocation. Fits all 5 known encodings in a single `u8`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncodingSet(u8);

impl EncodingSet {
    /// Insert an encoding into the set.
    #[inline]
    pub fn insert(&mut self, enc: GrpcEncoding) {
        self.0 |= 1 << (enc as u8);
    }

    /// Check if an encoding is in the set.
    #[inline]
    pub fn contains(self, enc: GrpcEncoding) -> bool {
        self.0 & (1 << (enc as u8)) != 0
    }

    /// Returns `true` if the set is empty.
    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Parse the `grpc-accept-encoding` header into a set of accepted encodings.
///
/// The header value is a comma-separated list of encoding names.
/// Unknown encodings are silently ignored.
///
/// Returns a stack-allocated [`EncodingSet`] -- zero heap allocation.
pub fn accepted_encodings_from_headers(headers: &HeaderMap) -> EncodingSet {
    let mut set = EncodingSet::default();
    if let Some(value) = headers.get("grpc-accept-encoding").and_then(|v| v.to_str().ok()) {
        for token in value.split(',') {
            if let Some(enc) = GrpcEncoding::from_header(token.trim()) {
                set.insert(enc);
            }
        }
    }
    set
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

/// Select the best encoding from the client's accepted set that we support.
///
/// Returns `Identity` if no common encoding is found (identity is always
/// implicitly accepted per the gRPC spec).
pub fn negotiate_encoding(accepted: EncodingSet) -> GrpcEncoding {
    // Prefer in order: zstd > gzip > snappy > deflate > identity
    const PREFERENCE: &[GrpcEncoding] = &[
        GrpcEncoding::Zstd,
        GrpcEncoding::Gzip,
        GrpcEncoding::Snappy,
        GrpcEncoding::Deflate,
    ];

    for &pref in PREFERENCE {
        if accepted.contains(pref) {
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
        assert!(accepted.contains(GrpcEncoding::Gzip));
        assert!(accepted.contains(GrpcEncoding::Identity));
        assert!(accepted.contains(GrpcEncoding::Deflate));
        assert!(!accepted.contains(GrpcEncoding::Zstd));
        assert!(!accepted.contains(GrpcEncoding::Snappy));
    }

    #[test]
    fn accepted_encodings_ignores_unknown() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "grpc-accept-encoding",
            HeaderValue::from_static("gzip, brotli, zstd"),
        );
        let accepted = accepted_encodings_from_headers(&headers);
        assert!(accepted.contains(GrpcEncoding::Gzip));
        assert!(accepted.contains(GrpcEncoding::Zstd));
        assert!(!accepted.contains(GrpcEncoding::Deflate));
    }

    #[test]
    fn negotiate_prefers_zstd() {
        let mut accepted = EncodingSet::default();
        accepted.insert(GrpcEncoding::Gzip);
        accepted.insert(GrpcEncoding::Zstd);
        assert_eq!(negotiate_encoding(accepted), GrpcEncoding::Zstd);
    }

    #[test]
    fn negotiate_falls_back_to_identity() {
        let accepted = EncodingSet::default();
        assert_eq!(negotiate_encoding(accepted), GrpcEncoding::Identity);
    }
}
