//! HTTP health checker -- validates a backend via raw HTTP request over TCP.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use expressgateway_core::{HealthCheckResult, HealthChecker, HealthState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// HTTP health checker that sends a configurable HTTP request over a raw
/// TCP socket and validates the response status code.
///
/// No external HTTP client dependency (e.g., hyper) is used.
pub struct HttpHealthChecker {
    method: String,
    path: String,
    expected_status: Vec<u16>,
    timeout: Duration,
}

impl HttpHealthChecker {
    /// Create a new HTTP health checker.
    pub fn new(method: String, path: String, expected_status: Vec<u16>, timeout: Duration) -> Self {
        Self {
            method,
            path,
            expected_status,
            timeout,
        }
    }

    /// Create a default HTTP health checker that sends `GET /` and expects 200.
    pub fn default_get(timeout: Duration) -> Self {
        Self::new("GET".into(), "/".into(), vec![200], timeout)
    }
}

#[async_trait::async_trait]
impl HealthChecker for HttpHealthChecker {
    async fn check(&self, addr: SocketAddr) -> HealthCheckResult {
        let start = Instant::now();

        let result = tokio::time::timeout(self.timeout, async {
            let mut stream = tokio::net::TcpStream::connect(addr).await?;

            // Build minimal HTTP/1.1 request.
            let request = format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                self.method, self.path, addr
            );
            stream.write_all(request.as_bytes()).await?;

            // Read response (only need the status line).
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "empty response",
                ));
            }

            let response = String::from_utf8_lossy(&buf[..n]);
            parse_status_code(&response)
        })
        .await;

        match result {
            Ok(Ok(status)) => {
                if self.expected_status.contains(&status) {
                    HealthCheckResult {
                        state: HealthState::Healthy,
                        latency: start.elapsed(),
                        message: None,
                    }
                } else {
                    HealthCheckResult {
                        state: HealthState::Unhealthy,
                        latency: start.elapsed(),
                        message: Some(format!(
                            "unexpected status {status}, expected one of {:?}",
                            self.expected_status
                        )),
                    }
                }
            }
            Ok(Err(e)) => HealthCheckResult {
                state: HealthState::Unhealthy,
                latency: start.elapsed(),
                message: Some(format!("HTTP check failed: {e}")),
            },
            Err(_) => HealthCheckResult {
                state: HealthState::Timeout,
                latency: start.elapsed(),
                message: Some(format!("HTTP check timed out after {:?}", self.timeout)),
            },
        }
    }
}

/// Parse the HTTP status code from the first line of an HTTP response.
fn parse_status_code(response: &str) -> std::io::Result<u16> {
    // Expected format: "HTTP/1.1 200 OK\r\n..."
    let status_line = response
        .lines()
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no status line"))?;

    let parts: Vec<&str> = status_line.splitn(3, ' ').collect();
    if parts.len() < 2 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("malformed status line: {status_line}"),
        ));
    }

    parts[1].parse::<u16>().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid status code: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_code_200() {
        let response = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(parse_status_code(response).unwrap(), 200);
    }

    #[test]
    fn test_parse_status_code_404() {
        let response = "HTTP/1.1 404 Not Found\r\n\r\n";
        assert_eq!(parse_status_code(response).unwrap(), 404);
    }

    #[test]
    fn test_parse_status_code_malformed() {
        let response = "GARBAGE";
        assert!(parse_status_code(response).is_err());
    }

    #[tokio::test]
    async fn test_http_check_healthy() {
        // Spawn a minimal HTTP server.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let response = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        let checker = HttpHealthChecker::default_get(Duration::from_secs(1));
        let result = checker.check(addr).await;

        assert_eq!(result.state, HealthState::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn test_http_check_unexpected_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let response = "HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        let checker = HttpHealthChecker::default_get(Duration::from_secs(1));
        let result = checker.check(addr).await;

        assert_eq!(result.state, HealthState::Unhealthy);
        assert!(result.message.unwrap().contains("503"));
    }

    #[tokio::test]
    async fn test_http_check_multiple_expected_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let response = "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        let checker = HttpHealthChecker::new(
            "GET".into(),
            "/health".into(),
            vec![200, 204],
            Duration::from_secs(1),
        );
        let result = checker.check(addr).await;

        assert_eq!(result.state, HealthState::Healthy);
    }
}
