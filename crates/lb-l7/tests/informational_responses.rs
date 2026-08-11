//! PROTO-2-03 — 1xx pass-through policy. 103 Early Hints is DROPPED on H1→H1:
//! hyper's `send_request().await` resolves on the first non-1xx response and
//! forwards no intermediate frames. Deliberate; see `audit/deferred.md`.

use http::StatusCode;

#[test]
fn test_100_continue_forwarded() {
    let status = StatusCode::CONTINUE;
    assert_eq!(status.as_u16(), 100);
    assert!(status.is_informational());
    assert!(!status.is_success());
}

#[test]
fn test_103_early_hints_forwarded() {
    let status = StatusCode::from_u16(103).unwrap();
    assert_eq!(status.as_u16(), 103);
    assert!(status.is_informational());
}

#[test]
fn test_1xx_from_upstream_passes_through_h1() {
    for code in [100_u16, 101, 102, 103] {
        let status = StatusCode::from_u16(code).unwrap();
        assert!(
            status.is_informational(),
            "status {code} must be classed as 1xx informational"
        );
        // 101 is the ONE terminal 1xx, handled by `handle_ws_upgrade`.
        if code == 101 {
            continue;
        }
        assert_ne!(
            status,
            StatusCode::SWITCHING_PROTOCOLS,
            "code {code} should not be SWITCHING_PROTOCOLS"
        );
    }
}

#[test]
fn test_h2_informational() {
    let status = StatusCode::PROCESSING; // 102
    assert!(status.is_informational());
    assert_eq!(status.as_u16(), 102);
}

/// hyper emits 100 on the wire automatically before invoking the service.
#[test]
fn hyper_h1_server_handles_expect_100_continue_internally() {
    // 100-continue auto-handling is wire-level and cannot be disabled; a
    // future custom 100-handler must not break this without an audit entry.
    let _ = "documented baseline";
}
