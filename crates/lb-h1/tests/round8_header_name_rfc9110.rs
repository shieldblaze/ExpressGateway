//! ROUND8-L7-03 — strict RFC 9110 §5.1 `field-name = 1*tchar` lexer.
//!
//! References:
//! - HAProxy CVE-2023-25725 (Critical CVSS 9.1, empty header name
//!   truncated the parsed list).
//! - nginx CVE-2019-9516 (zero-length names exhausted memory).
//! - RFC 9110 §5.1 / §5.6.2 (`tchar` ABNF).
//! - RFC 9112 §5.1 ("no whitespace allowed between field-name and
//!   colon").

use lb_h1::{H1Error, parse_headers};

#[test]
fn empty_name_rejected() {
    let buf = b":value\r\n\r\n";
    let err = parse_headers(buf).unwrap_err();
    assert!(matches!(err, H1Error::InvalidHeader(_)), "got {err:?}");
}

#[test]
fn whitespace_in_name_rejected() {
    // RFC 9112 §5.1 forbids whitespace inside the name token. The
    // previous lexer trimmed silently; this seed pins the strict
    // behaviour.
    let buf = b"X Token: v\r\n\r\n";
    let err = parse_headers(buf).unwrap_err();
    assert!(matches!(err, H1Error::InvalidHeader(_)));
}

#[test]
fn leading_whitespace_in_name_rejected() {
    let buf = b" X-Token: v\r\n\r\n";
    let err = parse_headers(buf).unwrap_err();
    assert!(matches!(err, H1Error::InvalidHeader(_)));
}

#[test]
fn control_char_in_name_rejected() {
    let buf = b"X\x01Token: v\r\n\r\n";
    let err = parse_headers(buf).unwrap_err();
    assert!(matches!(err, H1Error::InvalidHeader(_)));
}

#[test]
fn tab_in_name_rejected() {
    let buf = b"X\tToken: v\r\n\r\n";
    let err = parse_headers(buf).unwrap_err();
    assert!(matches!(err, H1Error::InvalidHeader(_)));
}

#[test]
fn null_byte_in_name_rejected() {
    let buf = b"X\x00Token: v\r\n\r\n";
    let err = parse_headers(buf).unwrap_err();
    assert!(matches!(err, H1Error::InvalidHeader(_)));
}

#[test]
fn valid_token_chars_accepted() {
    // Every RFC 9110 §5.6.2 special tchar that's not a letter/digit.
    let buf = b"X-!-#-$-%-&-'-*-+--.-^-_-`-|-~: ok\r\n\r\n";
    let (headers, _consumed) = parse_headers(buf).unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].1, "ok");
}

#[test]
fn value_whitespace_still_trimmed() {
    // Regression: OWS around the value MUST still be trimmed per
    // RFC 9110 §5.5; only the name side is strict.
    let buf = b"X-Token:   v\r\n\r\n";
    let (headers, _) = parse_headers(buf).unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].0, "X-Token");
    assert_eq!(headers[0].1, "v");
}

#[test]
fn underscore_in_name_accepted_default() {
    // ROUND8-L7-05 is a separate policy knob (default reject) — but
    // until that knob lands the lexer must accept underscore in the
    // name because the RFC 9110 token grammar does (HTTP/2 disallows
    // it implicitly via lowercase ASCII rules; H1 allows it).
    let buf = b"X_Internal: v\r\n\r\n";
    let (headers, _) = parse_headers(buf).unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].0, "X_Internal");
}
