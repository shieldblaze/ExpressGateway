//! H1-to-H1 protocol bridging tests.

use bytes::Bytes;
use lb_l7::{BridgeRequest, Protocol, create_bridge};

#[test]
fn test_bridge_h1_to_h1() {
    let bridge = create_bridge(Protocol::Http1, Protocol::Http1);
    assert_eq!(bridge.source_protocol(), Protocol::Http1);
    assert_eq!(bridge.dest_protocol(), Protocol::Http1);

    let body = Bytes::from_static(b"request body payload");
    let req = BridgeRequest {
        method: "POST".into(),
        uri: "/api/data".into(),
        headers: vec![
            ("Host".into(), "example.com".into()),
            ("Content-Type".into(), "application/json".into()),
            ("X-Request-Id".into(), "abc-123".into()),
            ("Connection".into(), "keep-alive".into()),
            ("Keep-Alive".into(), "timeout=5".into()),
        ],
        body: body.clone(),
        scheme: None,
    };

    let bridged = bridge.bridge_request(&req).unwrap();

    // H1->H1 lowercases all header names.
    for (k, _) in &bridged.headers {
        assert_eq!(k, &k.to_lowercase(), "header name should be lowercased");
    }

    // Host and regular headers survive.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "host" && v == "example.com"),
        "host header must be preserved in H1->H1"
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/json"),
        "content-type header must be preserved"
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "x-request-id" && v == "abc-123"),
        "custom header must be preserved"
    );

    // Hop-by-hop headers stripped.
    assert!(
        !bridged.headers.iter().any(|(k, _)| k == "connection"),
        "connection header must be stripped"
    );
    assert!(
        !bridged.headers.iter().any(|(k, _)| k == "keep-alive"),
        "keep-alive header must be stripped"
    );

    // No pseudo-headers in H1 output.
    assert!(
        !bridged.headers.iter().any(|(k, _)| k.starts_with(':')),
        "H1 output must not contain pseudo-headers"
    );

    // Method, URI, body preserved.
    assert_eq!(bridged.method, "POST");
    assert_eq!(bridged.uri, "/api/data");
    assert_eq!(bridged.body, body);
}
