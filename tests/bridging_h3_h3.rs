//! H3-to-H3 protocol bridging tests.

use bytes::Bytes;
use lb_l7::{BridgeRequest, Protocol, create_bridge};

#[test]
fn test_bridge_h3_to_h3() {
    let bridge = create_bridge(Protocol::Http3, Protocol::Http3);
    assert_eq!(bridge.source_protocol(), Protocol::Http3);
    assert_eq!(bridge.dest_protocol(), Protocol::Http3);

    let body = Bytes::from_static(b"h3 passthrough body content");
    let req = BridgeRequest {
        method: "POST".into(),
        uri: "/v2/ingest".into(),
        headers: vec![
            (":method".into(), "POST".into()),
            (":path".into(), "/v2/ingest".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "ingest.example.com".into()),
            ("Content-Type".into(), "application/grpc".into()),
            ("Grpc-Timeout".into(), "5S".into()),
            ("X-B3-TraceId".into(), "deadbeef".into()),
        ],
        body: body.clone(),
        scheme: None,
    };

    let bridged = bridge.bridge_request(&req).unwrap();

    // Pseudo-headers preserved verbatim (same protocol).
    let find = |name: &str| {
        bridged
            .headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(find(":method"), Some("POST"));
    assert_eq!(find(":path"), Some("/v2/ingest"));
    assert_eq!(find(":scheme"), Some("https"));
    assert_eq!(find(":authority"), Some("ingest.example.com"));

    // Regular headers lowercased.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/grpc")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "grpc-timeout" && v == "5S")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "x-b3-traceid" && v == "deadbeef")
    );

    // Body, method, URI preserved.
    assert_eq!(bridged.body, body);
    assert_eq!(bridged.method, "POST");
    assert_eq!(bridged.uri, "/v2/ingest");
}
