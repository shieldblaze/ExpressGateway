#![no_main]

use libfuzzer_sys::fuzz_target;

// Drive arbitrary bytes into the HTTP/1.x header parser. Any Err return
// is acceptable; the only real finding is a panic or abort inside the
// parser, which libFuzzer surfaces as a crash + reproducer.
fuzz_target!(|data: &[u8]| {
    let _ = lb_h1::parse_headers_with_limit(data, lb_h1::MAX_HEADER_BYTES);
});
