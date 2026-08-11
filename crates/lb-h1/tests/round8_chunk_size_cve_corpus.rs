//! ROUND8-L7-02 — CVE-class seeds for the chunk-size lexer; a failure here is
//! a smuggling-class regression. nginx CVE-2013-2028, hyper
//! GHSA-5h46-h7hh-c6x9, HAProxy `BUG/MAJOR: mux_h1: fix stack buffer overflow
//! in h1_append_chunk_size`.

use lb_h1::ChunkedDecoder;

/// `+5` — RFC 9112 §7.1.1 disallows leading signs; pre-fix we accepted.
#[test]
fn rejects_plus_sign() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"+5\r\nhello\r\n0\r\n\r\n").is_err());
}

/// `-5` — same class as `+`, distinct code path.
#[test]
fn rejects_minus_sign() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"-5\r\nhello\r\n0\r\n\r\n").is_err());
}

/// Leading space — the HAProxy `h1_append_chunk_size` smuggle class.
#[test]
fn rejects_leading_whitespace_space() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b" 5\r\nhello\r\n0\r\n\r\n").is_err());
}

/// Leading tab — called out specifically in HAProxy's post-fix changelog.
#[test]
fn rejects_leading_whitespace_tab() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"\t5\r\nhello\r\n0\r\n\r\n").is_err());
}

/// `5 ` before CRLF — invalid per RFC 9112 §7.1.1; the pre-fix `.trim()`
/// silently accepted it.
#[test]
fn rejects_trailing_whitespace_in_size_token() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"5 \r\nhello\r\n0\r\n\r\n").is_err());
}

/// 17 hex digits must reject even when the VALUE fits u64 — the nginx
/// CVE-2013-2028 leading-zero pad class.
#[test]
fn rejects_overlong_hex_zero_pad() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"00000000000000005\r\nhello\r\n0\r\n\r\n").is_err());
}

/// 17 hex digits with a non-zero leading nibble — the genuine overflow path.
#[test]
fn rejects_overflow_via_checked_shl() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"10000000000000000\r\nhello\r\n0\r\n\r\n").is_err());
}

/// 16 hex digits (`u64::MAX`) is a VALID size token. The assertion is
/// deliberately weak because the outcome is platform-dependent: on 32-bit
/// `usize::try_from` surfaces `InvalidChunkEncoding` here. Either outcome is
/// fine; a panic is not.
#[cfg(target_pointer_width = "64")]
#[test]
fn accepts_u64_max_size_token_on_64bit() {
    let mut d = ChunkedDecoder::new();
    // No panic and no error on the LEXER step is the whole point.
    let r = d.feed(b"ffffffffffffffff\r\n");
    assert!(
        r.is_ok(),
        "u64::MAX size should be accepted by lexer; got {r:?}"
    );
}

/// Negative control: chunk extensions (`5;ext=foo`) MUST still be accepted.
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

/// Empty size token before `;` must reject.
#[test]
fn rejects_empty_size_token_before_extension() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b";ext=foo\r\nhello\r\n0\r\n\r\n").is_err());
}

/// An entirely empty line must reject (zero hex digits).
#[test]
fn rejects_empty_size_line() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"\r\nhello\r\n0\r\n\r\n").is_err());
}

/// A non-hexdig byte inside the size token (`5x`) must reject.
#[test]
fn rejects_non_hex_inside_size_token() {
    let mut d = ChunkedDecoder::new();
    assert!(d.feed(b"5x\r\nhello\r\n0\r\n\r\n").is_err());
}

/// Negative control: `HEXDIG` is case-insensitive, so `AaBb` is valid.
#[test]
fn accepts_mixed_case_hex() {
    let mut d = ChunkedDecoder::new();
    // Only the lexer step is under test; the body is never supplied.
    let r = d.feed(b"AaBb\r\n");
    assert!(r.is_ok(), "mixed-case hex should be accepted; got {r:?}");
}

/// Negative control: a well-formed multi-chunk decode still works.
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
