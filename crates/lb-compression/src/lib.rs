//! Streaming compression and decompression: zstd, brotli, gzip, deflate.
//!
//! Provides [`Compressor`] and [`Decompressor`] with decompression-bomb protection,
//! [`transcode`] for re-encoding between algorithms, [`BreachGuard`] for BREACH
//! mitigation, and [`negotiate`] for `Accept-Encoding` content negotiation.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![warn(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::{Read, Write};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors that can occur during compression or decompression.
#[derive(Debug, thiserror::Error)]
pub enum CompressionError {
    /// A compression operation failed.
    #[error("compression failed: {0}")]
    CompressFailed(String),

    /// A decompression operation failed.
    #[error("decompression failed: {0}")]
    DecompressFailed(String),

    /// Decompression output expanded beyond safe ratio limits.
    #[error(
        "decompression bomb detected: {decompressed_bytes} bytes from {compressed_bytes} bytes (ratio {ratio:.1})"
    )]
    BombDetected {
        /// Total compressed bytes consumed.
        compressed_bytes: u64,
        /// Total decompressed bytes produced.
        decompressed_bytes: u64,
        /// Expansion ratio (`decompressed / compressed`).
        ratio: f64,
    },

    /// Decompressed output exceeds the caller-specified ceiling.
    #[error("decompressed output exceeds maximum size {max_bytes}")]
    OutputTooLarge {
        /// The limit that was exceeded.
        max_bytes: u64,
    },

    /// The requested algorithm is not supported.
    #[error("unsupported algorithm")]
    Unsupported,
}

// ---------------------------------------------------------------------------
// Algorithm
// ---------------------------------------------------------------------------

/// Supported compression algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Algorithm {
    /// gzip (RFC 1952).
    Gzip,
    /// raw DEFLATE (RFC 1951).
    Deflate,
    /// Brotli (RFC 7932).
    Brotli,
    /// Zstandard (RFC 8878).
    Zstd,
}

