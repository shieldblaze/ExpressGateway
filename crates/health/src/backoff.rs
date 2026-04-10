//! Exponential backoff for health check retries.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Exponential backoff calculator.
///
/// Computes delay as `base_ms * 2^(failures-1)`, capped at `max_ms`.
/// Resets to zero delay on success.
pub struct ExponentialBackoff {
    base_ms: u64,
    max_ms: u64,
    failures: AtomicU32,
}

impl ExponentialBackoff {
    /// Create a new exponential backoff with the given base and max delays.
    pub fn new(base_ms: u64, max_ms: u64) -> Self {
        Self {
            base_ms,
            max_ms,
            failures: AtomicU32::new(0),
        }
    }

    /// Compute the next delay based on current failure count.
    ///
    /// Returns `Duration::ZERO` if no failures have been recorded.
    pub fn next_delay(&self) -> Duration {
        let failures = self.failures.load(Ordering::Relaxed);
        if failures == 0 {
            return Duration::ZERO;
        }

        let exponent = (failures - 1).min(31);
        let delay_ms = self
            .base_ms
            .saturating_mul(1u64 << exponent)
            .min(self.max_ms);

        Duration::from_millis(delay_ms)
    }

    /// Record a failure, incrementing the backoff.
    pub fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a success, resetting the backoff to zero.
    pub fn record_success(&self) {
        self.failures.store(0, Ordering::Relaxed);
    }

    /// Current failure count.
    pub fn failure_count(&self) -> u32 {
        self.failures.load(Ordering::Relaxed)
    }
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self::new(1000, 60_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_failures_zero_delay() {
        let backoff = ExponentialBackoff::default();
        assert_eq!(backoff.next_delay(), Duration::ZERO);
    }

    #[test]
    fn test_first_failure_base_delay() {
        let backoff = ExponentialBackoff::default();
        backoff.record_failure();
        assert_eq!(backoff.next_delay(), Duration::from_millis(1000));
    }

    #[test]
    fn test_exponential_increase() {
        let backoff = ExponentialBackoff::new(1000, 60_000);

        backoff.record_failure(); // failures = 1: 1000 * 2^0 = 1000
        assert_eq!(backoff.next_delay(), Duration::from_millis(1000));

        backoff.record_failure(); // failures = 2: 1000 * 2^1 = 2000
        assert_eq!(backoff.next_delay(), Duration::from_millis(2000));

        backoff.record_failure(); // failures = 3: 1000 * 2^2 = 4000
        assert_eq!(backoff.next_delay(), Duration::from_millis(4000));

        backoff.record_failure(); // failures = 4: 1000 * 2^3 = 8000
        assert_eq!(backoff.next_delay(), Duration::from_millis(8000));

        backoff.record_failure(); // failures = 5: 1000 * 2^4 = 16000
        assert_eq!(backoff.next_delay(), Duration::from_millis(16000));

        backoff.record_failure(); // failures = 6: 1000 * 2^5 = 32000
        assert_eq!(backoff.next_delay(), Duration::from_millis(32000));
    }

    #[test]
    fn test_max_cap() {
        let backoff = ExponentialBackoff::new(1000, 60_000);
        for _ in 0..20 {
            backoff.record_failure();
        }
        assert_eq!(backoff.next_delay(), Duration::from_millis(60_000));
    }

    #[test]
    fn test_success_resets() {
        let backoff = ExponentialBackoff::default();
        backoff.record_failure();
        backoff.record_failure();
        assert_eq!(backoff.failure_count(), 2);
        assert!(backoff.next_delay() > Duration::ZERO);

        backoff.record_success();
        assert_eq!(backoff.failure_count(), 0);
        assert_eq!(backoff.next_delay(), Duration::ZERO);
    }

    #[test]
    fn test_overflow_protection() {
        let backoff = ExponentialBackoff::new(1000, 60_000);
        // Push failures very high to test overflow protection
        for _ in 0..100 {
            backoff.record_failure();
        }
        // Should not panic, should be capped at max
        assert_eq!(backoff.next_delay(), Duration::from_millis(60_000));
    }
}
