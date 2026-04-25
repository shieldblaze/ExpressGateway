use bytes::Bytes;
use lb_l7::*;

#[test]
fn test_bridge_h2_to_h2() {
    let bridge = create_bridge(Protocol::Http2, Protocol::Http2);
    let req = BridgeRequest {
        method: "PUT".into(),
        uri: "/data".into(),
        headers: vec![
            (":method".into(), "PUT".into()),
            (":path".into(), "/data".into()),
            (":scheme".into(), "https".into()),
            (":authority".into(), "api.example.com".into()),
            ("content-type".into(), "application/octet-stream".into()),
        ],
        body: Bytes::from_static(b"binary data"),
        scheme: None,
    };
    let bridged = bridge.bridge_request(&req).unwrap();
    // Pseudo-headers preserved
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":method" && v == "PUT")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == ":path" && v == "/data")
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
    assert_eq!(bridged.body, &b"binary data"[..]);
}
