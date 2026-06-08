#![no_main]

use libfuzzer_sys::fuzz_target;

// Drive arbitrary bytes through the hand-rolled HPACK decoder
// (lb_h2::HpackDecoder, RFC 7541). Targets: the variable-length integer
// prefix decoder (multi-byte continuation / overflow class), Huffman vs
// raw string literals, dynamic-table insert/evict (reference to evicted
// entry), and the HpackBombDetector size bound. A panic/abort/OOM is the
// finding; an Err return (malformed block) is expected and fine.
//
// 4096 is the RFC 7541 default SETTINGS_HEADER_TABLE_SIZE; the decoder
// must keep its dynamic table within this regardless of attacker input.
fuzz_target!(|data: &[u8]| {
    let mut dec = lb_h2::HpackDecoder::new(4096);
    let _ = dec.decode(data);
    // A second decode on the SAME decoder exercises dynamic-table state
    // carried across header blocks (an attacker controls both blocks on
    // one connection).
    let _ = dec.decode(data);
});
