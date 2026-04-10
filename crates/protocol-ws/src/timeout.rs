//! WebSocket timeout management.
//!
//! Three timeout types per the requirements:
//! - **Idle timeout**: close with 1001 if no frames exchanged within the duration.
//! - **Handshake timeout**: abort if the upgrade handshake doesn't complete in time.
//! - **Close handshake timeout**: force-close if the close frame exchange doesn't
//!   complete in time (prevents hanging on unresponsive peers).

use std::time::Duration;

use crate::frames::CLOSE_CODE_GOING_AWAY;

/// Default idle timeout: 300 seconds (5 minutes).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Default handshake timeout: 10 seconds.
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Default close handshake timeout: 5 seconds.
pub const DEFAULT_CLOSE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

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
    #[inline]
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
    #[inline]
    pub fn close_code() -> u16 {
        CLOSE_CODE_GOING_AWAY
    }

    /// The close reason string.
    #[inline]
    pub fn close_reason() -> &'static str {
        "idle timeout"
    }
}

impl Default for IdleTimeout {
    fn default() -> Self {
        Self::new(DEFAULT_IDLE_TIMEOUT)
    }
}

/// Handshake timeout configuration.
///
/// Enforced during the HTTP upgrade or Extended CONNECT exchange. If the
/// handshake doesn't complete within this duration, the connection is dropped.
#[derive(Debug, Clone, Copy)]
pub struct HandshakeTimeout {
    duration: Duration,
}

impl HandshakeTimeout {
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }

    #[inline]
    pub fn duration(&self) -> Duration {
        self.duration
    }
}

impl Default for HandshakeTimeout {
    fn default() -> Self {
        Self::new(DEFAULT_HANDSHAKE_TIMEOUT)
    }
}

/// Close handshake timeout configuration.
///
/// After sending a Close frame, the proxy waits for the peer's Close response.
/// If the response doesn't arrive within this duration, the connection is
/// force-closed to prevent resource leaks from unresponsive peers.
#[derive(Debug, Clone, Copy)]
pub struct CloseHandshakeTimeout {
    duration: Duration,
}

impl CloseHandshakeTimeout {
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }

    #[inline]
    pub fn duration(&self) -> Duration {
        self.duration
    }
}

impl Default for CloseHandshakeTimeout {
    fn default() -> Self {
        Self::new(DEFAULT_CLOSE_HANDSHAKE_TIMEOUT)
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

    #[test]
    fn handshake_timeout_default() {
        let t = HandshakeTimeout::default();
        assert_eq!(t.duration(), Duration::from_secs(10));
    }

    #[test]
    fn close_handshake_timeout_default() {
        let t = CloseHandshakeTimeout::default();
        assert_eq!(t.duration(), Duration::from_secs(5));
    }
}
