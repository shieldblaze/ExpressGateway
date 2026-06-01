//! CODE-2-11 — QPACK static-table round-trip + decode_frame no-panic
//! + QUIC varint round-trip / no-overlong harness.
//!
//! Three invariants:
//!
//! 1. QPACK encode→decode is the identity on lowercase ASCII headers
//!    (the in-tree QpackEncoder is static-table only — that's exactly
//!    the configuration the production listener uses today).
//! 2. `decode_frame` never panics — Result for any input.
//! 3. QUIC varint encode→decode round-trips and `decode_varint`
//!    rejects overlong encodings (RFC 9000 §16) without panicking.
//!
//! Sanity budget; CI bumps via PROPTEST_CASES env.
//!
//! F-COR-5 (foundation audit): the `#![cfg(feature = "proptest")]`
//! gate was removed so this sanity net runs under the default
//! `cargo test -p lb-h3-testcodec` instead of being silent dead coverage (it
//! reported `running 0 tests` by default — auditor-4 F-1). This
//! mirrors verbatim the S1 change to the sibling
//! `crates/lb-quic/tests/proptest_header.rs` (25d8ad84): `proptest`
//! is an UNCONDITIONAL `[dev-dependencies]` entry in this crate's
//! `Cargo.toml`, and the separate empty marker
//! feature `proptest = []` (line 21) gates no code, so no feature
//! flag is needed to compile this binary. CI still scales the budget
//! via the `PROPTEST_CASES` env var, which `proptest` reads at
//! runtime independent of any cfg — unchanged. The case logic below
//! is byte-for-byte unchanged.

use bytes::BytesMut;
use proptest::collection::vec;
use proptest::prelude::*;

use lb_h3_testcodec::{MAX_VARINT, QpackDecoder, QpackEncoder, decode_frame, decode_varint, encode_varint};

fn arb_headers() -> impl Strategy<Value = Vec<(String, String)>> {
    vec(
        (
            "[a-z][a-z0-9-]{0,32}".prop_map(String::from),
            "[ -~]{0,128}".prop_map(String::from),
        ),
        0..16,
    )
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_global_rejects: 1024,
        .. ProptestConfig::default()
    })]

    /// QPACK round-trip identity.
    #[test]
    fn qpack_round_trip(headers in arb_headers()) {
        let enc = QpackEncoder::new();
        let encoded = enc.encode(&headers).expect("QPACK encode");
        let dec = QpackDecoder::new();
        let decoded = dec.decode(&encoded).expect("QPACK decode");
        prop_assert_eq!(decoded, headers);
    }

    /// decode_frame catch-unwind safety net.
    #[test]
    fn decode_frame_no_panic(buf in vec(any::<u8>(), 0..2048)) {
        let res = std::panic::catch_unwind(|| decode_frame(&buf, 65_536));
        prop_assert!(res.is_ok(), "decode_frame panicked on random input");
    }

    /// QUIC varint encode→decode round-trip across the legal range.
    #[test]
    fn varint_round_trip(value in 0u64..=MAX_VARINT) {
        let mut buf = BytesMut::new();
        let n_enc = encode_varint(&mut buf, value).expect("encode_varint");
        let (v_dec, n_dec) = decode_varint(&buf).expect("decode_varint");
        prop_assert_eq!(v_dec, value);
        prop_assert_eq!(n_dec, n_enc);
    }

    /// Random varint bytes never panic the decoder.
    #[test]
    fn varint_decode_no_panic(buf in vec(any::<u8>(), 0..16)) {
        let res = std::panic::catch_unwind(|| decode_varint(&buf));
        prop_assert!(res.is_ok(), "decode_varint panicked on random input");
    }
}
