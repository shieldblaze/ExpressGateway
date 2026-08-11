//! Slowloris attack detection — a header-phase deadline plus a minimum byte rate.

use crate::SecurityError;

/// Two independent thresholds against slow-header attacks: a wall-clock header-phase cap and a
/// bytes-per-second floor over a recent window.
pub struct SlowlorisDetector {
    header_timeout_ms: u64,
    min_rate_bytes_per_sec: u64,
    last_check_time_ms: u64,
    last_check_bytes: u64,
}

impl SlowlorisDetector {
    /// Create a detector.
    #[must_use]
    pub const fn new(header_timeout_ms: u64, min_rate_bytes_per_sec: u64) -> Self {
        Self {
            header_timeout_ms,
            min_rate_bytes_per_sec,
            last_check_time_ms: 0,
            last_check_bytes: 0,
        }
    }

    /// Record cumulative header bytes and check the rate SINCE THE LAST CHECK — a lifetime
    /// average would let an attacker bank an early burst and then trickle.
    ///
    /// # Errors
    ///
    /// [`SecurityError::SlowlorisRate`] when the windowed rate is below the floor.
    pub fn record_bytes(
        &mut self,
        bytes_received: u64,
        elapsed_ms: u64,
    ) -> Result<(), SecurityError> {
        let window_ms = elapsed_ms.saturating_sub(self.last_check_time_ms);
        let window_bytes = bytes_received.saturating_sub(self.last_check_bytes);

        // Too short a window to conclude anything.
        if window_ms < 1000 {
            return Ok(());
        }

        let rate_bps = window_bytes.saturating_mul(1000) / window_ms;

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

    /// Check the header phase against its wall-clock cap.
    ///
    /// # Errors
    ///
    /// [`SecurityError::SlowlorisTimeout`].
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
