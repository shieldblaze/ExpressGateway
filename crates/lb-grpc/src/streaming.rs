//! gRPC streaming mode detection.

/// The four gRPC streaming modes.
///
/// Determined by the service definition (`.proto` file). In a proxy context,
/// the mode can be inferred from the service method descriptor or from
/// observing the message flow pattern.
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
