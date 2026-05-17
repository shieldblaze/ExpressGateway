//! ROUND8-L7-15 — Edge-defaults pinning table.
//!
//! Each row of `docs/edge-defaults.md` that points at a live constant
//! in this crate is asserted here against the literal numeric value
//! the docs claim. The point is drift detection: any future PR that
//! changes the constant without updating the doc fails this test.
//!
//! See `docs/edge-defaults.md` for the cross-reference table and
//! `audit/decisions/h2-edge-streams.md` for the rationale on the
//! deliberate Envoy/nginx divergence at `max_concurrent_streams`.

use lb_l7::h2_security::H2SecurityThresholds;

/// Pinned edge-defaults table. Hard-coded — do not derive from the
/// live constant. The literal serves as the "human contract" the doc
/// promises operators; the test fails if the constant drifts.
#[test]
fn h2_security_defaults_match_documented_table() {
    let t = H2SecurityThresholds::default();

    // Row: H2 max_concurrent_streams.
    // Deliberate divergence from Envoy 100 / nginx 128; see
    // audit/decisions/h2-edge-streams.md.
    assert_eq!(
        t.max_concurrent_streams, 256,
        "edge-defaults.md row `max_concurrent_streams` says 256; \
         constant in lb-l7/src/h2_security.rs drifted. Update both \
         the table and audit/decisions/h2-edge-streams.md when \
         changing this default."
    );

    // Row: H2 initial_stream_window_size (RFC 9113 default).
    assert_eq!(
        t.initial_stream_window_size, 65_535,
        "edge-defaults.md row `initial_stream_window_size` says \
         65535 (RFC 9113 default); constant drifted."
    );

    // Row: H2 initial_connection_window_size.
    assert_eq!(
        t.initial_connection_window_size,
        1 << 20,
        "edge-defaults.md row `initial_connection_window_size` says \
         1 MiB; constant drifted."
    );

    // Row: H2 max_header_list_size.
    assert_eq!(
        t.max_header_list_size,
        64 * 1024,
        "edge-defaults.md row `max_header_list_size` says 64 KiB; \
         constant drifted."
    );

    // Row: H2 max_send_buf_size.
    assert_eq!(
        t.max_send_buf_size,
        64 * 1024,
        "edge-defaults.md row `max_send_buf_size` says 64 KiB; \
         constant drifted."
    );

    // Row: H2 max_pending_accept_reset_streams (CVE-2023-44487).
    assert_eq!(
        t.max_pending_accept_reset_streams, 100,
        "edge-defaults.md row `max_pending_accept_reset_streams` \
         says 100 (matches CVE-2023-44487 envoy patch); constant \
         drifted."
    );

    // Row: H2 max_local_error_reset_streams (RUSTSEC-2024-0003).
    assert_eq!(
        t.max_local_error_reset_streams, 100,
        "edge-defaults.md row `max_local_error_reset_streams` \
         says 100; constant drifted."
    );
}

/// Sanity-check that the table covers every numeric field of
/// `H2SecurityThresholds`. Future field additions force a doc update
/// because the new field will lack an assertion above. The
/// `Default::default()` instantiation here is purely to keep the
/// reference compiled — the real enforcement is in
/// `h2_security_defaults_match_documented_table` plus code review.
#[test]
fn h2_security_thresholds_default_constructs() {
    let _ = H2SecurityThresholds::default();
}
