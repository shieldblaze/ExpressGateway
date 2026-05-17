//! CODE-2-11 — QUIC long / short header parse no-panic harness.
//!
//! Single invariant: `quiche::Header::from_slice` never panics on any
//! random byte slice up to MAX_UDP. The proxy's router calls
//! `Header::from_slice` on every inbound datagram before any further
//! validation — a panic in BoringSSL's header decoder would brick the
//! router, so we keep a catch-unwind safety net in CI.
//!
//! Sanity budget; CI raises to 100 000 cases via PROPTEST_CASES env.
//!
//! S1-A (2026-05-16, task B.1): the `#![cfg(feature = "proptest")]`
//! gate was removed so this 256-case sanity net runs under the default
//! `cargo test -p lb-quic` instead of being silent dead coverage (it
//! never ran by default — flagged in
//! `audit/h3-program/s1-inventory.md`). `proptest` is an
//! UNCONDITIONAL `[dev-dependencies]` entry in
//! `crates/lb-quic/Cargo.toml`, so no feature flag is needed to
//! compile this binary. CI still scales the budget to 100 000 cases
//! via the `PROPTEST_CASES` env var, which `proptest` reads at runtime
//! independent of any cfg — that behaviour is unchanged. The case
//! logic below is byte-for-byte the original 256-case sanity budget.

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
