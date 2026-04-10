//! gRPC length-prefixed message framing.
//!
//! Every gRPC message on the wire is prefixed by a 5-byte header:
//!
//! ```text
//! ┌──────────┬──────────────┬─────────────────┐
//! │ 1 byte   │ 4 bytes      │ N bytes         │
//! │ compress │ length (BE)  │ message payload │
//! └──────────┴──────────────┴─────────────────┘
//! ```
//!
//! - Byte 0: compression flag (0 = uncompressed, 1 = compressed per
//!   `grpc-encoding`).
//! - Bytes 1-4: big-endian u32 payload length.
//! - Bytes 5..5+N: the protobuf (or other codec) payload.
//!
//! This module provides zero-copy framing and deframing using `Bytes`/`BytesMut`.

use bytes::{Buf, BufMut, Bytes, BytesMut};

/// The size of the gRPC length-prefixed frame header.
pub const FRAME_HEADER_SIZE: usize = 5;

/// Maximum allowed message size (4 MB default, matching most gRPC
/// implementations).
pub const DEFAULT_MAX_MESSAGE_SIZE: u32 = 4 * 1024 * 1024;

/// Errors from gRPC frame parsing.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("incomplete frame header: need {FRAME_HEADER_SIZE} bytes, have {available}")]
    IncompleteHeader { available: usize },

    #[error("incomplete frame payload: need {needed} bytes, have {available}")]
    IncompletePayload { needed: u32, available: usize },

    #[error("message too large: {size} > {max}")]
    MessageTooLarge { size: u32, max: u32 },

    #[error("invalid compression flag: {0}")]
    InvalidCompressionFlag(u8),
}

/// A parsed gRPC frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Whether the payload is compressed.
    pub compressed: bool,
    /// The length of the payload in bytes.
    pub length: u32,
}

impl FrameHeader {
    /// Parse a frame header from a buffer without consuming it.
    ///
    /// Returns `None` if fewer than [`FRAME_HEADER_SIZE`] bytes are available.
    pub fn peek(buf: &[u8]) -> Result<Option<Self>, FrameError> {
        if buf.len() < FRAME_HEADER_SIZE {
            return Ok(None);
        }

        let flag = buf[0];
        if flag > 1 {
            return Err(FrameError::InvalidCompressionFlag(flag));
        }

        let length = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);

        Ok(Some(Self {
            compressed: flag == 1,
            length,
        }))
    }

    /// Encode this header into a 5-byte array.
    #[inline]
    pub fn encode(&self) -> [u8; FRAME_HEADER_SIZE] {
        let len_bytes = self.length.to_be_bytes();
        [
            if self.compressed { 1 } else { 0 },
            len_bytes[0],
            len_bytes[1],
            len_bytes[2],
            len_bytes[3],
        ]
    }
}

/// Encode a gRPC message frame into the provided buffer.
///
/// Writes the 5-byte header followed by the payload. Zero-copy when `payload`
/// is already a `Bytes`.
pub fn encode_frame(buf: &mut BytesMut, compressed: bool, payload: &[u8]) {
    let header = FrameHeader {
        compressed,
        length: payload.len() as u32,
    };
    buf.reserve(FRAME_HEADER_SIZE + payload.len());
    buf.put_slice(&header.encode());
    buf.put_slice(payload);
}

