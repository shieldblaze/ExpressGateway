#![no_main]

use libfuzzer_sys::fuzz_target;

// Drive arbitrary bytes through OUR hand-rolled QUIC public-header parser
// (lb_quic::public_header::parse_public_header). This is the parser that
// runs on EVERY inbound datagram on the Mode A passthrough datapath
// (internet-facing, no decryption) — see crates/lb-quic/src/router.rs.
//
// The pre-existing `quic_initial` target fuzzes quiche's parser, NOT this
// one, so this hand-rolled parser (which the no-decrypt security property
// is layered on top of) had no coverage-guided fuzz target. The parser
// claims "never panics on arbitrary input"; this proves it under libFuzzer.
//
// Short headers need a caller-supplied DCID length (recovered out-of-band
// per the design doc). We derive it from the first byte so the fuzzer
// explores the full 0..=20 (and beyond, to test the bound) range.
fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    // Spread short_dcid_len across 0..=24 — includes valid (0..=20) and
    // out-of-range (21..=24) to confirm the parser tolerates a bad
    // caller-supplied length without panicking.
    let short_dcid_len = (data[0] as usize) % 25;
    let pkt = &data[1..];
    let _ = lb_quic::public_header::parse_public_header(pkt, short_dcid_len);
});
