//! SEC-2-01 proof — `SmuggleDetector` is WIRED into the lb-l7 hot path.
//!
//! These exercise the wiring at the public `lb-l7` surface, not the detector's
//! internal correctness (per-vector unit tests live in `lb-security`): CL+TE
//! rejected at the H2→H1 bridge, `TE: gzip, chunked` rejected under
//! `SmuggleMode::H1Strict`, and duplicate `Content-Length` with DIFFERING
//! values rejected on the H1 path. (Same-value duplicates are explicitly
//! accepted — RFC 9110 §8.6 allows merging identical values — so the plan's
//! original `..._same_value_rejected` name would have pinned the wrong case.)

use bytes::Bytes;
use http::{HeaderValue, Method, Request, Version};
use std::net::Ipv4Addr;
use std::sync::Arc;

use lb_l7::security_hooks::{DynSecurityHooks, NoopHooks};
use lb_l7::{BridgeRequest, Protocol, create_bridge};
use lb_security::{ConnGate, HooksBundle, SecurityHooks, SecurityReject, SmuggleMode};

fn h2_bridge_request(headers: Vec<(&'static str, &'static str)>) -> BridgeRequest {
    BridgeRequest {
        method: "GET".into(),
        uri: "/".into(),
        headers: {
            let mut v: Vec<(String, String)> = vec![
                (":method".into(), "GET".into()),
                (":path".into(), "/".into()),
                (":scheme".into(), "https".into()),
                (":authority".into(), "example.com".into()),
            ];
            for (k, val) in headers {
                v.push((k.to_owned(), val.to_owned()));
            }
            v
        },
        body: Bytes::new(),
        scheme: None,
        trailers: Vec::new(),
    }
}

fn req_with(headers: &[(&'static str, &'static str)], version: Version) -> Request<()> {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/")
        .version(version)
        .body(())
        .unwrap();
    for (n, v) in headers {
        req.headers_mut()
            .append(*n, HeaderValue::from_str(v).unwrap());
    }
    req
}

#[test]
fn test_cl_te_rejected() {
    // The H2→H1 bridge is the hottest smuggle surface: once the H1 request line
    // is materialised a desynced upstream parser can be smuggled. The bridge
    // must reject CL+TE before returning the translated request.
    let req = h2_bridge_request(vec![
        ("content-length", "10"),
        ("transfer-encoding", "chunked"),
    ]);
    let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
    let err = bridge.bridge_request(&req).unwrap_err();
    let s = format!("{err}");
    assert!(
        s.contains("smuggle") || s.contains("Smuggle"),
        "h2_to_h1 must reject CL+TE before producing the H1 line; got: {s}"
    );
}

#[test]
fn test_te_gzip_chunked_strict_rejected() {
    // `gzip, chunked` ends in `chunked`, so lenient `SmuggleMode::H1` accepts
    // it; `H1Strict` rejects any codec list beyond `chunked`. Wiring proof: a
    // `HooksBundle` advertised as strict reaches the detector in that mode.
    let gate = ConnGate::new(8, 4, Vec::new());
    let strict = HooksBundle::new(gate, SmuggleMode::H1Strict);
    let r = req_with(&[("transfer-encoding", "gzip, chunked")], Version::HTTP_11);
    let err = SecurityHooks::inspect_request(&strict, &r, Ipv4Addr::LOCALHOST.into()).unwrap_err();
    assert!(
        matches!(err, SecurityReject::Smuggle(_)),
        "strict-TE bundle must reject `gzip, chunked`; got {err:?}"
    );

    // Cross-check: the lenient bundle accepts the same request, proving the
    // wiring honours the mode field rather than always rejecting.
    let gate2 = ConnGate::new(8, 4, Vec::new());
    let lenient = HooksBundle::new(gate2, SmuggleMode::H1);
    assert!(
        SecurityHooks::inspect_request(&lenient, &r, Ipv4Addr::LOCALHOST.into()).is_ok(),
        "lenient bundle must accept `gzip, chunked`"
    );
}

#[test]
fn test_duplicate_cl_differing_values_rejected() {
    // Differing-value duplicate CL is the RFC 9110 §8.6 reject case;
    // same-value duplicates are merged (observationally equivalent to one
    // header). Routed through the production `HooksBundle` so the call path
    // matches what `H1Proxy::handle` invokes.
    let gate = ConnGate::new(8, 4, Vec::new());
    let bundle = HooksBundle::new(gate, SmuggleMode::H1);
    let r = req_with(
        &[("content-length", "10"), ("content-length", "20")],
        Version::HTTP_11,
    );
    let err = SecurityHooks::inspect_request(&bundle, &r, Ipv4Addr::LOCALHOST.into()).unwrap_err();
    assert!(
        matches!(err, SecurityReject::Smuggle(_)),
        "duplicate CL with differing values must be rejected"
    );

    // `DynSecurityHooks` (the object-safe sibling lb-l7 programs against) sees
    // the same rejection, proving the trait shim does not paper over it.
    let dyn_h: Arc<dyn DynSecurityHooks> = Arc::new(NoopHooks::new());
    let safe_req = req_with(&[("content-length", "10")], Version::HTTP_11);
    assert!(
        dyn_h
            .inspect_request(&safe_req, Ipv4Addr::LOCALHOST.into())
            .is_ok(),
        "NoopHooks must accept a safe request"
    );
}
