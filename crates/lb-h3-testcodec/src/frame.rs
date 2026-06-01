//! HTTP/3 frame encoding and decoding (RFC 9114).

use bytes::{BufMut, Bytes, BytesMut};

use crate::error::H3Error;
use crate::varint::{decode_varint, encode_varint};

// Frame type constants (RFC 9114 §7.2).
const FRAME_DATA: u64 = 0x00;
const FRAME_HEADERS: u64 = 0x01;
const FRAME_CANCEL_PUSH: u64 = 0x03;
const FRAME_SETTINGS: u64 = 0x04;
const FRAME_PUSH_PROMISE: u64 = 0x05;
const FRAME_GOAWAY: u64 = 0x07;
const FRAME_MAX_PUSH_ID: u64 = 0x0D;

/// An HTTP/3 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H3Frame {
    /// `DATA` frame (type 0x00).
    Data {
        /// Frame payload.
        payload: Bytes,
    },
    /// `HEADERS` frame (type 0x01).
    Headers {
        /// QPACK-encoded header block.
        header_block: Bytes,
    },
    /// `CANCEL_PUSH` frame (type 0x03).
    CancelPush {
        /// Push ID to cancel.
        push_id: u64,
    },
    /// `SETTINGS` frame (type 0x04).
    Settings {
        /// Setting parameters (identifier, value).
        params: Vec<(u64, u64)>,
    },
    /// `PUSH_PROMISE` frame (type 0x05).
    PushPromise {
        /// Push ID.
        push_id: u64,
        /// QPACK-encoded header block.
        header_block: Bytes,
    },
    /// `GOAWAY` frame (type 0x07).
    GoAway {
        /// Stream ID.
        stream_id: u64,
    },
    /// `MAX_PUSH_ID` frame (type 0x0D).
    MaxPushId {
        /// Maximum push ID the peer will accept.
        push_id: u64,
    },
    /// An unknown frame type that MUST be ignored per RFC 9114 Section 7.2.8.
    Unknown {
        /// The frame type identifier.
        frame_type: u64,
        /// The raw payload bytes.
        payload: Bytes,
    },
}

/// Default maximum payload size for H3 frames (1 MB).
pub const DEFAULT_MAX_PAYLOAD_SIZE: usize = 1_048_576;

/// Decode a single HTTP/3 frame from the front of `buf`.
///
/// `max_payload_size` caps the decoded payload length. Frames whose varint
/// payload length exceeds this limit produce `H3Error::FrameTooLarge`.
///
/// Unknown frame types are returned as `H3Frame::Unknown` per RFC 9114
/// Section 7.2.8 (implementations MUST ignore unknown frame types).
///
/// Returns `(frame, bytes_consumed)` on success.
///
/// # Errors
///
/// Returns `H3Error::Incomplete` if the buffer does not contain a complete frame.
/// Returns `H3Error::FrameTooLarge` if the payload exceeds `max_payload_size`.
/// Returns `H3Error::InvalidFrame` on malformed payloads.
pub fn decode_frame(buf: &[u8], max_payload_size: usize) -> Result<(H3Frame, usize), H3Error> {
    let (frame_type, type_len) = decode_varint(buf)?;
    let remaining = buf.get(type_len..).ok_or(H3Error::Incomplete)?;
    let (payload_len, len_len) = decode_varint(remaining)?;

    if payload_len > max_payload_size as u64 {
        return Err(H3Error::FrameTooLarge {
            size: payload_len,
            limit: max_payload_size,
        });
    }

    let header_total = type_len + len_len;
    let payload_len_usize = usize::try_from(payload_len)
        .map_err(|_| H3Error::InvalidFrame("payload too large".to_string()))?;
    let total = header_total + payload_len_usize;

    let payload_buf = buf.get(header_total..total).ok_or(H3Error::Incomplete)?;

    let frame = match frame_type {
        FRAME_DATA => H3Frame::Data {
            payload: Bytes::copy_from_slice(payload_buf),
        },
        FRAME_HEADERS => H3Frame::Headers {
            header_block: Bytes::copy_from_slice(payload_buf),
        },
        FRAME_CANCEL_PUSH => {
            let (push_id, _) = decode_varint(payload_buf)?;
            H3Frame::CancelPush { push_id }
        }
        FRAME_SETTINGS => {
            let mut params = Vec::new();
            let mut pos = 0;
            while pos < payload_buf.len() {
                let rest = payload_buf.get(pos..).ok_or(H3Error::Incomplete)?;
                let (id, id_len) = decode_varint(rest)?;
                let rest2 = rest.get(id_len..).ok_or(H3Error::Incomplete)?;
                let (val, val_len) = decode_varint(rest2)?;
                params.push((id, val));
                pos += id_len + val_len;
            }
            H3Frame::Settings { params }
        }
        FRAME_PUSH_PROMISE => {
            let (push_id, id_len) = decode_varint(payload_buf)?;
            let hdr = payload_buf.get(id_len..).ok_or(H3Error::Incomplete)?;
            H3Frame::PushPromise {
                push_id,
                header_block: Bytes::copy_from_slice(hdr),
            }
        }
        FRAME_GOAWAY => {
            let (stream_id, _) = decode_varint(payload_buf)?;
            H3Frame::GoAway { stream_id }
        }
        FRAME_MAX_PUSH_ID => {
            let (push_id, _) = decode_varint(payload_buf)?;
            H3Frame::MaxPushId { push_id }
        }
        // RFC 9114 §7.2.8: Implementations MUST ignore unknown frame types.
        _ => H3Frame::Unknown {
            frame_type,
            payload: Bytes::copy_from_slice(payload_buf),
        },
    };

    Ok((frame, total))
}

