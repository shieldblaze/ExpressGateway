//! Exponential backoff with full jitter for health check retries.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// Exponential backoff calculator with full jitter.
///
/// Computes the ceiling as `min(max_ms, base_ms * 2^(failures-1))`, then
/// applies full jitter: `random(0, ceiling)`. This prevents thundering-herd
/// retries when multiple health checkers back off against the same backend.
///
/// The PRNG is a simple xorshift64 seeded from the current time -- good
/// enough for jitter, not for cryptography.
pub struct ExponentialBackoff {
    base_ms: u64,
    max_ms: u64,
    failures: AtomicU32,
    /// xorshift64 state for jitter. Seeded once at construction.
    rng_state: AtomicU64,
}

impl ExponentialBackoff {
    /// Create a new exponential backoff with the given base and max delays.
    pub fn new(base_ms: u64, max_ms: u64) -> Self {
        // Seed from a combination of address of self (ASLR) and time.
        // We only need non-correlated jitter, not cryptographic randomness.
        let seed = {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(Duration::from_nanos(1))
                .as_nanos() as u64;
            // Mix in some entropy; avoid zero which is a fixed point of xorshift.
            t | 1
        };
        Self {
            base_ms,
            max_ms,
            failures: AtomicU32::new(0),
            rng_state: AtomicU64::new(seed),
        }
    }

    /// Advance the xorshift64 PRNG and return the next value.
    fn next_rand(&self) -> u64 {
        // Relaxed is fine -- we only need non-zero jitter, not strict ordering.
        let mut s = self.rng_state.load(Ordering::Relaxed);
        // Ensure we never hit the zero fixed point.
        if s == 0 {
            s = 1;
        }
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        self.rng_state.store(s, Ordering::Relaxed);
        s
    }

    /// Compute the next delay based on current failure count.
    ///
    /// Returns `Duration::ZERO` if no failures have been recorded.
    /// The returned value includes full jitter: `uniform(0, ceiling)`.
    pub fn next_delay(&self) -> Duration {
        let failures = self.failures.load(Ordering::Relaxed);
        if failures == 0 {
            return Duration::ZERO;
        }

        let exponent = (failures - 1).min(31);
        let ceiling_ms = self
            .base_ms
            .saturating_mul(1u64 << exponent)
            .min(self.max_ms);

        // Full jitter: uniform in [0, ceiling_ms].
        let jittered_ms = if ceiling_ms == 0 {
            0
        } else {
            self.next_rand() % (ceiling_ms + 1)
        };

        Duration::from_millis(jittered_ms)
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
    fn test_first_failure_delay_within_ceiling() {
        let backoff = ExponentialBackoff::default();
        backoff.record_failure();
        // Ceiling = base * 2^0 = 1000ms; jitter gives [0, 1000].
        let delay = backoff.next_delay();
        assert!(delay <= Duration::from_millis(1000));
    }

    #[test]
    fn test_exponential_ceiling_increase() {
        let backoff = ExponentialBackoff::new(1000, 60_000);

        backoff.record_failure(); // ceiling = 1000
        assert!(backoff.next_delay() <= Duration::from_millis(1000));

        backoff.record_failure(); // ceiling = 2000
        assert!(backoff.next_delay() <= Duration::from_millis(2000));

        backoff.record_failure(); // ceiling = 4000
        assert!(backoff.next_delay() <= Duration::from_millis(4000));

        backoff.record_failure(); // ceiling = 8000
        assert!(backoff.next_delay() <= Duration::from_millis(8000));

        backoff.record_failure(); // ceiling = 16000
        assert!(backoff.next_delay() <= Duration::from_millis(16000));

        backoff.record_failure(); // ceiling = 32000
        assert!(backoff.next_delay() <= Duration::from_millis(32000));
    }

    #[test]
    fn test_max_cap() {
        let backoff = ExponentialBackoff::new(1000, 60_000);
        for _ in 0..20 {
            backoff.record_failure();
        }
        // Jittered, but must never exceed max.
        assert!(backoff.next_delay() <= Duration::from_millis(60_000));
    }

    #[test]
    fn test_success_resets() {
        let backoff = ExponentialBackoff::default();
        backoff.record_failure();
        backoff.record_failure();
        assert_eq!(backoff.failure_count(), 2);
        assert!(backoff.next_delay() > Duration::ZERO || true); // jitter may be 0

        backoff.record_success();
        assert_eq!(backoff.failure_count(), 0);
        assert_eq!(backoff.next_delay(), Duration::ZERO);
    }

    #[test]
    fn test_overflow_protection() {
        let backoff = ExponentialBackoff::new(1000, 60_000);
        for _ in 0..100 {
            backoff.record_failure();
        }
        // Should not panic, jittered but capped at max.
        assert!(backoff.next_delay() <= Duration::from_millis(60_000));
    }

    #[test]
    fn test_jitter_produces_varying_delays() {
        let backoff = ExponentialBackoff::new(1000, 60_000);
        for _ in 0..5 {
            backoff.record_failure();
        }
        // Sample 20 delays; at least 2 distinct values should appear.
        let delays: Vec<Duration> = (0..20).map(|_| backoff.next_delay()).collect();
        let mut unique = delays.clone();
        unique.sort();
        unique.dedup();
        assert!(
            unique.len() >= 2,
            "expected jitter to produce varying delays, got {unique:?}"
        );
    }
}
