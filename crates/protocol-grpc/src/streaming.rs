//! gRPC streaming types.
//!
//! gRPC defines four communication patterns based on whether the client and/or
//! server send a stream of messages. The streaming type affects how the proxy
//! handles flow control, buffering, and connection lifecycle.

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
    #[inline]
    pub fn is_client_streaming(self) -> bool {
        matches!(self, Self::ClientStreaming | Self::Bidirectional)
    }

    /// Whether the server side is streaming.
    #[inline]
    pub fn is_server_streaming(self) -> bool {
        matches!(self, Self::ServerStreaming | Self::Bidirectional)
    }

    /// Whether either side is streaming (i.e. not unary).
    #[inline]
    pub fn is_streaming(self) -> bool {
        !matches!(self, Self::Unary)
    }
}

impl std::fmt::Display for StreamingType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unary => f.write_str("unary"),
            Self::ServerStreaming => f.write_str("server_streaming"),
            Self::ClientStreaming => f.write_str("client_streaming"),
            Self::Bidirectional => f.write_str("bidirectional"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_streaming_check() {
        assert!(!StreamingType::Unary.is_client_streaming());
        assert!(!StreamingType::ServerStreaming.is_client_streaming());
        assert!(StreamingType::ClientStreaming.is_client_streaming());
        assert!(StreamingType::Bidirectional.is_client_streaming());
    }

    #[test]
    fn server_streaming_check() {
        assert!(!StreamingType::Unary.is_server_streaming());
        assert!(StreamingType::ServerStreaming.is_server_streaming());
        assert!(!StreamingType::ClientStreaming.is_server_streaming());
        assert!(StreamingType::Bidirectional.is_server_streaming());
    }

    #[test]
    fn is_streaming_check() {
        assert!(!StreamingType::Unary.is_streaming());
        assert!(StreamingType::ServerStreaming.is_streaming());
        assert!(StreamingType::ClientStreaming.is_streaming());
        assert!(StreamingType::Bidirectional.is_streaming());
    }

    #[test]
    fn display() {
        assert_eq!(StreamingType::Unary.to_string(), "unary");
        assert_eq!(
            StreamingType::ServerStreaming.to_string(),
            "server_streaming"
        );
        assert_eq!(
            StreamingType::ClientStreaming.to_string(),
            "client_streaming"
        );
        assert_eq!(StreamingType::Bidirectional.to_string(), "bidirectional");
    }
}
