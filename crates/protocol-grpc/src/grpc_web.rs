//! gRPC-Web translation layer.
//!
//! gRPC-Web is a variant of gRPC designed for browser clients that cannot use
//! native HTTP/2 gRPC. It uses HTTP/1.1 with the following differences:
//!
//! - Content-Type: `application/grpc-web` (binary) or
//!   `application/grpc-web-text` (base64-encoded).
//! - Trailers are encoded as a length-prefixed block appended to the response
//!   body (since HTTP/1.1 trailers are unreliable in browsers).
//! - The trailer block has bit 7 of the compression flag set (0x80) to
//!   distinguish it from a data frame.
//!
//! This module handles translation between gRPC-Web (HTTP/1.1) and native
//! gRPC (HTTP/2 or HTTP/3).

use bytes::{BufMut, Bytes, BytesMut};
use http::header::HeaderName;
use http::{HeaderMap, HeaderValue};

use crate::framing::FRAME_HEADER_SIZE;

/// The trailer frame flag: bit 7 set in the compression byte indicates
/// this frame carries trailers, not data.
const TRAILER_FRAME_FLAG: u8 = 0x80;

/// Content type for binary gRPC-Web.
pub const GRPC_WEB_CONTENT_TYPE: &str = "application/grpc-web";

/// Content type for text (base64) gRPC-Web.
pub const GRPC_WEB_TEXT_CONTENT_TYPE: &str = "application/grpc-web-text";

/// Content type for binary gRPC-Web with proto subtype.
pub const GRPC_WEB_PROTO_CONTENT_TYPE: &str = "application/grpc-web+proto";

/// Encode HTTP/2 trailers into a gRPC-Web trailer frame.
///
/// gRPC-Web trailers are encoded as:
/// ```text
/// [0x80] [4-byte BE length] [header1: value1\r\n header2: value2\r\n ...]
/// ```
///
/// The 0x80 flag distinguishes trailer frames from data frames.
pub fn encode_trailers_frame(trailers: &HeaderMap) -> Bytes {
    // Serialize trailers to the wire format: "key: value\r\n" pairs.
    let mut trailer_buf = BytesMut::new();
    for (name, value) in trailers.iter() {
        trailer_buf.put_slice(name.as_str().as_bytes());
        trailer_buf.put_slice(b": ");
        trailer_buf.put_slice(value.as_bytes());
        trailer_buf.put_slice(b"\r\n");
    }

    let trailer_len = trailer_buf.len() as u32;
    let mut frame = BytesMut::with_capacity(FRAME_HEADER_SIZE + trailer_buf.len());

    // Trailer frame flag.
    frame.put_u8(TRAILER_FRAME_FLAG);
    // 4-byte big-endian length.
    frame.put_u32(trailer_len);
    // Trailer data.
    frame.put(trailer_buf);

    frame.freeze()
}

/// Decode a gRPC-Web trailer frame from raw bytes.
///
/// Returns the parsed trailers as a `HeaderMap`, or `None` if the frame
/// is not a trailer frame (bit 7 not set).
///
/// The input should be the complete frame including the 5-byte header.
pub fn decode_trailers_frame(frame: &[u8]) -> Option<HeaderMap> {
    if frame.len() < FRAME_HEADER_SIZE {
        return None;
    }

    // Check trailer flag.
    if frame[0] & TRAILER_FRAME_FLAG == 0 {
        return None;
    }

    let length = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
    if frame.len() < FRAME_HEADER_SIZE + length {
        return None;
    }

    let trailer_data = &frame[FRAME_HEADER_SIZE..FRAME_HEADER_SIZE + length];
    parse_trailer_block(trailer_data)
}

/// Returns `true` if the given frame byte indicates a trailer frame.
#[inline]
pub fn is_trailer_frame(first_byte: u8) -> bool {
    first_byte & TRAILER_FRAME_FLAG != 0
}

/// Translate a gRPC-Web content-type to the corresponding native gRPC
/// content-type for upstream forwarding.
///
/// - `application/grpc-web` -> `application/grpc`
/// - `application/grpc-web+proto` -> `application/grpc+proto`
/// - `application/grpc-web-text` -> `application/grpc`
/// - `application/grpc-web-text+proto` -> `application/grpc+proto`
pub fn translate_content_type(grpc_web_ct: &str) -> &'static str {
    if grpc_web_ct.contains("+proto") {
        "application/grpc+proto"
    } else if grpc_web_ct.contains("+json") {
        "application/grpc+json"
    } else {
        "application/grpc"
    }
}

/// Parse a trailer block (the payload after the 5-byte header) into headers.
///
/// Format: `name: value\r\n` pairs, like HTTP/1.1 headers.
fn parse_trailer_block(data: &[u8]) -> Option<HeaderMap> {
    let text = std::str::from_utf8(data).ok()?;
    let mut headers = HeaderMap::new();

    for line in text.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        let colon = line.find(':')?;
        let name = line[..colon].trim();
        let value = line[colon + 1..].trim();

        let header_name = HeaderName::from_bytes(name.as_bytes()).ok()?;
        let header_value = HeaderValue::from_str(value).ok()?;
        headers.insert(header_name, header_value);
    }

    Some(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_trailers_roundtrip() {
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", HeaderValue::from_static("0"));
        trailers.insert("grpc-message", HeaderValue::from_static("OK"));

        let frame = encode_trailers_frame(&trailers);

        // Verify trailer flag.
        assert_eq!(frame[0] & TRAILER_FRAME_FLAG, TRAILER_FRAME_FLAG);

        let decoded = decode_trailers_frame(&frame).unwrap();
        assert_eq!(decoded.get("grpc-status").unwrap(), "0");
        assert_eq!(decoded.get("grpc-message").unwrap(), "OK");
    }

    #[test]
    fn trailer_frame_detection() {
        assert!(is_trailer_frame(0x80));
        assert!(is_trailer_frame(0x81));
        assert!(!is_trailer_frame(0x00));
        assert!(!is_trailer_frame(0x01));
    }

    #[test]
    fn content_type_translation() {
        assert_eq!(
            translate_content_type("application/grpc-web"),
            "application/grpc"
        );
        assert_eq!(
            translate_content_type("application/grpc-web+proto"),
            "application/grpc+proto"
        );
        assert_eq!(
            translate_content_type("application/grpc-web-text"),
            "application/grpc"
        );
        assert_eq!(
            translate_content_type("application/grpc-web+json"),
            "application/grpc+json"
        );
    }

    #[test]
    fn decode_non_trailer_frame() {
        // Data frame (flag = 0x00).
        let frame = [0x00, 0x00, 0x00, 0x00, 0x02, 0x08, 0x01];
        assert!(decode_trailers_frame(&frame).is_none());
    }

    #[test]
    fn decode_too_short() {
        assert!(decode_trailers_frame(&[0x80, 0x00]).is_none());
    }

    #[test]
    fn empty_trailers() {
        let trailers = HeaderMap::new();
        let frame = encode_trailers_frame(&trailers);
        // Should have 5-byte header with zero length.
        assert_eq!(frame.len(), FRAME_HEADER_SIZE);
        assert_eq!(frame[0], TRAILER_FRAME_FLAG);
        assert_eq!(&frame[1..5], &[0, 0, 0, 0]);
    }
}
