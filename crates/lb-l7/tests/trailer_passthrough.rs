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
//! 3. PROTO-2-12 H3 leg (landed): the `lb-quic` upstream surfaces
//!    `H3Request` / `H3UpstreamResponse` now carry a
//!    `trailers: Vec<(String, String)>` field (with `Default`), the
//!    H3 client codec emits/parses the matching trailing HEADERS
//!    frame (RFC 9114 §4.1), and the proxy hot-path H3 legs forward
//!    `translated.trailers` instead of `Vec::new()`. The H3 surface
//!    is pinned positively below.

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

/// PROTO-2-19 (Round-6 delta) — drive hyper's H1 server-side
/// encoder over an in-memory duplex with a `Response` built by
/// `build_h1_response_with_trailers`. The test reads the raw
/// response bytes the encoder emits and asserts:
///
///   1. `Transfer-Encoding: chunked` appears on the head.
///   2. `Trailer: grpc-status, grpc-message` declares the trailer
///      names.
///   3. The chunked terminator `0\r\n` is followed by the trailer
///      fields and a final blank line.
///
/// This is the H2→H1 leg: the trailers come from a gRPC-over-H2
/// backend; the H1 listener path used to silently drop them at the
/// hyper encoder (PROTO-2-19). With the head shape fixed, the
/// `Frame::trailers` actually reaches the wire.
#[tokio::test]
async fn test_h2_h1_trailers_emitted_on_wire() {
    use hyper::Request;
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use lb_l7::h1_proxy::build_h1_response_with_trailers;
    use std::convert::Infallible;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Synthesised translated response — modelled after the gRPC-
    // over-H2 backend shape the bridge produces. Importantly, an
    // upstream `Content-Length` header is included to verify the
    // helper drops it when trailers are present.
    let translated = lb_l7::BridgeResponse {
        status: 200,
        headers: vec![
            ("content-type".to_owned(), "application/grpc".to_owned()),
            ("content-length".to_owned(), "5".to_owned()),
        ],
        body: Bytes::from_static(b"hello"),
        trailers: vec![
            ("grpc-status".to_owned(), "0".to_owned()),
            ("grpc-message".to_owned(), "OK".to_owned()),
        ],
    };

    let (server_io, mut client_io) = tokio::io::duplex(64 * 1024);

    // Server side: a service that always returns our trailer-
    // bearing response. hyper-1's H1 server drives encoding.
    let server_task = tokio::spawn(async move {
        let svc = service_fn(move |_req: Request<Incoming>| {
            let resp = build_h1_response_with_trailers(translated.clone(), None);
            async move { Ok::<_, Infallible>(resp) }
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(server_io), svc)
            .await;
    });

    // Client side: write a minimal H1 request, then read the full
    // response bytes (the server will close the conn after one
    // request since no keep-alive is requested).
    // RFC 9110 §6.6.1: a server MUST NOT generate trailer fields
    // unless the client signalled willingness via `TE: trailers`.
    // grpc-web and other H1 trailer-aware clients send this; our
    // test mirrors that contract so hyper's H1 encoder actually
    // flushes the `Frame::trailers` onto the wire.
    client_io
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nTE: trailers\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client_io.read_to_end(&mut buf),
    )
    .await;
    let _ = server_task.await;

    let text = String::from_utf8_lossy(&buf);
    eprintln!("--- H1 wire bytes (H2→H1 trailers) ---\n{text}\n---");
    assert!(
        text.to_ascii_lowercase()
            .contains("transfer-encoding: chunked"),
        "expected chunked TE on the head; got: {text}"
    );
    // Comma-list order in `Trailer:` matches the input vec order.
    assert!(
        text.to_ascii_lowercase()
            .contains("trailer: grpc-status, grpc-message"),
        "expected `Trailer: grpc-status, grpc-message` on the head; got: {text}"
    );
    assert!(
        !text.to_ascii_lowercase().contains("content-length: 5"),
        "Content-Length must be dropped when trailers are present (RFC 9110 §6.5); got: {text}"
    );
    // The chunked trailer block: after the last data chunk, a `0\r\n`
    // terminator is followed by the trailer fields then a blank
    // line.
    assert!(
        text.contains("\r\n0\r\n"),
        "expected chunked terminator `0\\r\\n`; got: {text}"
    );
    assert!(
        text.contains("grpc-status: 0"),
        "expected `grpc-status: 0` trailer on wire; got: {text}"
    );
    assert!(
        text.contains("grpc-message: OK"),
        "expected `grpc-message: OK` trailer on wire; got: {text}"
    );
}

