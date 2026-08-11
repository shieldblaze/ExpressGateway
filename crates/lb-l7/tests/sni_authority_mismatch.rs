//! PROTO-2-15 — SNI ↔ authority disagreement validator unit proofs. The
//! hot-path wiring is proven separately in `tests/sni_authority_421.rs`.

use http::StatusCode;
use lb_l7::sni_authority::{check_sni_authority, misdirected_response};

#[test]
fn test_421_on_mismatch() {
    // Canonical attack: TLS to a benign hostname, request authority elsewhere.
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
    // Plain TCP or an RFC 6066 §3 SNI-omitting client: the validator returns
    // Ok; requiring SNI presence is the operator's TLS-config policy.
    assert!(check_sni_authority(None, "example.test").is_ok());
}

#[test]
fn port_in_authority_ignored() {
    // SNI never carries a port (RFC 6066 §3); compare on host only.
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
