//! CODE-2-11 — HPACK round-trip + decode_frame no-panic harness.
//!
//! Two invariants:
//!
//! 1. HPACK encode→decode is the identity over the generated
//!    `Vec<(HeaderName, HeaderValue)>` (string-equivalence on lowercased
//!    names per RFC 7541 §6.2.1).
//! 2. `decode_frame` on a random `Vec<u8>` never panics — it returns
//!    `Result` for any input. This is the catch-unwind safety net the
//!    round-2 review §CODE-2-11 calls out for the smuggling-risk
//!    parsers.
//!
//! Sanity budget; CI raises to 200 000 cases via PROPTEST_CASES env.

#![cfg(feature = "proptest")]

use proptest::collection::vec;
use proptest::prelude::*;

use lb_h2::{HpackDecoder, HpackEncoder, decode_frame};

/// Lowercase ASCII identifier for header names (HTTP/2 spec).
fn arb_header_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,32}".prop_map(String::from)
}

/// Printable-ASCII header values.
fn arb_header_value() -> impl Strategy<Value = String> {
    "[ -~]{0,128}".prop_map(String::from)
}

fn arb_headers() -> impl Strategy<Value = Vec<(String, String)>> {
    vec((arb_header_name(), arb_header_value()), 0..16)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_global_rejects: 1024,
        .. ProptestConfig::default()
    })]

    /// HPACK encode→decode is the identity on the lower-cased name set.
    #[test]
    fn hpack_round_trip(headers in arb_headers()) {
        let mut enc = HpackEncoder::new(4096);
        let encoded = enc.encode(&headers).expect("encode HPACK");
        let mut dec = HpackDecoder::new(4096);
        let decoded = dec.decode(&encoded).expect("decode HPACK");

        // Names normalise to lowercase per RFC 7541; values are byte-
        // identical. We compare against the input directly since
        // arb_header_name() already produces lowercase.
        prop_assert_eq!(decoded.len(), headers.len());
        for (a, b) in decoded.iter().zip(headers.iter()) {
            prop_assert_eq!(&a.0, &b.0);
            prop_assert_eq!(&a.1, &b.1);
        }
    }

    /// decode_frame never panics — only returns Result.
    #[test]
    fn decode_frame_no_panic(buf in vec(any::<u8>(), 0..2048)) {
        let res = std::panic::catch_unwind(|| decode_frame(&buf, 16_384));
        prop_assert!(res.is_ok(), "decode_frame panicked on random input");
    }
}
