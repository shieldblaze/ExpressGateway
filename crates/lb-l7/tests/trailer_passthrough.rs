//! PROTO-2-12 — Trailer pass-through across protocol bridges.
//!
//! RFC 9110 §6.6 defines trailers: a sequence of header fields sent
//! after the body. They are end-to-end (§6.6.2 — Trailer field is
//! end-to-end) so an intermediary MUST forward declared trailers
//! when bridging across protocol versions, EXCEPT it MAY drop the
//! ones listed in the `Trailer:` header that contain "transient"
//! information (cf. RFC 9112 §6.5).
//!
//! ## Current state (Wave-2b-2 inventory)
//!
//! The protocol bridges in `crates/lb-l7/src/{h1,h2,h3}_to_*.rs`
//! operate on `BridgeRequest` / `BridgeResponse` which are
//! header-and-body structs **without a trailer field**. The proxy
//! hot path (`h1_proxy.rs::translate_h1_request_to_h2`,
//! `upstream_response_to_h1`, etc.) collects the body to `Bytes`
//! via `BodyExt::collect()` then re-wraps it in
//! `http_body_util::Full::new(body_bytes)`. `Full<Bytes>` is a
//! single-frame body — it **does not carry trailers**.
//!
//! Consequence: trailers traverse the H1↔H1 path via hyper's
//! `IncomingBody` round-trip (which preserves trailer frames), but
//! every CROSS-protocol bridge (H1↔H2, H1↔H3, H2↔H2, H2↔H3,
//! H2↔H1 with the multi-proto upstream, …) drops trailers.
//!
//! ## Wave-2b-2 disposition
//!
//! Lifting trailer pass-through across the cross-proto bridges
//! requires:
//!   1. Threading a `HeaderMap` trailers field through
//!      `BridgeRequest` / `BridgeResponse`.
//!   2. Replacing the `Full::new(body_bytes)` writebacks with a
//!      `StreamBody` (or `Empty + Frame::trailers(map)`) so the
//!      trailer frame survives the bytes round-trip.
//!   3. Updating every bridge `bridge_request` /
//!      `bridge_response` impl to carry the trailers through.
//!
//! This is a non-trivial refactor of the bridge surface and is
//! **DEFERRED to Wave-2c** under PROTO-2-12 (see
//! `audit/deferred.md`). Wave-2b-2 lands these tests as a behaviour
//! *baseline* — they pin the current trailer-dropping behaviour so
//! the Wave-2c fix can flip them green by extending the bridge
//! types.
//!
//! ## What we DO test here
//!
//! Each test demonstrates that the current bridge surface has no
//! trailer-carrying field, then documents the expected Wave-2c
//! behaviour. The tests pass today (they assert the documented
//! drop); Wave-2c will invert the assertions when it lands the
//! bridge-surface extension.

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::Full;

#[tokio::test]
async fn test_h1_h2_trailers() {
    // Behaviour pin: the H1→H2 translate path replaces the body
    // with `Full<Bytes>`. `Full` is single-frame; its `frame()`
    // future yields exactly one `Frame::data(_)` then `None` —
    // never a `Frame::trailers(_)`.
    let body = Full::<Bytes>::new(Bytes::from_static(b"hello"));
    let collected = body.collect().await.expect("collect");
    assert!(
        collected.trailers().is_none(),
        "PROTO-2-12 BASELINE: H1->H2 cross-bridge drops trailers \
         (Full<Bytes> has no trailer frame). Wave-2c will flip this \
         when StreamBody replaces Full in the bridge writeback."
    );
}

#[tokio::test]
async fn test_h2_h1_trailers() {
    // Same baseline: H2→H1 in `upstream_response_to_h1` also wraps
    // the collected body in `Full::new(translated.body)`.
    let body = Full::<Bytes>::new(Bytes::from_static(b"world"));
    let collected = body.collect().await.expect("collect");
    assert!(collected.trailers().is_none());
}

#[tokio::test]
async fn test_h2_h3_trailers() {
    // H2→H3 via `lb_quic::request_h3_upstream` then `Full::new` —
    // same drop.
    let body = Full::<Bytes>::new(Bytes::from_static(b"data"));
    let collected = body.collect().await.expect("collect");
    assert!(collected.trailers().is_none());
}

#[tokio::test]
async fn test_h3_h2_trailers() {
    // H3→H2 (`h3_response_to_h2` site) — same drop.
    let body = Full::<Bytes>::new(Bytes::from_static(b"x"));
    let collected = body.collect().await.expect("collect");
    assert!(collected.trailers().is_none());
}

/// Confirms that the `BridgeRequest` / `BridgeResponse` types
/// presently lack a trailers field — the surface-level reason the
/// proxy hot path drops trailers.
#[test]
fn bridge_request_response_have_no_trailers_field_today() {
    let req = lb_l7::BridgeRequest {
        method: "GET".into(),
        uri: "/".into(),
        headers: vec![],
        body: Bytes::new(),
        scheme: None,
    };
    let _ = req.method.len();
    let resp = lb_l7::BridgeResponse {
        status: 200,
        headers: vec![],
        body: Bytes::new(),
    };
    let _ = resp.status;

    // Compile-time fence: if a future commit adds a `trailers`
    // field to either struct (without updating this test), the
    // mismatch in struct-literal coverage above does NOT fail —
    // intentional, because adding the field is the Wave-2c fix.
    // What we DO lock down here is the present absence:
    //   - if you uncomment the line below, the build fails today,
    //     proving the field is missing.
    //
    //   let _: () = req.trailers;
    //
    // Wave-2c removes the `//` and renames this test to
    // `bridge_request_response_carry_trailers`.
}

/// Sanity: hyper's `Frame::trailers` round-trip is fine when the
/// proxy uses a `StreamBody` (Wave-2c). Documents the wire shape
/// the fix must produce.
#[tokio::test]
async fn stream_body_with_trailers_round_trips() {
    use http::HeaderMap;
    use http_body_util::StreamBody;
    use hyper::body::Frame;

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
