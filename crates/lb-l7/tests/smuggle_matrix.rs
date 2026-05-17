//! PROTO-2-10 — Smuggle defence matrix proof tests.
//!
//! Each test exercises one row of `audit/protocol/SMUGGLE-MATRIX.md`,
//! confirming what the gateway-level `SmuggleDetector` catches in
//! default `SmuggleMode::H1` vs. `SmuggleMode::H1Strict` vs.
//! `SmuggleMode::H2`. Hyper-level (wire-decoder) behaviour is
//! recorded in the matrix doc but not asserted here — those checks
//! belong inside `hyper` itself.
//!
//! The proxy hot path already wires the detector (see commits
//! `e00e85a` SEC-2-01 + `dc02517` CODE-2-01); this test surface
//! locks the per-mode behaviour so any future detector refactor that
//! drifts the matrix is caught at CI.

use lb_security::{SmuggleDetector, SmuggleMode};

fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn test_default_strict_te() {
    // Cell #7 — `Transfer-Encoding: gzip, chunked` (codec chain,
    // final chunked).
    //   default H1 → pass (final encoding is chunked).
    //   H1Strict  → REJECT (codec list contains a non-chunked
    //                token).
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
    // Cell #4 — Content-Length AND Transfer-Encoding both present.
    // Both modes must reject — RFC 9112 §6.1 forbids ambiguity.
    let headers = h(&[("Content-Length", "10"), ("Transfer-Encoding", "chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_err());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_err());
}

#[test]
fn test_duplicate_cl_differing() {
    // Cell #3 — duplicate Content-Length headers with differing
    // values. RFC 9110 §8.6: reject.
    let headers = h(&[("Content-Length", "10"), ("Content-Length", "20")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_err());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}

#[test]
fn test_duplicate_cl_identical_accepted() {
    // Cell #2 — duplicate CL with identical values is allowed by
    // RFC 9110 §8.6 ("having different values"). Detector accepts.
    let headers = h(&[("Content-Length", "10"), ("Content-Length", "10")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_ok());
}

#[test]
fn test_te_non_chunked_final() {
    // Cell #6 — TE without final chunked. Both modes reject.
    let headers = h(&[("Transfer-Encoding", "gzip")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_err());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}

#[test]
fn test_te_identity_rejected() {
    // Cell #10 — `Transfer-Encoding: identity` is not a valid final
    // encoding per RFC 9112 §6.1.
    let headers = h(&[("Transfer-Encoding", "identity")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_err());
}

#[test]
fn test_h2_downgrade_te_chunked_rejected() {
    // Cell #12 — H2 request carrying Transfer-Encoding: chunked is
    // forbidden by RFC 9113 §8.2.2.
    let headers = h(&[("transfer-encoding", "chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_err());
}

#[test]
fn test_h2_downgrade_connection_rejected() {
    // Cell #13 — H2 request carrying Connection: keep-alive is
    // forbidden by RFC 9113 §8.2.2.
    let headers = h(&[("connection", "keep-alive")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_err());
}

#[test]
fn test_h2_te_trailers_accepted() {
    // Cell #14 — H2 carrying `TE: trailers` is the one allowed TE
    // value per RFC 9113 §8.2.2.
    let headers = h(&[("te", "trailers")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_ok());
}

#[test]
fn test_h2_te_non_trailers_rejected() {
    // Cell #15 — Any other TE value in H2 must reject.
    let headers = h(&[("te", "gzip")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H2).is_err());
}

#[test]
fn test_h1_default_accepts_plain_chunked() {
    // Cell #5 — `Transfer-Encoding: chunked` is the canonical
    // accepted form across all modes.
    let headers = h(&[("Transfer-Encoding", "chunked")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_ok());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_ok());
}

#[test]
fn test_strict_rejects_codec_chain_with_chunked_first() {
    // Cell #8 — `Transfer-Encoding: chunked, gzip` (final NOT
    // chunked). Both modes reject.
    let headers = h(&[("Transfer-Encoding", "chunked, gzip")]);
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_err());
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}

#[test]
fn test_strict_rejects_empty_codec() {
    // Cell #18 — leading-empty codec inside a TE codec list.
    let headers = h(&[("Transfer-Encoding", " , chunked")]);
    // Default H1 mode: the final codec is `chunked`, so this passes
    // the `check_te_cl` final-encoding check.
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1).is_ok());
    // H1Strict: rejects because the first codec is empty.
    assert!(SmuggleDetector::check_all_mode(&headers, SmuggleMode::H1Strict).is_err());
}
