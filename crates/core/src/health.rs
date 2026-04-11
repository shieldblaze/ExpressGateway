//! Health checking types and traits.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Health assessment for a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Health {
    /// >= 95% successful checks.
    Good,
    /// 75-95% successful checks.
    Medium,
    /// < 75% successful checks.
    Bad,
    /// No samples yet.
    Unknown,
}

/// Detailed health state with success/failure tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    /// Health check succeeded.
    Healthy,
    /// Health check failed.
    Unhealthy,
    /// Health check timed out.
    Timeout,
}

/// Result of a single health check.
#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    pub state: HealthState,
    pub latency: Duration,
    pub message: Option<String>,
}

/// Configuration for health checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    /// Check interval.
    #[serde(with = "duration_secs")]
    pub interval: Duration,
    /// Per-check timeout.
    #[serde(with = "duration_millis")]
    pub timeout: Duration,
    /// Consecutive successes required to mark ONLINE.
    pub rise: u32,
    /// Consecutive failures required to mark OFFLINE.
    pub fall: u32,
    /// Sample window size.
    pub samples: u32,
}

impl HealthCheckConfig {
    /// Validate that the configuration is internally consistent.
    ///
    /// Returns an error string describing the first inconsistency found:
    /// - `timeout` must be less than `interval`
    /// - `rise` and `fall` must be > 0
    /// - `samples` must be >= max(rise, fall)
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.timeout >= self.interval {
            return Err(format!(
                "timeout ({:?}) must be less than interval ({:?})",
                self.timeout, self.interval
            ));
        }
        if self.rise == 0 {
            return Err("rise must be > 0".into());
        }
        if self.fall == 0 {
            return Err("fall must be > 0".into());
        }
        if self.samples < self.rise.max(self.fall) {
            return Err(format!(
                "samples ({}) must be >= max(rise, fall) ({})",
                self.samples,
                self.rise.max(self.fall)
            ));
        }
        Ok(())
    }
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(10),
            timeout: Duration::from_millis(5000),
            rise: 2,
            fall: 3,
            samples: 100,
        }
    }
}

/// Health checker for backend nodes.
#[async_trait::async_trait]
pub trait HealthChecker: Send + Sync {
    /// Perform a health check on the given node.
    async fn check(&self, addr: std::net::SocketAddr) -> HealthCheckResult;
}

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CircuitBreakerState {
    /// Normal operation, requests flow through.
    Closed,
    /// Failures exceeded threshold, requests are blocked.
    Open,
    /// Testing if the backend has recovered.
    HalfOpen,
}

/// Circuit breaker configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures to trip to OPEN.
    pub failure_threshold: u32,
    /// Number of consecutive successes to close from HALF_OPEN.
    pub success_threshold: u32,
    /// Duration to stay in OPEN before transitioning to HALF_OPEN.
    #[serde(with = "duration_secs")]
    pub open_duration: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 3,
            open_duration: Duration::from_secs(30),
        }
    }
}

mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

mod duration_millis {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(duration.as_millis() as u64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_check_config_defaults() {
        let config = HealthCheckConfig::default();
        assert_eq!(config.interval, Duration::from_secs(10));
        assert_eq!(config.timeout, Duration::from_millis(5000));
        assert_eq!(config.rise, 2);
        assert_eq!(config.fall, 3);
        assert_eq!(config.samples, 100);
    }

    #[test]
    fn test_circuit_breaker_config_defaults() {
        let config = CircuitBreakerConfig::default();
        assert_eq!(config.failure_threshold, 5);
        assert_eq!(config.success_threshold, 3);
        assert_eq!(config.open_duration, Duration::from_secs(30));
    }
}
