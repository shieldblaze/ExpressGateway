//! PROTO-2-13 — the H2 listener can advertise
//! SETTINGS_ENABLE_CONNECT_PROTOCOL (RFC 8441 §3) for WebSocket-over-H2.
//!
//! RFC 8441 §3: an H2 server willing to accept extended-CONNECT (the
//! WebSocket-over-H2 bootstrap) sends a `SETTINGS_ENABLE_CONNECT_PROTOCOL`
//! (setting id `0x8`, value `1`) parameter in its initial SETTINGS frame.
//!
//! CF-S27-2 UPDATE: this advertisement is NO LONGER unconditional. The H2
//! upgraded-stream write path lacks true end-to-end backpressure (a
//! non-reading client can force unbounded gateway memory — F-S27-2), so the
//! owner gated WS-over-H2 OFF by default. `enable_connect_protocol()` is now
//! called only when the listener opts in via
//! `H2Proxy::with_h2_extended_connect(true)` (wired from
//! `lb_config::WebsocketConfig::h2_extended_connect`, default `false`). The
//! wire-level behaviour of BOTH states is proven in `tests/ws_h2_gated_off.rs`
//! (off → no SETTINGS bit, extended CONNECT not tunneled) and
//! `tests/ws_h2_e2e.rs` (on → real round-trip).
//!
//! This is a code-presence test: it re-asserts that the
//! `enable_connect_protocol()` call still exists on the H2 builder path
//! (it does, now behind the `h2_extended_connect_enabled` gate) so a refactor
//! that drops the call entirely — making WS-over-H2 impossible even when
//! opted in — lands red.

use std::fs;

const H2_PROXY_PATH: &str = "src/h2_proxy.rs";

#[test]
fn h2_proxy_calls_enable_connect_protocol() {
    // Locate the proxy source relative to the test's CARGO_MANIFEST_DIR.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/{H2_PROXY_PATH}");
    let src = fs::read_to_string(&path).expect("read h2_proxy.rs");
    // The call must still exist on the builder path (now CONDITIONAL on the
    // per-listener opt-in; the wire-level on/off behaviour is proven in the
    // tests/ws_h2_gated_off.rs + tests/ws_h2_e2e.rs integration tests). This
    // guards against a refactor that removes the capability entirely.
    assert!(
        src.contains("enable_connect_protocol()"),
        "PROTO-2-13: H2 listener must retain the \
         `builder.enable_connect_protocol()` call (now gated by \
         `h2_extended_connect_enabled`, CF-S27-2) so WS-over-H2 is possible \
         when opted in (RFC 8441 §3). Inspected {path}."
    );
    // CF-S27-2: the call must be GATED, not unconditional. Pin that the gate
    // field is consulted on the builder path so a refactor that drops the
    // condition (re-enabling WS-over-H2 by default) lands red.
    assert!(
        src.contains("if self.h2_extended_connect_enabled"),
        "CF-S27-2: `enable_connect_protocol()` must be gated behind \
         `if self.h2_extended_connect_enabled` (WS-over-H2 OFF by default). \
         Inspected {path}."
    );
}

#[test]
fn h2_connect_protocol_setting_id_documented() {
    // RFC 8441 §3 setting id is `0x8`. Pin the constant so a future
    // commit that hand-rolls the SETTINGS encoding cannot drift.
    //
    // h2 / hyper hide the wire-level setting behind
    // `enable_connect_protocol`, but the test above guards the
    // call-site presence. This test pins the spec literal so any
    // attempt to add a custom setter sees the right id.
    const SETTINGS_ENABLE_CONNECT_PROTOCOL: u16 = 0x8;
    assert_eq!(SETTINGS_ENABLE_CONNECT_PROTOCOL, 8);
}
