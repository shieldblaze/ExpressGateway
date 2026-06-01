//! SESSION 23 / INC-0 — `quiche::h3::Config` construction for the
//! migration of the hand-rolled H3 termination front to
//! [`quiche::h3::Connection`].
//!
//! This module is **infrastructure only**: it constructs the H3 config
//! the migrated server front (`quiche::h3::Connection::with_transport`)
//! will use, with defaults chosen to **match the current hand-rolled
//! behaviour** so the re-point is a framing change, not a policy change
//! (S23 mission R3 — preserve the KEEP-surface behaviour). It deletes
//! nothing and changes no live path; it is exercised by the INC-1
//! go/no-go experiment (`tests/inc1_quiche_h3_experiment.rs`) and a unit
//! test below. The production wiring into `conn_actor` lands only if
//! INC-1 is GO (owner gate, `audit/h3spec/s23-migration-plan.md`).
//!
//! ## Default rationale (industry-safe, matching current behaviour)
//!
//! * **`set_max_field_section_size(MAX_FIELD_SECTION_SIZE)`** — the
//!   largest *uncompressed* header list the server will accept before
//!   raising `H3_EXCESSIVE_LOAD`. `1 << 20` (1 MiB) is well above any
//!   sane request-header set (browsers cap far lower) and below an
//!   unbounded-growth DoS — i.e. industry-safe. (This preserves the
//!   1-MiB HEADERS acceptance envelope the gateway used before the
//!   migration to `quiche::h3`.)
//! * **`set_qpack_max_table_capacity(0)`** — the gateway's QPACK stays
//!   **static-table only** (no dynamic table; quiche::h3 never inserts).
//!   Advertising a `0` dynamic-table capacity tells peers not to use
//!   dynamic insertions against us (RFC 9204 §3.2.2). Same simplifying
//!   choice quiche itself makes (static-only in 0.28).
//! * **`set_qpack_blocked_streams(0)`** — with a `0`-capacity dynamic
//!   table there can be no blocked streams; `0` is the only consistent
//!   value (no stream ever blocks on a dynamic-table reference).
//!
//! These are pre-authorized "sane defaults matching current behaviour"
//! per S23 R7; any future tuning is documented, not silent.

/// Largest uncompressed header list the migrated server front will
/// accept — see the module rationale. `1 << 20` (1 MiB) preserves the
/// HEADERS payload acceptance envelope used before the `quiche::h3`
/// migration.
pub const MAX_FIELD_SECTION_SIZE: u64 = 1 << 20;

/// Build the [`quiche::h3::Config`] for the **server** termination
/// front, with industry-safe static-table-only QPACK defaults.
///
/// SESSION 27 / WS-over-H3 (RFC 9220) Stage A: `ws_enabled` gates the
/// `SETTINGS_ENABLE_CONNECT_PROTOCOL` advertisement. When `true`, the
/// server calls [`quiche::h3::Config::enable_extended_connect`] so a
/// peer may send an RFC 8441/9220 Extended CONNECT (`:method=CONNECT` +
/// `:protocol=websocket`) to bootstrap a WebSocket. When `false` (every
/// pre-S27 caller + every listener without an opted-in `[listeners.
/// websocket]`) the SETTINGS bit is NOT advertised and the H3 settings
/// frame is byte-identical to before (R3) — a client that sends Extended
/// CONNECT anyway has its `:protocol` rejected by
/// [`crate::h3_bridge::validate_request_pseudo_headers`] (which is the
/// sole pseudo-header authority — quiche does not validate them).
///
/// # Errors
///
/// Propagates [`quiche::h3::Error`] from `quiche::h3::Config::new`
/// (allocation / internal init failure — never expected on a healthy
/// host, but surfaced rather than panicked so the caller decides).
pub fn build_server_h3_config(ws_enabled: bool) -> Result<quiche::h3::Config, quiche::h3::Error> {
    let mut cfg = quiche::h3::Config::new()?;
    cfg.set_max_field_section_size(MAX_FIELD_SECTION_SIZE);
    // Static-table-only QPACK: no dynamic table, no blocked streams (the
    // gateway never inserts into the dynamic table).
    cfg.set_qpack_max_table_capacity(0);
    cfg.set_qpack_blocked_streams(0);
    // WS-over-H3 (RFC 9220): advertise SETTINGS_ENABLE_CONNECT_PROTOCOL
    // ONLY when this listener opted into WebSocket. R3: absent ⇒ the bit
    // is not set ⇒ settings frame byte-identical to the pre-S27 server.
    if ws_enabled {
        cfg.enable_extended_connect(true);
    }
    Ok(cfg)
}

