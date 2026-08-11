use bytes::Bytes;
use lb_l7::*;

#[test]
fn test_bridge_h1_to_h3() {
    let bridge = create_bridge(Protocol::Http1, Protocol::Http3);
    let req = BridgeRequest {
        method: "POST".into(),
        uri: "/api".into(),
        headers: vec![
            ("host".into(), "example.com".into()),
            ("connection".into(), "keep-alive".into()),
            ("content-type".into(), "application/json".into()),
        ],
        body: Bytes::from_static(b"{}"),
        scheme: None,
        trailers: Vec::new(),
    };
    let bridged = bridge.bridge_request(&req).unwrap();
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":method" && v == "POST")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":path" && v == "/api")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":authority" && v == "example.com")
    );
    assert!(!bridged.headers.iter().any(|(k, _)| k == "connection"));
    assert!(!bridged.headers.iter().any(|(k, _)| k == "host"));
    assert!(bridged.headers.iter().any(|(k, _)| k == "content-type"));
}
