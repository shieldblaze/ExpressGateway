use bytes::Bytes;
use lb_l7::*;

#[test]
fn test_bridge_h3_to_h2() {
    let bridge = create_bridge(Protocol::Http3, Protocol::Http2);
    let req = BridgeRequest {
        method: "PATCH".into(),
        uri: "/update".into(),
        headers: vec![
            (":method".into(), "PATCH".into()),
            (":path".into(), "/update".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "api.example.com".into()),
            ("content-type".into(), "application/merge-patch+json".into()),
        ],
        body: Bytes::from_static(b"{\"name\":\"new\"}"),
        scheme: None,
    };
    let bridged = bridge.bridge_request(&req).unwrap();
    // Pseudo-headers preserved (same scheme between H3 and H2)
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":method" && v == "PATCH")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":path" && v == "/update")
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
            .any(|(k, v)| k == ":authority" && v == "api.example.com")
    );
    // Regular headers preserved
    assert!(bridged.headers.iter().any(|(k, _)| k == "content-type"));
    // Body preserved
    assert_eq!(bridged.body, &b"{\"name\":\"new\"}"[..]);
}
