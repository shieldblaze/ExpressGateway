//! HTTP/2 frame encoding and decoding (RFC 9113).

use bytes::{BufMut, Bytes, BytesMut};

use crate::H2Error;

// Frame type constants (RFC 9113 §4).
const FRAME_DATA: u8 = 0x0;
const FRAME_HEADERS: u8 = 0x1;
const FRAME_PRIORITY: u8 = 0x2;
const FRAME_RST_STREAM: u8 = 0x3;
const FRAME_SETTINGS: u8 = 0x4;
const FRAME_PUSH_PROMISE: u8 = 0x5;
const FRAME_PING: u8 = 0x6;
const FRAME_GOAWAY: u8 = 0x7;
const FRAME_WINDOW_UPDATE: u8 = 0x8;
const FRAME_CONTINUATION: u8 = 0x9;

// Flag constants.
const FLAG_END_STREAM: u8 = 0x1;
const FLAG_ACK: u8 = 0x1;
const FLAG_END_HEADERS: u8 = 0x4;
const FLAG_PRIORITY_FLAG: u8 = 0x20;

/// An HTTP/2 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H2Frame {
    /// `DATA` frame (type 0x0).
    Data {
        /// Stream identifier.
        stream_id: u32,
        /// Frame payload.
        payload: Bytes,
        /// Whether this frame ends the stream.
        end_stream: bool,
    },
    /// `HEADERS` frame (type 0x1).
    Headers {
        /// Stream identifier.
        stream_id: u32,
        /// HPACK-encoded header block fragment.
        header_block: Bytes,
        /// Whether this frame ends the stream.
        end_stream: bool,
        /// Whether this frame ends the header block.
        end_headers: bool,
    },
    /// `PRIORITY` frame (type 0x2).
    Priority {
        /// Stream identifier.
        stream_id: u32,
        /// Stream dependency.
        dependency: u32,
        /// Priority weight (1-256, stored as 0-255 on wire).
        weight: u8,
        /// Whether the dependency is exclusive.
        exclusive: bool,
    },
    /// `RST_STREAM` frame (type 0x3).
    RstStream {
        /// Stream identifier.
        stream_id: u32,
        /// Error code.
        error_code: u32,
    },
    /// `SETTINGS` frame (type 0x4).
    Settings {
        /// Whether this is an ACK.
        ack: bool,
        /// Setting parameters (id, value).
        params: Vec<(u16, u32)>,
    },
    /// `PUSH_PROMISE` frame (type 0x5).
    PushPromise {
        /// Stream identifier.
        stream_id: u32,
        /// Promised stream identifier.
        promised_id: u32,
        /// HPACK-encoded header block fragment.
        header_block: Bytes,
    },
    /// `PING` frame (type 0x6).
    Ping {
        /// Whether this is an ACK.
        ack: bool,
        /// 8 bytes of opaque data.
        data: [u8; 8],
    },
    /// `GOAWAY` frame (type 0x7).
    GoAway {
        /// Last stream identifier the sender will process.
        last_stream_id: u32,
        /// Error code.
        error_code: u32,
        /// Additional debug data.
        debug_data: Bytes,
    },
    /// `WINDOW_UPDATE` frame (type 0x8).
    WindowUpdate {
        /// Stream identifier (0 for connection-level).
        stream_id: u32,
        /// Window size increment.
        increment: u32,
    },
    /// `CONTINUATION` frame (type 0x9).
    Continuation {
        /// Stream identifier.
        stream_id: u32,
        /// HPACK-encoded header block fragment.
        header_block: Bytes,
        /// Whether this frame ends the header block.
        end_headers: bool,
    },
    /// An unknown frame type that MUST be ignored per RFC 9113 Section 4.1.
    Unknown {
        /// The frame type identifier.
        frame_type: u8,
        /// Stream identifier.
        stream_id: u32,
        /// Flags byte (preserved as-is).
        flags: u8,
        /// The raw payload bytes.
        payload: Bytes,
    },
}

/// Read a `u32` from a 4-byte big-endian slice.
fn read_u32_be(buf: &[u8]) -> Result<u32, H2Error> {
    let b0 = u32::from(*buf.first().ok_or(H2Error::Incomplete)?);
    let b1 = u32::from(*buf.get(1).ok_or(H2Error::Incomplete)?);
    let b2 = u32::from(*buf.get(2).ok_or(H2Error::Incomplete)?);
    let b3 = u32::from(*buf.get(3).ok_or(H2Error::Incomplete)?);
    Ok((b0 << 24) | (b1 << 16) | (b2 << 8) | b3)
}