/// Build the [`quiche::h3::Config`] for the **client** (upstream) front,
/// symmetric with the server config (static-table-only QPACK both ways).
///
/// SESSION 25 / INC-4: the migrated H3→H3 upstream connector
/// (`h3_bridge::stream_request_to_h3_upstream`) wraps the pooled,
/// established upstream `quiche::Connection` via
/// `quiche::h3::Connection::with_transport(qconn, &cfg)` in CLIENT mode.
/// The config is **symmetric** with [`build_server_h3_config`]: the
/// gateway's QPACK is static-table only in BOTH directions and the
/// field-section acceptance envelope is the same 1 MiB. H3 has no
/// client-only knob the gateway needs today (extended-CONNECT /
/// WebSockets-over-H3 is an S26 item). Kept as a
/// distinct constructor (not a shared `build_server_h3_config` reuse) so
/// the client/server intents read explicitly at each call site and either
/// can be tuned independently later without a silent coupling.
///
/// # Errors
///
/// Propagates [`quiche::h3::Error`] from `quiche::h3::Config::new`
/// (allocation / internal init failure — never expected on a healthy
/// host, but surfaced rather than panicked so the caller decides).
pub fn build_client_h3_config() -> Result<quiche::h3::Config, quiche::h3::Error> {
    let mut cfg = quiche::h3::Config::new()?;
    cfg.set_max_field_section_size(MAX_FIELD_SECTION_SIZE);
    // Static-table-only QPACK: no dynamic table, no blocked streams (same
    // as the server front; the gateway never inserts into the dynamic table).
    cfg.set_qpack_max_table_capacity(0);
    cfg.set_qpack_blocked_streams(0);
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// INC-0: the H3 config builds with the documented static-only,
    /// behaviour-matching defaults. This is the only thing INC-0 ships;
    /// it links nothing into the live path (no `conn_actor` change).
    #[test]
    fn server_h3_config_builds_with_static_only_defaults() {
        // The constructor must succeed; a failure here would block the
        // whole migration before INC-1 even runs.
        let _cfg = build_server_h3_config(false).expect("h3::Config must build");
        // `quiche::h3::Config` exposes no getters, so the assertion is
        // construction-success + the documented constants; the INC-1
        // wire experiment is what proves the values interoperate.
        assert_eq!(MAX_FIELD_SECTION_SIZE, 1 << 20);
    }

    /// SESSION 27 / WS-over-H3 Stage A: the server H3 config also builds
    /// with extended-CONNECT advertisement enabled. `quiche::h3::Config`
    /// exposes no getter for `connect_protocol_enabled`, so this is a
    /// construction-success assertion; the SETTINGS-on-the-wire +
    /// peer-observability behaviour is proven by the Stage-C real-wire
    /// suite. The point here is that flipping `ws_enabled` does not panic
    /// or error.
    #[test]
    fn server_h3_config_builds_with_extended_connect_enabled() {
        let _cfg =
            build_server_h3_config(true).expect("h3::Config with extended-connect must build");
    }

    /// INC-4: the CLIENT (upstream) H3 config builds with the same
    /// static-only, behaviour-matching defaults as the server front.
    /// Construction-success is the assertion (no getters); the H3→H3 wire
    /// suite is what proves the migrated client interoperates end-to-end.
    #[test]
    fn client_h3_config_builds_with_static_only_defaults() {
        let _cfg = build_client_h3_config().expect("client h3::Config must build");
        assert_eq!(MAX_FIELD_SECTION_SIZE, 1 << 20);
    }
}
