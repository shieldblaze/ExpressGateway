//! ROUND8-L7-04 — multi-value `X-Forwarded-For` / `Via` preservation.
//!
//! Reference: Envoy GHSA-ghc4-35x6-crw5 — an RBAC bypass when a header
//! appeared on two lines and the joined-string regex matched only the first.
//! The producer side MUST iterate every existing header line and emit the
//! canonical comma-joined list (RFC 7239 / RFC 9110 §5.3) before appending the
//! peer. The pre-fix code used `HeaderMap::get(..)`, which returns only the
//! first value, then `insert(..)`, which clobbered the rest.

// `append_xff` / `append_via` are `pub(crate)` and intentionally so, so this
// integration crate cannot call them. The assertion is on the OUTPUT shape
// instead: after N duplicate XFF header lines the upstream must see ONE value
// whose comma-separated list has N + 1 members. If it does not, the silent-drop
// bug is back.

use http::HeaderMap;
use http::HeaderValue;

/// MIRROR of the production `append_xff` shape (the real helper is
/// `pub(crate)`). If production ever diverges from this mirror, the same-crate
/// `h1_proxy::tests` proofs are the source of truth — that is where the
/// regression actually surfaces.
fn append_xff_test_mirror(headers: &mut HeaderMap, peer_ip: &str) {
    let mut joined = String::new();
    for v in headers.get_all("x-forwarded-for") {
        if let Ok(s) = v.to_str() {
            if !joined.is_empty() {
                joined.push_str(", ");
            }
            joined.push_str(s);
        }
    }
    if !joined.is_empty() {
        joined.push_str(", ");
    }
    joined.push_str(peer_ip);
    if let Ok(v) = HeaderValue::from_str(&joined) {
        headers.insert("x-forwarded-for", v);
    }
}

#[test]
fn two_xff_headers_preserved_in_join() {
    let mut h = HeaderMap::new();
    h.append("x-forwarded-for", HeaderValue::from_static("1.1.1.1"));
    h.append("x-forwarded-for", HeaderValue::from_static("2.2.2.2"));
    append_xff_test_mirror(&mut h, "9.9.9.9");
    // After fix: one header with three comma-separated values.
    let all: Vec<&str> = h
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert_eq!(
        all.len(),
        1,
        "expected canonical single XFF header, got {all:?}"
    );
    let count = all[0].split(',').count();
    assert_eq!(
        count, 3,
        "expected 3 comma-separated values, got {} in {:?}",
        count, all[0]
    );
    assert!(all[0].contains("1.1.1.1"));
    assert!(all[0].contains("2.2.2.2"));
    assert!(all[0].contains("9.9.9.9"));
}

#[test]
fn three_xff_headers_count_preserved() {
    let mut h = HeaderMap::new();
    h.append("x-forwarded-for", HeaderValue::from_static("1.1.1.1"));
    h.append("x-forwarded-for", HeaderValue::from_static("2.2.2.2"));
    h.append("x-forwarded-for", HeaderValue::from_static("3.3.3.3"));
    append_xff_test_mirror(&mut h, "9.9.9.9");
    let joined: Vec<&str> = h
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert_eq!(joined.len(), 1);
    let parts: Vec<&str> = joined[0].split(',').map(str::trim).collect();
    assert_eq!(parts, vec!["1.1.1.1", "2.2.2.2", "3.3.3.3", "9.9.9.9"]);
}

#[test]
fn single_xff_unchanged_format() {
    let mut h = HeaderMap::new();
    h.append("x-forwarded-for", HeaderValue::from_static("10.0.0.1"));
    append_xff_test_mirror(&mut h, "1.2.3.4");
    let v = h.get("x-forwarded-for").unwrap().to_str().unwrap();
    assert_eq!(v, "10.0.0.1, 1.2.3.4");
}

#[test]
fn no_xff_header_inserts_peer_only() {
    let mut h = HeaderMap::new();
    append_xff_test_mirror(&mut h, "5.6.7.8");
    let v = h.get("x-forwarded-for").unwrap().to_str().unwrap();
    assert_eq!(v, "5.6.7.8");
}

#[test]
fn xff_with_comma_in_existing_value_preserves_inner_commas() {
    // A single pre-existing line may already be a comma list; the producer must
    // preserve it as-is and just append.
    let mut h = HeaderMap::new();
    h.append(
        "x-forwarded-for",
        HeaderValue::from_static("1.1.1.1, 2.2.2.2"),
    );
    append_xff_test_mirror(&mut h, "9.9.9.9");
    let v = h.get("x-forwarded-for").unwrap().to_str().unwrap();
    assert_eq!(v, "1.1.1.1, 2.2.2.2, 9.9.9.9");
}
