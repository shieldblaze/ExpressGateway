//! Data plane agent that connects to the control plane.

use std::time::Duration;

use anyhow::Result;
use tracing;

/// Data plane agent that communicates with the control plane.
///
/// Handles registration, heartbeating, and configuration fetching.
/// The actual gRPC calls are stubbed until proto files are available.
pub struct DataPlaneAgent {
    /// Address of the control plane (e.g., "http://127.0.0.1:50051").
    control_plane_addr: String,
    /// Unique identifier for this data plane node.
    node_id: String,
    /// Session token received after successful registration.
    session_token: Option<String>,
    /// Interval between heartbeats.
    heartbeat_interval: Duration,
}

impl DataPlaneAgent {
    /// Create a new data plane agent.
    pub fn new(cp_addr: &str, node_id: &str) -> Self {
        Self {
            control_plane_addr: cp_addr.to_string(),
            node_id: node_id.to_string(),
            session_token: None,
            heartbeat_interval: Duration::from_secs(10),
        }
    }

    /// Create a new agent with a custom heartbeat interval.
    pub fn with_heartbeat_interval(cp_addr: &str, node_id: &str, interval: Duration) -> Self {
        Self {
            control_plane_addr: cp_addr.to_string(),
            node_id: node_id.to_string(),
            session_token: None,
            heartbeat_interval: interval,
        }
    }

    /// Get the control plane address.
    pub fn control_plane_addr(&self) -> &str {
        &self.control_plane_addr
    }

    /// Get the node ID.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Get the session token, if registered.
    pub fn session_token(&self) -> Option<&str> {
        self.session_token.as_deref()
    }

    /// Get the heartbeat interval.
    pub fn heartbeat_interval(&self) -> Duration {
        self.heartbeat_interval
    }

    /// Whether the agent has been registered (has a session token).
    pub fn is_registered(&self) -> bool {
        self.session_token.is_some()
    }

    /// Register this node with the control plane.
    ///
    /// Sends the auth_token to obtain a session_token.
    ///
    /// TODO: Implement actual gRPC call when proto files are set up.
    /// Currently stubbed to simulate successful registration.
    pub async fn register(&mut self, auth_token: &str) -> Result<()> {
        tracing::info!(
            node_id = %self.node_id,
            cp_addr = %self.control_plane_addr,
            "Registering with control plane (stub)"
        );

        // Stub: In a real implementation, this would make a gRPC call to
        // NodeRegistrationService.Register with the auth_token and receive
        // a session_token back.
        let _ = auth_token;

        // Simulate receiving a session token
        let session_token = uuid::Uuid::new_v4().to_string();
        self.session_token = Some(session_token);

        tracing::info!(
            node_id = %self.node_id,
            "Registration successful (stub)"
        );

        Ok(())
    }

    /// Send a heartbeat to the control plane.
    ///
    /// TODO: Implement actual gRPC call when proto files are set up.
    pub async fn heartbeat(&self) -> Result<()> {
        let session_token = self
            .session_token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("not registered"))?;

        tracing::debug!(
            node_id = %self.node_id,
            session_token = %session_token,
            "Sending heartbeat (stub)"
        );

        // Stub: In a real implementation, this would make a gRPC call to
        // NodeRegistrationService.Heartbeat.

        Ok(())
    }

    /// Fetch the current configuration from the control plane.
    ///
    /// TODO: Implement actual gRPC call when proto files are set up.
    pub async fn fetch_config(&self) -> Result<()> {
        let session_token = self
            .session_token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("not registered"))?;

        tracing::debug!(
            node_id = %self.node_id,
            session_token = %session_token,
            "Fetching config (stub)"
        );

        // Stub: In a real implementation, this would make a gRPC call to
        // ConfigDistributionService.FetchConfig.

        Ok(())
    }

    /// Clear the session token (e.g., after disconnection).
    pub fn clear_session(&mut self) {
        self.session_token = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_agent() {
        let agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1");
        assert_eq!(agent.control_plane_addr(), "http://127.0.0.1:50051");
        assert_eq!(agent.node_id(), "node-1");
        assert!(!agent.is_registered());
        assert!(agent.session_token().is_none());
        assert_eq!(agent.heartbeat_interval(), Duration::from_secs(10));
    }

    #[test]
    fn custom_heartbeat_interval() {
        let agent = DataPlaneAgent::with_heartbeat_interval(
            "http://127.0.0.1:50051",
            "node-1",
            Duration::from_secs(5),
        );
        assert_eq!(agent.heartbeat_interval(), Duration::from_secs(5));
    }

    #[tokio::test]
    async fn register_and_heartbeat() {
        let mut agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1");
        assert!(!agent.is_registered());

        agent.register("secret-token").await.unwrap();
        assert!(agent.is_registered());
        assert!(agent.session_token().is_some());

        // Heartbeat should succeed after registration
        agent.heartbeat().await.unwrap();
    }

    #[tokio::test]
    async fn heartbeat_without_registration_fails() {
        let agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1");
        let result = agent.heartbeat().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fetch_config_without_registration_fails() {
        let agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1");
        let result = agent.fetch_config().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fetch_config_after_registration() {
        let mut agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1");
        agent.register("secret").await.unwrap();
        agent.fetch_config().await.unwrap();
    }

    #[tokio::test]
    async fn clear_session() {
        let mut agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1");
        agent.register("secret").await.unwrap();
        assert!(agent.is_registered());

        agent.clear_session();
        assert!(!agent.is_registered());

        let result = agent.heartbeat().await;
        assert!(result.is_err());
    }
}