/// Encode an HTTP/3 frame into bytes.
///
/// # Errors
///
/// Returns `H3Error::InvalidFrame` if the frame cannot be encoded.
#[allow(clippy::cast_possible_truncation)]
pub fn encode_frame(frame: &H3Frame) -> Result<Bytes, H3Error> {
    let mut buf = BytesMut::new();

    match frame {
        H3Frame::Data { payload } => {
            encode_varint(&mut buf, FRAME_DATA)?;
            encode_varint(&mut buf, payload.len() as u64)?;
            buf.put_slice(payload);
        }
        H3Frame::Headers { header_block } => {
            encode_varint(&mut buf, FRAME_HEADERS)?;
            encode_varint(&mut buf, header_block.len() as u64)?;
            buf.put_slice(header_block);
        }
        H3Frame::CancelPush { push_id } => {
            let mut inner = BytesMut::new();
            encode_varint(&mut inner, *push_id)?;
            encode_varint(&mut buf, FRAME_CANCEL_PUSH)?;
            encode_varint(&mut buf, inner.len() as u64)?;
            buf.put_slice(&inner);
        }
        H3Frame::Settings { params } => {
            let mut inner = BytesMut::new();
            for &(id, val) in params {
                encode_varint(&mut inner, id)?;
                encode_varint(&mut inner, val)?;
            }
            encode_varint(&mut buf, FRAME_SETTINGS)?;
            encode_varint(&mut buf, inner.len() as u64)?;
            buf.put_slice(&inner);
        }
        H3Frame::PushPromise {
            push_id,
            header_block,
        } => {
            let mut inner = BytesMut::new();
            encode_varint(&mut inner, *push_id)?;
            inner.put_slice(header_block);
            encode_varint(&mut buf, FRAME_PUSH_PROMISE)?;
            encode_varint(&mut buf, inner.len() as u64)?;
            buf.put_slice(&inner);
        }
        H3Frame::GoAway { stream_id } => {
            let mut inner = BytesMut::new();
            encode_varint(&mut inner, *stream_id)?;
            encode_varint(&mut buf, FRAME_GOAWAY)?;
            encode_varint(&mut buf, inner.len() as u64)?;
            buf.put_slice(&inner);
        }
        H3Frame::MaxPushId { push_id } => {
            let mut inner = BytesMut::new();
            encode_varint(&mut inner, *push_id)?;
            encode_varint(&mut buf, FRAME_MAX_PUSH_ID)?;
            encode_varint(&mut buf, inner.len() as u64)?;
            buf.put_slice(&inner);
        }
        H3Frame::Unknown {
            frame_type,
            payload,
        } => {
            encode_varint(&mut buf, *frame_type)?;
            encode_varint(&mut buf, payload.len() as u64)?;
            buf.put_slice(payload);
        }
    }

    Ok(buf.freeze())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_frame_roundtrip() {
        let frame = H3Frame::Data {
            payload: Bytes::from_static(b"hello"),
        };
        let encoded = encode_frame(&frame).unwrap();
        let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn headers_frame_roundtrip() {
        let frame = H3Frame::Headers {
            header_block: Bytes::from_static(b"\x00\x00"),
        };
        let encoded = encode_frame(&frame).unwrap();
        let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn settings_frame_roundtrip() {
        let frame = H3Frame::Settings {
            params: vec![(0x06, 4096), (0x01, 100)],
        };
        let encoded = encode_frame(&frame).unwrap();
        let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn goaway_frame_roundtrip() {
        let frame = H3Frame::GoAway { stream_id: 42 };
        let encoded = encode_frame(&frame).unwrap();
        let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn incomplete_input() {
        assert!(matches!(
            decode_frame(&[], DEFAULT_MAX_PAYLOAD_SIZE),
            Err(H3Error::Incomplete)
        ));
        assert!(matches!(
            decode_frame(&[0x00], DEFAULT_MAX_PAYLOAD_SIZE),
            Err(H3Error::Incomplete)
        ));
    }

    #[test]
    fn frame_too_large_rejected() {
        // Construct a frame with type=DATA (0x00) and a varint payload length of 2000.
        // Use max_payload_size=100 to trigger rejection.
        let mut buf = BytesMut::new();
        encode_varint(&mut buf, FRAME_DATA).unwrap();
        encode_varint(&mut buf, 2000).unwrap();
        buf.extend_from_slice(&vec![0u8; 2000]);

        let result = decode_frame(&buf, 100);
        assert!(matches!(
            result,
            Err(H3Error::FrameTooLarge {
                size: 2000,
                limit: 100
            })
        ));

        // Same frame succeeds with a larger limit.
        let result = decode_frame(&buf, 4096);
        assert!(result.is_ok());
    }

    #[test]
    fn unknown_frame_type_ignored() {
        // RFC 9114 §7.2.8: unknown frame types MUST be ignored.
        // Use frame type 0xFF (not assigned).
        let mut buf = BytesMut::new();
        encode_varint(&mut buf, 0xFF).unwrap();
        let payload = b"some unknown data";
        encode_varint(&mut buf, payload.len() as u64).unwrap();
        buf.extend_from_slice(payload);

        let (frame, consumed) = decode_frame(&buf, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(
            frame,
            H3Frame::Unknown {
                frame_type: 0xFF,
                payload: Bytes::copy_from_slice(payload),
            }
        );
    }
}
