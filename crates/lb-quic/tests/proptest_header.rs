//! CODE-2-11 — QUIC header parse no-panic harness. Single invariant:
//! `quiche::Header::from_slice` never panics on any random byte slice up to
//! MAX_UDP. The router calls it on EVERY inbound datagram before any further
//! validation, so a panic in the header decoder would brick the router — hence
//! the catch-unwind safety net.
//!
//! The `#![cfg(feature = "proptest")]` gate was removed so this sanity net runs
//! under the default `cargo test -p lb-quic` instead of being silent dead
//! coverage; `proptest` is an unconditional dev-dependency. CI still scales the
//! budget via `PROPTEST_CASES`, which proptest reads at runtime.

use proptest::collection::vec;
use proptest::prelude::*;

use quiche::Header;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_global_rejects: 1024,
        .. ProptestConfig::default()
    })]

    #[test]
    fn header_from_slice_no_panic(buf in vec(any::<u8>(), 0..1500)) {
        let res = std::panic::catch_unwind(|| {
            let mut owned = buf.clone();
            Header::from_slice(&mut owned, quiche::MAX_CONN_ID_LEN).map(|_| ())
        });
        prop_assert!(res.is_ok(), "Header::from_slice panicked on random bytes");
    }
}
