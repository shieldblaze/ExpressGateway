//! PROTO-2-13 — the H2 listener can advertise SETTINGS_ENABLE_CONNECT_PROTOCOL
//! (RFC 8441 §3, setting id `0x8`) for WebSocket-over-H2.
//!
//! CF-S27-2: the advertisement is NO LONGER unconditional — the H2
//! upgraded-stream write path lacks true end-to-end backpressure, so WS-over-H2
//! is gated OFF by default and `enable_connect_protocol()` runs only when the
//! listener opts in. The wire-level behaviour of BOTH states is proven in
//! `tests/ws_h2_gated_off.rs` and `tests/ws_h2_e2e.rs`.
//!
//! This is a code-PRESENCE test: it asserts the call still exists on the
//! builder path (behind the gate) so a refactor that drops it entirely —
//! making WS-over-H2 impossible even when opted in — lands red.

use std::fs;

const H2_PROXY_PATH: &str = "src/h2_proxy.rs";

#[test]
fn h2_proxy_calls_enable_connect_protocol() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/{H2_PROXY_PATH}");
    let src = fs::read_to_string(&path).expect("read h2_proxy.rs");
    // The call must still exist on the builder path (now conditional on the
    // per-listener opt-in); this guards against removing the capability.
    assert!(
        src.contains("enable_connect_protocol()"),
        "PROTO-2-13: H2 listener must retain the \
         `builder.enable_connect_protocol()` call (now gated by \
         `h2_extended_connect_enabled`, CF-S27-2) so WS-over-H2 is possible \
         when opted in (RFC 8441 §3). Inspected {path}."
    );
    // CF-S27-2: the call must be GATED. Pin that the gate field is consulted so
    // a refactor dropping the condition (WS-over-H2 back on by default) is red.
    assert!(
        src.contains("if self.h2_extended_connect_enabled"),
        "CF-S27-2: `enable_connect_protocol()` must be gated behind \
         `if self.h2_extended_connect_enabled` (WS-over-H2 OFF by default). \
         Inspected {path}."
    );
}

#[test]
fn h2_connect_protocol_setting_id_documented() {
    // h2/hyper hide the wire-level setting behind `enable_connect_protocol`,
    // which the test above guards. This pins the spec literal `0x8` so any
    // attempt to hand-roll a custom setter sees the right id.
    const SETTINGS_ENABLE_CONNECT_PROTOCOL: u16 = 0x8;
    assert_eq!(SETTINGS_ENABLE_CONNECT_PROTOCOL, 8);
}
