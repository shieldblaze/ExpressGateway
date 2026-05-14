//! Named CVE-class regression seeds for the chunk-size lexer hardening
//! shipped by ROUND8-L7-02.
//!
//! Every test cites the CVE/GHSA that motivates it; failure of any
//! assertion is a smuggling-class regression.
//!
//! References:
//! - nginx CVE-2013-2028 (RCE-territory stack overflow on chunked).
//! - hyper GHSA-5h46-h7hh-c6x9 (integer overflow on chunk size).
//! - HAProxy `BUG/MAJOR: mux_h1: fix stack buffer overflow in
//!   h1_append_chunk_size`.

use lb_h1::ChunkedDecoder;

/// `+5` chunk-size byte — RFC 9112 §7.1.1 ABNF disallows leading
/// signs. nginx, hyper, and HAProxy all reject; pre-fix we accepted.
#[test]
fn rejects_plus_sign() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"+5\r\nhello\r\n0\r\n\r\n").is_err());
}

/// `-5` chunk-size byte — same class as `+`, distinct minus path.
#[test]
fn rejects_minus_sign() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"-5\r\nhello\r\n0\r\n\r\n").is_err());
}

/// Leading space — HAProxy `h1_append_chunk_size` smuggle class.
#[test]
fn rejects_leading_whitespace_space() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b" 5\r\nhello\r\n0\r\n\r\n").is_err());
}

/// Leading tab — same as above; HAProxy specifically called out
/// tabs in the post-fix changelog.
#[test]
fn rejects_leading_whitespace_tab() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"\t5\r\nhello\r\n0\r\n\r\n").is_err());
}

/// Trailing whitespace inside the size token (`5 ` before CRLF) —
/// strictly invalid per RFC 9112 §7.1.1. Pre-fix our `.trim()`
/// silently accepted.
#[test]
fn rejects_trailing_whitespace_in_size_token() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"5 \r\nhello\r\n0\r\n\r\n").is_err());
}

/// 17 hex digits — even when the numeric value would still fit in
/// u64 after some interpretation, the lexer must reject overlong
/// inputs (nginx CVE-2013-2028 class — leading-zero pad attack).
#[test]
fn rejects_overlong_hex_zero_pad() {
    // 17 hex chars, numeric value = 5.
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"00000000000000005\r\nhello\r\n0\r\n\r\n").is_err());
}

/// 17 hex digits with non-zero leading nibble — `checked_shl`
/// catches the genuine overflow path as a defence-in-depth assertion
/// on top of the 16-digit cap.
#[test]
fn rejects_overflow_via_checked_shl() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"10000000000000000\r\nhello\r\n0\r\n\r\n").is_err());
}

/// `ffffffffffffffff` (16 hex digits, u64::MAX). The lexer must
/// accept it as a valid size token. The framer's next-level size
/// caps (max_body_size) reject as a separate concern; that path is
/// platform-dependent — usize::try_from on 32-bit will fail and
/// surface `InvalidChunkEncoding` here. We only assert that the
/// lexer didn't reject for the wrong reason: on 64-bit u64::MAX
/// converts to usize cleanly. We only assert that the feed call
/// either succeeds in moving past the size line or rejects with
/// `InvalidChunkEncoding` (32-bit truncation guard), never panics.
#[cfg(target_pointer_width = "64")]
#[test]
fn accepts_u64_max_size_token_on_64bit() {
    let mut d = ChunkedDecoder::new();
    // Asks for u64::MAX bytes of data — the decoder will move
    // into ReadingData state and need more bytes; `feed` returns
    // Ok(false) because the chunk body is not yet supplied. The
    // important thing is no panic + no error on the lexer step.
    let r = d.feed(b"ffffffffffffffff\r\n");
    assert!(
        r.is_ok(),
        "u64::MAX size should be accepted by lexer; got {r:?}"
    );
}

/// Chunk extensions (`5;ext=foo`) MUST still be accepted — the
/// hex prefix is `5`, the extension is ignored per RFC 9112 §7.1.1.
#[test]
fn accepts_chunk_extensions() {
    let mut d = ChunkedDecoder::new();
    let ok = d.feed(b"5;ext=foo\r\nhello\r\n0\r\n\r\n").unwrap();
    assert!(ok, "chunked body with extension should decode");
    let body: Vec<u8> = d
        .take_body()
        .iter()
        .flat_map(|b| b.iter().copied())
        .collect();
    assert_eq!(body, b"hello");
}

/// Empty size token before `;` — `;ext=foo\r\n…` — must reject.
#[test]
fn rejects_empty_size_token_before_extension() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b";ext=foo\r\nhello\r\n0\r\n\r\n").is_err());
}

/// Empty line entirely — `\r\nhello…` — must reject (zero hex digits).
#[test]
fn rejects_empty_size_line() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"\r\nhello\r\n0\r\n\r\n").is_err());
}

/// Non-tchar / non-hexdig byte inside the size token (`5x` — `x`
/// is not a hex digit). Pre-fix `usize::from_str_radix` rejected;
/// keep behaviour pinned.
#[test]
fn rejects_non_hex_inside_size_token() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"5x\r\nhello\r\n0\r\n\r\n").is_err());
}

/// Both-case hex digits in the same token (`AaBb`) — the lexer is
/// case-insensitive per `HEXDIG` ABNF.
#[test]
fn accepts_mixed_case_hex() {
    let mut d = ChunkedDecoder::new();
    // 0xAaBb = 43_707 — we won't supply that much body in the test,
    // just verify the lexer moved past the size line without error.
    let r = d.feed(b"AaBb\r\n");
    assert!(r.is_ok(), "mixed-case hex should be accepted; got {r:?}");
}

/// Regression: well-formed multi-chunk decode still works end to end.
#[test]
fn regression_well_formed_chunked_decodes() {
    let mut d = ChunkedDecoder::new();
    let done = d.feed(b"5\r\nHello\r\n6\r\n World\r\n0\r\n\r\n").unwrap();
    assert!(done);
    let body: Vec<u8> = d
        .take_body()
        .iter()
        .flat_map(|b| b.iter().copied())
        .collect();
    assert_eq!(body, b"Hello World");
}
