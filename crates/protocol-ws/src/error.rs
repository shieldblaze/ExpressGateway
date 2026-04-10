//! WebSocket crate error types.
//!
//! Every fallible operation in this crate returns one of these typed errors
//! rather than stringly-typed `anyhow` or raw panics.

/// Errors from WebSocket protocol handling.
#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error("client connection error: {0}")]
    Client(#[source] tokio_tungstenite::tungstenite::Error),

    #[error("backend connection error: {0}")]
    Backend(#[source] tokio_tungstenite::tungstenite::Error),

    #[error("frame too large: {size} > {max}")]
    FrameTooLarge { size: usize, max: usize },

    #[error("idle timeout after {0:?}")]
    IdleTimeout(std::time::Duration),

    #[error("handshake timeout after {0:?}")]
    HandshakeTimeout(std::time::Duration),

    #[error("close handshake timeout after {0:?}")]
    CloseHandshakeTimeout(std::time::Duration),

    #[error("invalid upgrade request: {0}")]
    InvalidUpgrade(String),

    #[error("origin rejected: {origin}")]
    OriginRejected { origin: String },

    #[error("subprotocol mismatch: offered {offered}, got {got}")]
    SubprotocolMismatch { offered: String, got: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl WsError {
    /// Returns the appropriate WebSocket close code for this error.
    ///
    /// Per RFC 6455 section 7.4.1.
    pub fn close_code(&self) -> u16 {
        match self {
            Self::FrameTooLarge { .. } => 1009,     // Message Too Big
            Self::IdleTimeout(_) => 1001,            // Going Away
            Self::HandshakeTimeout(_) => 1001,       // Going Away
            Self::CloseHandshakeTimeout(_) => 1001,  // Going Away
            Self::InvalidUpgrade(_) => 1002,         // Protocol Error
            Self::OriginRejected { .. } => 1008,     // Policy Violation
            Self::SubprotocolMismatch { .. } => 1002, // Protocol Error
            _ => 1011,                                // Unexpected Condition
        }
    }
}
