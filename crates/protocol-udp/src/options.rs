//! UDP proxy configuration.

use std::time::Duration;

/// Configuration for the UDP proxy handler.
#[derive(Debug, Clone)]
pub struct UdpProxyConfig {
    /// Session timeout: expire sessions with no activity for this long.
    pub session_timeout: Duration,
    /// Maximum number of concurrent sessions.
    pub max_sessions: usize,
    /// Optional rate limit in packets-per-second per source IP.
    /// `None` means no rate limiting.
    pub rate_limit_pps: Option<u64>,
    /// Receive buffer size for the frontend socket, in bytes.
    /// Controls kernel-level backpressure.
    pub recv_buf_size: Option<usize>,
}

impl Default for UdpProxyConfig {
    fn default() -> Self {
        Self {
            session_timeout: Duration::from_secs(30),
            max_sessions: 100_000,
            rate_limit_pps: None,
            recv_buf_size: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = UdpProxyConfig::default();
        assert_eq!(config.session_timeout, Duration::from_secs(30));
        assert_eq!(config.max_sessions, 100_000);
        assert!(config.rate_limit_pps.is_none());
        assert!(config.recv_buf_size.is_none());
    }
}
