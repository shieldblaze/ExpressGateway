#![no_main]

use libfuzzer_sys::fuzz_target;

// Drive arbitrary bytes through the HTTP/1.1 request-line parser
// (lb_h1::parse_request_line) and, on success, continue into the header
// parser at the reported offset — mirroring how a real connection parses
// a request head. The existing `h1_parser` target only fed the header
// parser; the request line (method / URI via http::Uri::parse / version)
// was a coverage gap. A panic/abort is the finding; Err is expected.
fuzz_target!(|data: &[u8]| {
    if let Ok((_method, _uri, _version, consumed)) = lb_h1::parse_request_line(data) {
        if let Some(rest) = data.get(consumed..) {
            let _ = lb_h1::parse_headers_with_limit(rest, lb_h1::MAX_HEADER_BYTES);
        }
    }
});
