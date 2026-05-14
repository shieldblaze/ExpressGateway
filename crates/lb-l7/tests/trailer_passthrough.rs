//! PROTO-2-12 — Trailer pass-through across protocol bridges.
//!
//! RFC 9110 §6.6 defines trailers: a sequence of header fields sent
//! after the body. They are end-to-end (§6.6.2 — Trailer field is
//! end-to-end) so an intermediary MUST forward declared trailers
//! when bridging across protocol versions.
//!
//! ## Round-4 / Wave-2c fix landed
//!
//! `BridgeRequest` and `BridgeResponse` now carry a `trailers: Vec<(String, String)>`
//! field. Every bridge in `crates/lb-l7/src/{h1,h2,h3}_to_*.rs`
//! propagates the trailer list through `bridge_request` /
//! `bridge_response`. The proxy hot path
//! (`h1_proxy::translate_h1_request_to_h2`,
//! `h1_proxy::upstream_response_to_h1`,
//! `h2_proxy::translate_h2_request_to_h2`,
//! `h2_proxy::upstream_h2_response_to_h2`)
//! captures trailers via `Collected::trailers()` at body-collect time
//! and re-emits them via `http_body_util::StreamBody` with a
//! `Frame::trailers(HeaderMap)` frame on the writeback side. This
//! flips the Wave-2b-2 baseline tests green.
//!
//! ## What this test file pins
//!
//! 1. Every bridge passes both request and response trailers through
//!    unchanged.
//! 2. `Frame::trailers(...)` survives the `Full → StreamBody`
//!    writeback (the hyper API the bridges rely on).
//! 3. The H3 cross-protocol leg is a documented gap: the H3
//!    upstream surfaces (`H3Request` / `H3UpstreamResponse` in
//!    `lb-quic`) do not yet carry trailers, so the H3 side of any
//!    cross-bridge sends `Vec::new()`. The H3 leg is tracked
//!    separately in `audit/deferred.md` PROTO-2-12 H3 leg.

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::StreamBody;
use hyper::body::Frame;
use lb_l7::{BridgeRequest, BridgeResponse, Protocol, create_bridge};

fn req_with_trailers() -> BridgeRequest {
    BridgeRequest {
        method: "POST".into(),
        uri: "/".into(),
        headers: vec![
            (":method".into(), "POST".into()),
            (":path".into(), "/".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "example.com".into()),
            ("trailer".into(), "x-checksum".into()),
        ],
        body: Bytes::from_static(b"hello"),
        scheme: Some("https".into()),
        trailers: vec![("x-checksum".into(), "abc123".into())],
    }
}

fn resp_with_trailers() -> BridgeResponse {
    BridgeResponse {
        status: 200,
        headers: vec![("trailer".into(), "x-checksum".into())],
        body: Bytes::from_static(b"world"),
        trailers: vec![("x-checksum".into(), "def456".into())],
    }
}

/// Every cross-protocol bridge MUST forward the request trailer list.
#[test]
fn every_bridge_forwards_request_trailers() {
    let combos = [
        (Protocol::Http1, Protocol::Http1),
        (Protocol::Http1, Protocol::Http2),
        (Protocol::Http1, Protocol::Http3),
        (Protocol::Http2, Protocol::Http1),
        (Protocol::Http2, Protocol::Http2),
        (Protocol::Http2, Protocol::Http3),
        (Protocol::Http3, Protocol::Http1),
        (Protocol::Http3, Protocol::Http2),
        (Protocol::Http3, Protocol::Http3),
    ];
    for (src, dst) in combos {
        let bridge = create_bridge(src, dst);
        let req = req_with_trailers();
        let out = bridge.bridge_request(&req).expect("bridge_request");
        assert_eq!(
            out.trailers,
            vec![("x-checksum".to_owned(), "abc123".to_owned())],
            "request trailers dropped for {src:?} -> {dst:?}"
        );
    }
}

/// Every cross-protocol bridge MUST forward the response trailer list.
#[test]
fn every_bridge_forwards_response_trailers() {
    let combos = [
        (Protocol::Http1, Protocol::Http1),
        (Protocol::Http1, Protocol::Http2),
        (Protocol::Http1, Protocol::Http3),
        (Protocol::Http2, Protocol::Http1),
        (Protocol::Http2, Protocol::Http2),
        (Protocol::Http2, Protocol::Http3),
        (Protocol::Http3, Protocol::Http1),
        (Protocol::Http3, Protocol::Http2),
        (Protocol::Http3, Protocol::Http3),
    ];
    for (src, dst) in combos {
        let bridge = create_bridge(src, dst);
        let resp = resp_with_trailers();
        let out = bridge.bridge_response(&resp).expect("bridge_response");
        assert_eq!(
            out.trailers,
            vec![("x-checksum".to_owned(), "def456".to_owned())],
            "response trailers dropped for {src:?} -> {dst:?}"
        );
    }
}

/// `BridgeRequest` and `BridgeResponse` now carry a trailers field —
/// flipped from the Wave-2b-2 baseline `_have_no_trailers_field_today`
/// test which lived here previously.
#[test]
fn bridge_request_response_carry_trailers() {
    let req = lb_l7::BridgeRequest {
        method: "GET".into(),
        uri: "/".into(),
        headers: vec![],
        body: Bytes::new(),
        scheme: None,
        trailers: vec![("x-trailer".into(), "v".into())],
    };
    assert_eq!(req.trailers.len(), 1);
    let resp = lb_l7::BridgeResponse {
        status: 200,
        headers: vec![],
        body: Bytes::new(),
        trailers: vec![("x-trailer".into(), "v".into())],
    };
    assert_eq!(resp.trailers.len(), 1);
}

/// Sanity: hyper's `Frame::trailers` round-trip works as the proxy
/// hot path now uses — this matches the `build_body_with_trailers`
/// helper in `h1_proxy.rs` / `h2_proxy.rs`.
#[tokio::test]
async fn stream_body_with_trailers_round_trips() {
    use http::HeaderMap;

    let mut tmap = HeaderMap::new();
    tmap.insert("x-trailer", "value".parse().unwrap());

    let frames = vec![
        Ok::<_, std::convert::Infallible>(Frame::data(Bytes::from_static(b"hello"))),
        Ok::<_, std::convert::Infallible>(Frame::trailers(tmap)),
    ];
    let stream = futures_util::stream::iter(frames);
    let body = StreamBody::new(stream);
    let collected = body.collect().await.expect("collect");
    let trailers = collected.trailers().expect("trailers preserved");
    assert_eq!(trailers.get("x-trailer").unwrap(), "value");
}
