//! H2-to-H2 protocol bridging tests.

use bytes::Bytes;
use lb_l7::{BridgeRequest, Protocol, create_bridge};

#[test]
fn test_bridge_h2_to_h2() {
    let bridge = create_bridge(Protocol::Http2, Protocol::Http2);
    assert_eq!(bridge.source_protocol(), Protocol::Http2);
    assert_eq!(bridge.dest_protocol(), Protocol::Http2);

    let body = Bytes::from_static(b"h2 passthrough body");
    let req = BridgeRequest {
        method: "DELETE".into(),
        uri: "/items/99".into(),
        headers: vec![
            (":method".into(), "DELETE".into()),
            (":path".into(), "/items/99".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "store.example.com".into()),
            ("Accept".into(), "application/json".into()),
            ("X-Idempotency-Key".into(), "idem-789".into()),
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
    assert_eq!(find(":method"), Some("DELETE"));
    assert_eq!(find(":path"), Some("/items/99"));
    assert_eq!(find(":scheme"), Some("https"));
    assert_eq!(find(":authority"), Some("store.example.com"));

    // Regular headers lowercased.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "accept" && v == "application/json")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "x-idempotency-key" && v == "idem-789")
    );

    // Body, method, URI preserved.
    assert_eq!(bridged.body, body);
    assert_eq!(bridged.method, "DELETE");
    assert_eq!(bridged.uri, "/items/99");
}
