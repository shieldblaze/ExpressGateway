//! HTTP/1.1 conformance tests.

use lb_h1::*;

#[test]
fn test_h1_request_line_parsing() {
    // Parse "GET /path HTTP/1.1\r\n"
    let buf = b"GET /path HTTP/1.1\r\n";
    let (method, uri, version, consumed) = parse_request_line(buf).unwrap();
    assert_eq!(method, http::Method::GET);
    assert_eq!(uri, "/path");
    assert_eq!(version, http::Version::HTTP_11);
    assert_eq!(consumed, buf.len());

    // Parse "POST /api HTTP/1.0\r\n"
    let buf = b"POST /api HTTP/1.0\r\n";
    let (method, uri, version, consumed) = parse_request_line(buf).unwrap();
    assert_eq!(method, http::Method::POST);
    assert_eq!(uri, "/api");
    assert_eq!(version, http::Version::HTTP_10);
    assert_eq!(consumed, buf.len());

    // Malformed request line returns error
    let buf = b"INVALID\r\n";
    assert!(parse_request_line(buf).is_err());

    // Incomplete input returns Incomplete
    let buf = b"GET /path HTTP/1.1";
    let err = parse_request_line(buf).unwrap_err();
    assert!(matches!(err, H1Error::Incomplete));

    // Parse status line too
    let buf = b"HTTP/1.1 200 OK\r\n";
    let (version, status, consumed) = parse_status_line(buf).unwrap();
    assert_eq!(version, http::Version::HTTP_11);
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(consumed, buf.len());

    let buf = b"HTTP/1.0 404 Not Found\r\n";
    let (version, status, _) = parse_status_line(buf).unwrap();
    assert_eq!(version, http::Version::HTTP_10);
    assert_eq!(status, http::StatusCode::NOT_FOUND);

    // Header parsing
    let buf = b"Content-Type: text/html\r\nHost: example.com\r\n\r\n";
    let (headers, consumed) = parse_headers(buf).unwrap();
    assert_eq!(consumed, buf.len());
    assert_eq!(headers.len(), 2);
    assert_eq!(headers[0].0, "Content-Type");
    assert_eq!(headers[0].1, "text/html");
    assert_eq!(headers[1].0, "Host");
    assert_eq!(headers[1].1, "example.com");
}

#[test]
fn test_h1_chunked_encoding() {
    // Encode "Hello" as a single chunk, then finish
    let mut encoder = ChunkedEncoder::new();
    let chunk = encoder.encode(b"Hello").unwrap();
    let end = encoder.finish(&[]).unwrap();

    // The chunk should be "5\r\nHello\r\n"
    assert_eq!(&chunk[..], b"5\r\nHello\r\n");
    // The end should be "0\r\n\r\n"
    assert_eq!(&end[..], b"0\r\n\r\n");

    // Decode the whole thing
    let mut full = bytes::BytesMut::new();
    full.extend_from_slice(&chunk);
    full.extend_from_slice(&end);

    let mut decoder = ChunkedDecoder::new();
    let done = decoder.feed(&full).unwrap();
    assert!(done);
    assert!(decoder.is_done());

    let body = decoder.take_body();
    let all: Vec<u8> = body.iter().flat_map(|b| b.iter().copied()).collect();
    assert_eq!(all, b"Hello");

    // Multi-chunk encoding/decoding
    let mut encoder = ChunkedEncoder::new();
    let c1 = encoder.encode(b"Hello").unwrap();
    let c2 = encoder.encode(b" ").unwrap();
    let c3 = encoder.encode(b"World").unwrap();
    let end = encoder.finish(&[]).unwrap();

    let mut full = bytes::BytesMut::new();
    full.extend_from_slice(&c1);
    full.extend_from_slice(&c2);
    full.extend_from_slice(&c3);
    full.extend_from_slice(&end);

    let mut decoder = ChunkedDecoder::new();
    let done = decoder.feed(&full).unwrap();
    assert!(done);

    let body = decoder.take_body();
    let all: Vec<u8> = body.iter().flat_map(|b| b.iter().copied()).collect();
    assert_eq!(all, b"Hello World");

    // Incremental feed (byte by byte)
    let wire = b"3\r\nabc\r\n0\r\n\r\n";
    let mut decoder = ChunkedDecoder::new();
    for &b in wire.iter().take(wire.len() - 1) {
        assert!(!decoder.feed(&[b]).unwrap());
    }
    assert!(decoder.feed(&[*wire.last().unwrap()]).unwrap());
    let body = decoder.take_body();
    let all: Vec<u8> = body.iter().flat_map(|b| b.iter().copied()).collect();
    assert_eq!(all, b"abc");
}

#[test]
fn test_h1_trailer_handling() {
    // Create chunked body with trailers
    let mut encoder = ChunkedEncoder::new();
    let chunk = encoder.encode(b"payload").unwrap();
    let trailers = vec![
        ("Checksum".to_string(), "sha256-abc".to_string()),
        ("X-Trace-Id".to_string(), "12345".to_string()),
    ];
    let end = encoder.finish(&trailers).unwrap();

    let mut full = bytes::BytesMut::new();
    full.extend_from_slice(&chunk);
    full.extend_from_slice(&end);

    let mut decoder = ChunkedDecoder::new();
    let done = decoder.feed(&full).unwrap();
    assert!(done);

    // Verify body
    let body = decoder.take_body();
    let all: Vec<u8> = body.iter().flat_map(|b| b.iter().copied()).collect();
    assert_eq!(all, b"payload");

    // Verify trailers
    let t = decoder.trailers();
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].0, "Checksum");
    assert_eq!(t[0].1, "sha256-abc");
    assert_eq!(t[1].0, "X-Trace-Id");
    assert_eq!(t[1].1, "12345");

    // Also test parse_trailers directly
    let trailer_buf = b"Foo: bar\r\nBaz: qux\r\n\r\n";
    let (parsed, consumed) = parse_trailers(trailer_buf).unwrap();
    assert_eq!(consumed, trailer_buf.len());
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].0, "Foo");
    assert_eq!(parsed[0].1, "bar");
    assert_eq!(parsed[1].0, "Baz");
    assert_eq!(parsed[1].1, "qux");
}
