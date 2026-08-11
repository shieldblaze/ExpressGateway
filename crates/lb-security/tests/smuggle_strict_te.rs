//! Proof for the strict Transfer-Encoding codec policy (SEC-2-15).

use lb_security::{SmuggleDetector, SmuggleMode};

fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
        .collect()
}

#[test]
fn strict_te_chunked_alone_ok() {
    let headers = h(&[("transfer-encoding", "chunked")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_ok());
}

#[test]
fn strict_te_chunked_alone_case_insensitive_ok() {
    let headers = h(&[("Transfer-Encoding", "Chunked")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_ok());
}

#[test]
fn strict_te_gzip_chunked_rejected() {
    let headers = h(&[("transfer-encoding", "gzip, chunked")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_err());
}

#[test]
fn strict_te_chunked_gzip_rejected() {
    // Lenient check_te_cl already rejects this; strict must too.
    let headers = h(&[("transfer-encoding", "chunked, gzip")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_err());
}

#[test]
fn strict_te_identity_rejected() {
    let headers = h(&[("transfer-encoding", "identity")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_err());
}

#[test]
fn strict_te_deflate_rejected() {
    let headers = h(&[("transfer-encoding", "deflate, chunked")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_err());
}

#[test]
fn strict_te_trailing_empty_codec_rejected() {
    // Parses as ["chunked", ""]; the empty codec is its own smell.
    let headers = h(&[("transfer-encoding", "chunked,")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_err());
}

#[test]
fn strict_te_leading_empty_codec_rejected() {
    let headers = h(&[("transfer-encoding", ",chunked")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_err());
}

#[test]
fn strict_te_no_te_header_ok() {
    let headers = h(&[("content-type", "application/json")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_ok());
}

#[test]
fn strict_te_multiple_te_headers_each_checked() {
    // The RFC permits splitting across lines; a violation on ANY line must reject.
    let headers = h(&[
        ("transfer-encoding", "chunked"),
        ("transfer-encoding", "gzip"),
    ]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_err());
}

#[test]
fn strict_te_internal_whitespace_normalised() {
    let headers = h(&[("transfer-encoding", "   chunked   ")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_ok());
}

#[test]
fn check_all_mode_strict_rejects_gzip_chunked() {
    let headers = h(&[("transfer-encoding", "gzip, chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}

#[test]
fn check_all_mode_lenient_accepts_gzip_chunked() {
    // Regression guard: adding the strict path must not shift the lenient default.
    let headers = h(&[("transfer-encoding", "gzip, chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_ok());
}

#[test]
fn check_all_mode_strict_still_catches_cl_te() {
    let headers = h(&[("content-length", "5"), ("transfer-encoding", "chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}

#[test]
fn check_all_mode_h2_runs_downgrade_check() {
    let headers = h(&[("connection", "keep-alive")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_err());
}

#[test]
fn check_all_mode_h2_te_trailers_ok() {
    // RFC 9113 §8.2.2: `TE: trailers` is the ONLY accepted TE value under H2.
    let headers = h(&[("te", "trailers")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_ok());
}
