//! Error types for ExpressGateway.
//!
//! All public error types use `thiserror` for zero-boilerplate `Display` and
//! `From` implementations.  The top-level [`Error`] enum is the canonical
//! error type returned across crate boundaries.

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

    #[error("configuration error: {0}")]
    Config(String),

    #[error("no healthy backend available")]
    NoHealthyBackend,

    #[error("backend connection failed: {addr}: {reason}")]
    BackendConnectionFailed { addr: SocketAddr, reason: String },

    #[error("backend timeout: {addr}")]
    BackendTimeout { addr: SocketAddr },

    #[error("connection limit reached for node {node_id}: max {max}")]
    ConnectionLimitReached { node_id: String, max: u64 },

    #[error("circuit breaker open for node {node_id}")]
    CircuitBreakerOpen { node_id: String },

    #[error("rate limit exceeded for {remote}")]
    RateLimitExceeded { remote: String },

    #[error("access denied for {remote}")]
    AccessDenied { remote: String },

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("HTTP error: status {status}: {message}")]
    Http { status: u16, message: String },

    #[error("request body too large: {size} > {max}")]
    BodyTooLarge { size: u64, max: u64 },

    #[error("request timeout")]
    RequestTimeout,

    #[error("URI too long: {length} > {max}")]
    UriTooLong { length: usize, max: usize },

    #[error("header too large: {size} > {max}")]
    HeaderTooLarge { size: usize, max: usize },

    #[error("path traversal detected: {path}")]
    PathTraversal { path: String },

    #[error("invalid proxy protocol: {0}")]
    ProxyProtocol(String),

    #[error("gRPC error: code={code}, message={message}")]
    Grpc { code: u32, message: String },

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("shutdown in progress")]
    ShuttingDown,

    #[error("node not found: {0}")]
    NodeNotFound(String),

    #[error("cluster not found: {0}")]
    ClusterNotFound(String),

    #[error("invalid state transition: {from} -> {to}")]
    InvalidStateTransition { from: String, to: String },

    #[error("runtime error: {0}")]
    Runtime(String),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Returns the appropriate HTTP status code for this error.
    #[inline]
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

    /// Whether this error indicates the backend is unavailable (5xx).
    #[inline]
    pub fn is_backend_error(&self) -> bool {
        matches!(
            self,
            Error::NoHealthyBackend
                | Error::BackendConnectionFailed { .. }
                | Error::BackendTimeout { .. }
                | Error::CircuitBreakerOpen { .. }
                | Error::ShuttingDown
        )
    }

    /// Whether this error indicates a client-side problem (4xx).
    #[inline]
    pub fn is_client_error(&self) -> bool {
        let status = self.http_status();
        (400..500).contains(&status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_codes() {
        assert_eq!(Error::NoHealthyBackend.http_status(), 503);
        assert_eq!(Error::RequestTimeout.http_status(), 504);
        assert_eq!(
            Error::BodyTooLarge {
                size: 100,
                max: 50
            }
            .http_status(),
            413
        );
        assert_eq!(
            Error::RateLimitExceeded {
                remote: "1.2.3.4".into()
            }
            .http_status(),
            429
        );
        assert_eq!(Error::Other("x".into()).http_status(), 500);
    }

    #[test]
    fn backend_error_classification() {
        assert!(Error::NoHealthyBackend.is_backend_error());
        assert!(Error::ShuttingDown.is_backend_error());
        assert!(!Error::RequestTimeout.is_backend_error());
    }

    #[test]
    fn client_error_classification() {
        assert!(Error::RateLimitExceeded {
            remote: "x".into()
        }
        .is_client_error());
        assert!(!Error::NoHealthyBackend.is_client_error());
    }

    #[test]
    fn io_error_converts() {
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "test");
        let err: Error = io_err.into();
        assert!(matches!(err, Error::Io(_)));
    }
}
