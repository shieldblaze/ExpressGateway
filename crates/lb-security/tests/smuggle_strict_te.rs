//! Proof for the strict Transfer-Encoding codec policy (SEC-2-01 /
//! SEC-2-15 matrix). The lenient default in
//! `SmuggleDetector::check_te_cl` only enforces that the **final**
//! codec is `chunked`; strict mode collapses the accept set to the
//! single-token form so codec-chain mis-implementation in the
//! upstream cannot smuggle a body.
//!
//! Exercises the new `SmuggleDetector::check_te_strict` helper plus
//! the mode-aware `SmuggleDetector::check_all_mode(_, SmuggleMode::H1Strict)`
//! variant.

use lb_security::{SmuggleDetector, SmuggleMode};

fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
        .collect()
}

// ---- check_te_strict standalone ----

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
    // Final codec is gzip — already rejected by lenient check_te_cl,
    // but the strict policy must also catch it.
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
    // `chunked,` parses as ["chunked", ""] — the empty codec is its
    // own smell, reject.
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
    // No Transfer-Encoding at all is the trivially-OK case.
    let headers = h(&[("content-type", "application/json")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_ok());
}

#[test]
fn strict_te_multiple_te_headers_each_checked() {
    // Two separate Transfer-Encoding header lines (RFC permits
    // splitting). Strict mode must reject if any line violates.
    let headers = h(&[
        ("transfer-encoding", "chunked"),
        ("transfer-encoding", "gzip"),
    ]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_err());
}

#[test]
fn strict_te_internal_whitespace_normalised() {
    // `   chunked   ` after split-trim should accept.
    let headers = h(&[("transfer-encoding", "   chunked   ")]);
    assert!(SmuggleDetector::check_te_strict(&headers).is_ok());
}

// ---- check_all_mode(_, H1Strict) integration ----

#[test]
fn check_all_mode_strict_rejects_gzip_chunked() {
    let headers = h(&[("transfer-encoding", "gzip, chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}

#[test]
fn check_all_mode_lenient_accepts_gzip_chunked() {
    // Regression guard: the lenient default behaviour must NOT change
    // when the strict path is added.
    let headers = h(&[("transfer-encoding", "gzip, chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_ok());
}

#[test]
fn check_all_mode_strict_still_catches_cl_te() {
    // Strict mode is additive — the existing CL+TE check still fires.
    let headers = h(&[("content-length", "5"), ("transfer-encoding", "chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}

#[test]
fn check_all_mode_h2_runs_downgrade_check() {
    // Mode H2 path: connection header on H2 is a downgrade reject.
    let headers = h(&[("connection", "keep-alive")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_err());
}

#[test]
fn check_all_mode_h2_te_trailers_ok() {
    // Regression guard for the H2 path: `TE: trailers` is the one
    // accepted TE value under H2 per RFC 9113 §8.2.2.
    let headers = h(&[("te", "trailers")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_ok());
}
