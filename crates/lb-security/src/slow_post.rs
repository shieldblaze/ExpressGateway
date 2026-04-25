//! Slow POST attack detection.
//!
//! Detects clients that declare a large `Content-Length` but trickle the body
//! at an extremely slow rate, occupying backend connections for an extended
//! period.

use crate::SecurityError;

/// Tracks body receipt rate to detect slow POST attacks.
///
/// The detector enforces a minimum bytes-per-second throughput for the body
/// phase over a recent window. If the observed rate drops below the threshold,
/// the connection is flagged for termination.
pub struct SlowPostDetector {
    body_timeout_ms: u64,
    min_rate_bytes_per_sec: u64,
    last_check_time_ms: u64,
    last_check_bytes: u64,
}

impl SlowPostDetector {
    /// Create a new detector.
    ///
    /// # Arguments
    ///
    /// * `body_timeout_ms` - Maximum milliseconds allowed for the body phase.
    /// * `min_rate_bytes_per_sec` - Minimum bytes-per-second throughput.
    #[must_use]
    pub const fn new(body_timeout_ms: u64, min_rate_bytes_per_sec: u64) -> Self {
        Self {
            body_timeout_ms,
            min_rate_bytes_per_sec,
            last_check_time_ms: 0,
            last_check_bytes: 0,
        }
    }

    /// Record body bytes received and check the rate over the recent window.
    ///
    /// The rate is computed over the interval since the last check rather than
    /// as a lifetime average. If the window is shorter than 1000 ms the rate
    /// check is skipped (too little data to draw a conclusion).
    ///
    /// # Arguments
    ///
    /// * `total_body_bytes` - Total body bytes received so far (cumulative).
    /// * `elapsed_ms` - Milliseconds since the body phase began.
    /// * `content_length` - Declared `Content-Length` of the request.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::SlowPost`] if the observed rate over the recent
    /// window falls below `min_rate_bytes_per_sec`, or if `elapsed_ms` exceeds
    /// `body_timeout_ms`.
    pub fn record_body_bytes(
        &mut self,
        total_body_bytes: u64,
        elapsed_ms: u64,
        content_length: u64,
    ) -> Result<(), SecurityError> {
        let _ = content_length; // Available for future heuristics (e.g., expected rate).

        if elapsed_ms == 0 {
            return Ok(());
        }

        // Check absolute timeout.
        if elapsed_ms > self.body_timeout_ms {
            return Err(SecurityError::SlowPost {
                rate_bps: total_body_bytes.saturating_mul(1000) / elapsed_ms,
                min_rate_bps: self.min_rate_bytes_per_sec,
            });
        }

        // Windowed rate check.
        let window_ms = elapsed_ms.saturating_sub(self.last_check_time_ms);
        let window_bytes = total_body_bytes.saturating_sub(self.last_check_bytes);

        // Skip if window is too short to be meaningful.
        if window_ms < 1000 {
            return Ok(());
        }

        let rate_bps = window_bytes.saturating_mul(1000) / window_ms;

        // Update checkpoint regardless of pass/fail.
        self.last_check_time_ms = elapsed_ms;
        self.last_check_bytes = total_body_bytes;

        if rate_bps < self.min_rate_bytes_per_sec {
            return Err(SecurityError::SlowPost {
                rate_bps,
                min_rate_bps: self.min_rate_bytes_per_sec,
            });
        }

        Ok(())
    }
}
