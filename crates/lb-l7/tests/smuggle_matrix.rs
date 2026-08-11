//! PROTO-2-10 — one test per row of `audit/protocol/SMUGGLE-MATRIX.md`, so a
//! detector refactor that drifts the matrix is caught at CI.

use lb_security::{SmuggleDetector, SmuggleMode};

fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn test_default_strict_te() {
    // Cell #7 — final codec IS chunked, so only H1Strict rejects.
    let headers = h(&[("Transfer-Encoding", "gzip, chunked")]);
    assert!(
        SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_ok(),
        "default H1 mode accepts gzip,chunked per RFC 9112 §6.1"
    );
    assert!(
        SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err(),
        "H1Strict mode rejects any non-chunked codec"
    );
}

#[test]
fn test_pipelined_cl_te() {
    // Cell #4 — CL+TE: RFC 9112 §6.1 forbids the ambiguity.
    let headers = h(&[("Content-Length", "10"), ("Transfer-Encoding", "chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_err());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_err());
}

#[test]
fn test_duplicate_cl_differing() {
    // Cell #3 — duplicate CL, differing values: RFC 9110 §8.6 rejects.
    let headers = h(&[("Content-Length", "10"), ("Content-Length", "20")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_err());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}

#[test]
fn test_duplicate_cl_identical_accepted() {
    // Cell #2 — duplicate CL, IDENTICAL values: RFC 9110 §8.6 allows it.
    let headers = h(&[("Content-Length", "10"), ("Content-Length", "10")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_ok());
}

#[test]
fn test_te_non_chunked_final() {
    // Cell #6 — TE without final chunked.
    let headers = h(&[("Transfer-Encoding", "gzip")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_err());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}

#[test]
fn test_te_identity_rejected() {
    // Cell #10 — `identity` is not a valid final encoding (RFC 9112 §6.1).
    let headers = h(&[("Transfer-Encoding", "identity")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_err());
}

#[test]
fn test_h2_downgrade_te_chunked_rejected() {
    // Cell #12 — TE in H2 is forbidden (RFC 9113 §8.2.2).
    let headers = h(&[("transfer-encoding", "chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_err());
}

#[test]
fn test_h2_downgrade_connection_rejected() {
    // Cell #13 — hop-by-hop in H2 is forbidden (RFC 9113 §8.2.2).
    let headers = h(&[("connection", "keep-alive")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_err());
}

#[test]
fn test_h2_te_trailers_accepted() {
    // Cell #14 — `TE: trailers` is the one allowed H2 TE value.
    let headers = h(&[("te", "trailers")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_ok());
}

#[test]
fn test_h2_te_non_trailers_rejected() {
    // Cell #15 — any other H2 TE value rejects.
    let headers = h(&[("te", "gzip")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_err());
}

#[test]
fn test_h1_default_accepts_plain_chunked() {
    // Cell #5 — the canonical accepted form across all modes.
    let headers = h(&[("Transfer-Encoding", "chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_ok());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_ok());
}

#[test]
fn test_strict_rejects_codec_chain_with_chunked_first() {
    // Cell #8 — final codec NOT chunked.
    let headers = h(&[("Transfer-Encoding", "chunked, gzip")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_err());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}

#[test]
fn test_strict_rejects_empty_codec() {
    // Cell #18 — leading-empty codec: only H1Strict rejects.
    let headers = h(&[("Transfer-Encoding", " , chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_ok());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}
