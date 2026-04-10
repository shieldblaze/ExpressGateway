//! gRPC streaming types.
//!
//! gRPC defines four communication patterns based on whether the client and/or
//! server send a stream of messages.

/// The four gRPC streaming patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamingType {
    /// Single request, single response.
    Unary,
    /// Single request, stream of responses.
    ServerStreaming,
    /// Stream of requests, single response.
    ClientStreaming,
    /// Stream of requests, stream of responses.
    Bidirectional,
}

impl StreamingType {
    /// Whether the client side is streaming.
    pub fn is_client_streaming(self) -> bool {
        matches!(self, Self::ClientStreaming | Self::Bidirectional)
    }

    /// Whether the server side is streaming.
    pub fn is_server_streaming(self) -> bool {
        matches!(self, Self::ServerStreaming | Self::Bidirectional)
    }
}

impl std::fmt::Display for StreamingType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unary => write!(f, "unary"),
            Self::ServerStreaming => write!(f, "server_streaming"),
            Self::ClientStreaming => write!(f, "client_streaming"),
            Self::Bidirectional => write!(f, "bidirectional"),
        }
    }
}
