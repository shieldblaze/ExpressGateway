//! Control plane client for agent-to-controlplane communication.
//!
//! Provides the `CpClient` struct for connecting to and exchanging
//! configuration with the control plane.
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
#![warn(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Errors from the control plane client.
#[derive(Debug, thiserror::Error)]
pub enum CpClientError {
    /// Connection failed.
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    /// Endpoint not set.
    #[error("endpoint not configured")]
    NoEndpoint,
}

/// Control plane client.
///
/// Manages the connection to the control plane server and provides methods
/// for config synchronization.
#[derive(Debug)]
pub struct CpClient {
    endpoint: Option<String>,
    connected: bool,
}

impl CpClient {
    /// Create a new disconnected client.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            endpoint: None,
            connected: false,
        }
    }

    /// Set the control plane endpoint.
    pub fn set_endpoint(&mut self, endpoint: impl Into<String>) {
        self.endpoint = Some(endpoint.into());
        self.connected = false;
    }

    /// Whether the client has a configured endpoint.
    #[must_use]
    pub const fn has_endpoint(&self) -> bool {
        self.endpoint.is_some()
    }

    /// Whether the client is currently connected.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.connected
    }

    /// Simulate a connection attempt.
    ///
    /// # Errors
    ///
    /// Returns `CpClientError::NoEndpoint` if no endpoint is configured.
    pub fn connect(&mut self) -> Result<(), CpClientError> {
        if self.endpoint.is_none() {
            return Err(CpClientError::NoEndpoint);
        }
        self.connected = true;
        Ok(())
    }
}

impl Default for CpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_client_disconnected() {
        let client = CpClient::new();
        assert!(!client.is_connected());
        assert!(!client.has_endpoint());
    }

    #[test]
    fn connect_without_endpoint_errors() {
        let mut client = CpClient::new();
        assert!(client.connect().is_err());
    }

    #[test]
    fn connect_with_endpoint_ok() {
        let mut client = CpClient::new();
        client.set_endpoint("http://localhost:9090");
        assert!(client.has_endpoint());
        client.connect().unwrap();
        assert!(client.is_connected());
    }

    #[test]
    fn default_is_new() {
        let client = CpClient::default();
        assert!(!client.is_connected());
    }
}