/// Attempt to decode one gRPC message frame from `buf`.
///
/// On success, advances `buf` past the consumed frame and returns the
/// `(compressed, payload)` pair. Returns `Ok(None)` if the buffer does not
/// yet contain a complete frame.
///
/// `max_message_size` is enforced to prevent memory exhaustion from malicious
/// or buggy senders.
pub fn decode_frame(
    buf: &mut BytesMut,
    max_message_size: u32,
) -> Result<Option<(bool, Bytes)>, FrameError> {
    let header = match FrameHeader::peek(buf) {
        Ok(Some(h)) => h,
        Ok(None) => return Ok(None),
        Err(e) => return Err(e),
    };

    if header.length > max_message_size {
        return Err(FrameError::MessageTooLarge {
            size: header.length,
            max: max_message_size,
        });
    }

    let total = FRAME_HEADER_SIZE + header.length as usize;
    if buf.len() < total {
        return Ok(None);
    }

    // Consume the header.
    buf.advance(FRAME_HEADER_SIZE);
    // Split off the payload as a frozen Bytes (zero-copy).
    let payload = buf.split_to(header.length as usize).freeze();

    Ok(Some((header.compressed, payload)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let payload = b"hello gRPC";
        let mut buf = BytesMut::new();
        encode_frame(&mut buf, false, payload);

        assert_eq!(buf.len(), FRAME_HEADER_SIZE + payload.len());

        let (compressed, decoded) =
            decode_frame(&mut buf, DEFAULT_MAX_MESSAGE_SIZE).unwrap().unwrap();
        assert!(!compressed);
        assert_eq!(&decoded[..], payload);
        assert!(buf.is_empty());
    }

    #[test]
    fn encode_compressed() {
        let payload = b"compressed data";
        let mut buf = BytesMut::new();
        encode_frame(&mut buf, true, payload);

        assert_eq!(buf[0], 1); // compression flag
        let (compressed, _) =
            decode_frame(&mut buf, DEFAULT_MAX_MESSAGE_SIZE).unwrap().unwrap();
        assert!(compressed);
    }

    #[test]
    fn decode_incomplete_header() {
        let mut buf = BytesMut::from(&[0u8, 0, 0][..]);
        assert_eq!(
            decode_frame(&mut buf, DEFAULT_MAX_MESSAGE_SIZE).unwrap(),
            None
        );
    }

    #[test]
    fn decode_incomplete_payload() {
        let mut buf = BytesMut::new();
        encode_frame(&mut buf, false, b"data");
        buf.truncate(FRAME_HEADER_SIZE + 2); // remove last 2 bytes

        assert_eq!(
            decode_frame(&mut buf, DEFAULT_MAX_MESSAGE_SIZE).unwrap(),
            None
        );
    }

    #[test]
    fn decode_message_too_large() {
        let mut buf = BytesMut::new();
        // Encode a header claiming 10MB payload.
        let header = FrameHeader {
            compressed: false,
            length: 10 * 1024 * 1024,
        };
        buf.put_slice(&header.encode());
        // Add some dummy data.
        buf.put_slice(&[0u8; 100]);

        let err = decode_frame(&mut buf, DEFAULT_MAX_MESSAGE_SIZE).unwrap_err();
        assert!(matches!(err, FrameError::MessageTooLarge { .. }));
    }

    #[test]
    fn invalid_compression_flag() {
        let mut buf = BytesMut::from(&[2u8, 0, 0, 0, 1, 0xFF][..]);
        let err = decode_frame(&mut buf, DEFAULT_MAX_MESSAGE_SIZE).unwrap_err();
        assert!(matches!(err, FrameError::InvalidCompressionFlag(2)));
    }

    #[test]
    fn empty_payload() {
        let mut buf = BytesMut::new();
        encode_frame(&mut buf, false, b"");

        let (compressed, payload) =
            decode_frame(&mut buf, DEFAULT_MAX_MESSAGE_SIZE).unwrap().unwrap();
        assert!(!compressed);
        assert!(payload.is_empty());
    }

    #[test]
    fn multiple_frames() {
        let mut buf = BytesMut::new();
        encode_frame(&mut buf, false, b"first");
        encode_frame(&mut buf, true, b"second");

        let (c1, p1) = decode_frame(&mut buf, DEFAULT_MAX_MESSAGE_SIZE).unwrap().unwrap();
        assert!(!c1);
        assert_eq!(&p1[..], b"first");

        let (c2, p2) = decode_frame(&mut buf, DEFAULT_MAX_MESSAGE_SIZE).unwrap().unwrap();
        assert!(c2);
        assert_eq!(&p2[..], b"second");

        assert!(buf.is_empty());
    }

    #[test]
    fn frame_header_peek() {
        let buf = [0u8, 0, 0, 0, 5, 1, 2, 3, 4, 5];
        let header = FrameHeader::peek(&buf).unwrap().unwrap();
        assert!(!header.compressed);
        assert_eq!(header.length, 5);
    }

    #[test]
    fn frame_header_encode() {
        let header = FrameHeader {
            compressed: true,
            length: 256,
        };
        let encoded = header.encode();
        assert_eq!(encoded[0], 1);
        assert_eq!(u32::from_be_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]), 256);
    }
}
