//! WebSocket idle timeout.
//!
//! If no frames are exchanged within the configured duration, the proxy sends
//! a Close frame with code 1001 (Going Away).

use std::time::Duration;

use crate::frames::CLOSE_CODE_GOING_AWAY;

/// Default idle timeout: 300 seconds (5 minutes).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Idle timeout configuration and state.
#[derive(Debug, Clone)]
pub struct IdleTimeout {
    /// The configured timeout duration.
    duration: Duration,
    /// Timestamp of the last activity.
    last_activity: std::time::Instant,
}

impl IdleTimeout {
    /// Create a new idle timeout with the given duration.
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            last_activity: std::time::Instant::now(),
        }
    }

    /// Record that activity occurred (frame sent or received).
    pub fn record_activity(&mut self) {
        self.last_activity = std::time::Instant::now();
    }

    /// Returns the configured timeout duration.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Returns the time remaining before the idle timeout triggers.
    ///
    /// Returns `Duration::ZERO` if the timeout has already elapsed.
    pub fn remaining(&self) -> Duration {
        let elapsed = self.last_activity.elapsed();
        self.duration.saturating_sub(elapsed)
    }

    /// Returns `true` if the idle timeout has elapsed.
    pub fn is_expired(&self) -> bool {
        self.last_activity.elapsed() >= self.duration
    }

    /// The close code to use when the timeout fires.
    pub fn close_code() -> u16 {
        CLOSE_CODE_GOING_AWAY
    }

    /// The close reason string.
    pub fn close_reason() -> &'static str {
        "idle timeout"
    }
}

impl Default for IdleTimeout {
    fn default() -> Self {
        Self::new(DEFAULT_IDLE_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_timeout_is_300s() {
        let timeout = IdleTimeout::default();
        assert_eq!(timeout.duration(), Duration::from_secs(300));
    }

    #[test]
    fn custom_timeout() {
        let timeout = IdleTimeout::new(Duration::from_secs(60));
        assert_eq!(timeout.duration(), Duration::from_secs(60));
    }

    #[test]
    fn not_expired_immediately() {
        let timeout = IdleTimeout::new(Duration::from_secs(10));
        assert!(!timeout.is_expired());
    }

    #[test]
    fn expired_after_duration() {
        let mut timeout = IdleTimeout::new(Duration::from_millis(1));
        // Force the last_activity to be in the past.
        timeout.last_activity = std::time::Instant::now() - Duration::from_millis(10);
        assert!(timeout.is_expired());
    }

    #[test]
    fn record_activity_resets_timer() {
        let mut timeout = IdleTimeout::new(Duration::from_millis(1));
        timeout.last_activity = std::time::Instant::now() - Duration::from_millis(10);
        assert!(timeout.is_expired());
        timeout.record_activity();
        assert!(!timeout.is_expired());
    }

    #[test]
    fn remaining_decreases() {
        let timeout = IdleTimeout::new(Duration::from_secs(300));
        let remaining = timeout.remaining();
        // Should be close to 300s (allow small delta for test execution time).
        assert!(remaining <= Duration::from_secs(300));
        assert!(remaining > Duration::from_secs(299));
    }

    #[test]
    fn close_code_is_1001() {
        assert_eq!(IdleTimeout::close_code(), 1001);
    }

    #[test]
    fn close_reason_set() {
        assert_eq!(IdleTimeout::close_reason(), "idle timeout");
    }
}
