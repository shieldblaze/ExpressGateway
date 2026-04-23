#![no_main]

use libfuzzer_sys::fuzz_target;

// Drive arbitrary bytes into the HTTP/2 frame decoder. Bound payload
// size to the spec default (16 KiB) so the fuzzer spends time on frame
// structure rather than on allocation storms.
fuzz_target!(|data: &[u8]| {
    let _ = lb_h2::decode_frame(data, lb_h2::DEFAULT_MAX_FRAME_SIZE);
});
