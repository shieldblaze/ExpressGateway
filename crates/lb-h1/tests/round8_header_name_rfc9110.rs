//! ROUND8-L7-03 — strict RFC 9110 §5.1 `field-name = 1*tchar` lexer. HAProxy
//! CVE-2023-25725 (CVSS 9.1, an empty name truncated the parsed list); nginx
//! CVE-2019-9516 (zero-length names exhausted memory); RFC 9112 §5.1 forbids
//! whitespace between field-name and colon.

use lb_h1::{H1Error, parse_headers};

#[test]
fn empty_name_rejected() {
    let buf = b":value\r\n\r\n";
    let err = parse_headers(buf).unwrap_err();
    assert!(matches!(err, H1Error::InvalidHeader(_)), "got {err:?}");
}

#[test]
fn whitespace_in_name_rejected() {
    // The previous lexer trimmed silently; this pins the strict behaviour.
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
    let buf = b"X-!-#-$-%-&-'-*-+--.-^-_-`-|-~: ok\r\n\r\n";
    let (headers, _consumed) = parse_headers(buf).unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].1, "ok");
}

#[test]
fn value_whitespace_still_trimmed() {
    // Only the NAME side is strict: OWS around the value is still trimmed.
    let buf = b"X-Token:   v\r\n\r\n";
    let (headers, _) = parse_headers(buf).unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].0, "X-Token");
    assert_eq!(headers[0].1, "v");
}

#[test]
fn underscore_in_name_accepted_default() {
    // The lexer must ACCEPT `_` because RFC 9110's token grammar does;
    // rejecting it is the separate ROUND8-L7-05 policy knob.
    let buf = b"X_Internal: v\r\n\r\n";
    let (headers, _) = parse_headers(buf).unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].0, "X_Internal");
}
