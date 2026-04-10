//! Protocol definitions.

use serde::{Deserialize, Serialize};

/// Application protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Protocol {
    /// Raw TCP (L4).
    Tcp,
    /// Raw UDP (L4).
    Udp,
    /// HTTP/1.1 cleartext.
    Http,
    /// HTTPS (HTTP/1.1 or HTTP/2 over TLS).
    Https,
    /// HTTP/2 cleartext (prior knowledge / h2c).
    H2c,
    /// QUIC / HTTP/3.
    Quic,
    /// gRPC over HTTP/2.
    Grpc,
    /// WebSocket.
    WebSocket,
}

impl Protocol {
    /// Whether this protocol uses TLS.
    pub fn is_tls(&self) -> bool {
        matches!(self, Protocol::Https | Protocol::Quic)
    }

    /// Whether this is an L4 (transport layer) protocol.
    pub fn is_l4(&self) -> bool {
        matches!(self, Protocol::Tcp | Protocol::Udp)
    }

    /// Whether this is an L7 (application layer) protocol.
    pub fn is_l7(&self) -> bool {
        !self.is_l4()
    }

    /// Default port for this protocol.
    pub fn default_port(&self) -> u16 {
        match self {
            Protocol::Http | Protocol::H2c => 80,
            Protocol::Https | Protocol::Quic => 443,
            Protocol::Grpc => 50051,
            Protocol::Tcp | Protocol::Udp | Protocol::WebSocket => 0,
        }
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::Tcp => write!(f, "tcp"),
            Protocol::Udp => write!(f, "udp"),
            Protocol::Http => write!(f, "http"),
            Protocol::Https => write!(f, "https"),
            Protocol::H2c => write!(f, "h2c"),
            Protocol::Quic => write!(f, "quic"),
            Protocol::Grpc => write!(f, "grpc"),
            Protocol::WebSocket => write!(f, "websocket"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_properties() {
        assert!(Protocol::Tcp.is_l4());
        assert!(Protocol::Udp.is_l4());
        assert!(!Protocol::Http.is_l4());

        assert!(Protocol::Https.is_tls());
        assert!(Protocol::Quic.is_tls());
        assert!(!Protocol::Http.is_tls());

        assert!(Protocol::Http.is_l7());
        assert!(!Protocol::Tcp.is_l7());
    }
}
