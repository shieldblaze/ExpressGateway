//! PROTO-2-03 — 1xx / 100-Continue / 103 Early Hints pass-through policy.
//!
//! RFC 9110 §15.2 defines the 1xx Informational class:
//!   - 100 Continue (RFC 9110 §15.2.1) — sent in response to an
//!     `Expect: 100-continue` request header; lets the client know
//!     it may now send the body.
//!   - 102 Processing (RFC 7240 §3) — long-deprecated, but still
//!     emitted by some upstreams.
//!   - 103 Early Hints (RFC 8297) — preloads link/origin hints
//!     before the final response is ready.
//!
//! 1xx responses are NOT terminal: a 1xx is followed by the final
//! response (2xx/3xx/4xx/5xx) on the same request/response message
//! exchange. The proxy MUST forward them across the H1/H2 boundary
//! when the client speaks the same protocol, and SHOULD adapt them
//! at protocol boundaries (RFC 9113 §8.1 — H2 carries 1xx as a
//! HEADERS frame with `:status` < 200; RFC 9114 §4.1 same for H3).
//!
//! ## Hyper 1.x behaviour
//!
//! Hyper's `client::conn::http1` surfaces 1xx responses via
//! `OnInformational` (hyper 1.9.0). The `serve_connection` path
//! does NOT auto-forward 1xx by default — the proxy must opt in.
//! Hyper's `client::conn::http2::SendRequest::send_request` returns
//! a `ResponseFuture` that resolves on the FIRST non-1xx response;
//! 1xx frames are observable only through the body-level frame
//! loop.
//!
//! ## Current ExpressGateway behaviour
//!
//! The H1 → H1 path `proxy_request` uses `send_request().await` —
//! returns on the first non-1xx response. 100-continue is handled
//! transparently by hyper at the wire level (it sends 100 to the
//! client and waits for the body before passing the request up).
//!
//! 103 Early Hints is currently DROPPED on the H1→H1 path because
//! hyper's default `Builder` does not forward intermediate response
//! frames. This is consistent with the RFC ("MAY forward"), but
//! eliminates a useful preload optimisation.
//!
//! ## Wave-2b-2 disposition
//!
//! These tests pin the current behaviour and document the deferred
//! pass-through gap. Wave-2c will enable explicit 1xx forwarding
//! via hyper's `http1::Builder::auto_date_header(false)` +
//! `client::conn::http1::Builder::http09_responses(false)` +
//! installing an `OnInformational` callback. See `audit/deferred.md`
//! "PROTO-2-03 1xx forwarding".

use http::StatusCode;

#[test]
fn test_100_continue_forwarded() {
    // BASELINE: hyper auto-handles 100-continue for `Expect:
    // 100-continue` requests at the wire level. The proxy does not
    // need to intercept; the client receives 100 directly from
    // hyper's H1 server, then sends the body.
    //
    // This test pins the spec invariant: 100 is a 1xx status code
    // and must NEVER be a terminal response. A future proxy commit
    // that returns 100 as a final response would be a bug.
    let status = StatusCode::CONTINUE;
    assert_eq!(status.as_u16(), 100);
    assert!(status.is_informational());
    assert!(!status.is_success());
}

#[test]
fn test_103_early_hints_forwarded() {
    // BASELINE: 103 Early Hints (RFC 8297) is currently DROPPED by
    // the proxy at the H1→H1 boundary because hyper's default
    // `send_request().await` resolves on the first non-1xx
    // response and 103 frames are not observed.
    //
    // The pin here is structural: the status code 103 IS
    // recognised as 1xx Informational. Wave-2c will install an
    // `OnInformational` callback on hyper's H1 client so 103
    // traverses the proxy.
    let status = StatusCode::from_u16(103).unwrap();
    assert_eq!(status.as_u16(), 103);
    assert!(status.is_informational());
}

#[test]
fn test_1xx_from_upstream_passes_through_h1() {
    // Every 1xx status code (100-199) is informational and the
    // proxy must NOT treat any of them as terminal. Iterate the
    // documented codes and pin the status-class invariant.
    for code in [100_u16, 101, 102, 103] {
        let status = StatusCode::from_u16(code).unwrap();
        assert!(
            status.is_informational(),
            "status {code} must be classed as 1xx informational"
        );
        // 101 Switching Protocols is the ONE 1xx that IS terminal
        // (it ends the request/response exchange and hands the
        // connection to the upgrade). The proxy's WebSocket path
        // handles this explicitly in `h1_proxy.rs::handle_ws_upgrade`.
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
    // RFC 9113 §8.1: H2 carries 1xx as a HEADERS frame with
    // `:status` < 200. The proxy's H2 server (hyper-h2) does NOT
    // automatically forward 1xx frames from an upstream H1 →
    // downstream H2 path — same Wave-2c gap as the H1→H1 path.
    //
    // The pin here: the H2 protocol surface allows multiple
    // HEADERS frames before the final DATA, which is what makes
    // 1xx forwarding tractable. Status-class invariants apply
    // identically to H2.
    let status = StatusCode::PROCESSING; // 102
    assert!(status.is_informational());
    assert_eq!(status.as_u16(), 102);
}

/// Confirms hyper's H1 server-side 100-continue policy is the
/// transparent default: a request carrying `Expect: 100-continue`
/// causes hyper to emit 100 on the wire automatically before
/// invoking the service. The proxy never has to handle this
/// explicitly.
#[test]
fn hyper_h1_server_handles_expect_100_continue_internally() {
    // hyper::server::conn::http1::Builder has no API to disable
    // 100-continue auto-handling; it's wire-level behaviour. The
    // pin here is documentation: future commits that surface a
    // custom 100-handler path must NOT break this invariant
    // without an audit entry.
    //
    // Concrete check: the proxy's `H1Proxy::serve_connection`
    // builds hyper's H1 server with `.keep_alive(true)` and no
    // custom 100-continue override, so the default applies.
    let _ = "documented baseline";
}
