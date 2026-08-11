//! CODE-2-11 — two invariants over the lb-h1 parsers: they never panic (hence
//! the `catch_unwind`), and a returned consumed-byte count never exceeds the
//! buffer length. Ships at the proptest default; CI bumps `PROPTEST_CASES`.

#![cfg(feature = "proptest")]

use proptest::collection::vec;
use proptest::prelude::*;

use lb_h1::{parse_headers, parse_request_line};

fn arb_method() -> impl Strategy<Value = &'static [u8]> {
    prop_oneof![
        Just(b"GET ".as_ref()),
        Just(b"HEAD ".as_ref()),
        Just(b"POST ".as_ref()),
        Just(b"PUT ".as_ref()),
        Just(b"DELETE ".as_ref()),
        Just(b"OPTIONS ".as_ref()),
        Just(b"PATCH ".as_ref()),
        Just(b"CONNECT ".as_ref()),
        Just(b"TRACE ".as_ref()),
    ]
}

/// Printable-ASCII target token, 0..256 bytes.
fn arb_target() -> impl Strategy<Value = Vec<u8>> {
    // A direct range strategy, NOT `any::<u8>().prop_filter(..)`: the filter
    // rejected ~25% of samples and blew proptest's `max_local_rejects` at
    // PROPTEST_CASES=20000+, aborting the property.
    prop::collection::vec(0x20u8..0x7Fu8, 0..256)
}

fn arb_version() -> impl Strategy<Value = &'static [u8]> {
    prop_oneof![Just(b"HTTP/1.0".as_ref()), Just(b"HTTP/1.1".as_ref()),]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_global_rejects: 1024,
        .. ProptestConfig::default()
    })]

    #[test]
    fn request_line_no_panic(method in arb_method(),
                             target in arb_target(),
                             version in arb_version()) {
        let mut buf = Vec::with_capacity(method.len() + target.len() + version.len() + 4);
        buf.extend_from_slice(method);
        buf.extend_from_slice(&target);
        buf.push(b' ');
        buf.extend_from_slice(version);
        buf.extend_from_slice(b"\r\n");

        // catch_unwind so a panic fails the test rather than the runner.
        let buf_for_unwind = buf.clone();
        let res = std::panic::catch_unwind(move || parse_request_line(&buf_for_unwind));
        prop_assert!(res.is_ok(), "parser panicked on generated input");

        if let Ok(Ok((_, _, _, n))) = res {
            prop_assert!(n <= buf.len(),
                         "consumed {n} > input {}", buf.len());
        }
    }

    #[test]
    fn headers_no_panic(payload in vec(any::<u8>(), 0..512)) {
        let res = std::panic::catch_unwind(|| parse_headers(&payload));
        prop_assert!(res.is_ok(), "headers parser panicked on random input");

        if let Ok(Ok((_, n))) = res {
            prop_assert!(n <= payload.len());
        }
    }
}
