use bytes::Bytes;
use lb_l7::*;

#[test]
fn test_bridge_h1_to_h1() {
    let bridge = create_bridge(Protocol::Http1, Protocol::Http1);
    let req = BridgeRequest {
        method: "GET".into(),
        uri: "/path".into(),
        headers: vec![
            ("host".into(), "example.com".into()),
            ("accept".into(), "*/*".into()),
        ],
        body: Bytes::new(),
        scheme: None,
    };
    let bridged = bridge.bridge_request(&req).unwrap();
    assert_eq!(bridged.method, "GET");
    assert_eq!(bridged.uri, "/path");
    // Host header preserved
    assert!(bridged.headers.iter().any(|(k, _)| k == "host"));
}
