//! Lock-free circuit breaker with CAS-based state transitions.

use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use expressgateway_core::health::{CircuitBreakerConfig, CircuitBreakerState};

/// Maximum concurrent requests allowed in HALF_OPEN state.
const DEFAULT_HALF_OPEN_PERMITS: u32 = 3;

const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// Lock-free circuit breaker implementation.
///
/// State machine:
/// ```text
/// CLOSED --(failure_threshold failures)--> OPEN
/// OPEN --(open_duration elapsed)--> HALF_OPEN
/// HALF_OPEN --(success_threshold successes)--> CLOSED
/// HALF_OPEN --(any failure)--> OPEN
/// ```
pub struct CircuitBreaker {
    state: AtomicU8,
    failure_count: AtomicU32,
    success_count: AtomicU32,
    last_failure_time: AtomicU64,
    config: CircuitBreakerConfig,
    half_open_permits: AtomicU32,
    max_half_open_permits: u32,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            state: AtomicU8::new(STATE_CLOSED),
            failure_count: AtomicU32::new(0),
            success_count: AtomicU32::new(0),
            last_failure_time: AtomicU64::new(0),
            config,
            half_open_permits: AtomicU32::new(DEFAULT_HALF_OPEN_PERMITS),
            max_half_open_permits: DEFAULT_HALF_OPEN_PERMITS,
        }
    }

    /// Current circuit breaker state.
    ///
    /// This method may trigger a lazy OPEN -> HALF_OPEN transition if the
    /// open duration has elapsed. The transition is performed via CAS so
    /// exactly one caller will execute it.
    pub fn state(&self) -> CircuitBreakerState {
        let raw = self.state.load(Ordering::Acquire);
        // Check for OPEN -> HALF_OPEN transition based on elapsed time.
        if raw == STATE_OPEN {
            let last_failure_nanos = self.last_failure_time.load(Ordering::Acquire);
            if last_failure_nanos > 0 {
                let now_nanos = nanos_since_epoch();
                let elapsed = Duration::from_nanos(now_nanos.saturating_sub(last_failure_nanos));
                if elapsed >= self.config.open_duration {
                    // Reset counters BEFORE the CAS so that any thread
                    // observing STATE_HALF_OPEN (via Acquire on the CAS)
                    // sees the zeroed counters. The extra stores are
                    // harmless if the CAS fails (we remain OPEN and the
                    // values are unused until the next successful transition).
                    self.success_count.store(0, Ordering::Release);
                    self.half_open_permits
                        .store(self.max_half_open_permits, Ordering::Release);

                    if self
                        .state
                        .compare_exchange(
                            STATE_OPEN,
                            STATE_HALF_OPEN,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return CircuitBreakerState::HalfOpen;
                    }
                }
            }
            return u8_to_state(self.state.load(Ordering::Acquire));
        }
        u8_to_state(raw)
    }

    /// Whether a request should be allowed through.
    pub fn allow_request(&self) -> bool {
        match self.state() {
            CircuitBreakerState::Closed => true,
            CircuitBreakerState::Open => false,
            CircuitBreakerState::HalfOpen => {
                // Allow a limited number of concurrent requests.
                loop {
                    let permits = self.half_open_permits.load(Ordering::Acquire);
                    if permits == 0 {
                        return false;
                    }
                    if self
                        .half_open_permits
                        .compare_exchange_weak(
                            permits,
                            permits - 1,
                            Ordering::AcqRel,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        return true;
                    }
                }
            }
        }
    }

    /// Record a successful request.
    pub fn record_success(&self) {
        let current = self.state.load(Ordering::Acquire);
        match current {
            STATE_CLOSED => {
                // Reset failure count on success while closed.
                self.failure_count.store(0, Ordering::Release);
            }
            STATE_HALF_OPEN => {
                let prev = self.success_count.fetch_add(1, Ordering::AcqRel);
                if prev + 1 >= self.config.success_threshold {
                    // Transition HALF_OPEN -> CLOSED.
                    if self
                        .state
                        .compare_exchange(
                            STATE_HALF_OPEN,
                            STATE_CLOSED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        self.failure_count.store(0, Ordering::Release);
                        self.success_count.store(0, Ordering::Release);
                    }
                }
            }
            _ => {}
        }
    }

    /// Record a failed request.
    pub fn record_failure(&self) {
        let current = self.state.load(Ordering::Acquire);
        match current {
            STATE_CLOSED => {
                let prev = self.failure_count.fetch_add(1, Ordering::AcqRel);
                if prev + 1 >= self.config.failure_threshold {
                    // Transition CLOSED -> OPEN.
                    if self
                        .state
                        .compare_exchange(
                            STATE_CLOSED,
                            STATE_OPEN,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        self.last_failure_time
                            .store(nanos_since_epoch(), Ordering::Release);
                    }
                }
            }
            STATE_HALF_OPEN => {
                // Any failure in HALF_OPEN transitions back to OPEN.
                if self
                    .state
                    .compare_exchange(
                        STATE_HALF_OPEN,
                        STATE_OPEN,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    self.last_failure_time
                        .store(nanos_since_epoch(), Ordering::Release);
                    self.success_count.store(0, Ordering::Release);
                }
            }
            _ => {
                // Already OPEN; update failure time.
                self.last_failure_time
                    .store(nanos_since_epoch(), Ordering::Release);
            }
        }
    }

    /// Current failure count (for testing/monitoring).
    pub fn failure_count(&self) -> u32 {
        self.failure_count.load(Ordering::Relaxed)
    }

    /// Current success count (for testing/monitoring).
    pub fn success_count(&self) -> u32 {
        self.success_count.load(Ordering::Relaxed)
    }
}

