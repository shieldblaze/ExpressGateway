//! H2-to-H1 protocol bridging tests.

use bytes::Bytes;
use lb_l7::{BridgeRequest, Protocol, create_bridge};

#[test]
fn test_bridge_h2_to_h1() {
    let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
    assert_eq!(bridge.source_protocol(), Protocol::Http2);
    assert_eq!(bridge.dest_protocol(), Protocol::Http1);

    let body = Bytes::from_static(b"h2 to h1 body content");
    let req = BridgeRequest {
        method: "PUT".into(),
        uri: "/resource/42".into(),
        headers: vec![
            (":method".into(), "PUT".into()),
            (":path".into(), "/resource/42".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "backend.internal".into()),
            ("content-type".into(), "application/octet-stream".into()),
            ("x-forwarded-for".into(), "10.0.0.1".into()),
        ],
        body: body.clone(),
        scheme: None,
    };

    let bridged = bridge.bridge_request(&req).unwrap();

    // No pseudo-headers in H1 output.
    assert!(
        !bridged.headers.iter().any(|(k, _)| k.starts_with(':')),
        "H1 output must not contain any pseudo-headers"
    );

    // Host header must be synthesized from :authority.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "host" && v == "backend.internal"),
        "host header must be derived from :authority"
    );

    // Host should be first in the header list (the impl inserts at position 0).
    assert_eq!(
        bridged.headers.first().map(|(k, _)| k.as_str()),
        Some("host")
    );

    // Regular headers preserved, lowercased.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/octet-stream")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "x-forwarded-for" && v == "10.0.0.1")
    );

    // Method, URI, body preserved.
    assert_eq!(bridged.method, "PUT");
    assert_eq!(bridged.uri, "/resource/42");
    assert_eq!(bridged.body, body);
}
