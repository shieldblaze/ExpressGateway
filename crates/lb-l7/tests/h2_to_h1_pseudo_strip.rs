//! SEC-2-01 proof — the H2→H1 bridge REJECTS a downgrade smuggle.
//!
//! Pre-Wave-2b the bridge silently produced an H1 request carrying
//! RFC-9113-forbidden hop-by-hop headers (Connection, Keep-Alive,
//! Transfer-Encoding, Upgrade, Proxy-Connection); an H1 upstream that
//! mis-handles them desyncs its response queue. This fires the wired
//! `check_h2_downgrade` call inside `H2ToH1Bridge` and asserts a structural
//! error BEFORE the H1 line is produced. The four vectors in `lb-security`
//! exercise the detector in isolation; this exercises the call site.

use bytes::Bytes;
use lb_l7::{BridgeRequest, Protocol, create_bridge};

fn req_with(headers: Vec<(&'static str, &'static str)>) -> BridgeRequest {
    BridgeRequest {
        method: "POST".into(),
        uri: "/".into(),
        headers: {
            let mut v: Vec<(String, String)> = vec![
                (":method".into(), "POST".into()),
                (":path".into(), "/".into()),
                (":scheme".into(), "https".into()),
                (":authority".into(), "example.com".into()),
            ];
            for (k, val) in headers {
                v.push((k.to_owned(), val.to_owned()));
            }
            v
        },
        body: Bytes::new(),
        scheme: None,
        trailers: Vec::new(),
    }
}

#[test]
fn h2_to_h1_connection_header_rejected() {
    let req = req_with(vec![("connection", "keep-alive")]);
    let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
    let err = bridge.bridge_request(&req).unwrap_err();
    let s = format!("{err}");
    assert!(
        s.contains("smuggle") || s.contains("Smuggle"),
        "h2_to_h1 must reject Connection header in H2 downgrade; got: {s}"
    );
}

#[test]
fn h2_to_h1_keep_alive_header_rejected() {
    let req = req_with(vec![("keep-alive", "timeout=5")]);
    let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
    let err = bridge.bridge_request(&req).unwrap_err();
    let s = format!("{err}");
    assert!(
        s.contains("smuggle") || s.contains("Smuggle"),
        "h2_to_h1 must reject Keep-Alive header in H2 downgrade; got: {s}"
    );
}

#[test]
fn h2_to_h1_transfer_encoding_rejected() {
    let req = req_with(vec![("transfer-encoding", "chunked")]);
    let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
    let err = bridge.bridge_request(&req).unwrap_err();
    let s = format!("{err}");
    assert!(
        s.contains("smuggle") || s.contains("Smuggle"),
        "h2_to_h1 must reject Transfer-Encoding in H2 downgrade; got: {s}"
    );
}

#[test]
fn h2_to_h1_te_non_trailers_rejected() {
    // RFC 9113 §8.2.2: TE in H2 is allowed only with the exact value
    // `trailers`. Anything else is a downgrade smuggle vector.
    let req = req_with(vec![("te", "gzip")]);
    let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
    let err = bridge.bridge_request(&req).unwrap_err();
    let s = format!("{err}");
    assert!(
        s.contains("smuggle") || s.contains("Smuggle"),
        "h2_to_h1 must reject `TE: gzip`; got: {s}"
    );
}

#[test]
fn h2_to_h1_te_trailers_ok() {
    // Negative control: `TE: trailers` is the one allowed value; must succeed.
    let req = req_with(vec![("te", "trailers")]);
    let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
    let bridged = bridge.bridge_request(&req).expect("TE: trailers must pass");
    // Pseudo-headers stripped, `Host` produced from `:authority`.
    let names: Vec<&str> = bridged.headers.iter().map(|(k, _)| k.as_str()).collect();
    assert!(names.contains(&"host"), "host header synthesised");
    assert!(
        names.iter().all(|n| !n.starts_with(':')),
        "no pseudo-headers leaked: {names:?}"
    );
}

#[test]
fn h2_to_h1_clean_request_ok() {
    // Negative control: only-safe-headers must bridge successfully.
    let req = req_with(vec![("accept", "text/html"), ("user-agent", "test")]);
    let bridge = create_bridge(Protocol::Http2, Protocol::Http1);
    let bridged = bridge.bridge_request(&req).expect("clean H2 must pass");
    let names: Vec<&str> = bridged.headers.iter().map(|(k, _)| k.as_str()).collect();
    assert!(names.contains(&"host"));
    assert!(names.contains(&"accept"));
    assert!(names.contains(&"user-agent"));
}
