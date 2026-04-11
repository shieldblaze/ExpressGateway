//! Compression algorithm definitions and compress/decompress operations.

use std::io::{Read, Write};

use bytes::Bytes;
use thiserror::Error;
use tracing::trace;

/// Errors that can occur during compression or decompression.
#[derive(Debug, Error)]
pub enum CompressionError {
    #[error("compression failed: {0}")]
    CompressFailed(#[source] std::io::Error),

    #[error("decompression failed: {0}")]
    DecompressFailed(#[source] std::io::Error),
}

/// Supported compression algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompressionAlgorithm {
    Brotli,
    Zstd,
    Gzip,
    Deflate,
}

impl CompressionAlgorithm {
    /// Returns the `Content-Encoding` header value for this algorithm.
    pub fn content_encoding(&self) -> &'static str {
        match self {
            CompressionAlgorithm::Brotli => "br",
            CompressionAlgorithm::Zstd => "zstd",
            CompressionAlgorithm::Gzip => "gzip",
            CompressionAlgorithm::Deflate => "deflate",
        }
    }

    /// Try to parse a `Content-Encoding` / `Accept-Encoding` token into an algorithm.
    ///
    /// Zero-allocation: uses a stack buffer for case-insensitive matching.
    pub fn from_encoding(encoding: &str) -> Option<Self> {
        let trimmed = encoding.trim().as_bytes();
        // Longest known token is "deflate" (7 bytes).
        let mut buf = [0u8; 16];
        if trimmed.len() > buf.len() {
            return None;
        }
        let lower = &mut buf[..trimmed.len()];
        for (dst, src) in lower.iter_mut().zip(trimmed) {
            *dst = src.to_ascii_lowercase();
        }
        match &*lower {
            b"br" => Some(CompressionAlgorithm::Brotli),
            b"zstd" => Some(CompressionAlgorithm::Zstd),
            b"gzip" => Some(CompressionAlgorithm::Gzip),
            b"deflate" => Some(CompressionAlgorithm::Deflate),
            _ => None,
        }
    }

    /// Priority used when two algorithms have equal quality values.
    /// Higher is better.
    pub(crate) fn priority(&self) -> u8 {
        match self {
            CompressionAlgorithm::Brotli => 4,
            CompressionAlgorithm::Zstd => 3,
            CompressionAlgorithm::Gzip => 2,
            CompressionAlgorithm::Deflate => 1,
        }
    }

    /// Compress `data` using this algorithm.
    pub fn compress(&self, data: &[u8]) -> Result<Bytes, CompressionError> {
        trace!(algorithm = %self.content_encoding(), len = data.len(), "compressing");
        match self {
            CompressionAlgorithm::Brotli => compress_brotli(data),
            CompressionAlgorithm::Zstd => compress_zstd(data),
            CompressionAlgorithm::Gzip => compress_gzip(data),
            CompressionAlgorithm::Deflate => compress_deflate(data),
        }
    }

    /// Decompress `data` that was compressed with this algorithm.
    pub fn decompress(&self, data: &[u8]) -> Result<Bytes, CompressionError> {
        trace!(algorithm = %self.content_encoding(), len = data.len(), "decompressing");
        match self {
            CompressionAlgorithm::Brotli => decompress_brotli(data),
            CompressionAlgorithm::Zstd => decompress_zstd(data),
            CompressionAlgorithm::Gzip => decompress_gzip(data),
            CompressionAlgorithm::Deflate => decompress_deflate(data),
        }
    }
}

impl std::fmt::Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.content_encoding())
    }
}

// ---------------------------------------------------------------------------
// Brotli
// ---------------------------------------------------------------------------

fn compress_brotli(data: &[u8]) -> Result<Bytes, CompressionError> {
    // Pre-size to input length; compressed output is typically smaller.
    let mut output = Vec::with_capacity(data.len());
    {
        // quality 6 is a good speed/ratio trade-off for proxies.
        let mut writer = brotli::CompressorWriter::new(&mut output, 4096, 6, 22);
        writer
            .write_all(data)
            .map_err(CompressionError::CompressFailed)?;
        // CompressorWriter finalizes the brotli stream on Drop.  Flush
        // pushes the internal buffer through; the Vec<u8> sink cannot fail
        // on write so the Drop path is infallible.
        writer.flush().map_err(CompressionError::CompressFailed)?;
    }
    Ok(Bytes::from(output))
}

fn decompress_brotli(data: &[u8]) -> Result<Bytes, CompressionError> {
    let mut output = Vec::with_capacity(data.len() * 2);
    let mut reader = brotli::Decompressor::new(data, 4096);
    reader
        .read_to_end(&mut output)
        .map_err(CompressionError::DecompressFailed)?;
    Ok(Bytes::from(output))
}

