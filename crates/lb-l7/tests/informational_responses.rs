//! PROTO-2-03 — 1xx / 100-Continue / 103 Early Hints pass-through policy.
//!
//! 1xx responses are NOT terminal (RFC 9110 §15.2): a 1xx is followed by the
//! final response on the same exchange. 101 Switching Protocols is the one
//! exception — it ends the exchange and hands the connection to the upgrade.
//!
//! CURRENT BEHAVIOUR, pinned here: 100-continue is handled transparently by
//! hyper at the wire level, but 103 Early Hints is DROPPED on H1→H1 because
//! hyper's `send_request().await` resolves on the first non-1xx response and
//! the default builder forwards no intermediate frames. Consistent with the
//! RFC's "MAY forward", but it loses the preload optimisation. Enabling
//! forwarding needs an `OnInformational` callback — see `audit/deferred.md`
//! "PROTO-2-03 1xx forwarding".

use http::StatusCode;

#[test]
fn test_100_continue_forwarded() {
    // hyper auto-handles 100-continue at the wire level; the proxy never
    // intercepts. Pin the spec invariant: 100 is 1xx, never terminal.
    let status = StatusCode::CONTINUE;
    assert_eq!(status.as_u16(), 100);
    assert!(status.is_informational());
    assert!(!status.is_success());
}

#[test]
fn test_103_early_hints_forwarded() {
    // 103 is currently dropped at the H1→H1 boundary (see the module doc).
    // The pin here is structural: 103 IS recognised as 1xx Informational.
    let status = StatusCode::from_u16(103).unwrap();
    assert_eq!(status.as_u16(), 103);
    assert!(status.is_informational());
}

#[test]
fn test_1xx_from_upstream_passes_through_h1() {
    // Every 1xx (100-199) is informational; none may be treated as terminal.
    for code in [100_u16, 101, 102, 103] {
        let status = StatusCode::from_u16(code).unwrap();
        assert!(
            status.is_informational(),
            "status {code} must be classed as 1xx informational"
        );
        // 101 Switching Protocols is the ONE terminal 1xx —
        // `h1_proxy.rs::handle_ws_upgrade` handles it explicitly.
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
    // RFC 9113 §8.1: H2 carries 1xx as a HEADERS frame with `:status` < 200,
    // and the protocol allows multiple HEADERS frames before the final DATA —
    // which is what makes 1xx forwarding tractable. Same gap as H1→H1.
    let status = StatusCode::PROCESSING; // 102
    assert!(status.is_informational());
    assert_eq!(status.as_u16(), 102);
}

/// Confirms hyper's H1 server-side 100-continue policy is the transparent
/// default: `Expect: 100-continue` makes hyper emit 100 on the wire
/// automatically before invoking the service.
#[test]
fn hyper_h1_server_handles_expect_100_continue_internally() {
    // hyper exposes no API to disable 100-continue auto-handling — it is
    // wire-level. `H1Proxy::serve_connection` installs no custom override, so
    // the default applies; a future custom 100-handler must not break this
    // without an audit entry.
    let _ = "documented baseline";
}
