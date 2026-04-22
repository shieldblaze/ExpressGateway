use bytes::Bytes;
use lb_l7::*;

#[test]
fn test_bridge_h2_to_h3() {
    let bridge = create_bridge(Protocol::Http2, Protocol::Http3);
    let req = BridgeRequest {
        method: "DELETE".into(),
        uri: "/item/42".into(),
        headers: vec![
            (":method".into(), "DELETE".into()),
            (":path".into(), "/item/42".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "store.example.com".into()),
            ("authorization".into(), "Bearer token123".into()),
        ],
        body: Bytes::new(),
        scheme: None,
    };
    let bridged = bridge.bridge_request(&req).unwrap();
    // Pseudo-headers preserved (same scheme between H2 and H3)
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":method" && v == "DELETE")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":path" && v == "/item/42")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":scheme" && v == "https")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":authority" && v == "store.example.com")
    );
    // Regular headers preserved
    assert!(bridged.headers.iter().any(|(k, _)| k == "authorization"));
}
