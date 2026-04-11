//! Data plane agent that connects to the control plane.
//!
//! Features:
//! - Registration with pre-shared auth token
//! - Heartbeat at configurable interval
//! - Configuration sync from control plane
//! - Health status reporting
//! - Graceful disconnect on shutdown
//! - Reconnection with exponential backoff

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::watch;
use tracing;

use crate::error::AgentError;

/// Configuration for the agent's reconnection behavior.
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Initial backoff delay after a connection failure.
    pub initial_backoff: Duration,
    /// Maximum backoff delay.
    pub max_backoff: Duration,
    /// Multiplier for exponential backoff.
    pub backoff_multiplier: f64,
    /// Maximum number of reconnection attempts (0 = unlimited).
    pub max_retries: u32,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            max_retries: 0,
        }
    }
}

/// Current state of the agent's connection to the control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Not yet connected.
    Disconnected,
    /// Attempting to connect/register.
    Connecting,
    /// Successfully registered and heartbeating.
    Connected,
    /// Reconnecting after a failure.
    Reconnecting,
    /// Gracefully shutting down.
    ShuttingDown,
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => f.write_str("disconnected"),
            Self::Connecting => f.write_str("connecting"),
            Self::Connected => f.write_str("connected"),
            Self::Reconnecting => f.write_str("reconnecting"),
            Self::ShuttingDown => f.write_str("shutting_down"),
        }
    }
}

/// Data plane agent that communicates with the control plane.
///
/// Handles registration, heartbeating, configuration fetching, and
/// health status reporting. The actual gRPC calls are stubbed until
/// proto files are available -- the control flow and state machine
/// are fully functional.
pub struct DataPlaneAgent {
    /// Address of the control plane (e.g., "http://127.0.0.1:50051").
    control_plane_addr: String,
    /// Unique identifier for this data plane node.
    node_id: String,
    /// Pre-shared auth token for registration.
    auth_token: String,
    /// Session token received after successful registration.
    session_token: Option<String>,
    /// Interval between heartbeats.
    heartbeat_interval: Duration,
    /// Reconnection configuration.
    reconnect_config: ReconnectConfig,
    /// Last known config version from the control plane.
    last_config_version: Option<u64>,
    /// Current agent state.
    state: AgentState,
    /// Shutdown signal sender.
    shutdown_tx: watch::Sender<bool>,
    /// Shutdown signal receiver (cloneable).
    shutdown_rx: watch::Receiver<bool>,
}