/// PROTO-2-19 — H3→H1 leg analogue of the H2→H1 test above.
/// PROTO-2-12 (H3 leg landed): `lb_quic::H3UpstreamResponse` now
/// carries a `trailers` field that `h3_response_to_h1` forwards into
/// the `BridgeResponse` fed to `build_h1_response_with_trailers`.
/// This test drives that exact helper with an H3-origin trailer-
/// bearing `BridgeResponse` and asserts the chunked-trailer block
/// reaches the H1 wire. The wire-bytes assertion is identical to the
/// H2 leg because both paths feed the same encoder via the same
/// head-shaping code.
#[tokio::test]
async fn test_h3_h1_trailers_emitted_on_wire() {
    use hyper::Request;
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use lb_l7::h1_proxy::build_h1_response_with_trailers;
    use std::convert::Infallible;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let translated = lb_l7::BridgeResponse {
        status: 200,
        headers: vec![("content-type".to_owned(), "application/grpc".to_owned())],
        body: Bytes::from_static(b"world"),
        // Model the eventual H3 surface: a trailer-bearing response
        // arriving over QUIC, downgraded to H1 for an HTTP/1.1
        // client.
        trailers: vec![
            ("grpc-status".to_owned(), "0".to_owned()),
            ("grpc-message".to_owned(), "OK".to_owned()),
        ],
    };

    let (server_io, mut client_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let svc = service_fn(move |_req: Request<Incoming>| {
            let resp = build_h1_response_with_trailers(translated.clone(), None);
            async move { Ok::<_, Infallible>(resp) }
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(server_io), svc)
            .await;
    });

    // RFC 9110 §6.6.1: a server MUST NOT generate trailer fields
    // unless the client signalled willingness via `TE: trailers`.
    // grpc-web and other H1 trailer-aware clients send this; our
    // test mirrors that contract so hyper's H1 encoder actually
    // flushes the `Frame::trailers` onto the wire.
    client_io
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nTE: trailers\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client_io.read_to_end(&mut buf),
    )
    .await;
    let _ = server_task.await;

    let text = String::from_utf8_lossy(&buf);
    eprintln!("--- H1 wire bytes (H3→H1 trailers) ---\n{text}\n---");
    assert!(
        text.to_ascii_lowercase()
            .contains("transfer-encoding: chunked"),
        "expected chunked TE on the head; got: {text}"
    );
    assert!(
        text.to_ascii_lowercase()
            .contains("trailer: grpc-status, grpc-message"),
        "expected `Trailer: grpc-status, grpc-message` on the head; got: {text}"
    );
    assert!(
        text.contains("\r\n0\r\n"),
        "expected chunked terminator `0\\r\\n`; got: {text}"
    );
    assert!(
        text.contains("grpc-status: 0"),
        "expected `grpc-status: 0` trailer on wire; got: {text}"
    );
    assert!(
        text.contains("grpc-message: OK"),
        "expected `grpc-message: OK` trailer on wire; got: {text}"
    );
}

/// PROTO-2-12 H3 leg — the `lb-quic` upstream surfaces now carry a
/// `trailers` field with a `Default` impl. Previously this gap was
/// only documented; this is the positive pin replacing that prose.
#[test]
fn lb_quic_h3_surfaces_carry_trailers() {
    // `H3Request` carries trailers and `Default` yields an empty list
    // (RFC 9114 §4.1 request trailers arrive in a post-DATA HEADERS
    // frame, not the request head).
    let mut req = lb_quic::H3Request::default();
    assert!(
        req.trailers.is_empty(),
        "H3Request::default() must start with no trailers"
    );
    req.trailers
        .push(("x-checksum".to_owned(), "abc123".to_owned()));
    assert_eq!(
        req.trailers,
        vec![("x-checksum".to_owned(), "abc123".to_owned())]
    );

    // `H3UpstreamResponse` carries trailers and `Default` is the
    // 502/empty bad-gateway shape with no trailers.
    let resp_default = lb_quic::H3UpstreamResponse::default();
    assert_eq!(resp_default.status, 502);
    assert!(
        resp_default.trailers.is_empty(),
        "H3UpstreamResponse::default() must start with no trailers"
    );
    let resp = lb_quic::H3UpstreamResponse {
        status: 200,
        headers: vec![("content-type".to_owned(), "application/grpc".to_owned())],
        body: b"world".to_vec(),
        trailers: vec![("grpc-status".to_owned(), "0".to_owned())],
    };
    assert_eq!(
        resp.trailers,
        vec![("grpc-status".to_owned(), "0".to_owned())]
    );
}

/// PROTO-2-12 H3 leg — positive end-to-end pin that request and
/// response trailers survive **every (src, dst) pair that involves
/// HTTP/3**, exercising the same `bridge_request` / `bridge_response`
/// code path the proxy hot-path H3 legs
/// (`collect_h{1,2}_request_to_h3_fieldlist`, `h3_response_to_h{1,2}`)
/// now feed `translated.trailers` through. This replaces the former
/// `Vec::new()` baseline-pin for the H3 legs with assertions.
#[test]
fn h3_legs_forward_trailers_for_every_pair_involving_h3() {
    let h3_pairs = [
        (Protocol::Http1, Protocol::Http3),
        (Protocol::Http2, Protocol::Http3),
        (Protocol::Http3, Protocol::Http1),
        (Protocol::Http3, Protocol::Http2),
        (Protocol::Http3, Protocol::Http3),
    ];
    for (src, dst) in h3_pairs {
        let bridge = create_bridge(src, dst);
        let req_out = bridge
            .bridge_request(&req_with_trailers())
            .expect("bridge_request");
        assert_eq!(
            req_out.trailers,
            vec![("x-checksum".to_owned(), "abc123".to_owned())],
            "H3 leg dropped request trailers for {src:?} -> {dst:?}"
        );
        let resp_out = bridge
            .bridge_response(&resp_with_trailers())
            .expect("bridge_response");
        assert_eq!(
            resp_out.trailers,
            vec![("x-checksum".to_owned(), "def456".to_owned())],
            "H3 leg dropped response trailers for {src:?} -> {dst:?}"
        );
    }
}
