//! gRPC crate error types.
//!
//! Every fallible operation in this crate returns one of these typed errors
//! rather than stringly-typed `anyhow` or raw panics.

/// Errors from gRPC protocol handling.
#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    #[error("invalid grpc-timeout header: {0}")]
    InvalidDeadline(#[from] crate::deadline::DeadlineError),

    #[error("unsupported grpc-encoding: {0}")]
    UnsupportedEncoding(String),

    #[error("invalid gRPC frame: {0}")]
    InvalidFrame(String),

    #[error("gRPC status {code}: {message}")]
    Status { code: u32, message: String },

    #[error("transport error mapped to gRPC UNAVAILABLE: {0}")]
    TransportUnavailable(String),

    #[error("deadline exceeded")]
    DeadlineExceeded,

    #[error("RST_STREAM received: error code {0}")]
    RstStream(u32),

    #[error("gRPC-Web translation error: {0}")]
    GrpcWebTranslation(String),
}
