#![no_main]

use libfuzzer_sys::fuzz_target;

// Exercise quiche's public packet-header parser with arbitrary bytes.
// This is the same entry point `lb_quic::router` uses to demultiplex
// datagrams (see crates/lb-quic/src/router.rs:157). We stop at header
// parse — spinning up a full `quiche::Connection` would blow past the
// fuzzer's per-input budget and muddy any crash that *is* in the
// header parser.
fuzz_target!(|data: &[u8]| {
    let _ = quiche::Header::from_slice(
        &mut data.to_vec(),
        quiche::MAX_CONN_ID_LEN,
    );
});
