use bytes::Bytes;
use lb_l7::*;

#[test]
fn test_bridge_h3_to_h1() {
    let bridge = create_bridge(Protocol::Http3, Protocol::Http1);
    let req = BridgeRequest {
        method: "GET".into(),
        uri: "/".into(),
        headers: vec![
            (":method".into(), "GET".into()),
            (":path".into(), "/resource".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "example.com".into()),
            ("accept".into(), "text/html".into()),
        ],
        body: Bytes::new(),
        scheme: None,
    };
    let bridged = bridge.bridge_request(&req).unwrap();
    // :method extracted into method field
    assert_eq!(bridged.method, "GET");
    // :path extracted into uri field
    assert_eq!(bridged.uri, "/resource");
    // :authority becomes Host header
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "host" && v == "example.com")
    );
    // Pseudo-headers removed from headers list
    assert!(!bridged.headers.iter().any(|(k, _)| k == ":method"));
    assert!(!bridged.headers.iter().any(|(k, _)| k == ":path"));
    assert!(!bridged.headers.iter().any(|(k, _)| k == ":scheme"));
    assert!(!bridged.headers.iter().any(|(k, _)| k == ":authority"));
    // Regular headers preserved
    assert!(bridged.headers.iter().any(|(k, _)| k == "accept"));
}
