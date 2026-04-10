//! TCP health checker -- validates connectivity by attempting a TCP connection.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use expressgateway_core::{HealthCheckResult, HealthChecker, HealthState};

/// TCP health checker that validates a backend is reachable by establishing
/// a TCP connection within the configured timeout.
pub struct TcpHealthChecker {
    timeout: Duration,
}

impl TcpHealthChecker {
    /// Create a new TCP health checker with the given timeout.
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

#[async_trait::async_trait]
impl HealthChecker for TcpHealthChecker {
    async fn check(&self, addr: SocketAddr) -> HealthCheckResult {
        let start = Instant::now();

        match tokio::time::timeout(self.timeout, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(_stream)) => HealthCheckResult {
                state: HealthState::Healthy,
                latency: start.elapsed(),
                message: None,
            },
            Ok(Err(e)) => HealthCheckResult {
                state: HealthState::Unhealthy,
                latency: start.elapsed(),
                message: Some(format!("TCP connect failed: {e}")),
            },
            Err(_) => HealthCheckResult {
                state: HealthState::Timeout,
                latency: start.elapsed(),
                message: Some(format!("TCP connect timed out after {:?}", self.timeout)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_tcp_check_healthy() {
        // Bind a listener on an available port.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let checker = TcpHealthChecker::new(Duration::from_secs(1));
        let result = checker.check(addr).await;

        assert_eq!(result.state, HealthState::Healthy);
        assert!(result.message.is_none());
        assert!(result.latency < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn test_tcp_check_refused() {
        // Use a port that is not listening.
        // Port 1 is almost certainly not listening and should refuse quickly.
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let checker = TcpHealthChecker::new(Duration::from_secs(1));
        let result = checker.check(addr).await;

        assert!(
            result.state == HealthState::Unhealthy || result.state == HealthState::Timeout,
            "Expected Unhealthy or Timeout, got {:?}",
            result.state
        );
    }

    #[tokio::test]
    async fn test_tcp_check_timeout() {
        // Use a non-routable address to trigger timeout.
        let addr: SocketAddr = "192.0.2.1:80".parse().unwrap();
        let checker = TcpHealthChecker::new(Duration::from_millis(100));
        let result = checker.check(addr).await;

        assert_eq!(result.state, HealthState::Timeout);
        assert!(result.message.is_some());
    }
}
