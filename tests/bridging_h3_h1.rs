//! H3-to-H1 protocol bridging tests.

use bytes::Bytes;
use lb_l7::{BridgeRequest, Protocol, create_bridge};

#[test]
fn test_bridge_h3_to_h1() {
    let bridge = create_bridge(Protocol::Http3, Protocol::Http1);
    assert_eq!(bridge.source_protocol(), Protocol::Http3);
    assert_eq!(bridge.dest_protocol(), Protocol::Http1);

    let body = Bytes::from_static(b"h3 to h1 downgrade body");
    let req = BridgeRequest {
        method: "POST".into(),
        uri: "/legacy/endpoint".into(),
        headers: vec![
            (":method".into(), "POST".into()),
            (":path".into(), "/legacy/endpoint".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "legacy.backend.internal".into()),
            ("content-type".into(), "text/xml".into()),
            ("x-correlation-id".into(), "corr-001".into()),
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

    // Host header synthesized from :authority.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "host" && v == "legacy.backend.internal"),
        "host header must be derived from :authority"
    );

    // Host should be first (inserted at position 0).
    assert_eq!(
        bridged.headers.first().map(|(k, _)| k.as_str()),
        Some("host")
    );

    // Regular headers preserved, lowercased.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "text/xml")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "x-correlation-id" && v == "corr-001")
    );

    // Method, URI, body preserved.
    assert_eq!(bridged.method, "POST");
    assert_eq!(bridged.uri, "/legacy/endpoint");
    assert_eq!(bridged.body, body);
}
