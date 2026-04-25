//! Active and passive health checking for upstream backends.
//!
//! Provides `HealthStatus` for representing backend health and `HealthChecker`
//! for tracking health state transitions.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![allow(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Health status of an upstream backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Backend is healthy and accepting traffic.
    Healthy,
    /// Backend is unhealthy and should not receive traffic.
    Unhealthy,
    /// Health state has not yet been determined.
    Unknown,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => f.write_str("healthy"),
            Self::Unhealthy => f.write_str("unhealthy"),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

/// Errors from health checking.
#[derive(Debug, thiserror::Error)]
pub enum HealthError {
    /// Check timed out.
    #[error("health check timed out")]
    Timeout,

    /// Check failed with a message.
    #[error("health check failed: {0}")]
    CheckFailed(String),
}

/// Simple health checker that tracks consecutive successes and failures
/// against configurable thresholds.
#[derive(Debug)]
pub struct HealthChecker {
    status: HealthStatus,
    consecutive_successes: u32,
    consecutive_failures: u32,
    healthy_threshold: u32,
    unhealthy_threshold: u32,
}

impl HealthChecker {
    /// Create a new `HealthChecker` with the given thresholds.
    ///
    /// * `healthy_threshold` -- consecutive successes required to become healthy.
    ///   Clamped to a minimum of 1.
    /// * `unhealthy_threshold` -- consecutive failures required to become unhealthy.
    ///   Clamped to a minimum of 1.
    #[must_use]
    pub const fn new(healthy_threshold: u32, unhealthy_threshold: u32) -> Self {
        let ht = if healthy_threshold == 0 {
            1
        } else {
            healthy_threshold
        };
        let ut = if unhealthy_threshold == 0 {
            1
        } else {
            unhealthy_threshold
        };
        Self {
            status: HealthStatus::Unknown,
            consecutive_successes: 0,
            consecutive_failures: 0,
            healthy_threshold: ht,
            unhealthy_threshold: ut,
        }
    }

    /// Record a successful health check.
    pub fn record_success(&mut self) {
        self.consecutive_successes += 1;
        self.consecutive_failures = 0;
        if self.consecutive_successes >= self.healthy_threshold {
            self.status = HealthStatus::Healthy;
        }
    }

    /// Record a failed health check.
    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.consecutive_successes = 0;
        if self.consecutive_failures >= self.unhealthy_threshold {
            self.status = HealthStatus::Unhealthy;
        }
    }

    /// Current health status.
    #[must_use]
    pub const fn status(&self) -> HealthStatus {
        self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_unknown() {
        let checker = HealthChecker::new(3, 2);
        assert_eq!(checker.status(), HealthStatus::Unknown);
    }

    #[test]
    fn becomes_healthy_after_threshold() {
        let mut checker = HealthChecker::new(3, 2);
        checker.record_success();
        checker.record_success();
        assert_eq!(checker.status(), HealthStatus::Unknown);
        checker.record_success();
        assert_eq!(checker.status(), HealthStatus::Healthy);
    }

    #[test]
    fn becomes_unhealthy_after_threshold() {
        let mut checker = HealthChecker::new(3, 2);
        checker.record_failure();
        assert_eq!(checker.status(), HealthStatus::Unknown);
        checker.record_failure();
        assert_eq!(checker.status(), HealthStatus::Unhealthy);
    }

    #[test]
    fn success_resets_failure_count() {
        let mut checker = HealthChecker::new(3, 2);
        checker.record_failure();
        checker.record_success();
        checker.record_failure();
        // Only one consecutive failure, not two
        assert_ne!(checker.status(), HealthStatus::Unhealthy);
    }

    #[test]
    fn display_status() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "unhealthy");
        assert_eq!(HealthStatus::Unknown.to_string(), "unknown");
    }

    #[test]
    fn zero_threshold_clamped_to_one() {
        // A zero healthy_threshold should be clamped to 1: one success = healthy.
        let mut checker = HealthChecker::new(0, 0);
        assert_eq!(checker.status(), HealthStatus::Unknown);

        checker.record_success();
        assert_eq!(checker.status(), HealthStatus::Healthy);

        // A zero unhealthy_threshold should be clamped to 1: one failure = unhealthy.
        let mut checker2 = HealthChecker::new(0, 0);
        checker2.record_failure();
        assert_eq!(checker2.status(), HealthStatus::Unhealthy);
    }
}
