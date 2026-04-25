//! D4-4 — H3 upstream `verify_peer` + CA wiring.
//!
//! Round-3 D3-1 wired the binary's H3 backend dial through
//! `build_h3_upstream_pool`, but the factory hard-coded
//! `quiche::Config::verify_peer(false)` — any on-path attacker between
//! the gateway and the upstream H3 origin could MITM. Round-4 auditor
//! flagged this as D4-4 (MEDIUM).
//!
//! The closure: `BackendConfig` gains three knobs — `tls_ca_path`,
//! `tls_verify_hostname`, `tls_verify_peer` (defaulting to `true`).
//! The validator (`lb_config::validate_config`) enforces:
//!
//! 1. An H3 backend with `tls_verify_peer = true` (default) and no
//!    `tls_ca_path` is rejected with a diagnostic that names the
//!    explicit-opt-out alternative.
//! 2. An H3 backend with `tls_verify_peer = false` accepts a missing
//!    `tls_ca_path` (the NOT-RECOMMENDED mesh-encryption escape hatch).
//! 3. An H3 backend with `tls_verify_peer = true` and a non-empty
//!    `tls_ca_path` validates cleanly.
//! 4. Non-H3 backends MUST NOT carry the new knobs (catches operator
//!    confusion when the protocol field is wrong but the TLS knobs are
//!    set).
//!
//! Companion: `tests/proto_translation_e2e.rs::proxy_h1_listener_h3_backend`
//! already exercises the **handshake-with-valid-CA** code path
//! end-to-end via `quiche::Config::load_verify_locations_from_file` +
//! `verify_peer(true)`, so this file does not re-spin a quiche
//! handshake. The factory-side wiring in `crates/lb/src/main.rs::
//! build_h3_upstream_pool` is private (binary crate) — its precondition
//! is `validate_config` having already enforced the mandatory CA path,
//! which is what these tests pin down.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use lb_config::{BackendConfig, LbConfig, ListenerConfig, validate_config};

fn make_listener(backend: BackendConfig) -> LbConfig {
    LbConfig {
        listeners: vec![ListenerConfig {
            address: "0.0.0.0:80".into(),
            protocol: "h1".into(),
            tls: None,
            quic: None,
            alt_svc: None,
            http: None,
            h2_security: None,
            websocket: None,
            grpc: None,
            backends: vec![backend],
        }],
        runtime: None,
        observability: None,
    }
}

#[test]
fn h3_backend_without_ca_path_rejected_when_verify_peer_default() {
    // D4-4 #1 — verify defaults to true; without a CA path the
    // validator must refuse the config so a misconfigured operator
    // gets a startup error rather than a silently-unverified dial.
    let cfg = make_listener(BackendConfig {
        address: "backend.example:443".into(),
        protocol: "h3".into(),
        weight: 1,
        tls_ca_path: None,
        tls_verify_hostname: None,
        tls_verify_peer: true,
    });
    let err = validate_config(&cfg).expect_err("must reject H3 without ca_path");
    let msg = err.to_string();
    assert!(
        msg.contains("tls_ca_path"),
        "diagnostic must name tls_ca_path; got {msg:?}"
    );
    assert!(
        msg.contains("tls_verify_peer = false"),
        "diagnostic must name the explicit-opt-out alternative; got {msg:?}"
    );
}

#[test]
fn h3_backend_without_ca_path_accepted_when_verify_peer_explicitly_false() {
    // D4-4 #2 — operators using mesh encryption (e.g. WireGuard,
    // ambient sidecar) explicitly opt out of QUIC peer verification.
    // The validator accepts that combination.
    let cfg = make_listener(BackendConfig {
        address: "backend.internal:443".into(),
        protocol: "h3".into(),
        weight: 1,
        tls_ca_path: None,
        tls_verify_hostname: None,
        tls_verify_peer: false,
    });
    validate_config(&cfg).expect("opt-out config must validate");
}

