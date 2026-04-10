//! `Accept-Encoding` header negotiation.
//!
//! Parses the value of an `Accept-Encoding` HTTP header and selects the best
//! compression algorithm that both the client and server support.

use crate::algorithm::CompressionAlgorithm;

/// A single parsed entry from an `Accept-Encoding` header value.
#[derive(Debug, Clone)]
struct EncodingEntry {
    /// The encoding token (e.g. `br`, `gzip`, `*`).
    token: String,
    /// Quality value in the range `[0.0, 1.0]`.  Defaults to `1.0` when the
    /// `q` parameter is absent.
    quality: f32,
}

/// All algorithms that this server is willing to use, in preference order.
const ALL_ALGORITHMS: [CompressionAlgorithm; 4] = [
    CompressionAlgorithm::Brotli,
    CompressionAlgorithm::Zstd,
    CompressionAlgorithm::Gzip,
    CompressionAlgorithm::Deflate,
];

/// Parse a quality-value string (e.g. `"0.8"`) into an `f32`.
///
/// Returns `1.0` when the string is empty or cannot be parsed, matching HTTP
/// semantics where the absence of `q` implies `q=1.0`.
fn parse_quality(s: &str) -> f32 {
    let s = s.trim();
    if s.is_empty() {
        return 1.0;
    }
    s.parse::<f32>().unwrap_or(1.0).clamp(0.0, 1.0)
}

/// Parse the full `Accept-Encoding` header value into a list of entries.
///
/// Example input: `"br;q=1.0, gzip;q=0.8, *;q=0.1"`
fn parse_accept_encoding(header: &str) -> Vec<EncodingEntry> {
    header
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }

            let mut segments = part.splitn(2, ';');
            let token = segments.next()?.trim().to_ascii_lowercase();
            if token.is_empty() {
                return None;
            }

            let quality = segments
                .next()
                .and_then(|params| {
                    // Look for `q=<value>` among the parameters.
                    params.split(';').find_map(|p| {
                        let mut kv = p.splitn(2, '=');
                        let key = kv.next()?.trim();
                        if key.eq_ignore_ascii_case("q") {
                            Some(parse_quality(kv.next().unwrap_or("")))
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or(1.0);

            Some(EncodingEntry { token, quality })
        })
        .collect()
}

/// Select the best compression algorithm based on an `Accept-Encoding` header.
///
/// Returns `None` when no acceptable algorithm could be negotiated (e.g. the
/// client only accepts `identity`).
///
/// Preference rules:
/// 1. Higher quality value wins.
/// 2. When quality values are equal, the server-side preference order is used:
///    `br > zstd > gzip > deflate`.
/// 3. The wildcard `*` matches any encoding the server supports.
pub fn negotiate(accept_encoding: &str) -> Option<CompressionAlgorithm> {
    let entries = parse_accept_encoding(accept_encoding);
    if entries.is_empty() {
        return None;
    }

    // Build a candidate list: (algorithm, quality, priority).
    let mut candidates: Vec<(CompressionAlgorithm, f32, u8)> = Vec::new();

    // Wildcard quality (if present).
    let wildcard_quality = entries.iter().find(|e| e.token == "*").map(|e| e.quality);

    for algo in &ALL_ALGORITHMS {
        let encoding_name = algo.content_encoding();

        // Check for a direct match first.
        if let Some(entry) = entries.iter().find(|e| e.token == encoding_name) {
            if entry.quality > 0.0 {
                candidates.push((*algo, entry.quality, algo.priority()));
            }
        } else if let Some(wq) = wildcard_quality {
            // Fall back to wildcard.
            if wq > 0.0 {
                candidates.push((*algo, wq, algo.priority()));
            }
        }
    }

    // Sort: highest quality first, then highest priority first.
    candidates.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.2.cmp(&a.2))
    });

    candidates.first().map(|c| c.0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_preference() {
        let result = negotiate("br;q=1.0, gzip;q=0.8, deflate;q=0.5");
        assert_eq!(result, Some(CompressionAlgorithm::Brotli));
    }

    #[test]
    fn gzip_preferred_when_higher_quality() {
        let result = negotiate("gzip;q=1.0, br;q=0.5");
        assert_eq!(result, Some(CompressionAlgorithm::Gzip));
    }

    #[test]
    fn equal_quality_uses_server_preference() {
        // All have q=1.0 (implicit), so server preference wins: br > zstd > gzip > deflate.
        let result = negotiate("gzip, deflate, br, zstd");
        assert_eq!(result, Some(CompressionAlgorithm::Brotli));
    }

    #[test]
    fn wildcard_matches_best_server_algorithm() {
        let result = negotiate("*");
        assert_eq!(result, Some(CompressionAlgorithm::Brotli));
    }

    #[test]
    fn wildcard_with_quality() {
        // `gzip` has explicit q=1.0, wildcard q=0.1 gives br/zstd/deflate q=0.1.
        let result = negotiate("gzip;q=1.0, *;q=0.1");
        assert_eq!(result, Some(CompressionAlgorithm::Gzip));
    }

    #[test]
    fn wildcard_only_when_explicit_zero() {
        // `gzip;q=0` disables gzip, wildcard gives the rest q=0.5.
        let result = negotiate("gzip;q=0, *;q=0.5");
        assert_eq!(result, Some(CompressionAlgorithm::Brotli));
    }

    #[test]
    fn empty_header_returns_none() {
        assert_eq!(negotiate(""), None);
    }

    #[test]
    fn identity_only_returns_none() {
        assert_eq!(negotiate("identity"), None);
    }

    #[test]
    fn quality_zero_excluded() {
        let result = negotiate("br;q=0, gzip;q=0, deflate;q=0, zstd;q=0");
        assert_eq!(result, None);
    }

    #[test]
    fn case_insensitive() {
        let result = negotiate("BR;q=1.0, GZIP;q=0.5");
        assert_eq!(result, Some(CompressionAlgorithm::Brotli));
    }

    #[test]
    fn whitespace_handling() {
        let result = negotiate("  br ; q=0.8 , gzip ; q=0.9 ");
        assert_eq!(result, Some(CompressionAlgorithm::Gzip));
    }

    #[test]
    fn deflate_only() {
        let result = negotiate("deflate");
        assert_eq!(result, Some(CompressionAlgorithm::Deflate));
    }

    #[test]
    fn zstd_only() {
        let result = negotiate("zstd;q=0.7");
        assert_eq!(result, Some(CompressionAlgorithm::Zstd));
    }

    #[test]
    fn mixed_known_and_unknown() {
        let result = negotiate("snappy;q=1.0, gzip;q=0.5");
        assert_eq!(result, Some(CompressionAlgorithm::Gzip));
    }
}
