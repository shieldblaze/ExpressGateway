use bytes::Bytes;
use lb_l7::*;

#[test]
fn test_bridge_h3_to_h3() {
    let bridge = create_bridge(Protocol::Http3, Protocol::Http3);
    let req = BridgeRequest {
        method: "HEAD".into(),
        uri: "/status".into(),
        headers: vec![
            (":method".into(), "HEAD".into()),
            (":path".into(), "/status".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "health.example.com".into()),
        ],
        body: Bytes::new(),
        scheme: None,
    };
    let bridged = bridge.bridge_request(&req).unwrap();
    // Pseudo-headers preserved
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":method" && v == "HEAD")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":path" && v == "/status")
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
            .any(|(k, v)| k == ":authority" && v == "health.example.com")
    );
    // Body preserved (empty)
    assert!(bridged.body.is_empty());
}
