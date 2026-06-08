#![no_main]

use libfuzzer_sys::fuzz_target;

// Drive arbitrary bytes through the HTTP/1.1 chunked-transfer decoder
// (lb_h1::ChunkedDecoder). This targets the chunk-size hex parser
// (parse_chunk_size_hex — CVE-2013-2028 leading-zero / overflow class),
// the chunk-extension skip, and the trailer parse, plus the incremental
// state machine across multiple feed() calls. Any Err is acceptable; a
// panic/abort/OOM is the finding. The first byte chooses a split offset
// so the fuzzer also exercises mid-frame resumption (state carried across
// feeds), which a single whole-buffer feed would not reach.
fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    // Whole-buffer feed.
    let mut whole = lb_h1::ChunkedDecoder::new();
    let _ = whole.feed(data);
    let _ = whole.take_body();
    let _ = whole.trailers();

    // Split feed: exercise the resumable state machine across a boundary
    // an attacker controls (partial chunk-size line, split CRLF, etc.).
    let split = (data[0] as usize) % data.len();
    let (a, b) = data.split_at(split);
    let mut inc = lb_h1::ChunkedDecoder::new();
    if inc.feed(a).is_ok() {
        let _ = inc.feed(b);
        let _ = inc.take_body();
        let _ = inc.trailers();
    }
});