/// Read a `u16` from a 2-byte big-endian slice.
fn read_u16_be(buf: &[u8]) -> Result<u16, H2Error> {
    let b0 = u16::from(*buf.first().ok_or(H2Error::Incomplete)?);
    let b1 = u16::from(*buf.get(1).ok_or(H2Error::Incomplete)?);
    Ok((b0 << 8) | b1)
}

/// Parsed frame header (9 bytes on the wire).
struct FrameHeader {
    length: usize,
    frame_type: u8,
    flags: u8,
    stream_id: u32,
}

/// Parse the 9-byte frame header from `buf`.
fn parse_frame_header(buf: &[u8]) -> Result<FrameHeader, H2Error> {
    if buf.len() < 9 {
        return Err(H2Error::Incomplete);
    }

    let b0 = u32::from(*buf.first().ok_or(H2Error::Incomplete)?);
    let b1 = u32::from(*buf.get(1).ok_or(H2Error::Incomplete)?);
    let b2 = u32::from(*buf.get(2).ok_or(H2Error::Incomplete)?);
    #[allow(clippy::cast_possible_truncation)]
    let length = ((b0 << 16) | (b1 << 8) | b2) as usize;
    let frame_type = *buf.get(3).ok_or(H2Error::Incomplete)?;
    let flags = *buf.get(4).ok_or(H2Error::Incomplete)?;

    let sid_bytes = buf.get(5..9).ok_or(H2Error::Incomplete)?;
    let raw_sid = read_u32_be(sid_bytes)?;
    let stream_id = raw_sid & 0x7FFF_FFFF;

    Ok(FrameHeader {
        length,
        frame_type,
        flags,
        stream_id,
    })
}

/// Decode a `DATA`, `HEADERS`, `PRIORITY`, or `RST_STREAM` frame.
fn decode_frame_low(
    frame_type: u8,
    flags: u8,
    stream_id: u32,
    payload: &[u8],
) -> Result<H2Frame, H2Error> {
    match frame_type {
        FRAME_DATA => Ok(H2Frame::Data {
            stream_id,
            payload: Bytes::copy_from_slice(payload),
            end_stream: flags & FLAG_END_STREAM != 0,
        }),
        FRAME_HEADERS => {
            let hdr_payload = if flags & FLAG_PRIORITY_FLAG != 0 {
                if payload.len() < 5 {
                    return Err(H2Error::InvalidFrame(
                        "HEADERS with PRIORITY flag but payload too short".to_string(),
                    ));
                }
                payload
                    .get(5..)
                    .ok_or_else(|| H2Error::InvalidFrame("HEADERS priority slice".to_string()))?
            } else {
                payload
            };
            Ok(H2Frame::Headers {
                stream_id,
                header_block: Bytes::copy_from_slice(hdr_payload),
                end_stream: flags & FLAG_END_STREAM != 0,
                end_headers: flags & FLAG_END_HEADERS != 0,
            })
        }
        FRAME_PRIORITY => {
            if payload.len() != 5 {
                return Err(H2Error::InvalidFrame(
                    "PRIORITY frame must be 5 bytes".to_string(),
                ));
            }
            let dep_raw = read_u32_be(payload.get(0..4).ok_or(H2Error::Incomplete)?)?;
            let exclusive = dep_raw & 0x8000_0000 != 0;
            let dependency = dep_raw & 0x7FFF_FFFF;
            let weight = *payload.get(4).ok_or(H2Error::Incomplete)?;
            Ok(H2Frame::Priority {
                stream_id,
                dependency,
                weight,
                exclusive,
            })
        }
        FRAME_RST_STREAM => {
            if payload.len() != 4 {
                return Err(H2Error::InvalidFrame(
                    "RST_STREAM frame must be 4 bytes".to_string(),
                ));
            }
            let error_code = read_u32_be(payload)?;
            Ok(H2Frame::RstStream {
                stream_id,
                error_code,
            })
        }
        _ => Err(H2Error::InvalidFrame(format!(
            "unexpected frame type in decode_frame_low: {frame_type}"
        ))),
    }
}

