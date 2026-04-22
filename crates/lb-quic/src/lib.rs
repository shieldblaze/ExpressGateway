//! QUIC transport layer simulation.
//!
//! Since we cannot run real QUIC without a network stack in CI, this crate
//! provides simulated datagram and stream forwarding with validation.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![warn(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Errors from the QUIC layer.
#[derive(Debug, thiserror::Error)]
pub enum QuicError {
    /// Datagram payload is empty.
    #[error("empty datagram payload")]
    EmptyPayload,

    /// Stream data is empty.
    #[error("empty stream data")]
    EmptyStreamData,

    /// Invalid connection ID.
    #[error("invalid connection id: {0}")]
    InvalidConnectionId(u64),
}

/// Simulated QUIC datagram for testing.
#[derive(Debug, Clone)]
pub struct QuicDatagram {
    /// Connection ID this datagram belongs to.
    pub connection_id: u64,
    /// Raw payload bytes.
    pub data: Vec<u8>,
}

/// Simulated QUIC stream.
#[derive(Debug, Clone)]
pub struct QuicStream {
    /// Stream ID within the connection.
    pub stream_id: u64,
    /// Stream payload bytes.
    pub data: Vec<u8>,
    /// Whether this is the final frame on the stream.
    pub fin: bool,
}

/// Forward a QUIC datagram. Validates the payload is non-empty and returns
/// a copy representing the forwarded datagram.
///
/// # Errors
///
/// Returns `QuicError::EmptyPayload` if the datagram has no data.
pub fn forward_datagram(dg: &QuicDatagram) -> Result<QuicDatagram, QuicError> {
    if dg.data.is_empty() {
        return Err(QuicError::EmptyPayload);
    }
    Ok(dg.clone())
}

/// Forward a QUIC stream frame. Validates the data is non-empty and returns
/// a copy representing the forwarded stream frame.
///
/// # Errors
///
/// Returns `QuicError::EmptyStreamData` if the stream has no data.
pub fn forward_stream(stream: &QuicStream) -> Result<QuicStream, QuicError> {
    if stream.data.is_empty() {
        return Err(QuicError::EmptyStreamData);
    }
    Ok(stream.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datagram_forward_ok() {
        let dg = QuicDatagram {
            connection_id: 42,
            data: b"hello".to_vec(),
        };
        let fwd = forward_datagram(&dg).unwrap();
        assert_eq!(fwd.connection_id, 42);
        assert_eq!(fwd.data, b"hello");
    }

    #[test]
    fn datagram_empty_rejected() {
        let dg = QuicDatagram {
            connection_id: 1,
            data: vec![],
        };
        assert!(forward_datagram(&dg).is_err());
    }

    #[test]
    fn stream_forward_ok() {
        let stream = QuicStream {
            stream_id: 1,
            data: b"payload".to_vec(),
            fin: true,
        };
        let fwd = forward_stream(&stream).unwrap();
        assert_eq!(fwd.stream_id, 1);
        assert!(fwd.fin);
    }

    #[test]
    fn stream_empty_rejected() {
        let stream = QuicStream {
            stream_id: 0,
            data: vec![],
            fin: false,
        };
        assert!(forward_stream(&stream).is_err());
    }
}
