#![no_main]

use libfuzzer_sys::fuzz_target;

// Drive arbitrary bytes into the HTTP/3 frame decoder. Uses the crate's
// DEFAULT_MAX_PAYLOAD_SIZE (1 MiB) limit so the varint length fields
// cannot force the decoder into an unreasonable allocation.
fuzz_target!(|data: &[u8]| {
    let _ = lb_h3_testcodec::decode_frame(data, lb_h3_testcodec::DEFAULT_MAX_PAYLOAD_SIZE);
});
