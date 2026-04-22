//! H3-to-H2 protocol bridging tests.

use bytes::Bytes;
use lb_l7::{BridgeRequest, Protocol, create_bridge};

#[test]
fn test_bridge_h3_to_h2() {
    let bridge = create_bridge(Protocol::Http3, Protocol::Http2);
    assert_eq!(bridge.source_protocol(), Protocol::Http3);
    assert_eq!(bridge.dest_protocol(), Protocol::Http2);

    let body = Bytes::from_static(b"h3 to h2 passthrough");
    let req = BridgeRequest {
        method: "GET".into(),
        uri: "/stream/events".into(),
        headers: vec![
            (":method".into(), "GET".into()),
            (":path".into(), "/stream/events".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "events.svc.local".into()),
            ("Accept".into(), "text/event-stream".into()),
            ("Cache-Control".into(), "no-cache".into()),
        ],
        body: body.clone(),
        scheme: None,
    };

    let bridged = bridge.bridge_request(&req).unwrap();

    // Pseudo-headers preserved (H3 and H2 share the same scheme).
    let find = |name: &str| {
        bridged
            .headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(find(":method"), Some("GET"));
    assert_eq!(find(":path"), Some("/stream/events"));
    assert_eq!(find(":scheme"), Some("https"));
    assert_eq!(find(":authority"), Some("events.svc.local"));

    // Regular headers lowercased.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "accept" && v == "text/event-stream")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "cache-control" && v == "no-cache")
    );

    // Body, method, URI preserved.
    assert_eq!(bridged.body, body);
    assert_eq!(bridged.method, "GET");
    assert_eq!(bridged.uri, "/stream/events");
}
