//! HTTP/3 conformance tests.

use bytes::Bytes;
use lb_h3::*;

#[test]
fn test_h3_frame_decoder() {
    // DATA frame roundtrip
    let data_frame = H3Frame::Data {
        payload: Bytes::from_static(b"hello h3"),
    };
    let encoded = encode_frame(&data_frame).unwrap();
    let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded, data_frame);

    // HEADERS frame roundtrip
    let headers_frame = H3Frame::Headers {
        header_block: Bytes::from_static(b"\x00\x00\xd1\xd7"),
    };
    let encoded = encode_frame(&headers_frame).unwrap();
    let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded, headers_frame);

    // SETTINGS frame roundtrip
    let settings_frame = H3Frame::Settings {
        params: vec![(0x06, 4096), (0x01, 100)],
    };
    let encoded = encode_frame(&settings_frame).unwrap();
    let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded, settings_frame);

    // GOAWAY frame roundtrip
    let goaway_frame = H3Frame::GoAway { stream_id: 42 };
    let encoded = encode_frame(&goaway_frame).unwrap();
    let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded, goaway_frame);

    // CANCEL_PUSH frame roundtrip
    let cancel_frame = H3Frame::CancelPush { push_id: 7 };
    let encoded = encode_frame(&cancel_frame).unwrap();
    let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded, cancel_frame);

    // MAX_PUSH_ID frame roundtrip
    let max_push_frame = H3Frame::MaxPushId { push_id: 255 };
    let encoded = encode_frame(&max_push_frame).unwrap();
    let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_PAYLOAD_SIZE).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded, max_push_frame);

    // Variable-length integer encoding/decoding
    let mut buf = bytes::BytesMut::new();

    // 1-byte: value < 64
    let written = encode_varint(&mut buf, 37).unwrap();
    assert_eq!(written, 1);
    let (val, consumed) = decode_varint(&buf).unwrap();
    assert_eq!(val, 37);
    assert_eq!(consumed, 1);

    // 2-byte: 64 <= value < 16384
    buf.clear();
    let written = encode_varint(&mut buf, 15293).unwrap();
    assert_eq!(written, 2);
    let (val, consumed) = decode_varint(&buf).unwrap();
    assert_eq!(val, 15293);
    assert_eq!(consumed, 2);

    // 4-byte: 16384 <= value < 2^30
    buf.clear();
    let written = encode_varint(&mut buf, 494_878).unwrap();
    assert_eq!(written, 4);
    let (val, consumed) = decode_varint(&buf).unwrap();
    assert_eq!(val, 494_878);
    assert_eq!(consumed, 4);

    // 8-byte: >= 2^30
    buf.clear();
    let written = encode_varint(&mut buf, 2_000_000_000).unwrap();
    assert_eq!(written, 8);
    let (val, consumed) = decode_varint(&buf).unwrap();
    assert_eq!(val, 2_000_000_000);
    assert_eq!(consumed, 8);

    // Incomplete input
    assert!(matches!(
        decode_frame(&[], DEFAULT_MAX_PAYLOAD_SIZE),
        Err(H3Error::Incomplete)
    ));
}

#[test]
fn test_h3_qpack_roundtrip() {
    let encoder = QpackEncoder::new();
    let decoder = QpackDecoder::new();

    // Test with static table entries
    let headers = vec![
        (":method".to_string(), "GET".to_string()),
        (":path".to_string(), "/".to_string()),
        (":scheme".to_string(), "https".to_string()),
        (":status".to_string(), "200".to_string()),
    ];

    let encoded = encoder.encode(&headers).unwrap();
    let decoded = decoder.decode(&encoded).unwrap();
    assert_eq!(decoded, headers);

    // Test with mixed static + literal
    let headers = vec![
        (":method".to_string(), "POST".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        ("x-custom-header".to_string(), "custom-value".to_string()),
    ];

    let encoded = encoder.encode(&headers).unwrap();
    let decoded = decoder.decode(&encoded).unwrap();
    assert_eq!(decoded, headers);

    // Test name-reference with different value
    let headers = vec![
        ("content-type".to_string(), "application/xml".to_string()),
        ("accept-encoding".to_string(), "zstd".to_string()),
    ];

    let encoded = encoder.encode(&headers).unwrap();
    let decoded = decoder.decode(&encoded).unwrap();
    assert_eq!(decoded, headers);
}

#[test]
fn test_h3_qpack_bomb_mitigation() {
    let detector = QpackBombDetector::new(100, 65536);

    // Normal case: ratio well within limits
    assert!(detector.check(1000, 2000).is_ok());

    // Bomb by ratio: 1KB encoded -> 200KB decoded = ratio 200
    let result = detector.check(1024, 204_800);
    assert!(result.is_err());

    // Bomb by absolute size: exceeds 64KB regardless of ratio
    let result = detector.check(50_000, 100_000);
    assert!(result.is_err());

    // Zero encoded bytes with non-zero decoded
    let result = detector.check(0, 100_000);
    assert!(result.is_err());

    // Exact boundary: 64KB decoded is within limit
    assert!(detector.check(1000, 65536).is_ok());

    // One byte over
    assert!(detector.check(1000, 65537).is_err());
}
