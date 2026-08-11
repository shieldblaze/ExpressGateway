//! ROUND8-L7-15 — every `docs/edge-defaults.md` row that names a live constant
//! is asserted here, so changing one without the doc fails.

use lb_l7::h2_security::H2SecurityThresholds;

/// Hard-coded on purpose: the literal IS the contract. Do NOT derive it from
/// the live constant.
#[test]
fn h2_security_defaults_match_documented_table() {
    let t = H2SecurityThresholds::default();

    // Deliberate divergence from Envoy 100 / nginx 128
    // (audit/decisions/h2-edge-streams.md).
    assert_eq!(
        t.max_concurrent_streams, 256,
        "edge-defaults.md row `max_concurrent_streams` says 256; \
         constant in lb-l7/src/h2_security.rs drifted. Update both \
         the table and audit/decisions/h2-edge-streams.md when \
         changing this default."
    );

    assert_eq!(
        t.initial_stream_window_size, 65_535,
        "edge-defaults.md row `initial_stream_window_size` says \
         65535 (RFC 9113 default); constant drifted."
    );

    assert_eq!(
        t.initial_connection_window_size,
        1 << 20,
        "edge-defaults.md row `initial_connection_window_size` says \
         1 MiB; constant drifted."
    );

    assert_eq!(
        t.max_header_list_size,
        64 * 1024,
        "edge-defaults.md row `max_header_list_size` says 64 KiB; \
         constant drifted."
    );

    assert_eq!(
        t.max_send_buf_size,
        64 * 1024,
        "edge-defaults.md row `max_send_buf_size` says 64 KiB; \
         constant drifted."
    );

    assert_eq!(
        t.max_pending_accept_reset_streams, 100,
        "edge-defaults.md row `max_pending_accept_reset_streams` \
         says 100 (matches CVE-2023-44487 envoy patch); constant \
         drifted."
    );

    assert_eq!(
        t.max_local_error_reset_streams, 100,
        "edge-defaults.md row `max_local_error_reset_streams` \
         says 100; constant drifted."
    );
}

/// This does NOT force a doc update on a new field; the real enforcement is
/// `h2_security_defaults_match_documented_table` plus code review.
#[test]
fn h2_security_thresholds_default_constructs() {
    let _ = H2SecurityThresholds::default();
}
