//! Slow-POST detection: a large declared `Content-Length` trickled slowly to pin backend
//! connections.

use crate::SecurityError;

/// Enforces a bytes-per-second floor on the body phase over a recent window.
pub struct SlowPostDetector {
    body_timeout_ms: u64,
    min_rate_bytes_per_sec: u64,
    last_check_time_ms: u64,
    last_check_bytes: u64,
}

impl SlowPostDetector {
    /// Create a detector.
    #[must_use]
    pub const fn new(body_timeout_ms: u64, min_rate_bytes_per_sec: u64) -> Self {
        Self {
            body_timeout_ms,
            min_rate_bytes_per_sec,
            last_check_time_ms: 0,
            last_check_bytes: 0,
        }
    }

    /// Record cumulative body bytes and check the rate SINCE THE LAST CHECK — a lifetime average
    /// would let an attacker bank an early burst and then trickle.
    pub fn record_body_bytes(
        &mut self,
        total_body_bytes: u64,
        elapsed_ms: u64,
        content_length: u64,
    ) -> Result<(), SecurityError> {
        let _ = content_length; // Reserved for expected-rate heuristics.

        if elapsed_ms == 0 {
            return Ok(());
        }

        if elapsed_ms > self.body_timeout_ms {
            return Err(SecurityError::SlowPost {
                rate_bps: total_body_bytes.saturating_mul(1000) / elapsed_ms,
                min_rate_bps: self.min_rate_bytes_per_sec,
            });
        }

        let window_ms = elapsed_ms.saturating_sub(self.last_check_time_ms);
        let window_bytes = total_body_bytes.saturating_sub(self.last_check_bytes);

        // Too short a window to conclude anything.
        if window_ms < 1000 {
            return Ok(());
        }

        let rate_bps = window_bytes.saturating_mul(1000) / window_ms;

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
