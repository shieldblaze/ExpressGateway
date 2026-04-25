//! Slowloris attack detection.
//!
//! Detects connections that send HTTP headers at an extremely slow rate,
//! tying up server resources. Enforces both a maximum header phase duration
//! and a minimum byte rate.

use crate::SecurityError;

/// Tracks connection timing to detect slowloris-style attacks.
///
/// A slowloris attack opens many connections and sends headers very slowly,
/// keeping each connection alive just below server timeouts. This detector
/// enforces two independent thresholds:
///
/// 1. **Header timeout**: absolute wall-clock limit on the header phase.
/// 2. **Minimum rate**: bytes-per-second floor over a recent window below
///    which the connection is killed.
pub struct SlowlorisDetector {
    header_timeout_ms: u64,
    min_rate_bytes_per_sec: u64,
    last_check_time_ms: u64,
    last_check_bytes: u64,
}

impl SlowlorisDetector {
    /// Create a new detector.
    ///
    /// # Arguments
    ///
    /// * `header_timeout_ms` - Maximum milliseconds allowed for the header phase.
    /// * `min_rate_bytes_per_sec` - Minimum bytes-per-second rate during header receipt.
    #[must_use]
    pub const fn new(header_timeout_ms: u64, min_rate_bytes_per_sec: u64) -> Self {
        Self {
            header_timeout_ms,
            min_rate_bytes_per_sec,
            last_check_time_ms: 0,
            last_check_bytes: 0,
        }
    }

    /// Record header bytes received and check the byte rate over the recent window.
    ///
    /// The rate is computed over the interval since the last check rather than
    /// as a lifetime average. If the window is shorter than 1000 ms the check
    /// is skipped (too little data to draw a conclusion).
    ///
    /// # Arguments
    ///
    /// * `bytes_received` - Total bytes received so far (cumulative).
    /// * `elapsed_ms` - Milliseconds since connection start.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::SlowlorisRate`] if the observed rate over the
    /// recent window falls below `min_rate_bytes_per_sec`.
    pub fn record_bytes(
        &mut self,
        bytes_received: u64,
        elapsed_ms: u64,
    ) -> Result<(), SecurityError> {
        let window_ms = elapsed_ms.saturating_sub(self.last_check_time_ms);
        let window_bytes = bytes_received.saturating_sub(self.last_check_bytes);

        // Skip the rate check if the window is too short to be meaningful.
        if window_ms < 1000 {
            return Ok(());
        }

        let rate_bps = window_bytes.saturating_mul(1000) / window_ms;

        // Update checkpoint regardless of pass/fail so the next window
        // starts fresh.
        self.last_check_time_ms = elapsed_ms;
        self.last_check_bytes = bytes_received;

        if rate_bps < self.min_rate_bytes_per_sec {
            return Err(SecurityError::SlowlorisRate {
                rate_bps,
                min_rate_bps: self.min_rate_bytes_per_sec,
            });
        }

        Ok(())
    }

    /// Check whether the header phase has exceeded its timeout.
    ///
    /// # Arguments
    ///
    /// * `elapsed_ms` - Milliseconds since connection start.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::SlowlorisTimeout`] if `elapsed_ms` exceeds
    /// the configured `header_timeout_ms`.
    pub const fn check_header_timeout(&self, elapsed_ms: u64) -> Result<(), SecurityError> {
        if elapsed_ms > self.header_timeout_ms {
            return Err(SecurityError::SlowlorisTimeout {
                elapsed_ms,
                timeout_ms: self.header_timeout_ms,
            });
        }
        Ok(())
    }
}