#[test]
fn h3_backend_with_ca_path_validates_cleanly() {
    // D4-4 #3 — happy path: H3 backend points at a CA bundle, verify
    // is on (default), and SNI override is left empty so the address
    // host wins. Validator accepts.
    let cfg = make_listener(BackendConfig {
        address: "backend.example:443".into(),
        protocol: "h3".into(),
        weight: 1,
        tls_ca_path: Some("/etc/ssl/internal-ca.pem".into()),
        tls_verify_hostname: None,
        tls_verify_peer: true,
    });
    validate_config(&cfg).expect("CA-bundle config must validate");
}

#[test]
fn h3_backend_with_explicit_sni_override_validates() {
    // SNI override accepted when non-empty.
    let cfg = make_listener(BackendConfig {
        address: "10.0.0.7:443".into(),
        protocol: "h3".into(),
        weight: 1,
        tls_ca_path: Some("/etc/ssl/ca.pem".into()),
        tls_verify_hostname: Some("backend.example".into()),
        tls_verify_peer: true,
    });
    validate_config(&cfg).expect("SNI override must validate");
}

#[test]
fn h3_backend_with_empty_sni_override_rejected() {
    let cfg = make_listener(BackendConfig {
        address: "backend.example:443".into(),
        protocol: "h3".into(),
        weight: 1,
        tls_ca_path: Some("/etc/ssl/ca.pem".into()),
        tls_verify_hostname: Some("   ".into()),
        tls_verify_peer: true,
    });
    let err = validate_config(&cfg).expect_err("empty SNI must be rejected");
    assert!(err.to_string().contains("tls_verify_hostname"));
}

#[test]
fn non_h3_backend_with_tls_knobs_rejected() {
    // D4-4 #4 — TLS knobs on a non-H3 backend signal operator confusion
    // (probably wrong `protocol` field). Reject so the misconfig
    // surfaces immediately.
    let cfg = make_listener(BackendConfig {
        address: "127.0.0.1:3000".into(),
        protocol: "h1".into(),
        weight: 1,
        tls_ca_path: Some("/etc/ssl/ca.pem".into()),
        tls_verify_hostname: None,
        tls_verify_peer: true,
    });
    let err = validate_config(&cfg).expect_err("tls knobs on non-H3 must be rejected");
    assert!(
        err.to_string()
            .contains("only meaningful for protocol = \"h3\"")
    );
}

#[test]
fn non_h3_backend_with_verify_peer_false_rejected() {
    // The `tls_verify_peer = false` opt-out is meaningless on a non-H3
    // backend; the validator catches that too.
    let cfg = make_listener(BackendConfig {
        address: "127.0.0.1:3000".into(),
        protocol: "tcp".into(),
        weight: 1,
        tls_ca_path: None,
        tls_verify_hostname: None,
        tls_verify_peer: false,
    });
    let err = validate_config(&cfg).expect_err("opt-out flag on non-H3 backend must be rejected");
    assert!(
        err.to_string()
            .contains("only meaningful for protocol = \"h3\"")
    );
}

#[test]
fn h3_backend_default_protocol_parses_with_verify_peer_true() {
    // Round-trip a TOML snippet to confirm the new fields default
    // correctly when omitted entirely (TOML with no tls_* keys must
    // still produce verify_peer=true). This is the precondition that
    // makes the verify_peer-default-true serde knob actually default.
    let input = r#"
[[listeners]]
address = "0.0.0.0:80"
protocol = "h1"

[[listeners.backends]]
address = "backend.example:443"
protocol = "h3"
tls_ca_path = "/etc/ssl/ca.pem"
"#;
    let cfg = lb_config::parse_config(input).unwrap();
    let backend = &cfg.listeners[0].backends[0];
    assert_eq!(backend.protocol, "h3");
    assert!(
        backend.tls_verify_peer,
        "tls_verify_peer must default to true on TOML-omitted key"
    );
    assert_eq!(backend.tls_ca_path.as_deref(), Some("/etc/ssl/ca.pem"));
    validate_config(&cfg).expect("H3 backend with CA path validates");
}