impl Algorithm {
    /// Default compression level per algorithm.
    ///
    /// These are middle-of-the-road levels that balance speed and ratio for a
    /// reverse-proxy hot path.
    const fn default_level(self) -> i32 {
        match self {
            Self::Gzip | Self::Deflate => 6,
            Self::Brotli => 4,
            Self::Zstd => 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Compressor
// ---------------------------------------------------------------------------

/// Inner encoder variants.
enum CompressorInner {
    Gzip(flate2::write::GzEncoder<Vec<u8>>),
    Deflate(flate2::write::DeflateEncoder<Vec<u8>>),
    Brotli(Box<brotli::CompressorWriter<Vec<u8>>>),
    Zstd(zstd::stream::Encoder<'static, Vec<u8>>),
}

/// A streaming compressor.
///
/// Feed data via [`compress`](Compressor::compress), then call
/// [`finish`](Compressor::finish) to flush the final frame and retrieve all
/// remaining output.
pub struct Compressor {
    inner: CompressorInner,
}

impl Compressor {
    /// Create a new compressor for `algorithm`.
    ///
    /// `level` overrides the default compression level.  Pass `None` for a
    /// sensible default.
    ///
    /// # Errors
    ///
    /// Returns [`CompressionError::CompressFailed`] if the encoder cannot be
    /// initialised (e.g. invalid zstd level).
    pub fn new(algorithm: Algorithm, level: Option<i32>) -> Result<Self, CompressionError> {
        let lvl = level.unwrap_or_else(|| algorithm.default_level());
        let inner = match algorithm {
            Algorithm::Gzip => {
                let enc = flate2::write::GzEncoder::new(
                    Vec::new(),
                    flate2::Compression::new(clamp_u32(lvl)),
                );
                CompressorInner::Gzip(enc)
            }
            Algorithm::Deflate => {
                let enc = flate2::write::DeflateEncoder::new(
                    Vec::new(),
                    flate2::Compression::new(clamp_u32(lvl)),
                );
                CompressorInner::Deflate(enc)
            }
            Algorithm::Brotli => {
                let quality = clamp_u32(lvl);
                let enc = brotli::CompressorWriter::new(Vec::new(), 4096, quality, 22);
                CompressorInner::Brotli(Box::new(enc))
            }
            Algorithm::Zstd => {
                let enc = zstd::stream::Encoder::new(Vec::new(), lvl)
                    .map_err(|e| CompressionError::CompressFailed(e.to_string()))?;
                CompressorInner::Zstd(enc)
            }
        };
        Ok(Self { inner })
    }

    /// Compress `input` and return any output bytes produced so far.
    ///
    /// Streaming encoders may buffer internally; not every call produces
    /// output.  Call [`finish`](Compressor::finish) after the last chunk.
    ///
    /// # Errors
    ///
    /// Returns [`CompressionError::CompressFailed`] on I/O or codec errors.
    pub fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>, CompressionError> {
        let map = |e: std::io::Error| CompressionError::CompressFailed(e.to_string());
        match &mut self.inner {
            CompressorInner::Gzip(enc) => {
                enc.write_all(input).map_err(map)?;
                Ok(drain_vec(enc.get_mut()))
            }
            CompressorInner::Deflate(enc) => {
                enc.write_all(input).map_err(map)?;
                Ok(drain_vec(enc.get_mut()))
            }
            CompressorInner::Brotli(enc) => {
                enc.write_all(input).map_err(map)?;
                Ok(drain_vec(enc.get_mut()))
            }
            CompressorInner::Zstd(enc) => {
                enc.write_all(input).map_err(map)?;
                Ok(drain_vec(enc.get_mut()))
            }
        }
    }

    /// Finalise the compressor and return any remaining buffered output.
    ///
    /// This consumes the `Compressor`.
    ///
    /// # Errors
    ///
    /// Returns [`CompressionError::CompressFailed`] on flush/finish failure.
    pub fn finish(self) -> Result<Vec<u8>, CompressionError> {
        let map = |e: std::io::Error| CompressionError::CompressFailed(e.to_string());
        match self.inner {
            CompressorInner::Gzip(enc) => enc.finish().map_err(map),
            CompressorInner::Deflate(enc) => enc.finish().map_err(map),
            CompressorInner::Brotli(enc) => {
                // `into_inner` finalises the brotli stream and returns the
                // inner `Vec<u8>` containing all compressed output.
                Ok(enc.into_inner())
            }
            CompressorInner::Zstd(enc) => enc.finish().map_err(map),
        }
    }
}

// ---------------------------------------------------------------------------
// Decompressor
// ---------------------------------------------------------------------------

/// A streaming decompressor with bomb-detection guards.
pub struct Decompressor {
    algorithm: Algorithm,
}

impl Decompressor {
    /// Create a new decompressor for `algorithm`.
    ///
    /// # Errors
    ///
    /// Returns [`CompressionError::Unsupported`] if the algorithm is not
    /// supported (currently all four are).
    pub const fn new(algorithm: Algorithm) -> Result<Self, CompressionError> {
        Ok(Self { algorithm })
    }

    /// Decompress `input` with bomb protection.
    ///
    /// # Guards
    ///
    /// - **`max_bytes`** -- if the decompressed output exceeds this many bytes,
    ///   return [`CompressionError::OutputTooLarge`].
    /// - **`max_ratio`** -- if `decompressed_size / compressed_size` exceeds
    ///   this factor, return [`CompressionError::BombDetected`].
    ///
    /// # Errors
    ///
    /// Returns [`CompressionError::DecompressFailed`] on codec errors,
    /// [`CompressionError::OutputTooLarge`] or [`CompressionError::BombDetected`]
    /// when safety limits are breached.
    pub fn decompress(
        &mut self,
        input: &[u8],
        max_bytes: u64,
        max_ratio: f64,
    ) -> Result<Vec<u8>, CompressionError> {
        let compressed_len = input.len() as u64;
        match self.algorithm {
            Algorithm::Gzip => decompress_read(
                flate2::read::GzDecoder::new(input),
                compressed_len,
                max_bytes,
                max_ratio,
            ),
            Algorithm::Deflate => decompress_read(
                flate2::read::DeflateDecoder::new(input),
                compressed_len,
                max_bytes,
                max_ratio,
            ),
            Algorithm::Brotli => decompress_read(
                brotli::Decompressor::new(input, 4096),
                compressed_len,
                max_bytes,
                max_ratio,
            ),
            Algorithm::Zstd => {
                let decoder = zstd::stream::Decoder::new(input)
                    .map_err(|e| CompressionError::DecompressFailed(e.to_string()))?;
                decompress_read(decoder, compressed_len, max_bytes, max_ratio)
            }
        }
    }
}

/// Read from `reader` in chunks, enforcing `max_bytes` and `max_ratio` limits.
fn decompress_read<R: Read>(
    mut reader: R,
    compressed_len: u64,
    max_bytes: u64,
    max_ratio: f64,
) -> Result<Vec<u8>, CompressionError> {
    let mut output = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| CompressionError::DecompressFailed(e.to_string()))?;
        if n == 0 {
            break;
        }
        output.extend_from_slice(buf.get(..n).ok_or_else(|| {
            CompressionError::DecompressFailed("read returned count beyond buffer".into())
        })?);

        let decompressed_bytes = output.len() as u64;

        // Check absolute size limit.
        if decompressed_bytes > max_bytes {
            return Err(CompressionError::OutputTooLarge { max_bytes });
        }

        // Check ratio limit (only meaningful when compressed input is non-empty).
        if compressed_len > 0 {
            #[allow(clippy::cast_precision_loss)]
            let ratio = decompressed_bytes as f64 / compressed_len as f64;
            if ratio > max_ratio {
                return Err(CompressionError::BombDetected {
                    compressed_bytes: compressed_len,
                    decompressed_bytes,
                    ratio,
                });
            }
        }
    }
    Ok(output)
}

// ---------------------------------------------------------------------------
// Transcode
// ---------------------------------------------------------------------------

/// Re-encode `input` from one compression algorithm to another in a
/// streaming fashion.
///
/// Instead of materialising the entire decompressed payload in memory, this
/// reads chunks from the decompressor and feeds them directly to the
/// compressor.  Bomb-detection limits (`max_bytes`, `max_ratio`) are
/// enforced incrementally.
///
/// # Errors
///
/// Propagates any [`CompressionError`] from the decompression or compression
/// stages, including bomb-detection violations.
pub fn transcode(
    input: &[u8],
    from: Algorithm,
    to: Algorithm,
    max_bytes: u64,
    max_ratio: f64,
) -> Result<Vec<u8>, CompressionError> {
    let compressed_len = input.len() as u64;

    // Build the decompressor reader.
    let map_dec = |e: std::io::Error| CompressionError::DecompressFailed(e.to_string());

    let mut comp = Compressor::new(to, None)?;
    let mut output = Vec::new();

    // Helper closure: pump chunks from a reader through the compressor with
    // bomb-detection guards, avoiding holding the full plaintext in memory.
    let mut pump = |mut reader: Box<dyn Read>| -> Result<(), CompressionError> {
        let mut buf = [0u8; 8192];
        let mut total_decompressed: u64 = 0;

        loop {
            let n = reader.read(&mut buf).map_err(map_dec)?;
            if n == 0 {
                break;
            }

            let chunk = buf.get(..n).ok_or_else(|| {
                CompressionError::DecompressFailed("read returned count beyond buffer".into())
            })?;

            total_decompressed += n as u64;

            // Bomb-detection: absolute size.
            if total_decompressed > max_bytes {
                return Err(CompressionError::OutputTooLarge { max_bytes });
            }

            // Bomb-detection: ratio.
            if compressed_len > 0 {
                #[allow(clippy::cast_precision_loss)]
                let ratio = total_decompressed as f64 / compressed_len as f64;
                if ratio > max_ratio {
                    return Err(CompressionError::BombDetected {
                        compressed_bytes: compressed_len,
                        decompressed_bytes: total_decompressed,
                        ratio,
                    });
                }
            }

            output.extend(comp.compress(chunk)?);
        }
        Ok(())
    };

    match from {
        Algorithm::Gzip => {
            pump(Box::new(flate2::read::GzDecoder::new(input)))?;
        }
        Algorithm::Deflate => {
            pump(Box::new(flate2::read::DeflateDecoder::new(input)))?;
        }
        Algorithm::Brotli => {
            pump(Box::new(brotli::Decompressor::new(input, 4096)))?;
        }
        Algorithm::Zstd => {
            let decoder = zstd::stream::Decoder::new(input)
                .map_err(|e| CompressionError::DecompressFailed(e.to_string()))?;
            pump(Box::new(decoder))?;
        }
    }

    output.extend(comp.finish()?);
    Ok(output)
}

// ---------------------------------------------------------------------------
// BREACH guard
// ---------------------------------------------------------------------------

/// Stateless BREACH-attack mitigation.
///
/// BREACH exploits HTTP compression of responses that include both
/// attacker-controlled (reflected) input and secret tokens.  If both
/// conditions hold, compression must be suppressed.
pub struct BreachGuard;

impl BreachGuard {
    /// Returns `true` if it is safe to compress the response.
    ///
    /// Returns `false` when the response contains secret-bearing headers
    /// **and** the request could contain attacker-controlled reflected input,
    /// blocking the compression side-channel.
    #[must_use]
    pub const fn should_compress(has_secret_headers: bool, has_reflected_input: bool) -> bool {
        !(has_secret_headers && has_reflected_input)
    }
}

// ---------------------------------------------------------------------------
// Content negotiation
// ---------------------------------------------------------------------------

/// Parse an `Accept-Encoding` header value and return the highest-priority
/// supported algorithm.
///
/// Respects `q`-values (RFC 7231 sect. 5.3.4).  When two algorithms have
/// equal `q`-values, the server prefers zstd > br > gzip > deflate.
/// Returns `None` when no supported algorithm is acceptable.
#[must_use]
pub fn negotiate(accept_encoding: &str) -> Option<Algorithm> {
    let mut best: Option<(Algorithm, f32)> = None;

    for token in accept_encoding.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        // Split "gzip;q=0.8" into ("gzip", 0.8).
        let mut parts = token.splitn(2, ';');
        let name = parts.next()?.trim().to_ascii_lowercase();

        // If a q-parameter is present but invalid (NaN, inf, non-numeric),
        // skip this entry entirely rather than defaulting to 1.0.
        let q: f32 = match parts.next() {
            Some(param) => match parse_qvalue(param) {
                Some(v) => v,
                None => continue,
            },
            None => 1.0,
        };

        if q <= 0.0 {
            continue;
        }

        let algo = match name.as_str() {
            "zstd" => Algorithm::Zstd,
            "br" => Algorithm::Brotli,
            "gzip" => Algorithm::Gzip,
            "deflate" => Algorithm::Deflate,
            _ => continue,
        };

        let dominated = best.is_some_and(|(best_algo, bq)| {
            #[allow(clippy::float_cmp)]
            // q-values are parsed, not computed; exact equality is intentional.
            let tie = bq == q;
            bq > q || (tie && server_preference(best_algo) >= server_preference(algo))
        });
        if !dominated {
            best = Some((algo, q));
        }
    }

    best.map(|(a, _)| a)
}

/// Server-side preference ordering for tie-breaking equal q-values.
///
/// Higher value = more preferred.
const fn server_preference(algo: Algorithm) -> u8 {
    match algo {
        Algorithm::Zstd => 4,
        Algorithm::Brotli => 3,
        Algorithm::Gzip => 2,
        Algorithm::Deflate => 1,
    }
}

/// Extract the `q=<value>` from a parameter string like ` q=0.8`.
///
/// Rejects non-finite values (NaN, Infinity) and clamps to `[0.0, 1.0]`.
fn parse_qvalue(s: &str) -> Option<f32> {
    let s = s.trim();
    let s = s.strip_prefix("q=")?;
    s.trim()
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Drain a `Vec<u8>` in-place, returning the contents while leaving an empty
/// vec with the same allocation.
fn drain_vec(v: &mut Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len());
    out.append(v);
    out
}

/// Clamp a signed level to `u32`, flooring at 0.
#[allow(clippy::cast_sign_loss)] // Guarded by the `v < 0` check above.
const fn clamp_u32(v: i32) -> u32 {
    if v < 0 { 0 } else { v as u32 }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_algorithms() {
        let data = b"The quick brown fox jumps over the lazy dog. Repeated enough to compress. \
                     The quick brown fox jumps over the lazy dog.";

        for algo in [
            Algorithm::Gzip,
            Algorithm::Deflate,
            Algorithm::Brotli,
            Algorithm::Zstd,
        ] {
            let mut c = Compressor::new(algo, None).unwrap();
            let mut compressed = c.compress(data).unwrap();
            compressed.extend(c.finish().unwrap());

            let mut d = Decompressor::new(algo).unwrap();
            let decompressed = d.decompress(&compressed, 1_000_000, 1000.0).unwrap();
            assert_eq!(decompressed, data, "roundtrip failed for {algo:?}");
        }
    }

    #[test]
    fn bomb_detection_max_bytes() {
        let zeros = vec![0u8; 100_000];
        let mut c = Compressor::new(Algorithm::Gzip, None).unwrap();
        let mut compressed = c.compress(&zeros).unwrap();
        compressed.extend(c.finish().unwrap());

        let mut d = Decompressor::new(Algorithm::Gzip).unwrap();
        let err = d.decompress(&compressed, 1000, 100_000.0).unwrap_err();
        assert!(
            matches!(err, CompressionError::OutputTooLarge { .. }),
            "expected OutputTooLarge, got {err:?}"
        );
    }

    #[test]
    fn bomb_detection_ratio() {
        let zeros = vec![0u8; 1_000_000];
        let mut c = Compressor::new(Algorithm::Gzip, None).unwrap();
        let mut compressed = c.compress(&zeros).unwrap();
        compressed.extend(c.finish().unwrap());

        let mut d = Decompressor::new(Algorithm::Gzip).unwrap();
        let err = d.decompress(&compressed, 10_000_000, 2.0).unwrap_err();
        assert!(
            matches!(err, CompressionError::BombDetected { .. }),
            "expected BombDetected, got {err:?}"
        );
    }

    #[test]
    fn transcode_gzip_to_zstd() {
        let original = b"Transcode test data that repeats repeats repeats for ratio.";
        let mut c = Compressor::new(Algorithm::Gzip, None).unwrap();
        let mut gz = c.compress(original).unwrap();
        gz.extend(c.finish().unwrap());

        let zstd_data =
            transcode(&gz, Algorithm::Gzip, Algorithm::Zstd, 1_000_000, 1000.0).unwrap();

        let mut d = Decompressor::new(Algorithm::Zstd).unwrap();
        let result = d.decompress(&zstd_data, 1_000_000, 1000.0).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn breach_guard_logic() {
        assert!(!BreachGuard::should_compress(true, true));
        assert!(BreachGuard::should_compress(true, false));
        assert!(BreachGuard::should_compress(false, true));
        assert!(BreachGuard::should_compress(false, false));
    }

    #[test]
    fn negotiate_picks_highest_q() {
        assert_eq!(
            negotiate("gzip;q=0.5, br;q=1.0, zstd;q=0.9"),
            Some(Algorithm::Brotli)
        );
        assert_eq!(negotiate("deflate"), Some(Algorithm::Deflate));
        assert_eq!(negotiate("identity"), None);
        assert_eq!(negotiate("gzip;q=0, br;q=0"), None);
    }

    #[test]
    fn negotiate_empty() {
        assert_eq!(negotiate(""), None);
    }

    #[test]
    fn negotiate_rejects_nan_and_inf() {
        assert_eq!(negotiate("gzip;q=NaN"), None);
        assert_eq!(negotiate("gzip;q=Infinity"), None);
        assert_eq!(negotiate("gzip;q=-Infinity"), None);
        assert_eq!(negotiate("gzip;q=inf"), None);
    }

    #[test]
    fn negotiate_clamps_q_above_one() {
        // q=1.5 is clamped to 1.0, so gzip is still chosen.
        assert_eq!(negotiate("gzip;q=1.5"), Some(Algorithm::Gzip));
    }

    #[test]
    fn negotiate_server_preference_tiebreak() {
        // All equal q=1.0 — server prefers zstd.
        assert_eq!(negotiate("gzip, br, zstd, deflate"), Some(Algorithm::Zstd));
        // Without zstd, prefer brotli.
        assert_eq!(negotiate("gzip, br, deflate"), Some(Algorithm::Brotli));
        // Without brotli, prefer gzip.
        assert_eq!(negotiate("gzip, deflate"), Some(Algorithm::Gzip));
    }
}
