//! `quiche::h3::Config` construction for the H3 front (server + upstream).
//!
//! The defaults deliberately match the pre-migration hand-rolled behaviour, so
//! the re-point onto `quiche::h3` was a framing change, not a policy change:
//!
//! * `set_max_field_section_size(MAX_FIELD_SECTION_SIZE)` — 1 MiB, well above
//!   any sane request-header set and below unbounded-growth DoS.
//! * `set_qpack_max_table_capacity(0)` — QPACK stays **static-table only**;
//!   advertising `0` tells peers not to use dynamic insertions (RFC 9204
//!   §3.2.2), the same simplifying choice quiche itself makes.
//! * `set_qpack_blocked_streams(0)` — with a 0-capacity dynamic table no
//!   stream can block on a dynamic reference, so `0` is the ONLY consistent
//!   value.

/// Largest uncompressed header list the server front accepts — 1 MiB,
/// preserving the pre-migration HEADERS acceptance envelope.
pub const MAX_FIELD_SECTION_SIZE: u64 = 1 << 20;

/// Build the [`quiche::h3::Config`] for the **server** termination front.
///
/// `ws_enabled` gates the `SETTINGS_ENABLE_CONNECT_PROTOCOL` advertisement:
/// `true` lets a peer send an RFC 8441/9220 Extended CONNECT; `false` leaves
/// the settings frame byte-identical to a pre-WS listener, and a client that
/// sends Extended CONNECT anyway has its `:protocol` rejected by
/// [`crate::h3_bridge::validate_request_pseudo_headers`] — the sole
/// pseudo-header authority, since quiche does not validate them.
///
/// # Errors
///
/// Propagates [`quiche::h3::Error`] from `quiche::h3::Config::new`.
pub fn build_server_h3_config(ws_enabled: bool) -> Result<quiche::h3::Config, quiche::h3::Error> {
    let mut cfg = quiche::h3::Config::new()?;
    cfg.set_max_field_section_size(MAX_FIELD_SECTION_SIZE);
    // Static-table-only QPACK: no dynamic table, no blocked streams.
    cfg.set_qpack_max_table_capacity(0);
    cfg.set_qpack_blocked_streams(0);
    // WS-over-H3 (RFC 9220): advertise SETTINGS_ENABLE_CONNECT_PROTOCOL.
    if ws_enabled {
        cfg.enable_extended_connect(true);
    }
    Ok(cfg)
}

/// Build the [`quiche::h3::Config`] for the **client** (upstream) front. The
/// gateway's QPACK is static-table only in BOTH directions and the field-section
/// envelope is the same 1 MiB. Kept as a distinct constructor rather than
/// reusing [`build_server_h3_config`] so client and server intents read
/// explicitly at each call site and either can be tuned without a silent
/// coupling.
///
/// # Errors
///
/// Propagates [`quiche::h3::Error`] from `quiche::h3::Config::new` rather than
/// panicking, so the caller decides.
pub fn build_client_h3_config() -> Result<quiche::h3::Config, quiche::h3::Error> {
    let mut cfg = quiche::h3::Config::new()?;
    cfg.set_max_field_section_size(MAX_FIELD_SECTION_SIZE);
    // Static-table-only QPACK: no dynamic table, no blocked streams.
    cfg.set_qpack_max_table_capacity(0);
    cfg.set_qpack_blocked_streams(0);
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The H3 config builds with the documented static-only defaults.
    #[test]
    fn server_h3_config_builds_with_static_only_defaults() {
        let _cfg = build_server_h3_config(false).expect("h3::Config must build");
        // `quiche::h3::Config` exposes no getters, so the assertion is
        // construction-success plus the documented constants.
        assert_eq!(MAX_FIELD_SECTION_SIZE, 1 << 20);
    }

    /// Flipping `ws_enabled` (extended-CONNECT advertisement) must not error.
    /// There is no getter, so this is construction-success only; the SETTINGS
    /// on the wire are proven by the real-wire WS suite.
    #[test]
    fn server_h3_config_builds_with_extended_connect_enabled() {
        let _cfg =
            build_server_h3_config(true).expect("h3::Config with extended-connect must build");
    }

    /// The CLIENT config builds with the same static-only defaults.
    #[test]
    fn client_h3_config_builds_with_static_only_defaults() {
        let _cfg = build_client_h3_config().expect("client h3::Config must build");
        assert_eq!(MAX_FIELD_SECTION_SIZE, 1 << 20);
    }
}
