//! Error types for ExpressGateway.

use std::net::SocketAddr;

/// Convenience alias for `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;

/// Top-level error type for ExpressGateway.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TLS error: {0}")]
    Tls(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("No healthy backend available")]
    NoHealthyBackend,

    #[error("Backend connection failed: {addr}: {reason}")]
    BackendConnectionFailed { addr: SocketAddr, reason: String },

    #[error("Backend timeout: {addr}")]
    BackendTimeout { addr: SocketAddr },

    #[error("Connection limit reached for node {node_id}: max {max}")]
    ConnectionLimitReached { node_id: String, max: u64 },

    #[error("Circuit breaker open for node {node_id}")]
    CircuitBreakerOpen { node_id: String },

    #[error("Rate limit exceeded for {remote}")]
    RateLimitExceeded { remote: String },

    #[error("Access denied for {remote}")]
    AccessDenied { remote: String },

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("HTTP error: status {status}: {message}")]
    Http { status: u16, message: String },

    #[error("Request body too large: {size} > {max}")]
    BodyTooLarge { size: u64, max: u64 },

    #[error("Request timeout")]
    RequestTimeout,

    #[error("URI too long: {length} > {max}")]
    UriTooLong { length: usize, max: usize },

    #[error("Header too large: {size} > {max}")]
    HeaderTooLarge { size: usize, max: usize },

    #[error("Path traversal detected: {path}")]
    PathTraversal { path: String },

    #[error("Invalid proxy protocol: {0}")]
    ProxyProtocol(String),

    #[error("gRPC error: code={code}, message={message}")]
    Grpc { code: u32, message: String },

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("Shutdown in progress")]
    ShuttingDown,

    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Cluster not found: {0}")]
    ClusterNotFound(String),

    #[error("Invalid state transition: {from} -> {to}")]
    InvalidStateTransition { from: String, to: String },

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Returns the appropriate HTTP status code for this error.
    pub fn http_status(&self) -> u16 {
        match self {
            Error::NoHealthyBackend | Error::ShuttingDown => 503,
            Error::BackendConnectionFailed { .. } => 502,
            Error::BackendTimeout { .. } | Error::RequestTimeout => 504,
            Error::BodyTooLarge { .. } => 413,
            Error::UriTooLong { .. } => 414,
            Error::HeaderTooLarge { .. } => 431,
            Error::RateLimitExceeded { .. } => 429,
            Error::AccessDenied { .. } => 403,
            Error::PathTraversal { .. } | Error::Protocol(_) => 400,
            Error::Http { status, .. } => *status,
            _ => 500,
        }
    }
}
