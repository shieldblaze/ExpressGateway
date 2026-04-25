//! HTTP/2 conformance tests.

use bytes::Bytes;
use lb_h2::*;

#[test]
fn test_h2_frame_decoder() {
    // DATA frame roundtrip
    let data_frame = H2Frame::Data {
        stream_id: 1,
        payload: Bytes::from_static(b"hello world"),
        end_stream: false,
    };
    let encoded = encode_frame(&data_frame).unwrap();
    let (decoded, consumed) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded, data_frame);

    // DATA frame with end_stream
    let data_frame_end = H2Frame::Data {
        stream_id: 3,
        payload: Bytes::from_static(b"fin"),
        end_stream: true,
    };
    let encoded = encode_frame(&data_frame_end).unwrap();
    let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(decoded, data_frame_end);

    // HEADERS frame
    let headers_frame = H2Frame::Headers {
        stream_id: 1,
        header_block: Bytes::from_static(b"\x82\x84\x87"),
        end_stream: false,
        end_headers: true,
    };
    let encoded = encode_frame(&headers_frame).unwrap();
    let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(decoded, headers_frame);

    // SETTINGS frame with parameters
    let settings_frame = H2Frame::Settings {
        ack: false,
        params: vec![(0x03, 100), (0x04, 65535)],
    };
    let encoded = encode_frame(&settings_frame).unwrap();
    let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(decoded, settings_frame);

    // SETTINGS ACK
    let settings_ack = H2Frame::Settings {
        ack: true,
        params: vec![],
    };
    let encoded = encode_frame(&settings_ack).unwrap();
    let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(decoded, settings_ack);

    // PING frame
    let ping_frame = H2Frame::Ping {
        ack: false,
        data: [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
    };
    let encoded = encode_frame(&ping_frame).unwrap();
    let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(decoded, ping_frame);

    // GOAWAY frame
    let goaway_frame = H2Frame::GoAway {
        last_stream_id: 7,
        error_code: 0x0,
        debug_data: Bytes::from_static(b"graceful"),
    };
    let encoded = encode_frame(&goaway_frame).unwrap();
    let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(decoded, goaway_frame);

    // RST_STREAM frame
    let rst_frame = H2Frame::RstStream {
        stream_id: 5,
        error_code: 0x02,
    };
    let encoded = encode_frame(&rst_frame).unwrap();
    let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(decoded, rst_frame);

    // WINDOW_UPDATE frame
    let wup_frame = H2Frame::WindowUpdate {
        stream_id: 0,
        increment: 65535,
    };
    let encoded = encode_frame(&wup_frame).unwrap();
    let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(decoded, wup_frame);

    // CONTINUATION frame
    let cont_frame = H2Frame::Continuation {
        stream_id: 1,
        header_block: Bytes::from_static(b"\x82"),
        end_headers: true,
    };
    let encoded = encode_frame(&cont_frame).unwrap();
    let (decoded, _) = decode_frame(&encoded, DEFAULT_MAX_FRAME_SIZE).unwrap();
    assert_eq!(decoded, cont_frame);

    // Incomplete input
    assert!(matches!(
        decode_frame(&[0; 5], DEFAULT_MAX_FRAME_SIZE),
        Err(H2Error::Incomplete)
    ));
}

#[test]
fn test_h2_hpack_roundtrip() {
    // Basic static-table headers
    let headers = vec![
        (":method".to_string(), "GET".to_string()),
        (":path".to_string(), "/".to_string()),
        ("content-type".to_string(), "text/html".to_string()),
    ];

    let mut encoder = HpackEncoder::new(4096);
    let encoded = encoder.encode(&headers).unwrap();

    let mut decoder = HpackDecoder::new(4096);
    let decoded = decoder.decode(&encoded).unwrap();
    assert_eq!(decoded, headers);

    // Multiple encode/decode cycles — dynamic table state accumulates
    let headers2 = vec![
        (":method".to_string(), "POST".to_string()),
        (":path".to_string(), "/api/v1".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        ("x-request-id".to_string(), "abc-123".to_string()),
    ];

    let encoded2 = encoder.encode(&headers2).unwrap();
    let decoded2 = decoder.decode(&encoded2).unwrap();
    assert_eq!(decoded2, headers2);

    // Third cycle — some headers from cycle 2 should be in the dynamic table
    let headers3 = vec![
        (":method".to_string(), "GET".to_string()),
        (":path".to_string(), "/api/v1".to_string()),
        ("x-request-id".to_string(), "def-456".to_string()),
    ];

    let encoded3 = encoder.encode(&headers3).unwrap();
    let decoded3 = decoder.decode(&encoded3).unwrap();
    assert_eq!(decoded3, headers3);

    // Fully-indexed static table entries compress to 1 byte each
    let fully_static = vec![
        (":method".to_string(), "GET".to_string()),
        (":path".to_string(), "/".to_string()),
        (":scheme".to_string(), "https".to_string()),
    ];
    let mut enc2 = HpackEncoder::new(4096);
    let encoded_static = enc2.encode(&fully_static).unwrap();
    // Each is a single indexed byte: 0x82, 0x84, 0x87
    assert_eq!(encoded_static.len(), 3);
}

#[test]
fn test_h2_rapid_reset_mitigation() {
    // Threshold = 10, window = 1000 ticks
    let mut detector = RapidResetDetector::new(10, 1000);

    // Feed 10 events — should be OK (threshold is > not >=)
    for i in 0..10 {
        assert!(
            detector.record(i).is_ok(),
            "event {i} should be within threshold"
        );
    }

    // 11th event triggers detection
    let result = detector.record(10);
    assert!(result.is_err(), "11th event should exceed threshold");

    // After window expires, events are evicted
    detector.reset();
    for i in 0..5 {
        assert!(detector.record(i).is_ok());
    }
    // These 5 are within threshold
    assert!(detector.record(5).is_ok());
}

#[test]
fn test_h2_continuation_flood_mitigation() {
    let mut detector = ContinuationFloodDetector::new(10);

    // Feed 10 CONTINUATION frames without END_HEADERS — should be OK
    for _ in 0..10 {
        assert!(detector.record(false).is_ok());
    }

    // 11th triggers detection
    let result = detector.record(false);
    assert!(result.is_err());

    // Reset via END_HEADERS
    detector.reset();
    for _ in 0..5 {
        assert!(detector.record(false).is_ok());
    }
    // END_HEADERS resets the counter
    assert!(detector.record(true).is_ok());
    // Can do 10 more
    for _ in 0..10 {
        assert!(detector.record(false).is_ok());
    }
    assert!(detector.record(false).is_err());
}

#[test]
fn test_h2_hpack_bomb_mitigation() {
    // max_ratio=100, max_size=64KB
    let detector = HpackBombDetector::new(100, 65536);

    // Normal: 1KB encoded, 2KB decoded — ratio 2, under limits
    assert!(detector.check(1024, 2048).is_ok());

    // Bomb by ratio: 1KB encoded, 10MB decoded — ratio ~10000
    let result = detector.check(1024, 10 * 1024 * 1024);
    assert!(result.is_err());

    // Bomb by absolute size: 100KB encoded, 100KB decoded — ratio 1, but > 64KB
    let result = detector.check(100_000, 100_000);
    assert!(result.is_err());

    // Edge case: 0 encoded bytes
    let result = detector.check(0, 100_000);
    assert!(result.is_err());

    // Small normal case
    assert!(detector.check(100, 1000).is_ok());
}
