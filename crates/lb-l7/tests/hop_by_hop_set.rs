//! PROTO-2-08 — the exact hop-by-hop set stripped by
//! `lb_l7::h1_proxy::strip_hop_by_hop`, per RFC 9110 §7.6.1.
//!
//! THE CATCH: `Trailer` (RFC 9110 §6.6.2) is the END-TO-END declaration header
//! and MUST traverse the proxy, while `Trailers` is not a header field name at
//! all — only a `TE` value-token — so it must not appear in the strip set.

use hyper::HeaderMap;
use hyper::header::{HeaderName, HeaderValue};

/// The canonical RFC 9110 §7.6.1 hop-by-hop set.
const EXPECTED_HOP_BY_HOP: &[&str] = &[
    "connection",
    "proxy-connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "transfer-encoding",
    "upgrade",
];

/// End-to-end headers that the strip MUST preserve (RFC 9110 §6.6).
const EXPECTED_END_TO_END: &[&str] = &[
    "trailer",        // §6.6.2 — end-to-end declaration header
    "content-length", // §8.6 — end-to-end framing
    "content-type",
    "host",
    "accept",
];

fn mk_map(names: &[&str]) -> HeaderMap {
    let mut m = HeaderMap::new();
    for n in names {
        m.append(
            HeaderName::try_from(*n).expect("valid header name"),
            HeaderValue::from_static("x"),
        );
    }
    m
}

#[test]
fn strip_removes_exactly_the_rfc_9110_set() {
    // Seed both sets; the strip must remove only the hop-by-hop names.
    let all: Vec<&str> = EXPECTED_HOP_BY_HOP
        .iter()
        .chain(EXPECTED_END_TO_END.iter())
        .copied()
        .collect();
    let mut h = mk_map(&all);
    lb_l7::h1_proxy::strip_hop_by_hop(&mut h);

    for name in EXPECTED_HOP_BY_HOP {
        assert!(
            h.get(*name).is_none(),
            "RFC 9110 §7.6.1 hop-by-hop `{name}` must be stripped"
        );
    }
    for name in EXPECTED_END_TO_END {
        assert!(
            h.get(*name).is_some(),
            "end-to-end header `{name}` must NOT be stripped"
        );
    }
}

#[test]
fn strip_does_not_remove_the_trailers_pseudo_token() {
    // `trailers` is only a TE value-token (RFC 9110 §10.1.4); the singular
    // `Trailer` header is end-to-end. Seed one and confirm the strip leaves it.
    let mut h = mk_map(&["trailer"]);
    lb_l7::h1_proxy::strip_hop_by_hop(&mut h);
    assert!(
        h.get("trailer").is_some(),
        "`Trailer` is end-to-end per RFC 9110 §6.6.2"
    );
}

#[test]
fn strip_removes_connection_listed_extras() {
    // RFC 9110 §7.6.1 also requires stripping names listed inside `Connection`.
    // Re-assert at the public surface so a refactor is caught here too.
    let mut h = HeaderMap::new();
    h.insert(
        hyper::header::CONNECTION,
        HeaderValue::from_static("keep-alive, x-custom"),
    );
    h.insert(
        HeaderName::from_static("x-custom"),
        HeaderValue::from_static("v"),
    );
    h.insert(
        HeaderName::from_static("x-not-listed"),
        HeaderValue::from_static("v"),
    );
    lb_l7::h1_proxy::strip_hop_by_hop(&mut h);
    assert!(h.get("connection").is_none());
    assert!(
        h.get("x-custom").is_none(),
        "Connection-listed extra must be stripped"
    );
    assert!(h.get("x-not-listed").is_some());
}
