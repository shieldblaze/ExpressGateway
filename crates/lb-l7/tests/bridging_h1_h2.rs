use bytes::Bytes;
use lb_l7::*;

#[test]
fn test_bridge_h1_to_h2() {
    let bridge = create_bridge(Protocol::Http1, Protocol::Http2);
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
    };
    let bridged = bridge.bridge_request(&req).unwrap();
    // Pseudo-headers added
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
    // Connection header removed
    assert!(!bridged.headers.iter().any(|(k, _)| k == "connection"));
    // Host header removed (replaced by :authority)
    assert!(!bridged.headers.iter().any(|(k, _)| k == "host"));
    // Content-type preserved
    assert!(bridged.headers.iter().any(|(k, _)| k == "content-type"));
}
