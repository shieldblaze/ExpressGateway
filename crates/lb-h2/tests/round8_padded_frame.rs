//! ROUND8-L7-11 — DATA / HEADERS PADDED flag handling.
//!
//! Reference: HAProxy `BUG/MEDIUM: mux-h2: Properly consume padding
//! for DATA frames` — without consuming the trailing padding the
//! decoder parses the next frame out of phase, creating a smuggling
//! primitive. RFC 9113 §6.1 (DATA), §6.2 (HEADERS).
//!
//! Status caveat: `lb-h2::frame::decode_frame` is test-only today
//! (hot path uses hyper). This is the "lesson-not-yet-paid-for" find:
//! the bug is fixed before live wire-in.

use lb_h2::{DEFAULT_MAX_FRAME_SIZE, H2Frame, decode_frame};

fn make_frame(frame_type: u8, flags: u8, stream_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(9 + payload.len());
    let len = payload.len();
    // 24-bit length.
    buf.push(((len >> 16) & 0xFF) as u8);
    buf.push(((len >> 8) & 0xFF) as u8);
    buf.push((len & 0xFF) as u8);
    buf.push(frame_type);
    buf.push(flags);
    buf.push(((stream_id >> 24) & 0xFF) as u8);
    buf.push(((stream_id >> 16) & 0xFF) as u8);
    buf.push(((stream_id >> 8) & 0xFF) as u8);
    buf.push((stream_id & 0xFF) as u8);
    buf.extend_from_slice(payload);
    buf
}

const FRAME_DATA: u8 = 0x0;
const FRAME_HEADERS: u8 = 0x1;
const FLAG_PADDED: u8 = 0x8;
const FLAG_PRIORITY: u8 = 0x20;
const FLAG_END_HEADERS: u8 = 0x4;

#[test]
fn data_with_padded_strips_padding() {
    // pad_len=1, body="hi!", 1 byte padding.
    // payload layout: [pad_len, b'h', b'i', b'!', 0]
    let payload = [1u8, b'h', b'i', b'!', 0];
    let buf = make_frame(FRAME_DATA, FLAG_PADDED, 1, &payload);
    let (frame, consumed) = decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(consumed, buf.len());
    match frame {
        H2Frame::Data {
            stream_id, payload, ..
        } => {
            assert_eq!(stream_id, 1);
            assert_eq!(&payload[..], b"hi!");
        }
        other => panic!("expected DATA frame, got {other:?}"),
    }
}

#[test]
fn data_pad_length_exceeds_payload_errors() {
    // pad_len=10, payload has only 3 bytes after pad_len byte.
    let payload = [10u8, b'a', b'b', b'c'];
    let buf = make_frame(FRAME_DATA, FLAG_PADDED, 1, &payload);
    let r = decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE);
    assert!(r.is_err(), "expected error, got {r:?}");
}

#[test]
fn data_padded_empty_payload_errors() {
    let buf = make_frame(FRAME_DATA, FLAG_PADDED, 1, &[]);
    let r = decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE);
    assert!(r.is_err(), "expected error, got {r:?}");
}

#[test]
fn headers_with_padded_and_priority_decoded_correctly() {
    // payload: pad_len(1) | E+Dep+Weight(5) | block(3) | padding(2)
    // Total: 1 + 5 + 3 + 2 = 11. pad_len = 2.
    let mut payload = Vec::new();
    payload.push(2u8); // pad len
    payload.extend_from_slice(&[0, 0, 0, 0, 0]); // priority block
    payload.extend_from_slice(b"abc"); // header block
    payload.extend_from_slice(&[0, 0]); // padding
    let buf = make_frame(
        FRAME_HEADERS,
        FLAG_PADDED | FLAG_PRIORITY | FLAG_END_HEADERS,
        3,
        &payload,
    );
    let (frame, _) = decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE).unwrap();
    match frame {
        H2Frame::Headers {
            stream_id,
            header_block,
            ..
        } => {
            assert_eq!(stream_id, 3);
            assert_eq!(&header_block[..], b"abc");
        }
        other => panic!("expected HEADERS frame, got {other:?}"),
    }
}

#[test]
fn data_without_padded_unaffected() {
    let buf = make_frame(FRAME_DATA, 0, 1, b"hello");
    let (frame, _) = decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE).unwrap();
    match frame {
        H2Frame::Data { payload, .. } => assert_eq!(&payload[..], b"hello"),
        other => panic!("expected DATA, got {other:?}"),
    }
}

#[test]
fn headers_without_padded_unaffected() {
    let buf = make_frame(FRAME_HEADERS, FLAG_END_HEADERS, 1, b"hpack-block");
    let (frame, _) = decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE).unwrap();
    match frame {
        H2Frame::Headers { header_block, .. } => {
            assert_eq!(&header_block[..], b"hpack-block");
        }
        other => panic!("expected HEADERS, got {other:?}"),
    }
}
