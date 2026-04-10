//! Crate-specific error types for the connection pool module.

/// Connection pool error type.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("pool exhausted for backend {backend_id}: max {max} connections")]
    Exhausted { backend_id: String, max: usize },

    #[error("connection health check failed for backend {backend_id}")]
    HealthCheckFailed { backend_id: String },

    #[error("backend {backend_id} is draining")]
    Draining { backend_id: String },

    #[error("buffer pool exhausted: capacity {capacity}")]
    BufferExhausted { capacity: usize },
}

/// Crate-specific Result alias.
pub type Result<T> = std::result::Result<T, PoolError>;