/// Decode `SETTINGS`, `PUSH_PROMISE`, `PING`, `GOAWAY`, `WINDOW_UPDATE`,
/// or `CONTINUATION` frames.
fn decode_frame_high(
    frame_type: u8,
    flags: u8,
    stream_id: u32,
    payload: &[u8],
) -> Result<H2Frame, H2Error> {
    match frame_type {
        FRAME_SETTINGS => {
            let ack = flags & FLAG_ACK != 0;
            if ack && !payload.is_empty() {
                return Err(H2Error::InvalidFrame(
                    "SETTINGS ACK must have empty payload".to_string(),
                ));
            }
            if payload.len() % 6 != 0 {
                return Err(H2Error::InvalidFrame(
                    "SETTINGS payload must be multiple of 6".to_string(),
                ));
            }
            let mut params = Vec::with_capacity(payload.len() / 6);
            let mut i = 0;
            while i + 6 <= payload.len() {
                let id = read_u16_be(payload.get(i..i + 2).ok_or(H2Error::Incomplete)?)?;
                let val = read_u32_be(payload.get(i + 2..i + 6).ok_or(H2Error::Incomplete)?)?;
                params.push((id, val));
                i += 6;
            }
            Ok(H2Frame::Settings { ack, params })
        }
        FRAME_PUSH_PROMISE => {
            if payload.len() < 4 {
                return Err(H2Error::InvalidFrame("PUSH_PROMISE too short".to_string()));
            }
            let promised_raw = read_u32_be(payload.get(0..4).ok_or(H2Error::Incomplete)?)?;
            let promised_id = promised_raw & 0x7FFF_FFFF;
            let header_block = Bytes::copy_from_slice(payload.get(4..).ok_or(H2Error::Incomplete)?);
            Ok(H2Frame::PushPromise {
                stream_id,
                promised_id,
                header_block,
            })
        }
        FRAME_PING => {
            if payload.len() != 8 {
                return Err(H2Error::InvalidFrame(
                    "PING frame must be 8 bytes".to_string(),
                ));
            }
            let mut data = [0u8; 8];
            let src = payload.get(..8).ok_or(H2Error::Incomplete)?;
            for (i, d) in data.iter_mut().enumerate() {
                *d = *src.get(i).ok_or(H2Error::Incomplete)?;
            }
            Ok(H2Frame::Ping {
                ack: flags & FLAG_ACK != 0,
                data,
            })
        }
        FRAME_GOAWAY => {
            if payload.len() < 8 {
                return Err(H2Error::InvalidFrame(
                    "GOAWAY frame must be at least 8 bytes".to_string(),
                ));
            }
            let last_raw = read_u32_be(payload.get(0..4).ok_or(H2Error::Incomplete)?)?;
            let last_stream_id = last_raw & 0x7FFF_FFFF;
            let error_code = read_u32_be(payload.get(4..8).ok_or(H2Error::Incomplete)?)?;
            let debug_data = Bytes::copy_from_slice(payload.get(8..).ok_or(H2Error::Incomplete)?);
            Ok(H2Frame::GoAway {
                last_stream_id,
                error_code,
                debug_data,
            })
        }
        FRAME_WINDOW_UPDATE => {
            if payload.len() != 4 {
                return Err(H2Error::InvalidFrame(
                    "WINDOW_UPDATE must be 4 bytes".to_string(),
                ));
            }
            let raw = read_u32_be(payload)?;
            let increment = raw & 0x7FFF_FFFF;
            Ok(H2Frame::WindowUpdate {
                stream_id,
                increment,
            })
        }
        FRAME_CONTINUATION => Ok(H2Frame::Continuation {
            stream_id,
            header_block: Bytes::copy_from_slice(payload),
            end_headers: flags & FLAG_END_HEADERS != 0,
        }),
        // RFC 9113 §4.1: Implementations MUST ignore unknown frame types.
        other => Ok(H2Frame::Unknown {
            frame_type: other,
            stream_id,
            flags,
            payload: Bytes::copy_from_slice(payload),
        }),
    }
}

/// Default maximum frame size per RFC 9113 Section 4.2.
pub const DEFAULT_MAX_FRAME_SIZE: usize = 16_384;

/// Decode a single HTTP/2 frame from the front of `buf`.
///
/// `max_frame_size` is the `SETTINGS_MAX_FRAME_SIZE` value (default 16,384).
/// Frames with a payload length exceeding this limit produce
/// `H2Error::FrameTooLarge`.
///
/// Returns `(frame, bytes_consumed)` on success.
///
/// # Errors
///
/// Returns `H2Error::Incomplete` if `buf` does not contain a complete frame.
/// Returns `H2Error::FrameTooLarge` if the payload exceeds `max_frame_size`.
/// Returns `H2Error::InvalidFrame` on malformed input.
pub fn decode_frame(buf: &[u8], max_frame_size: usize) -> Result<(H2Frame, usize), H2Error> {
    let hdr = parse_frame_header(buf)?;

    if hdr.length > max_frame_size {
        return Err(H2Error::FrameTooLarge {
            size: hdr.length,
            limit: max_frame_size,
        });
    }

    let total = 9 + hdr.length;

    if buf.len() < total {
        return Err(H2Error::Incomplete);
    }

    let payload = buf.get(9..total).ok_or(H2Error::Incomplete)?;

    let frame = match hdr.frame_type {
        FRAME_DATA | FRAME_HEADERS | FRAME_PRIORITY | FRAME_RST_STREAM => {
            decode_frame_low(hdr.frame_type, hdr.flags, hdr.stream_id, payload)?
        }
        _ => decode_frame_high(hdr.frame_type, hdr.flags, hdr.stream_id, payload)?,
    };

    Ok((frame, total))
}

