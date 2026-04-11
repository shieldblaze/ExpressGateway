//! UDP health checker -- validates connectivity by sending a PING datagram.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::{Duration, Instant};

use expressgateway_core::{HealthCheckResult, HealthChecker, HealthState};

/// Ephemeral bind addresses, constructed as consts to avoid runtime parsing.
const BIND_V4: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
const BIND_V6: SocketAddr =
    SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0));

/// UDP health checker that validates a backend by sending a "PING" datagram
/// and expecting a "PING" or "PONG" response within the configured timeout.
pub struct UdpHealthChecker {
    timeout: Duration,
}

impl UdpHealthChecker {
    /// Create a new UDP health checker with the given timeout.
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

#[async_trait::async_trait]
impl HealthChecker for UdpHealthChecker {
    async fn check(&self, addr: SocketAddr) -> HealthCheckResult {
        let start = Instant::now();

        let result = tokio::time::timeout(self.timeout, async {
            // Bind to an ephemeral port matching the target address family.
            let bind_addr = if addr.is_ipv4() { BIND_V4 } else { BIND_V6 };

            let socket = tokio::net::UdpSocket::bind(bind_addr).await?;
            socket.connect(addr).await?;
            socket.send(b"PING").await?;

            let mut buf = [0u8; 16];
            let n = socket.recv(&mut buf).await?;
            let response = &buf[..n];

            if response == b"PING" || response == b"PONG" {
                Ok(())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "unexpected response: {:?}",
                        String::from_utf8_lossy(response)
                    ),
                ))
            }
        })
        .await;

        match result {
            Ok(Ok(())) => HealthCheckResult {
                state: HealthState::Healthy,
                latency: start.elapsed(),
                message: None,
            },
            Ok(Err(e)) => HealthCheckResult {
                state: HealthState::Unhealthy,
                latency: start.elapsed(),
                message: Some(format!("UDP check failed: {e}")),
            },
            Err(_) => HealthCheckResult {
                state: HealthState::Timeout,
                latency: start.elapsed(),
                message: Some(format!("UDP check timed out after {:?}", self.timeout)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_udp_check_healthy_ping() {
        // Spawn a UDP echo server that responds with "PONG".
        let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();

        tokio::spawn(async move {
            let mut buf = [0u8; 16];
            if let Ok((n, peer)) = server.recv_from(&mut buf).await {
                if &buf[..n] == b"PING" {
                    let _ = server.send_to(b"PONG", peer).await;
                }
            }
        });

        let checker = UdpHealthChecker::new(Duration::from_secs(1));
        let result = checker.check(addr).await;

        assert_eq!(result.state, HealthState::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn test_udp_check_healthy_ping_echo() {
        // Spawn a UDP echo server that echoes back "PING".
        let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();

        tokio::spawn(async move {
            let mut buf = [0u8; 16];
            if let Ok((n, peer)) = server.recv_from(&mut buf).await {
                let _ = server.send_to(&buf[..n], peer).await;
            }
        });

        let checker = UdpHealthChecker::new(Duration::from_secs(1));
        let result = checker.check(addr).await;

        assert_eq!(result.state, HealthState::Healthy);
    }

    #[tokio::test]
    async fn test_udp_check_timeout() {
        // Bind but never respond.
        let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();

        // Keep the socket alive but don't respond.
        let _server = server;

        let checker = UdpHealthChecker::new(Duration::from_millis(100));
        let result = checker.check(addr).await;

        assert_eq!(result.state, HealthState::Timeout);
        assert!(result.message.is_some());
    }
}
