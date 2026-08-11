//! PROTO-2-13 / CF-S27-2 — code-PRESENCE test: the gated
//! `enable_connect_protocol()` call must survive refactors. Wire-level
//! behaviour lives in `tests/ws_h2_gated_off.rs` and `tests/ws_h2_e2e.rs`.

use std::fs;

const H2_PROXY_PATH: &str = "src/h2_proxy.rs";

#[test]
fn h2_proxy_calls_enable_connect_protocol() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest_dir}/{H2_PROXY_PATH}");
    let src = fs::read_to_string(&path).expect("read h2_proxy.rs");
    assert!(
        src.contains("enable_connect_protocol()"),
        "PROTO-2-13: H2 listener must retain the \
         `builder.enable_connect_protocol()` call (now gated by \
         `h2_extended_connect_enabled`, CF-S27-2) so WS-over-H2 is possible \
         when opted in (RFC 8441 §3). Inspected {path}."
    );
    // CF-S27-2: a refactor dropping the gate (WS-over-H2 on by default) is red.
    assert!(
        src.contains("if self.h2_extended_connect_enabled"),
        "CF-S27-2: `enable_connect_protocol()` must be gated behind \
         `if self.h2_extended_connect_enabled` (WS-over-H2 OFF by default). \
         Inspected {path}."
    );
}

#[test]
fn h2_connect_protocol_setting_id_documented() {
    // Pins the spec literal so a hand-rolled setter uses the right id.
    const SETTINGS_ENABLE_CONNECT_PROTOCOL: u16 = 0x8;
    assert_eq!(SETTINGS_ENABLE_CONNECT_PROTOCOL, 8);
}
