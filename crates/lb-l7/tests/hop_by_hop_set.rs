//! PROTO-2-08 — the exact RFC 9110 §7.6.1 hop-by-hop strip set.
//!
//! THE CATCH: `Trailer` (§6.6.2) is END-TO-END and must traverse the proxy,
//! while `Trailers` is not a field name at all — only a `TE` value-token.

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
    // `trailers` is only a TE value-token; `Trailer` is end-to-end.
    let mut h = mk_map(&["trailer"]);
    lb_l7::h1_proxy::strip_hop_by_hop(&mut h);
    assert!(
        h.get("trailer").is_some(),
        "`Trailer` is end-to-end per RFC 9110 §6.6.2"
    );
}

#[test]
fn strip_removes_connection_listed_extras() {
    // Names listed inside `Connection` must also be stripped.
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
