//! Protocol sniffing: detect TLS ClientHello vs HTTP/1.x vs HTTP/2 preface.
//!
//! Peeks at the first bytes of a TCP connection to determine the protocol
//! without consuming the data. This enables routing decisions before full
//! protocol negotiation:
//!
//! - TLS ClientHello → TLS termination or SNI-based passthrough
//! - HTTP/2 connection preface → direct HTTP/2 handling
//! - HTTP/1.x method → HTTP/1.1 handling
//! - Unknown → pass to default handler

use tokio::net::TcpStream;

use crate::error::{Result, TlsError};

/// The HTTP/2 connection preface magic bytes.
const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Maximum bytes we'll peek at.
const MAX_PEEK_BYTES: usize = 64;

/// Detected protocol from connection sniffing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedProtocol {
    /// TLS ClientHello detected. The connection should be TLS-terminated
    /// or passed through based on SNI.
    Tls,
    /// HTTP/2 connection preface detected (plaintext h2c).
    Http2,
    /// HTTP/1.x request detected (starts with a known HTTP method).
    Http1,
    /// Unknown protocol. Pass to default handler.
    Unknown,
}

impl std::fmt::Display for DetectedProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetectedProtocol::Tls => write!(f, "TLS"),
            DetectedProtocol::Http2 => write!(f, "HTTP/2"),
            DetectedProtocol::Http1 => write!(f, "HTTP/1.x"),
            DetectedProtocol::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Protocol sniffer that peeks at TCP stream bytes.
pub struct ProtocolSniffer;

impl ProtocolSniffer {
    /// Peek at the first bytes of a TCP stream to detect the protocol.
    ///
    /// Uses `TcpStream::peek()` which reads data without consuming it from the
    /// socket buffer, so subsequent reads will see the same bytes.
    pub async fn sniff(stream: &TcpStream) -> Result<DetectedProtocol> {
        let mut buf = [0u8; MAX_PEEK_BYTES];

        let n = stream.peek(&mut buf).await.map_err(|e| TlsError::SniffIo {
            source: e,
        })?;

        if n < 1 {
            return Err(TlsError::SniffInsufficient { got: 0, need: 1 });
        }

        Ok(Self::detect(&buf[..n]))
    }

    /// Detect protocol from a byte buffer. Pure function, no I/O.
    ///
    /// This is the core detection logic, separated for testing.
    #[inline]
    pub fn detect(buf: &[u8]) -> DetectedProtocol {
        if buf.is_empty() {
            return DetectedProtocol::Unknown;
        }

        // TLS record: content type 0x16 (handshake), version 0x03 0x0X
        if buf[0] == 0x16 && buf.len() >= 3 && buf[1] == 0x03 {
            return DetectedProtocol::Tls;
        }

        // HTTP/2 connection preface: "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" (24 bytes)
        // Per RFC 9113 section 3.4, the preface starts with this exact sequence.
        // We require at least "PRI * " (6 bytes) for partial matches to avoid
        // false positives (e.g., data starting with "PRIV" or "PRINT").
        if buf.len() >= H2_PREFACE.len() && &buf[..H2_PREFACE.len()] == H2_PREFACE {
            return DetectedProtocol::Http2;
        }
        if buf.len() >= 6 && &buf[..6] == b"PRI * " {
            return DetectedProtocol::Http2;
        }

        // HTTP/1.x: starts with a known method
        if is_http1_method(buf) {
            return DetectedProtocol::Http1;
        }

        DetectedProtocol::Unknown
    }
}

/// Check if the buffer starts with a known HTTP/1.x method.
#[inline]
fn is_http1_method(buf: &[u8]) -> bool {
    const METHODS: &[&[u8]] = &[
        b"GET ", b"POST ", b"PUT ", b"DELETE ", b"HEAD ", b"OPTIONS ",
        b"PATCH ", b"CONNECT ", b"TRACE ",
    ];

    for method in METHODS {
        if buf.len() >= method.len() && &buf[..method.len()] == *method {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_tls_client_hello() {
        // TLS 1.0 record header
        let buf = [0x16, 0x03, 0x01, 0x00, 0x05, 0x01, 0x00, 0x00];
        assert_eq!(ProtocolSniffer::detect(&buf), DetectedProtocol::Tls);
    }

    #[test]
    fn detect_tls_13_record() {
        let buf = [0x16, 0x03, 0x03, 0x00, 0x10];
        assert_eq!(ProtocolSniffer::detect(&buf), DetectedProtocol::Tls);
    }

    #[test]
    fn detect_http2_preface() {
        let buf = H2_PREFACE;
        assert_eq!(ProtocolSniffer::detect(buf), DetectedProtocol::Http2);
    }

    #[test]
    fn detect_http2_partial_preface() {
        // "PRI * " (6 bytes) is the minimum partial match.
        let buf = b"PRI * H";
        assert_eq!(ProtocolSniffer::detect(buf), DetectedProtocol::Http2);

        let buf = b"PRI * ";
        assert_eq!(ProtocolSniffer::detect(buf), DetectedProtocol::Http2);
    }

    #[test]
    fn three_byte_pri_is_not_http2() {
        // "PRI" alone is too short to be confident -- could be "PRIV", "PRINT", etc.
        let buf = b"PRI";
        assert_eq!(ProtocolSniffer::detect(buf), DetectedProtocol::Unknown);
    }

    #[test]
    fn detect_http1_get() {
        let buf = b"GET / HTTP/1.1\r\nHost: example.com\r\n";
        assert_eq!(ProtocolSniffer::detect(buf), DetectedProtocol::Http1);
    }

    #[test]
    fn detect_http1_post() {
        let buf = b"POST /api/data HTTP/1.1\r\n";
        assert_eq!(ProtocolSniffer::detect(buf), DetectedProtocol::Http1);
    }

    #[test]
    fn detect_http1_methods() {
        for method in &["GET ", "POST ", "PUT ", "DELETE ", "HEAD ", "OPTIONS ", "PATCH ", "CONNECT ", "TRACE "] {
            let buf = method.as_bytes();
            assert_eq!(
                ProtocolSniffer::detect(buf),
                DetectedProtocol::Http1,
                "should detect {method} as HTTP/1.x"
            );
        }
    }

    #[test]
    fn detect_unknown() {
        let buf = [0x00, 0x01, 0x02, 0x03];
        assert_eq!(ProtocolSniffer::detect(&buf), DetectedProtocol::Unknown);
    }

    #[test]
    fn detect_empty() {
        assert_eq!(ProtocolSniffer::detect(&[]), DetectedProtocol::Unknown);
    }

    #[test]
    fn display_variants() {
        assert_eq!(DetectedProtocol::Tls.to_string(), "TLS");
        assert_eq!(DetectedProtocol::Http2.to_string(), "HTTP/2");
        assert_eq!(DetectedProtocol::Http1.to_string(), "HTTP/1.x");
        assert_eq!(DetectedProtocol::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn not_tls_when_version_wrong() {
        // Content type 0x16 but version not 0x03
        let buf = [0x16, 0x02, 0x01];
        assert_eq!(ProtocolSniffer::detect(&buf), DetectedProtocol::Unknown);
    }
}