impl DataPlaneAgent {
    /// Create a new data plane agent.
    pub fn new(cp_addr: &str, node_id: &str, auth_token: &str) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            control_plane_addr: cp_addr.to_string(),
            node_id: node_id.to_string(),
            auth_token: auth_token.to_string(),
            session_token: None,
            heartbeat_interval: Duration::from_secs(10),
            reconnect_config: ReconnectConfig::default(),
            last_config_version: None,
            state: AgentState::Disconnected,
            shutdown_tx,
            shutdown_rx,
        }
    }

    /// Create a new agent with custom settings.
    pub fn with_config(
        cp_addr: &str,
        node_id: &str,
        auth_token: &str,
        heartbeat_interval: Duration,
        reconnect_config: ReconnectConfig,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            control_plane_addr: cp_addr.to_string(),
            node_id: node_id.to_string(),
            auth_token: auth_token.to_string(),
            session_token: None,
            heartbeat_interval,
            reconnect_config,
            last_config_version: None,
            state: AgentState::Disconnected,
            shutdown_tx,
            shutdown_rx,
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

    /// Get the current agent state.
    pub fn state(&self) -> AgentState {
        self.state
    }

    /// Get the last known config version.
    pub fn last_config_version(&self) -> Option<u64> {
        self.last_config_version
    }

    /// Register this node with the control plane.
    ///
    /// Sends the auth_token to obtain a session_token.
    ///
    /// Stub: In a real implementation, this makes a gRPC call to
    /// `NodeRegistrationService.Register`.
    pub async fn register(&mut self) -> Result<(), AgentError> {
        self.state = AgentState::Connecting;

        tracing::info!(
            node_id = %self.node_id,
            cp_addr = %self.control_plane_addr,
            "registering with control plane"
        );

        // Stub: simulate a gRPC call.
        // In production this would be:
        //   let channel = tonic::transport::Channel::from_shared(self.control_plane_addr.clone())
        //       .map_err(|e| AgentError::RegistrationFailed(e.to_string()))?
        //       .connect().await
        //       .map_err(|e| AgentError::RegistrationFailed(e.to_string()))?;
        //   let mut client = NodeRegistrationServiceClient::new(channel);
        //   let response = client.register(RegisterNodeRequest {
        //       node_id: self.node_id.clone(),
        //       address: self.control_plane_addr.clone(),
        //       auth_token: self.auth_token.clone(),
        //   }).await?;
        //   self.session_token = Some(response.session_token);
        //   self.heartbeat_interval = Duration::from_secs(response.heartbeat_interval_s);

        // Ensure auth_token is available for the real gRPC call.
        let _auth_token = &self.auth_token;
        let session_token = uuid::Uuid::new_v4().to_string();
        self.session_token = Some(session_token);
        self.state = AgentState::Connected;

        tracing::info!(
            node_id = %self.node_id,
            "registration successful"
        );

        Ok(())
    }

    /// Send a heartbeat to the control plane.
    ///
    /// Stub: In a real implementation, this makes a gRPC call to
    /// `NodeRegistrationService.Heartbeat`.
    pub async fn heartbeat(&self) -> Result<(), AgentError> {
        let session_token = self.session_token.as_deref().ok_or(AgentError::NotRegistered {
            operation: "heartbeat",
        })?;

        tracing::debug!(
            node_id = %self.node_id,
            session_token = %session_token,
            "sending heartbeat"
        );

        // Stub: simulate a gRPC call.
        // In production: client.heartbeat(HeartbeatRequest { ... }).await?;

        Ok(())
    }

    /// Fetch the current configuration from the control plane.
    ///
    /// Returns `true` if the configuration was updated.
    ///
    /// Stub: In a real implementation, this makes a gRPC call to
    /// `ConfigDistributionService.FetchConfig`.
    pub async fn fetch_config(&mut self) -> Result<bool, AgentError> {
        let session_token = self.session_token.as_deref().ok_or(AgentError::NotRegistered {
            operation: "fetch_config",
        })?;

        tracing::debug!(
            node_id = %self.node_id,
            session_token = %session_token,
            last_version = ?self.last_config_version,
            "fetching config"
        );

        // Stub: simulate a gRPC call.
        // In production:
        //   let response = client.fetch_config(FetchConfigRequest {
        //       node_id: self.node_id.clone(),
        //       session_token: session_token.to_string(),
        //       last_known_version: self.last_config_version,
        //   }).await?;
        //   if response.changed {
        //       self.last_config_version = Some(response.version);
        //       // Apply the config...
        //       return Ok(true);
        //   }

        Ok(false)
    }

    /// Report health status for backends to the control plane.
    ///
    /// Stub: In a real implementation, this makes a gRPC call to
    /// `HealthAggregationService.ReportHealth`.
    pub async fn report_health(
        &self,
        backend_statuses: &HashMap<String, bool>,
    ) -> Result<(), AgentError> {
        let _session_token =
            self.session_token
                .as_deref()
                .ok_or(AgentError::NotRegistered {
                    operation: "report_health",
                })?;

        tracing::debug!(
            node_id = %self.node_id,
            backends = backend_statuses.len(),
            "reporting health status"
        );

        // Stub: simulate a gRPC call.

        Ok(())
    }

    /// Run the agent's main loop: register, then heartbeat + config sync.
    ///
    /// This method blocks until shutdown is signaled or an unrecoverable
    /// error occurs. On transient failures, it reconnects with exponential
    /// backoff.
    pub async fn run(&mut self) -> Result<(), AgentError> {
        // Initial registration with backoff.
        self.register_with_backoff().await?;

        let heartbeat_interval = self.heartbeat_interval;
        let mut heartbeat_timer = tokio::time::interval(heartbeat_interval);
        // The first tick completes immediately; skip it since we just registered.
        heartbeat_timer.tick().await;

        let mut shutdown_rx = self.shutdown_rx.clone();

        loop {
            tokio::select! {
                _ = heartbeat_timer.tick() => {
                    if let Err(e) = self.heartbeat().await {
                        tracing::warn!(error = %e, "heartbeat failed, attempting reconnect");
                        self.handle_connection_failure().await?;
                    }

                    // Piggyback config sync on heartbeat.
                    match self.fetch_config().await {
                        Ok(true) => {
                            tracing::info!(
                                version = ?self.last_config_version,
                                "configuration updated"
                            );
                        }
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, "config fetch failed");
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!(node_id = %self.node_id, "shutdown signal received");
                        self.graceful_disconnect().await;
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Signal the agent to shut down gracefully.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Register with exponential backoff on failure.
    async fn register_with_backoff(&mut self) -> Result<(), AgentError> {
        let mut backoff = self.reconnect_config.initial_backoff;
        let mut attempts = 0u32;

        loop {
            match self.register().await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    attempts += 1;
                    let max = self.reconnect_config.max_retries;
                    if max > 0 && attempts >= max {
                        tracing::error!(
                            attempts,
                            "max registration retries exceeded"
                        );
                        return Err(AgentError::MaxRetriesExceeded { attempts });
                    }

                    tracing::warn!(
                        error = %e,
                        attempt = attempts,
                        backoff_ms = backoff.as_millis(),
                        "registration failed, retrying"
                    );

                    // Check for shutdown during backoff.
                    let mut shutdown_rx = self.shutdown_rx.clone();
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                return Err(AgentError::Shutdown);
                            }
                        }
                    }

                    // Exponential backoff with cap.  Guard against NaN / negative
                    // / infinity from a misconfigured multiplier -- clamp to a
                    // sane range so we never panic in `Duration::from_secs_f64`.
                    let next = backoff.as_secs_f64() * self.reconnect_config.backoff_multiplier;
                    let capped = next
                        .min(self.reconnect_config.max_backoff.as_secs_f64())
                        .clamp(0.0, self.reconnect_config.max_backoff.as_secs_f64());
                    backoff = if capped.is_finite() {
                        Duration::from_secs_f64(capped)
                    } else {
                        self.reconnect_config.max_backoff
                    };
                }
            }
        }
    }

    /// Handle a connection failure by clearing state and reconnecting.
    async fn handle_connection_failure(&mut self) -> Result<(), AgentError> {
        self.state = AgentState::Reconnecting;
        self.session_token = None;

        tracing::info!(node_id = %self.node_id, "reconnecting to control plane");
        self.register_with_backoff().await
    }

    /// Gracefully disconnect from the control plane.
    async fn graceful_disconnect(&mut self) {
        self.state = AgentState::ShuttingDown;

        tracing::info!(
            node_id = %self.node_id,
            "gracefully disconnecting from control plane"
        );

        // Stub: In production, send a disconnect notification to the control plane.
        // client.control(NodeControlRequest { node_id, command: "disconnect" }).await;

        self.session_token = None;
        self.state = AgentState::Disconnected;

        tracing::info!(node_id = %self.node_id, "disconnected");
    }

    /// Clear the session token (e.g., after disconnection).
    pub fn clear_session(&mut self) {
        self.session_token = None;
        self.state = AgentState::Disconnected;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_agent() {
        let agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1", "secret");
        assert_eq!(agent.control_plane_addr(), "http://127.0.0.1:50051");
        assert_eq!(agent.node_id(), "node-1");
        assert!(!agent.is_registered());
        assert!(agent.session_token().is_none());
        assert_eq!(agent.heartbeat_interval(), Duration::from_secs(10));
        assert_eq!(agent.state(), AgentState::Disconnected);
        assert!(agent.last_config_version().is_none());
    }

    #[test]
    fn custom_config() {
        let agent = DataPlaneAgent::with_config(
            "http://127.0.0.1:50051",
            "node-1",
            "secret",
            Duration::from_secs(5),
            ReconnectConfig {
                initial_backoff: Duration::from_millis(500),
                max_backoff: Duration::from_secs(30),
                backoff_multiplier: 1.5,
                max_retries: 10,
            },
        );
        assert_eq!(agent.heartbeat_interval(), Duration::from_secs(5));
    }

    #[tokio::test]
    async fn register_and_heartbeat() {
        let mut agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1", "secret");
        assert!(!agent.is_registered());
        assert_eq!(agent.state(), AgentState::Disconnected);

        agent.register().await.unwrap();
        assert!(agent.is_registered());
        assert!(agent.session_token().is_some());
        assert_eq!(agent.state(), AgentState::Connected);

        // Heartbeat should succeed after registration.
        agent.heartbeat().await.unwrap();
    }

    #[tokio::test]
    async fn heartbeat_without_registration_fails() {
        let agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1", "secret");
        let result = agent.heartbeat().await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AgentError::NotRegistered { .. }));
    }

    #[tokio::test]
    async fn fetch_config_without_registration_fails() {
        let mut agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1", "secret");
        let result = agent.fetch_config().await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AgentError::NotRegistered { .. }));
    }

    #[tokio::test]
    async fn fetch_config_after_registration() {
        let mut agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1", "secret");
        agent.register().await.unwrap();
        let changed = agent.fetch_config().await.unwrap();
        assert!(!changed); // Stub returns false.
    }

    #[tokio::test]
    async fn report_health_after_registration() {
        let mut agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1", "secret");
        agent.register().await.unwrap();

        let mut statuses = HashMap::new();
        statuses.insert("backend-1".into(), true);
        statuses.insert("backend-2".into(), false);
        agent.report_health(&statuses).await.unwrap();
    }

    #[tokio::test]
    async fn report_health_without_registration_fails() {
        let agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1", "secret");
        let statuses = HashMap::new();
        let result = agent.report_health(&statuses).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn clear_session() {
        let mut agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1", "secret");
        agent.register().await.unwrap();
        assert!(agent.is_registered());
        assert_eq!(agent.state(), AgentState::Connected);

        agent.clear_session();
        assert!(!agent.is_registered());
        assert_eq!(agent.state(), AgentState::Disconnected);

        let result = agent.heartbeat().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn graceful_shutdown() {
        let mut agent = DataPlaneAgent::new("http://127.0.0.1:50051", "node-1", "secret");
        agent.register().await.unwrap();

        // Signal shutdown from another "thread".
        agent.shutdown();

        // Run should exit cleanly.
        let result = agent.run().await;
        assert!(result.is_ok());
        assert_eq!(agent.state(), AgentState::Disconnected);
        assert!(!agent.is_registered());
    }

    #[tokio::test]
    async fn run_loop_heartbeats() {
        let mut agent = DataPlaneAgent::with_config(
            "http://127.0.0.1:50051",
            "node-1",
            "secret",
            Duration::from_millis(50),
            ReconnectConfig::default(),
        );

        // Let it run for a bit, then shut down.
        let shutdown_tx = agent.shutdown_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = shutdown_tx.send(true);
        });

        let result = agent.run().await;
        assert!(result.is_ok());
        assert_eq!(agent.state(), AgentState::Disconnected);
    }

    #[test]
    fn default_reconnect_config() {
        let config = ReconnectConfig::default();
        assert_eq!(config.initial_backoff, Duration::from_secs(1));
        assert_eq!(config.max_backoff, Duration::from_secs(60));
        assert_eq!(config.backoff_multiplier, 2.0);
        assert_eq!(config.max_retries, 0);
    }
}
