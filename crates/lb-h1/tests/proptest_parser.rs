//! CODE-2-11 — proptest harness for the lb-h1 request-line + headers
//! parser. Two invariants per generator:
//!
//! 1. **No panic / no unwrap**: `parse_request_line` and
//!    `parse_headers` MUST return `Result`, never panic, for any
//!    `Vec<u8>` derived from the grammar below. We wrap the call
//!    in `std::panic::catch_unwind` and assert `Ok(_)`.
//! 2. **Bounded consumption**: when the parser returns `Ok((..., n))`
//!    `n` is bounded by `buf.len()` — i.e. the parser cannot claim
//!    to have consumed more bytes than we fed it.
//!
//! Budget: ships at the proptest default (256 cases) so plain
//! `cargo test -p lb-h1` stays fast. CI bumps to 200 000 cases via
//! `PROPTEST_CASES=200000` per the round-2 review §CODE-2-11 audit
//! gate (HPACK / QPACK / H1 get the higher budget given their
//! smuggling-risk surface).

#![cfg(feature = "proptest")]

use proptest::collection::vec;
use proptest::prelude::*;

use lb_h1::{parse_headers, parse_request_line};

/// Method ∈ closed set (RFC 9110 §9.1 + a few non-standard).
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

/// Target token: 0..256 bytes drawn from a "URL-ish" byte set.
fn arb_target() -> impl Strategy<Value = Vec<u8>> {
    vec(
        prop_oneof![
            // ASCII visible chars; bias the distribution toward path-safe.
            any::<u8>().prop_filter("printable ASCII", |b| (0x20..0x7F).contains(b))
        ],
        0..256,
    )
}

/// HTTP version literal (HTTP/1.0 vs HTTP/1.1).
fn arb_version() -> impl Strategy<Value = &'static [u8]> {
    prop_oneof![Just(b"HTTP/1.0".as_ref()), Just(b"HTTP/1.1".as_ref()),]
}

proptest! {
    // PROPTEST_CASES env overrides this for CI's full-budget run.
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_global_rejects: 1024,
        .. ProptestConfig::default()
    })]

    /// Request line never panics, always returns Result, never claims
    /// to consume more bytes than fed.
    #[test]
    fn request_line_no_panic(method in arb_method(),
                             target in arb_target(),
                             version in arb_version()) {
        // Build "<METHOD> <TARGET> <VERSION>\r\n".
        let mut buf = Vec::with_capacity(method.len() + target.len() + version.len() + 4);
        buf.extend_from_slice(method);
        buf.extend_from_slice(&target);
        buf.push(b' ');
        buf.extend_from_slice(version);
        buf.extend_from_slice(b"\r\n");

        // Wrap in catch_unwind so any panic surfaces as a test failure
        // rather than tearing down the runner. CODE-2-11 invariant 2.
        let buf_for_unwind = buf.clone();
        let res = std::panic::catch_unwind(move || parse_request_line(&buf_for_unwind));
        prop_assert!(res.is_ok(), "parser panicked on generated input");

        if let Ok(Ok((_, _, _, n))) = res {
            prop_assert!(n <= buf.len(),
                         "consumed {n} > input {}", buf.len());
        }
    }

    /// Headers parser never panics; consumed-byte count bounded.
    #[test]
    fn headers_no_panic(payload in vec(any::<u8>(), 0..512)) {
        let res = std::panic::catch_unwind(|| parse_headers(&payload));
        prop_assert!(res.is_ok(), "headers parser panicked on random input");

        if let Ok(Ok((_, n))) = res {
            prop_assert!(n <= payload.len());
        }
    }
}
