//! `Accept-Encoding` header negotiation.
//!
//! Parses the value of an `Accept-Encoding` HTTP header and selects the best
//! compression algorithm that both the client and server support.
//!
//! The implementation is zero-allocation: it parses the header and selects the
//! best algorithm in a single pass over the comma-separated tokens without
//! any heap allocation.

use crate::algorithm::CompressionAlgorithm;

/// All algorithms that this server is willing to use.
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

/// Extract the quality value from the parameters portion of a token.
///
/// `params` is everything after the first `;` in a single comma-separated
/// entry, e.g. `"q=0.8"` or `"level=5;q=0.8"`.
fn extract_quality(params: &str) -> f32 {
    for p in params.split(';') {
        let mut kv = p.splitn(2, '=');
        let Some(key) = kv.next() else { continue };
        if key.trim().eq_ignore_ascii_case("q") {
            return parse_quality(kv.next().unwrap_or(""));
        }
    }
    1.0
}

/// Try to match a trimmed, lowercase token against one of our supported
/// algorithms.  Returns the algorithm and its server-side priority.
fn match_token(token: &str) -> Option<CompressionAlgorithm> {
    // We inline the matching instead of calling `from_encoding` to avoid
    // an extra `to_ascii_lowercase` allocation -- the caller already holds
    // a lowercase view.
    match token {
        "br" => Some(CompressionAlgorithm::Brotli),
        "zstd" => Some(CompressionAlgorithm::Zstd),
        "gzip" => Some(CompressionAlgorithm::Gzip),
        "deflate" => Some(CompressionAlgorithm::Deflate),
        _ => None,
    }
}

/// Returns `true` if candidate `(quality, priority)` beats the current best.
fn is_better(quality: f32, priority: u8, best_quality: f32, best_priority: u8) -> bool {
    quality > best_quality || (quality == best_quality && priority > best_priority)
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
///
/// This function performs zero heap allocations.
pub fn negotiate(accept_encoding: &str) -> Option<CompressionAlgorithm> {
    let mut best: Option<CompressionAlgorithm> = None;
    let mut best_quality: f32 = 0.0;
    let mut best_priority: u8 = 0;

    // First pass: find directly-named algorithms and remember the wildcard
    // quality, if any.
    //
    // We need two passes because a wildcard appearing *before* an explicit
    // `gzip;q=0` in the header should still respect the explicit exclusion.
    // Doing it in one pass would require tracking per-algorithm exclusion
    // state, which is more complex than a second trivial pass.
    let mut wildcard_quality: Option<f32> = None;

    // Per-algorithm quality from explicit entries.  `None` means the algorithm
    // was not explicitly mentioned.  `Some(0.0)` means explicitly excluded.
    let mut explicit: [Option<f32>; 4] = [None; 4];

    for part in accept_encoding.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let (token_raw, quality) = match part.find(';') {
            Some(pos) => {
                let (t, params) = part.split_at(pos);
                // Skip the ';' itself.
                (t.trim(), extract_quality(&params[1..]))
            }
            None => (part.trim(), 1.0_f32),
        };

        // We need a lowercase view for matching.  Rather than allocating a
        // String, we use a small stack buffer.  Accept-Encoding tokens are
        // at most 7 bytes ("deflate").  Anything longer is not a known
        // algorithm and we skip it (unless it is "*").
        if token_raw == "*" {
            wildcard_quality = Some(quality);
            continue;
        }

        // Stack-local lowercase: tokens are short (max "deflate" = 7 bytes).
        let mut buf = [0u8; 16];
        let token_bytes = token_raw.as_bytes();
        if token_bytes.len() > buf.len() {
            // Not a known algorithm.
            continue;
        }
        let lower = &mut buf[..token_bytes.len()];
        for (dst, src) in lower.iter_mut().zip(token_bytes) {
            *dst = src.to_ascii_lowercase();
        }
        // SAFETY: input is ASCII (HTTP header values), lowering ASCII preserves
        // validity.
        let token = std::str::from_utf8(lower).unwrap_or("");

        if let Some(algo) = match_token(token) {
            let idx = algo.priority() as usize - 1; // priority is 1-based
            explicit[idx] = Some(quality);
            if quality > 0.0 && is_better(quality, algo.priority(), best_quality, best_priority) {
                best = Some(algo);
                best_quality = quality;
                best_priority = algo.priority();
            }
        }
    }

    // Second pass: apply wildcard to algorithms that were not explicitly named.
    if let Some(wq) = wildcard_quality
        && wq > 0.0 {
            for algo in &ALL_ALGORITHMS {
                let idx = algo.priority() as usize - 1;
                if explicit[idx].is_none()
                    && is_better(wq, algo.priority(), best_quality, best_priority)
                {
                    best = Some(*algo);
                    best_quality = wq;
                    best_priority = algo.priority();
                }
            }
        }

    best
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
