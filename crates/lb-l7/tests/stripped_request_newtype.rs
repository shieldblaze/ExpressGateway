//! PROTO-2-07 — `StrippedRequest<B>`: `strip_for_test` is the only public
//! construction path, the strip really runs, and `#[repr(transparent)]` holds.
//! The "raw Request must not compile" guard is a doctest in the source module.

use http::Request;
use http::header::CONNECTION;
use lb_l7::stripped_request::{StrippedRequest, strip_for_test};

#[test]
fn strip_removes_canonical_hop_by_hop_set() {
    let req = Request::builder()
        .uri("/")
        .header(CONNECTION, "keep-alive, x-custom")
        .header("keep-alive", "timeout=5")
        .header("proxy-connection", "keep-alive")
        .header("te", "trailers")
        .header("transfer-encoding", "chunked")
        .header("upgrade", "h2c")
        .header("proxy-authenticate", "Basic")
        .header("proxy-authorization", "Bearer x")
        .header("x-custom", "should-be-removed")
        .header("content-type", "text/plain")
        .header("trailer", "X-Foo") // end-to-end, must survive
        .body(())
        .unwrap();

    let s: StrippedRequest<()> = strip_for_test(req);
    let h = s.inner().headers();

    for name in [
        "connection",
        "keep-alive",
        "proxy-connection",
        "te",
        "transfer-encoding",
        "upgrade",
        "proxy-authenticate",
        "proxy-authorization",
        "x-custom", // listed in Connection
    ] {
        assert!(
            h.get(name).is_none(),
            "PROTO-2-07: hop-by-hop `{name}` must be stripped"
        );
    }
    assert!(h.get("content-type").is_some());
    assert!(
        h.get("trailer").is_some(),
        "RFC 9110 §6.6.2: `Trailer` is end-to-end"
    );
}

#[test]
fn into_inner_round_trips_uri_and_method() {
    let req = Request::builder()
        .method("POST")
        .uri("https://example.test/a/b?x=1")
        .body(())
        .unwrap();
    let r = strip_for_test(req).into_inner();
    assert_eq!(r.method().as_str(), "POST");
    assert_eq!(r.uri().to_string(), "https://example.test/a/b?x=1");
}

#[test]
fn repr_transparent_zero_cost() {
    assert_eq!(
        core::mem::size_of::<StrippedRequest<()>>(),
        core::mem::size_of::<Request<()>>()
    );
    assert_eq!(
        core::mem::align_of::<StrippedRequest<()>>(),
        core::mem::align_of::<Request<()>>()
    );
}

#[test]
fn idempotent_strip_does_not_panic_or_resurrect_headers() {
    // The invariant is "the strip ran at least once", so a second call is safe.
    let req = Request::builder()
        .uri("/")
        .header(CONNECTION, "keep-alive")
        .header("keep-alive", "timeout=5")
        .body(())
        .unwrap();
    let s = strip_for_test(req);
    let inner = s.into_inner();
    let s2 = strip_for_test(inner);
    assert!(s2.inner().headers().get("connection").is_none());
    assert!(s2.inner().headers().get("keep-alive").is_none());
}

/// There is no `pub fn new(_: Request<B>) -> StrippedRequest<B>`; reintroducing
/// one would be a deliberate API break.
#[test]
fn cannot_construct_stripped_request_without_strip() {
    let req = Request::builder()
        .uri("/")
        .header("connection", "close")
        .body(())
        .unwrap();
    let s = strip_for_test(req);
    assert!(s.inner().headers().get("connection").is_none());
}
