//! PROTO-2-15 — SNI ↔ authority disagreement validator proof tests.
//!
//! Wave-2b-2 lands the validator function only. The TLS-accept-site
//! wiring (capturing the SNI from rustls and threading it down to
//! `H{1,2}Proxy::handle`) is deferred to Wave-2c because the only
//! call site is `crates/lb/src/main.rs`, which is out of scope for
//! Wave-2b. See `audit/deferred.md` for the deferred wiring entry.

use http::StatusCode;
use lb_l7::sni_authority::{check_sni_authority, misdirected_response};

#[test]
fn test_421_on_mismatch() {
    // The canonical attack: TLS opened to a benign hostname, HTTP
    // request authority points elsewhere.
    let err = check_sni_authority(Some("attacker.example"), "victim.example").unwrap_err();
    assert_eq!(err.sni, "attacker.example");
    assert_eq!(err.authority, "victim.example");

    // Renders as 421 Misdirected Request per RFC 9110 §15.5.20.
    let (status, body) = misdirected_response();
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    assert_eq!(status.as_u16(), 421);
    assert!(body.contains("SNI does not match request authority"));
    assert!(body.contains("RFC 9110 §15.5.20"));
}

#[test]
fn matching_sni_and_authority_passes() {
    assert!(check_sni_authority(Some("example.test"), "example.test").is_ok());
}

#[test]
fn missing_sni_does_not_falsely_reject() {
    // Plain TCP listener or RFC 6066 §3 SNI-omitted client. The
    // validator returns Ok; the operator's TLS configuration (or
    // the L7 ingress) is responsible for any policy that requires
    // SNI presence.
    assert!(check_sni_authority(None, "example.test").is_ok());
}

#[test]
fn port_in_authority_ignored() {
    // SNI never carries a port (RFC 6066 §3); the authority may.
    // We compare on host only.
    assert!(check_sni_authority(Some("example.test"), "example.test:8443").is_ok());
}

#[test]
fn case_insensitive_pass() {
    assert!(check_sni_authority(Some("EXAMPLE.test"), "example.TEST").is_ok());
}

#[test]
fn trailing_dot_normalised() {
    // FQDN form on either side normalises to the same comparison.
    assert!(check_sni_authority(Some("example.test."), "example.test").is_ok());
}

#[test]
fn ipv6_authority_with_brackets() {
    assert!(check_sni_authority(Some("[::1]"), "[::1]:443").is_ok());
    assert!(check_sni_authority(Some("[::1]"), "[::2]").is_err());
}
