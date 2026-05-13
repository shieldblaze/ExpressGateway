//! CODE-2-11 — QUIC long / short header parse no-panic harness.
//!
//! Single invariant: `quiche::Header::from_slice` never panics on any
//! random byte slice up to MAX_UDP. The proxy's router calls
//! `Header::from_slice` on every inbound datagram before any further
//! validation — a panic in BoringSSL's header decoder would brick the
//! router, so we keep a catch-unwind safety net in CI.
//!
//! Sanity budget; CI raises to 100 000 cases via PROPTEST_CASES env.

#![cfg(feature = "proptest")]

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
            // Use a buf clone since from_slice borrows.
            let mut owned = buf.clone();
            Header::from_slice(&mut owned, quiche::MAX_CONN_ID_LEN).map(|_| ())
        });
        prop_assert!(res.is_ok(), "Header::from_slice panicked on random bytes");
    }
}
