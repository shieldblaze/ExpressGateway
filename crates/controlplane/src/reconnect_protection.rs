//! Reconnect storm protection using a token bucket rate limiter.
//!
//! Prevents thundering herd when the control plane restarts and all
//! data plane nodes try to re-register simultaneously.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Token bucket rate limiter for protecting against reconnect storms.
///
/// Allows `capacity` registrations per `refill_interval` period.
pub struct ReconnectProtection {
    /// Maximum tokens in the bucket.
    capacity: u64,
    /// Tokens refilled per second.
    refill_rate: f64,
    /// Current token count (scaled by 1000 for sub-token precision).
    tokens: AtomicU64,
    /// Last refill timestamp.
    last_refill: parking_lot::Mutex<Instant>,
}

impl ReconnectProtection {
    /// Create a new rate limiter with the given capacity and refill rate (tokens/sec).
    pub fn new(capacity: u64, refill_rate: f64) -> Self {
        Self {
            capacity,
            refill_rate,
            tokens: AtomicU64::new(capacity * 1000),
            last_refill: parking_lot::Mutex::new(Instant::now()),
        }
    }

    /// Create a default rate limiter suitable for production use.
    /// Allows 50 registrations per second with a burst of 100.
    pub fn default_production() -> Self {
        Self::new(100, 50.0)
    }

    /// Try to acquire a token for a registration attempt.
    /// Returns `true` if the registration is allowed, `false` if rate-limited.
    pub fn try_acquire(&self) -> bool {
        self.refill();

        loop {
            let current = self.tokens.load(Ordering::Acquire);
            if current < 1000 {
                return false;
            }
            if self
                .tokens
                .compare_exchange_weak(current, current - 1000, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }
    }

    /// Refill tokens based on elapsed time.
    fn refill(&self) {
        let mut last = self.last_refill.lock();
        let now = Instant::now();
        let elapsed = now.duration_since(*last).as_secs_f64();

        if elapsed > 0.001 {
            let new_tokens = (elapsed * self.refill_rate * 1000.0) as u64;
            if new_tokens > 0 {
                let max_scaled = self.capacity * 1000;
                loop {
                    let current = self.tokens.load(Ordering::Acquire);
                    let target = (current + new_tokens).min(max_scaled);
                    if current == target {
                        break;
                    }
                    if self
                        .tokens
                        .compare_exchange_weak(current, target, Ordering::AcqRel, Ordering::Relaxed)
                        .is_ok()
                    {
                        break;
                    }
                }
                *last = now;
            }
        }
    }

    /// Get the approximate number of available tokens.
    pub fn available_tokens(&self) -> u64 {
        self.tokens.load(Ordering::Relaxed) / 1000
    }

    /// Get the capacity of the bucket.
    pub fn capacity(&self) -> u64 {
        self.capacity
    }
}

impl Default for ReconnectProtection {
    fn default() -> Self {
        Self::default_production()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_within_capacity() {
        let limiter = ReconnectProtection::new(10, 10.0);
        for _ in 0..10 {
            assert!(limiter.try_acquire());
        }
    }

    #[test]
    fn rejects_over_capacity() {
        let limiter = ReconnectProtection::new(5, 0.0);
        for _ in 0..5 {
            assert!(limiter.try_acquire());
        }
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn available_tokens_decreases() {
        let limiter = ReconnectProtection::new(10, 0.0);
        assert_eq!(limiter.available_tokens(), 10);
        limiter.try_acquire();
        assert_eq!(limiter.available_tokens(), 9);
    }

    #[test]
    fn capacity_getter() {
        let limiter = ReconnectProtection::new(42, 10.0);
        assert_eq!(limiter.capacity(), 42);
    }

    #[test]
    fn default_production_values() {
        let limiter = ReconnectProtection::default_production();
        assert_eq!(limiter.capacity(), 100);
    }
}
