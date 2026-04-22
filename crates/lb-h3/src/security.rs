//! HTTP/3 security mitigation detectors.
//!
//! - **QPACK Bomb**: detects decompression-ratio amplification attacks
//!   analogous to HPACK bombs in HTTP/2.

use crate::H3Error;

/// Detects QPACK decompression bombs by tracking the ratio of decoded header
/// size to encoded wire size.
#[derive(Debug)]
pub struct QpackBombDetector {
    max_ratio: u64,
    max_decoded_size: u64,
}

impl QpackBombDetector {
    /// Create a detector.
    ///
    /// * `max_ratio` — maximum allowed decoded/encoded byte ratio.
    /// * `max_decoded_size` — absolute cap on decoded header bytes.
    #[must_use]
    pub const fn new(max_ratio: u64, max_decoded_size: u64) -> Self {
        Self {
            max_ratio,
            max_decoded_size,
        }
    }

    /// Check whether the given encoded and decoded sizes are within limits.
    ///
    /// # Errors
    ///
    /// Returns `H3Error::QpackBomb` if either the ratio or absolute size
    /// exceeds the configured limits.
    pub const fn check(&self, encoded_size: u64, decoded_size: u64) -> Result<(), H3Error> {
        if decoded_size > self.max_decoded_size {
            let ratio = if encoded_size > 0 {
                decoded_size / encoded_size
            } else {
                decoded_size
            };
            return Err(H3Error::QpackBomb {
                decoded: decoded_size,
                encoded: encoded_size,
                ratio,
            });
        }

        if encoded_size > 0 {
            let ratio = decoded_size / encoded_size;
            if ratio > self.max_ratio {
                return Err(H3Error::QpackBomb {
                    decoded: decoded_size,
                    encoded: encoded_size,
                    ratio,
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_input_ok() {
        let det = QpackBombDetector::new(100, 65536);
        assert!(det.check(1000, 2000).is_ok());
    }

    #[test]
    fn ratio_exceeded() {
        let det = QpackBombDetector::new(100, 1_000_000);
        assert!(det.check(1024, 204_800).is_err());
    }

    #[test]
    fn size_exceeded() {
        let det = QpackBombDetector::new(100, 65536);
        assert!(det.check(10_000, 100_000).is_err());
    }

    #[test]
    fn zero_encoded() {
        let det = QpackBombDetector::new(100, 65536);
        assert!(det.check(0, 100_000).is_err());
    }
}
