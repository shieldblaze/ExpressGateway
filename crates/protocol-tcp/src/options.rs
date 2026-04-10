//! TCP proxy configuration.
//!
//! All socket-level tuning knobs for TCP proxying: timeouts, socket options,
//! backpressure water marks, and connection limits.

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
    /// TCP socket options applied to both client and backend sockets.
    pub socket_options: TcpSocketOptions,
}

/// TCP socket options.
#[derive(Debug, Clone)]
pub struct TcpSocketOptions {
    /// Set `TCP_NODELAY` (disable Nagle's algorithm).
    pub nodelay: bool,
    /// Set `TCP_QUICKACK` (Linux only: disable delayed ACKs).
    pub quickack: bool,
    /// Enable TCP keepalive with these parameters, or `None` to use OS defaults.
    pub keepalive: Option<TcpKeepalive>,
    /// Set `TCP_FASTOPEN` queue length, or `None` to disable.
    pub fastopen_qlen: Option<u32>,
}

/// TCP keepalive parameters.
#[derive(Debug, Clone)]
pub struct TcpKeepalive {
    /// Time before the first keepalive probe (`TCP_KEEPIDLE`).
    pub idle: Duration,
    /// Interval between keepalive probes (`TCP_KEEPINTVL`).
    pub interval: Duration,
    /// Number of unacknowledged probes before considering the connection dead
    /// (`TCP_KEEPCNT`).
    pub count: u32,
}

impl Default for TcpProxyConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(300),
            drain_timeout: Duration::from_secs(30),
            max_connections: 10_000,
            backpressure: WaterMarks::default(),
            socket_options: TcpSocketOptions::default(),
        }
    }
}

impl Default for TcpSocketOptions {
    fn default() -> Self {
        Self {
            nodelay: true,
            quickack: false,
            keepalive: Some(TcpKeepalive {
                idle: Duration::from_secs(60),
                interval: Duration::from_secs(10),
                count: 3,
            }),
            fastopen_qlen: None,
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
        assert!(config.socket_options.nodelay);
        assert!(config.socket_options.keepalive.is_some());
    }

    #[test]
    fn test_default_socket_options() {
        let opts = TcpSocketOptions::default();
        assert!(opts.nodelay);
        assert!(!opts.quickack);
        assert!(opts.keepalive.is_some());
        let ka = opts.keepalive.unwrap();
        assert_eq!(ka.idle, Duration::from_secs(60));
        assert_eq!(ka.interval, Duration::from_secs(10));
        assert_eq!(ka.count, 3);
        assert!(opts.fastopen_qlen.is_none());
    }
}
