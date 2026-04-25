//! H1-to-H2 protocol bridging tests.

use bytes::Bytes;
use lb_l7::{BridgeRequest, Protocol, create_bridge};

#[test]
fn test_bridge_h1_to_h2() {
    let bridge = create_bridge(Protocol::Http1, Protocol::Http2);
    assert_eq!(bridge.source_protocol(), Protocol::Http1);
    assert_eq!(bridge.dest_protocol(), Protocol::Http2);

    let body = Bytes::from_static(b"h1 to h2 body");
    let req = BridgeRequest {
        method: "GET".into(),
        uri: "/api/data".into(),
        headers: vec![
            ("Host".into(), "example.com".into()),
            ("Content-Type".into(), "application/json".into()),
            ("Connection".into(), "keep-alive".into()),
            ("Keep-Alive".into(), "timeout=5".into()),
            ("Transfer-Encoding".into(), "chunked".into()),
            ("X-Custom".into(), "preserved".into()),
        ],
        body: body.clone(),
        scheme: None,
    };

    let bridged = bridge.bridge_request(&req).unwrap();

    // Pseudo-headers must be present for H2.
    let find = |name: &str| {
        bridged
            .headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };

    assert_eq!(
        find(":method"),
        Some("GET"),
        ":method pseudo-header required"
    );
    assert_eq!(
        find(":path"),
        Some("/api/data"),
        ":path pseudo-header required"
    );
    assert_eq!(
        find(":scheme"),
        Some("https"),
        ":scheme pseudo-header required"
    );
    assert_eq!(
        find(":authority"),
        Some("example.com"),
        ":authority derived from Host"
    );

    // Hop-by-hop headers must be stripped.
    let hop_by_hop = ["connection", "keep-alive", "transfer-encoding"];
    for hdr in &hop_by_hop {
        assert!(
            !bridged.headers.iter().any(|(k, _)| k == hdr),
            "hop-by-hop header '{hdr}' must be stripped in H2 output"
        );
    }

    // Host header must be removed (replaced by :authority).
    assert!(
        !bridged.headers.iter().any(|(k, _)| k == "host"),
        "host header must be replaced by :authority"
    );

    // Custom headers survive, lowercased.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "x-custom" && v == "preserved"),
        "custom headers must survive bridging"
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/json"),
        "content-type must survive"
    );

    // Body preserved.
    assert_eq!(bridged.body, body);
    assert_eq!(bridged.method, "GET");
    assert_eq!(bridged.uri, "/api/data");
}
