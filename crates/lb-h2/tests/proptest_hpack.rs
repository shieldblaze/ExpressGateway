//! CODE-2-11 — HPACK encode→decode identity, and `decode_frame` never panics
//! on random bytes. Sanity budget; CI raises it via `PROPTEST_CASES`.

#![cfg(feature = "proptest")]

use proptest::collection::vec;
use proptest::prelude::*;

use lb_h2::{HpackDecoder, HpackEncoder, decode_frame};

fn arb_header_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,32}".prop_map(String::from)
}

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

    #[test]
    fn hpack_round_trip(headers in arb_headers()) {
        let mut enc = HpackEncoder::new(4096);
        let encoded = enc.encode(&headers).expect("encode HPACK");
        let mut dec = HpackDecoder::new(4096);
        let decoded = dec.decode(&encoded).expect("decode HPACK");

        prop_assert_eq!(decoded.len(), headers.len());
        for (a, b) in decoded.iter().zip(headers.iter()) {
            prop_assert_eq!(&a.0, &b.0);
            prop_assert_eq!(&a.1, &b.1);
        }
    }

    #[test]
    fn decode_frame_no_panic(buf in vec(any::<u8>(), 0..2048)) {
        let res = std::panic::catch_unwind(|| decode_frame(&buf, 16_384));
        prop_assert!(res.is_ok(), "decode_frame panicked on random input");
    }
}
