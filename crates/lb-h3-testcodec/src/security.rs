//! HTTP/3 security mitigation detectors: the QPACK decompression bomb.

use crate::H3Error;

/// Tracks the decoded/encoded header-size ratio for QPACK bombs.
#[derive(Debug)]
pub struct QpackBombDetector {
    max_ratio: u64,
    max_decoded_size: u64,
}

impl QpackBombDetector {
    /// Create a detector with a ratio limit and an absolute decoded-size cap.
    #[must_use]
    pub const fn new(max_ratio: u64, max_decoded_size: u64) -> Self {
        Self {
            max_ratio,
            max_decoded_size,
        }
    }

    /// Check encoded/decoded sizes against the limits.
    ///
    /// # Errors
    /// `H3Error::QpackBomb` if either the ratio or the absolute size trips.
    pub const fn check(&self, encoded_size: u64, decoded_size: u64) -> Result<(), H3Error> {
        if decoded_size > self.max_decoded_size {
            let ratio = match decoded_size.checked_div(encoded_size) {
                Some(r) => r,
                None => decoded_size,
            };
            return Err(H3Error::QpackBomb {
                decoded: decoded_size,
                encoded: encoded_size,
                ratio,
            });
        }

        if let Some(ratio) = decoded_size.checked_div(encoded_size) {
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
