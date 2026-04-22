//! H1-to-H3 protocol bridging tests.

use bytes::Bytes;
use lb_l7::{BridgeRequest, Protocol, create_bridge};

#[test]
fn test_bridge_h1_to_h3() {
    let bridge = create_bridge(Protocol::Http1, Protocol::Http3);
    assert_eq!(bridge.source_protocol(), Protocol::Http1);
    assert_eq!(bridge.dest_protocol(), Protocol::Http3);

    let body = Bytes::from_static(b"h1 to h3 payload");
    let req = BridgeRequest {
        method: "POST".into(),
        uri: "/submit".into(),
        headers: vec![
            ("Host".into(), "api.example.com".into()),
            ("Content-Type".into(), "text/plain".into()),
            ("Connection".into(), "close".into()),
            ("Keep-Alive".into(), "timeout=10".into()),
            ("Upgrade".into(), "websocket".into()),
            ("Proxy-Connection".into(), "keep-alive".into()),
            ("Te".into(), "trailers".into()),
            ("X-Trace-Id".into(), "trace-456".into()),
        ],
        body: body.clone(),
        scheme: None,
    };

    let bridged = bridge.bridge_request(&req).unwrap();

    // Pseudo-headers must be present for H3 (same scheme as H2).
    let find = |name: &str| {
        bridged
            .headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };

    assert_eq!(
        find(":method"),
        Some("POST"),
        ":method pseudo-header required"
    );
    assert_eq!(
        find(":path"),
        Some("/submit"),
        ":path pseudo-header required"
    );
    assert_eq!(
        find(":scheme"),
        Some("https"),
        ":scheme pseudo-header required"
    );
    assert_eq!(
        find(":authority"),
        Some("api.example.com"),
        ":authority derived from Host"
    );

    // Hop-by-hop headers must be stripped (except TE: trailers is allowed).
    let stripped = ["connection", "keep-alive", "upgrade", "proxy-connection"];
    for hdr in &stripped {
        assert!(
            !bridged.headers.iter().any(|(k, _)| k == hdr),
            "hop-by-hop header '{hdr}' must be stripped in H3 output"
        );
    }

    // TE: trailers IS allowed through per RFC 7540 sect. 8.1.2.2.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "te" && v == "trailers"),
        "TE: trailers must be preserved"
    );

    // Host replaced by :authority.
    assert!(
        !bridged.headers.iter().any(|(k, _)| k == "host"),
        "host header must be replaced by :authority"
    );

    // Custom headers preserved.
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "x-trace-id" && v == "trace-456")
    );
    assert!(
        bridged
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "text/plain")
    );

    // Body, method, URI preserved.
    assert_eq!(bridged.body, body);
    assert_eq!(bridged.method, "POST");
    assert_eq!(bridged.uri, "/submit");
}
