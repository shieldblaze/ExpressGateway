//! H2-to-H3 protocol bridging tests.

use bytes::Bytes;
use lb_l7::{BridgeRequest, Protocol, create_bridge};

#[test]
fn test_bridge_h2_to_h3() {
    let bridge = create_bridge(Protocol::Http2, Protocol::Http3);
    assert_eq!(bridge.source_protocol(), Protocol::Http2);
    assert_eq!(bridge.dest_protocol(), Protocol::Http3);

    let body = Bytes::from_static(b"h2 to h3 payload bytes");
    let req = BridgeRequest {
        method: "PATCH".into(),
        uri: "/users/7/profile".into(),
        headers: vec![
            (":method".into(), "PATCH".into()),
            (":path".into(), "/users/7/profile".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "users.svc.cluster.local".into()),
            ("Content-Type".into(), "application/merge-patch+json".into()),
            ("Authorization".into(), "Bearer tok_abc".into()),
        ],
        body: body.clone(),
        scheme: None,
    };

    let bridged = bridge.bridge_request(&req).unwrap();

    // Pseudo-headers preserved (H2 and H3 share the same pseudo-header scheme).
    let find = |name: &str| {
        bridged
            .headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(find(":method"), Some("PATCH"));
    assert_eq!(find(":path"), Some("/users/7/profile"));
    assert_eq!(find(":scheme"), Some("https"));
    assert_eq!(find(":authority"), Some("users.svc.cluster.local"));

    // Regular headers lowercased.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/merge-patch+json")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer tok_abc")
    );

    // Body, method, URI preserved.
    assert_eq!(bridged.body, body);
    assert_eq!(bridged.method, "PATCH");
    assert_eq!(bridged.uri, "/users/7/profile");
}