fn nanos_since_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as u64
}

fn u8_to_state(v: u8) -> CircuitBreakerState {
    match v {
        STATE_CLOSED => CircuitBreakerState::Closed,
        STATE_OPEN => CircuitBreakerState::Open,
        STATE_HALF_OPEN => CircuitBreakerState::HalfOpen,
        _ => CircuitBreakerState::Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            open_duration: Duration::from_millis(100),
        }
    }

    #[test]
    fn test_initial_state_is_closed() {
        let cb = CircuitBreaker::new(default_config());
        assert_eq!(cb.state(), CircuitBreakerState::Closed);
        assert!(cb.allow_request());
    }

    #[test]
    fn test_closed_to_open_on_failures() {
        let cb = CircuitBreaker::new(default_config());
        assert_eq!(cb.state(), CircuitBreakerState::Closed);

        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitBreakerState::Closed);

        cb.record_failure(); // Threshold = 3
        assert_eq!(cb.state(), CircuitBreakerState::Open);
        assert!(!cb.allow_request());
    }

    #[test]
    fn test_success_resets_failure_count_in_closed() {
        let cb = CircuitBreaker::new(default_config());
        cb.record_failure();
        cb.record_failure();
        cb.record_success(); // Resets failure count
        cb.record_failure();
        cb.record_failure();
        // Should still be closed (only 2 consecutive failures)
        assert_eq!(cb.state(), CircuitBreakerState::Closed);
    }

    #[test]
    fn test_open_to_half_open_after_duration() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            open_duration: Duration::from_millis(50),
        };
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitBreakerState::Open);

        // Wait for open_duration to elapse.
        std::thread::sleep(Duration::from_millis(60));

        assert_eq!(cb.state(), CircuitBreakerState::HalfOpen);
    }

    #[test]
    fn test_half_open_to_closed_on_successes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            open_duration: Duration::from_millis(10),
        };
        let cb = CircuitBreaker::new(config);

        // Trip to OPEN.
        cb.record_failure();
        assert_eq!(cb.state(), CircuitBreakerState::Open);

        // Wait for transition to HALF_OPEN.
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cb.state(), CircuitBreakerState::HalfOpen);

        // Record successes to close.
        cb.record_success();
        // Still half-open (need 2).
        assert_eq!(cb.state(), CircuitBreakerState::HalfOpen);

        cb.record_success();
        assert_eq!(cb.state(), CircuitBreakerState::Closed);
    }

    #[test]
    fn test_half_open_to_open_on_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            open_duration: Duration::from_millis(10),
        };
        let cb = CircuitBreaker::new(config);

        // Trip to OPEN.
        cb.record_failure();
        assert_eq!(cb.state(), CircuitBreakerState::Open);

        // Wait for transition to HALF_OPEN.
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cb.state(), CircuitBreakerState::HalfOpen);

        // Any failure in HALF_OPEN goes back to OPEN.
        cb.record_failure();
        assert_eq!(cb.state(), CircuitBreakerState::Open);
    }

    #[test]
    fn test_half_open_limited_permits() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 5,
            open_duration: Duration::from_millis(10),
        };
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        std::thread::sleep(Duration::from_millis(20));

        assert_eq!(cb.state(), CircuitBreakerState::HalfOpen);

        // Should allow DEFAULT_HALF_OPEN_PERMITS (3) requests.
        assert!(cb.allow_request());
        assert!(cb.allow_request());
        assert!(cb.allow_request());
        // Fourth should be denied.
        assert!(!cb.allow_request());
    }

    #[test]
    fn test_full_cycle() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            open_duration: Duration::from_millis(10),
        };
        let cb = CircuitBreaker::new(config);

        // CLOSED
        assert_eq!(cb.state(), CircuitBreakerState::Closed);

        // -> OPEN
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitBreakerState::Open);

        // -> HALF_OPEN
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cb.state(), CircuitBreakerState::HalfOpen);

        // -> OPEN (failure in half-open)
        cb.record_failure();
        assert_eq!(cb.state(), CircuitBreakerState::Open);

        // -> HALF_OPEN again
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cb.state(), CircuitBreakerState::HalfOpen);

        // -> CLOSED (enough successes)
        cb.record_success();
        cb.record_success();
        assert_eq!(cb.state(), CircuitBreakerState::Closed);
    }
}
