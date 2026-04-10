//! Listener trait for accepting incoming connections.

/// A listener that accepts incoming connections.
#[async_trait::async_trait]
pub trait Listener: Send + Sync {
    /// Start the listener (begin accepting connections).
    async fn start(&self) -> crate::error::Result<()>;

    /// Stop the listener (stop accepting new connections).
    async fn stop(&self) -> crate::error::Result<()>;

    /// Whether the listener is in drain mode.
    fn is_draining(&self) -> bool;

    /// Enter drain mode: stop accepting new connections,
    /// complete in-flight requests.
    fn start_draining(&self);
}
