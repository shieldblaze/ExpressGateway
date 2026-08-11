//! PROTO-2-12 — trailers are END-TO-END (RFC 9110 §6.6.2), so every bridge must
//! forward request AND response trailers unchanged, including the H3 legs.

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

/// `Frame::trailers` must round-trip the way `build_body_with_trailers` relies on.
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

/// PROTO-2-19 — RAW-byte assertion through hyper's real H1 encoder (the H2→H1
/// leg, where the listener used to silently drop trailers).
#[tokio::test]
async fn test_h2_h1_trailers_emitted_on_wire() {
    use hyper::Request;
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use lb_l7::h1_proxy::build_h1_response_with_trailers;
    use std::convert::Infallible;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // The upstream `Content-Length` is deliberate: the helper must drop it
    // when trailers are present.
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

    let server_task = tokio::spawn(async move {
        let svc = service_fn(move |_req: Request<Incoming>| {
            let resp = build_h1_response_with_trailers(translated.clone(), None);
            async move { Ok::<_, Infallible>(resp) }
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(server_io), svc)
            .await;
    });

    // RFC 9110 §6.6.1: `TE: trailers` from the client is what makes hyper's
    // encoder actually flush the `Frame::trailers` onto the wire.
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
    assert!(
        text.to_ascii_lowercase()
            .contains("trailer: grpc-status, grpc-message"),
        "expected `Trailer: grpc-status, grpc-message` on the head; got: {text}"
    );
    assert!(
        !text.to_ascii_lowercase().contains("content-length: 5"),
        "Content-Length must be dropped when trailers are present (RFC 9110 §6.5); got: {text}"
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

/// PROTO-2-19 — H3→H1 analogue of the test above.
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

/// PROTO-2-12 H3 leg — the `lb-quic` request surface carries a `trailers` field.
#[test]
fn lb_quic_h3_surfaces_carry_trailers() {
    // RFC 9114 §4.1: request trailers arrive post-DATA, so `Default` is empty.
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
}

/// PROTO-2-12 H3 leg — trailers survive EVERY (src, dst) pair involving HTTP/3.
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
