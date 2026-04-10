//! Connection trait and states.

use serde::{Deserialize, Serialize};

/// Connection states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConnectionState {
    /// Connection is being established.
    Connecting,
    /// Connection is active and usable.
    Active,
    /// Connection is idle (no in-flight requests).
    Idle,
    /// Connection is draining (finishing in-flight, no new requests).
    Draining,
    /// Connection is closed.
    Closed,
}

/// A connection to a backend.
#[async_trait::async_trait]
pub trait Connection: Send + Sync {
    /// Current state of this connection.
    fn state(&self) -> ConnectionState;

    /// Whether this connection can be used for new requests.
    fn is_usable(&self) -> bool;

    /// Close the connection.
    async fn close(&self) -> crate::error::Result<()>;

    /// Age of the connection since creation.
    fn age(&self) -> std::time::Duration;

    /// Time since last activity.
    fn idle_time(&self) -> std::time::Duration;
}
