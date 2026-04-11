//! Compressible MIME type detection.
//!
//! Contains a static list of MIME types that benefit from compression and a
//! helper to check whether a given `Content-Type` value is compressible.

/// Static set of MIME types that are considered compressible.
///
/// This list covers the most common text, application, font, and multipart
/// types that typically benefit from transfer-encoding compression.
const COMPRESSIBLE_TYPES: &[&str] = &[
    "text/html",
    "text/css",
    "text/plain",
    "text/javascript",
    "text/xml",
    "application/json",
    "application/javascript",
    "application/xml",
    "application/xhtml+xml",
    "application/rss+xml",
    "application/atom+xml",
    "application/svg+xml",
    "application/wasm",
    "image/svg+xml",
    "font/ttf",
    "font/otf",
    // font/woff and font/woff2 are already internally compressed (zlib / brotli).
    // application/font-woff, application/font-woff2 are deprecated aliases for
    // the same already-compressed formats.
    "application/vnd.ms-fontobject",
    "application/x-font-ttf",
    "application/x-font-opentype",
    // application/octet-stream is a catch-all for arbitrary binary data. Most
    // binary payloads are already compressed or incompressible.
    "multipart/bag",
    "multipart/mixed",
];

/// Returns `true` if the given `Content-Type` value represents a compressible
/// MIME type.
///
/// The check is case-insensitive and ignores any parameters (e.g.
/// `; charset=utf-8`) that may follow the media type.
pub fn is_compressible(content_type: &str) -> bool {
    // Strip optional parameters (everything after the first `;`).
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();

    let lower = media_type.to_ascii_lowercase();

    COMPRESSIBLE_TYPES.contains(&lower.as_str())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_is_compressible() {
        assert!(is_compressible("text/html"));
    }

    #[test]
    fn json_is_compressible() {
        assert!(is_compressible("application/json"));
    }

    #[test]
    fn svg_is_compressible() {
        assert!(is_compressible("image/svg+xml"));
    }

    #[test]
    fn wasm_is_compressible() {
        assert!(is_compressible("application/wasm"));
    }

    #[test]
    fn font_types_compressible() {
        assert!(is_compressible("font/ttf"));
        assert!(is_compressible("font/otf"));
        assert!(is_compressible("application/vnd.ms-fontobject"));
    }

    #[test]
    fn already_compressed_fonts_not_compressible() {
        assert!(!is_compressible("font/woff"));
        assert!(!is_compressible("font/woff2"));
        assert!(!is_compressible("application/font-woff"));
        assert!(!is_compressible("application/font-woff2"));
    }

    #[test]
    fn multipart_types() {
        assert!(is_compressible("multipart/bag"));
        assert!(is_compressible("multipart/mixed"));
    }

    #[test]
    fn parameters_are_ignored() {
        assert!(is_compressible("text/html; charset=utf-8"));
        assert!(is_compressible("application/json; charset=utf-8"));
    }

    #[test]
    fn case_insensitive() {
        assert!(is_compressible("TEXT/HTML"));
        assert!(is_compressible("Application/JSON"));
    }

    #[test]
    fn image_png_not_compressible() {
        assert!(!is_compressible("image/png"));
    }

    #[test]
    fn image_jpeg_not_compressible() {
        assert!(!is_compressible("image/jpeg"));
    }

    #[test]
    fn video_not_compressible() {
        assert!(!is_compressible("video/mp4"));
    }

    #[test]
    fn empty_string() {
        assert!(!is_compressible(""));
    }

    #[test]
    fn octet_stream_not_compressible() {
        assert!(!is_compressible("application/octet-stream"));
    }
}
