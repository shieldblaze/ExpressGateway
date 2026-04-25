//! gRPC error types.

/// Errors raised during gRPC frame processing.
#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    /// Not enough data to decode a complete frame.
    #[error("incomplete gRPC frame, need more data")]
    Incomplete,

    /// The frame data is malformed.
    #[error("invalid gRPC frame: {0}")]
    InvalidFrame(String),

    /// The gRPC status code is not in the valid range (0..=16).
    #[error("invalid gRPC status code: {0}")]
    InvalidStatus(u32),

    /// The `grpc-timeout` header value could not be parsed.
    #[error("invalid timeout format: {0}")]
    InvalidTimeout(String),

    /// The message exceeds the configured maximum size.
    #[error("message too large: {size} exceeds {limit}")]
    MessageTooLarge {
        /// Actual message size in bytes.
        size: u32,
        /// Configured limit in bytes.
        limit: u32,
    },
}