// ---------------------------------------------------------------------------
// Zstd
// ---------------------------------------------------------------------------

fn compress_zstd(data: &[u8]) -> Result<Bytes, CompressionError> {
    let output = zstd::encode_all(data, 3).map_err(CompressionError::CompressFailed)?;
    Ok(Bytes::from(output))
}

fn decompress_zstd(data: &[u8]) -> Result<Bytes, CompressionError> {
    let output = zstd::decode_all(data).map_err(CompressionError::DecompressFailed)?;
    Ok(Bytes::from(output))
}

// ---------------------------------------------------------------------------
// Gzip
// ---------------------------------------------------------------------------

fn compress_gzip(data: &[u8]) -> Result<Bytes, CompressionError> {
    let mut encoder =
        flate2::write::GzEncoder::new(Vec::with_capacity(data.len()), flate2::Compression::fast());
    encoder
        .write_all(data)
        .map_err(CompressionError::CompressFailed)?;
    let output = encoder.finish().map_err(CompressionError::CompressFailed)?;
    Ok(Bytes::from(output))
}

fn decompress_gzip(data: &[u8]) -> Result<Bytes, CompressionError> {
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut output = Vec::with_capacity(data.len() * 2);
    decoder
        .read_to_end(&mut output)
        .map_err(CompressionError::DecompressFailed)?;
    Ok(Bytes::from(output))
}

// ---------------------------------------------------------------------------
// Deflate
// ---------------------------------------------------------------------------

fn compress_deflate(data: &[u8]) -> Result<Bytes, CompressionError> {
    let mut encoder = flate2::write::DeflateEncoder::new(
        Vec::with_capacity(data.len()),
        flate2::Compression::fast(),
    );
    encoder
        .write_all(data)
        .map_err(CompressionError::CompressFailed)?;
    let output = encoder.finish().map_err(CompressionError::CompressFailed)?;
    Ok(Bytes::from(output))
}

fn decompress_deflate(data: &[u8]) -> Result<Bytes, CompressionError> {
    let mut decoder = flate2::read::DeflateDecoder::new(data);
    let mut output = Vec::with_capacity(data.len() * 2);
    decoder
        .read_to_end(&mut output)
        .map_err(CompressionError::DecompressFailed)?;
    Ok(Bytes::from(output))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"Hello, ExpressGateway! This is a test payload that should \
        compress reasonably well because it contains repetitive text. \
        Repetitive text repetitive text repetitive text repetitive text.";

    fn roundtrip(algo: CompressionAlgorithm) {
        let compressed = algo.compress(SAMPLE).expect("compress");
        let decompressed = algo.decompress(&compressed).expect("decompress");
        assert_eq!(decompressed.as_ref(), SAMPLE);
    }

    #[test]
    fn roundtrip_brotli() {
        roundtrip(CompressionAlgorithm::Brotli);
    }

    #[test]
    fn roundtrip_zstd() {
        roundtrip(CompressionAlgorithm::Zstd);
    }

    #[test]
    fn roundtrip_gzip() {
        roundtrip(CompressionAlgorithm::Gzip);
    }

    #[test]
    fn roundtrip_deflate() {
        roundtrip(CompressionAlgorithm::Deflate);
    }

    #[test]
    fn roundtrip_empty() {
        for algo in [
            CompressionAlgorithm::Brotli,
            CompressionAlgorithm::Zstd,
            CompressionAlgorithm::Gzip,
            CompressionAlgorithm::Deflate,
        ] {
            let compressed = algo.compress(b"").expect("compress empty");
            let decompressed = algo.decompress(&compressed).expect("decompress empty");
            assert!(decompressed.is_empty(), "{algo}: expected empty output");
        }
    }

    #[test]
    fn content_encoding_values() {
        assert_eq!(CompressionAlgorithm::Brotli.content_encoding(), "br");
        assert_eq!(CompressionAlgorithm::Zstd.content_encoding(), "zstd");
        assert_eq!(CompressionAlgorithm::Gzip.content_encoding(), "gzip");
        assert_eq!(CompressionAlgorithm::Deflate.content_encoding(), "deflate");
    }

    #[test]
    fn from_encoding_parsing() {
        assert_eq!(
            CompressionAlgorithm::from_encoding("br"),
            Some(CompressionAlgorithm::Brotli)
        );
        assert_eq!(
            CompressionAlgorithm::from_encoding("GZIP"),
            Some(CompressionAlgorithm::Gzip)
        );
        assert_eq!(
            CompressionAlgorithm::from_encoding(" Zstd "),
            Some(CompressionAlgorithm::Zstd)
        );
        assert_eq!(CompressionAlgorithm::from_encoding("identity"), None);
        assert_eq!(CompressionAlgorithm::from_encoding(""), None);
    }
}
