//! gRPC streaming mode detection.

/// The four gRPC streaming modes, set by the `.proto` service definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingMode {
    /// Single request, single response.
    Unary,
    /// Single request, stream of responses.
    ServerStreaming,
    /// Stream of requests, single response.
    ClientStreaming,
    /// Stream of requests, stream of responses.
    BidiStreaming,
}
