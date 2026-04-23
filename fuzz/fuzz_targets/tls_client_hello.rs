#![no_main]

use libfuzzer_sys::fuzz_target;
use rustls::server::Acceptor;
use std::io::Cursor;

// Feed arbitrary bytes through rustls' `Acceptor` — the same surface
// we use on the TLS-over-TCP listener. `read_tls` + `accept` together
// cover ClientHello parsing, including SNI extraction. Any Err is fine;
// only a panic/abort inside rustls counts as a finding.
fuzz_target!(|data: &[u8]| {
    let mut acceptor = Acceptor::default();
    let mut cursor = Cursor::new(data);
    // `read_tls` may return Err if the buffer is too short or malformed;
    // we intentionally ignore it and let `accept` try to make progress
    // on whatever bytes were consumed.
    let _ = acceptor.read_tls(&mut cursor);
    let _ = acceptor.accept();
});
