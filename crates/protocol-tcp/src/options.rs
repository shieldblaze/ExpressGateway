//! TCP proxy configuration.

use std::time::Duration;

use expressgateway_core::WaterMarks;

/// Configuration for the TCP proxy handler.
#[derive(Debug, Clone)]
pub struct TcpProxyConfig {
    /// Timeout for establishing a connection to the backend.
    pub connect_timeout: Duration,
    /// Idle timeout: close the connection if no data is transferred for this long.
    pub idle_timeout: Duration,
    /// Drain timeout: maximum time to wait for in-flight connections during graceful shutdown.
    pub drain_timeout: Duration,
    /// Maximum number of concurrent connections.
    pub max_connections: usize,
    /// Write buffer water marks for backpressure.
    pub backpressure: WaterMarks,
}

impl Default for TcpProxyConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(300),
            drain_timeout: Duration::from_secs(30),
            max_connections: 10_000,
            backpressure: WaterMarks::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TcpProxyConfig::default();
        assert_eq!(config.connect_timeout, Duration::from_secs(5));
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
        assert_eq!(config.drain_timeout, Duration::from_secs(30));
        assert_eq!(config.max_connections, 10_000);
        assert_eq!(config.backpressure.high, 65_536);
        assert_eq!(config.backpressure.low, 32_768);
    }
}
