//! ROUND8-L7-14 — named H2 CVE-class regression corpus.
//!
//! Each test cites the CVE / GHSA / bug-class it pins. Failure of
//! any of them is a regression of a fix we already paid for.

use lb_h2::{DEFAULT_MAX_FRAME_SIZE, H2Frame, decode_frame};

fn make_frame(frame_type: u8, flags: u8, stream_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(9 + payload.len());
    let len = payload.len();
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
const FLAG_PADDED: u8 = 0x8;

/// ROUND8-L7-11 / HAProxy `Properly consume padding` — DATA with
/// PADDED must strip both pad-length byte and trailing pad bytes.
#[test]
fn data_padded_strips_padding() {
    let payload = [3u8, b'a', b'b', b'c', 0, 0, 0];
    let buf = make_frame(FRAME_DATA, FLAG_PADDED, 1, &payload);
    let (frame, _) = decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE).unwrap();
    match frame {
        H2Frame::Data { payload, .. } => assert_eq!(&payload[..], b"abc"),
        other => panic!("expected DATA, got {other:?}"),
    }
}

/// ROUND8-L7-11 — pad_len > payload.len() - 1 must error.
#[test]
fn data_padded_pad_length_exceeds_payload_errors() {
    let payload = [200u8, b'a', b'b'];
    let buf = make_frame(FRAME_DATA, FLAG_PADDED, 1, &payload);
    assert!(decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE).is_err());
}

/// ROUND8-L7-11 — PADDED with zero-byte payload errors.
#[test]
fn data_padded_empty_payload_errors() {
    let buf = make_frame(FRAME_DATA, FLAG_PADDED, 1, &[]);
    assert!(decode_frame(&buf, DEFAULT_MAX_FRAME_SIZE).is_err());
}
