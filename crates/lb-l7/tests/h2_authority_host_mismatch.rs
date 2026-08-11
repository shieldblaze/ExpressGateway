//! PROTO-2-01 / RFC 9113 §8.3.1 — `:authority` vs `Host` disagreement, driven
//! through the exact function `H2Proxy::handle` calls, fed the shape hyper's
//! `into_parts` produces (so a hyper `uri.authority()` change trips it).

use http::Uri;
use http::header::HOST;
use hyper::HeaderMap;
use hyper::header::HeaderValue;
use lb_l7::h2_proxy::check_authority_host_agreement;

fn hdrs(host: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Some(v) = host {
        h.insert(HOST, HeaderValue::from_str(v).unwrap());
    }
    h
}

fn uri_with_auth(auth: Option<&str>) -> Uri {
    match auth {
        Some(a) => format!("https://{a}/path").parse().unwrap(),
        None => "/path".parse().unwrap(),
    }
}

#[test]
fn test_h2_400_on_disagreement() {
    // The canonical attack: `:authority` routes, `Host` authorises.
    let uri = uri_with_auth(Some("victim.example"));
    let headers = hdrs(Some("attacker.example"));
    let err = check_authority_host_agreement(&uri, &headers).unwrap_err();
    assert!(err.contains("RFC 9113"));
    assert!(err.contains(":authority"));
}

#[test]
fn matching_authority_and_host_pass() {
    let uri = uri_with_auth(Some("example.test"));
    let headers = hdrs(Some("example.test"));
    assert!(check_authority_host_agreement(&uri, &headers).is_ok());
}

#[test]
fn case_insensitive_host_match() {
    let uri = uri_with_auth(Some("EXAMPLE.test"));
    let headers = hdrs(Some("example.TEST"));
    assert!(check_authority_host_agreement(&uri, &headers).is_ok());
}

#[test]
fn port_match_when_both_explicit() {
    let uri = uri_with_auth(Some("example.test:8443"));
    let headers = hdrs(Some("example.test:8443"));
    assert!(check_authority_host_agreement(&uri, &headers).is_ok());
}

#[test]
fn port_mismatch_when_both_explicit_rejected() {
    let uri = uri_with_auth(Some("example.test:8443"));
    let headers = hdrs(Some("example.test:9999"));
    assert!(check_authority_host_agreement(&uri, &headers).is_err());
}

#[test]
fn port_elision_one_side_accepted() {
    // RFC 9113 §8.3.1 latitude: one side eliding the port is accepted.
    let uri = uri_with_auth(Some("example.test:443"));
    let headers = hdrs(Some("example.test"));
    assert!(check_authority_host_agreement(&uri, &headers).is_ok());

    let uri = uri_with_auth(Some("example.test"));
    let headers = hdrs(Some("example.test:443"));
    assert!(check_authority_host_agreement(&uri, &headers).is_ok());
}

#[test]
fn missing_authority_is_not_rejected_here() {
    // Missing-authority belongs to the pseudo-header layer, not here.
    let uri = uri_with_auth(None);
    let headers = hdrs(Some("example.test"));
    assert!(check_authority_host_agreement(&uri, &headers).is_ok());
}

#[test]
fn missing_host_is_not_rejected_here() {
    let uri = uri_with_auth(Some("example.test"));
    let headers = hdrs(None);
    assert!(check_authority_host_agreement(&uri, &headers).is_ok());
}

#[test]
fn ipv6_authority_match() {
    let uri = uri_with_auth(Some("[::1]"));
    let headers = hdrs(Some("[::1]"));
    assert!(check_authority_host_agreement(&uri, &headers).is_ok());

    let uri = uri_with_auth(Some("[::1]:443"));
    let headers = hdrs(Some("[::1]:8443"));
    assert!(check_authority_host_agreement(&uri, &headers).is_err());
}

#[test]
fn ipv6_vs_ipv4_disagreement() {
    let uri = uri_with_auth(Some("[::1]"));
    let headers = hdrs(Some("127.0.0.1"));
    assert!(check_authority_host_agreement(&uri, &headers).is_err());
}

#[test]
fn empty_host_value_rejects() {
    let uri = uri_with_auth(Some("example.test"));
    let headers = hdrs(Some(""));
    assert!(check_authority_host_agreement(&uri, &headers).is_err());
}