/// Encode an HTTP/2 frame into bytes.
///
/// # Errors
///
/// Returns `H2Error::InvalidFrame` if the frame cannot be encoded (e.g. payload too large).
pub fn encode_frame(frame: &H2Frame) -> Result<Bytes, H2Error> {
    let mut buf = BytesMut::new();

    match frame {
        H2Frame::Data { .. }
        | H2Frame::Headers { .. }
        | H2Frame::Priority { .. }
        | H2Frame::RstStream { .. }
        | H2Frame::Settings { .. } => encode_frame_lower(&mut buf, frame)?,
        _ => encode_frame_upper(&mut buf, frame)?,
    }

    Ok(buf.freeze())
}

/// Encode `DATA`, `HEADERS`, `PRIORITY`, `RST_STREAM`, and `SETTINGS` frames.
fn encode_frame_lower(buf: &mut BytesMut, frame: &H2Frame) -> Result<(), H2Error> {
    match frame {
        H2Frame::Data {
            stream_id,
            payload,
            end_stream,
        } => {
            let flags = if *end_stream { FLAG_END_STREAM } else { 0 };
            write_header(buf, payload.len(), FRAME_DATA, flags, *stream_id)?;
            buf.put_slice(payload);
        }
        H2Frame::Headers {
            stream_id,
            header_block,
            end_stream,
            end_headers,
        } => {
            let mut flags = 0u8;
            if *end_stream {
                flags |= FLAG_END_STREAM;
            }
            if *end_headers {
                flags |= FLAG_END_HEADERS;
            }
            write_header(buf, header_block.len(), FRAME_HEADERS, flags, *stream_id)?;
            buf.put_slice(header_block);
        }
        H2Frame::Priority {
            stream_id,
            dependency,
            weight,
            exclusive,
        } => {
            write_header(buf, 5, FRAME_PRIORITY, 0, *stream_id)?;
            let dep = if *exclusive {
                *dependency | 0x8000_0000
            } else {
                *dependency
            };
            buf.put_u32(dep);
            buf.put_u8(*weight);
        }
        H2Frame::RstStream {
            stream_id,
            error_code,
        } => {
            write_header(buf, 4, FRAME_RST_STREAM, 0, *stream_id)?;
            buf.put_u32(*error_code);
        }
        H2Frame::Settings { ack, params } => {
            let flags = if *ack { FLAG_ACK } else { 0 };
            write_header(buf, params.len() * 6, FRAME_SETTINGS, flags, 0)?;
            for &(id, val) in params {
                buf.put_u16(id);
                buf.put_u32(val);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Encode `PUSH_PROMISE`, `PING`, `GOAWAY`, `WINDOW_UPDATE`, and `CONTINUATION`.
fn encode_frame_upper(buf: &mut BytesMut, frame: &H2Frame) -> Result<(), H2Error> {
    match frame {
        H2Frame::PushPromise {
            stream_id,
            promised_id,
            header_block,
        } => {
            write_header(
                buf,
                4 + header_block.len(),
                FRAME_PUSH_PROMISE,
                0,
                *stream_id,
            )?;
            buf.put_u32(*promised_id & 0x7FFF_FFFF);
            buf.put_slice(header_block);
        }
        H2Frame::Ping { ack, data } => {
            let flags = if *ack { FLAG_ACK } else { 0 };
            write_header(buf, 8, FRAME_PING, flags, 0)?;
            buf.put_slice(data);
        }
        H2Frame::GoAway {
            last_stream_id,
            error_code,
            debug_data,
        } => {
            write_header(buf, 8 + debug_data.len(), FRAME_GOAWAY, 0, 0)?;
            buf.put_u32(*last_stream_id & 0x7FFF_FFFF);
            buf.put_u32(*error_code);
            buf.put_slice(debug_data);
        }
        H2Frame::WindowUpdate {
            stream_id,
            increment,
        } => {
            write_header(buf, 4, FRAME_WINDOW_UPDATE, 0, *stream_id)?;
            buf.put_u32(*increment & 0x7FFF_FFFF);
        }
        H2Frame::Continuation {
            stream_id,
            header_block,
            end_headers,
        } => {
            let flags = if *end_headers { FLAG_END_HEADERS } else { 0 };
            write_header(
                buf,
                header_block.len(),
                FRAME_CONTINUATION,
                flags,
                *stream_id,
            )?;
            buf.put_slice(header_block);
        }
        H2Frame::Unknown {
            frame_type,
            stream_id,
            flags,
            payload,
        } => {
            write_header(buf, payload.len(), *frame_type, *flags, *stream_id)?;
            buf.put_slice(payload);
        }
        _ => {}
    }
    Ok(())
}

/// Write a 9-byte HTTP/2 frame header.
fn write_header(
    buf: &mut BytesMut,
    length: usize,
    frame_type: u8,
    flags: u8,
    stream_id: u32,
) -> Result<(), H2Error> {
    if length > 0x00FF_FFFF {
        return Err(H2Error::InvalidFrame(format!(
            "payload length {length} exceeds 24-bit max"
        )));
    }
    #[allow(clippy::cast_possible_truncation)]
    let len = length as u32;
    buf.put_u8(((len >> 16) & 0xFF) as u8);
    buf.put_u8(((len >> 8) & 0xFF) as u8);
    buf.put_u8((len & 0xFF) as u8);
    buf.put_u8(frame_type);
    buf.put_u8(flags);
    buf.put_u32(stream_id & 0x7FFF_FFFF);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_frame_roundtrip() {
        let frame = H2Frame::Data {
            stream_id: 1,
            payload: Bytes::from_static(b"hello"),
            end_stream: true,
        };
        let encoded = encode_frame(&frame).unwrap();
        let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn settings_ack_roundtrip() {
        let frame = H2Frame::Settings {
            ack: true,
            params: vec![],
        };
        let encoded = encode_frame(&frame).unwrap();
        let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn ping_roundtrip() {
        let frame = H2Frame::Ping {
            ack: false,
            data: [1, 2, 3, 4, 5, 6, 7, 8],
        };
        let encoded = encode_frame(&frame).unwrap();
        let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn goaway_roundtrip() {
        let frame = H2Frame::GoAway {
            last_stream_id: 100,
            error_code: 0,
            debug_data: Bytes::from_static(b"bye"),
        };
        let encoded = encode_frame(&frame).unwrap();
        let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn incomplete_returns_error() {
        let buf = [0u8; 5];
        assert!(matches!(
            decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE),
            Err(H2Error::Incomplete)
        ));
    }

    #[test]
    fn frame_too_large_rejected() {
        // Encode a DATA frame with 20000-byte payload (> 16384 default).
        let payload = vec![0u8; 20_000];
        let frame = H2Frame::Data {
            stream_id: 1,
            payload: Bytes::from(payload),
            end_stream: false,
        };
        let encoded = encode_frame(&frame).unwrap();

        let result = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE);
        assert!(matches!(
            result,
            Err(H2Error::FrameTooLarge {
                size: 20_000,
                limit: 16_384
            })
        ));

        // Same frame succeeds with a larger limit.
        let result = decode_frame(&encoded, 32_768);
        assert!(result.is_ok());
    }

    #[test]
    fn unknown_frame_type_ignored() {
        // RFC 9113 §4.1: implementations MUST ignore unknown frame types.
        // Build a raw frame with type 0xFF, stream_id=7, flags=0x42, payload "xyz".
        let payload = b"xyz";
        let mut buf = BytesMut::new();
        // 3-byte length
        #[allow(clippy::cast_possible_truncation)]
        let len = payload.len() as u32;
        buf.put_u8(((len >> 16) & 0xFF) as u8);
        buf.put_u8(((len >> 8) & 0xFF) as u8);
        buf.put_u8((len & 0xFF) as u8);
        // type
        buf.put_u8(0xFF);
        // flags
        buf.put_u8(0x42);
        // stream id
        buf.put_u32(7);
        // payload
        buf.put_slice(payload);

        let (frame, consumed) = decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(
            frame,
            H2Frame::Unknown {
                frame_type: 0xFF,
                stream_id: 7,
                flags: 0x42,
                payload: Bytes::from_static(b"xyz"),
            }
        );
    }

    #[test]
    fn unknown_frame_roundtrip() {
        let frame = H2Frame::Unknown {
            frame_type: 0xFE,
            stream_id: 42,
            flags: 0x00,
            payload: Bytes::from_static(b"opaque"),
        };
        let encoded = encode_frame(&frame).unwrap();
        let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, frame);
    }
}
