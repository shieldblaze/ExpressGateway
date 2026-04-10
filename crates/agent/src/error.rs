//! Agent error types.

/// Errors from the data plane agent.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("not registered: must register before {operation}")]
    NotRegistered { operation: &'static str },

    #[error("registration failed: {0}")]
    RegistrationFailed(String),

    #[error("heartbeat failed: {0}")]
    HeartbeatFailed(String),

    #[error("config fetch failed: {0}")]
    ConfigFetchFailed(String),

    #[error("health report failed: {0}")]
    HealthReportFailed(String),

    #[error("connection lost: {0}")]
    ConnectionLost(String),

    #[error("shutdown requested")]
    Shutdown,

    #[error("max retries exceeded after {attempts} attempts")]
    MaxRetriesExceeded { attempts: u32 },
}
