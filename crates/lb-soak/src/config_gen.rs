//! Generate the gateway's TOML config + TLS material for each soak datapath.
//!
//! Mirrors the proven templates in `tests/reload_zero_drop.rs` and the QUIC
//! cert factory in `crates/lb-quic/tests/s16_raw_proxy_smoke.rs`, but produces
//! the EXACT block shapes the soak scenarios need (a wrong key silently
//! disables the datapath under test, so the shapes are pinned here and
//! validated by the apparatus smoke run before any long soak).
//!
//! Config facts pinned (verified against `crates/lb-config/src/lib.rs`):
//! * listener protocols: `h1` (plain TCP), `h1s` (TLS + ALPN h2/http1.1),
//!   `quic`; backend proto via `[[listeners.backends]].protocol` =
//!   `tcp|h1|h2|h3`.
//! * Mode B = `[listeners.quic.raw_proxy]` with `backend_addr` + `sni` +
//!   `backend_ca_path` (the gateway ALWAYS `verify_peer`s the backend, so the
//!   CA path is mandatory for a self-signed backend — omitting it makes the
//!   dial fail and the soak would test a dead path).
//! * Mode A = top-level `[passthrough]` (no `[[listeners]]` required;
//!   passthrough-only configs are valid).
//! * metrics endpoint = `[observability].metrics_bind`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Self-signed TLS material usable as BOTH a server cert (serverAuth EKU, SAN
/// for loopback + the SNI) and its own CA (so a peer can `verify_peer` it).
pub struct Certs {
    /// PEM cert chain path.
    pub cert: PathBuf,
    /// PEM private key path (written 0600).
    pub key: PathBuf,
    /// PEM trust anchor path (== the self-signed cert, usable as CA bundle).
    pub ca: PathBuf,
}

/// Generate a self-signed cert/key into `dir`, with SANs covering `127.0.0.1`,
/// `localhost` and `sni`. The cert is marked as a CA with the serverAuth EKU so
/// BoringSSL (QUIC) and rustls (TLS) both accept it as a loopback peer and a
/// trust anchor. The key is written with mode 0600 (the gateway rejects
/// group/other-readable keys in strict mode).
pub fn generate_certs(dir: &Path, sni: &str) -> anyhow::Result<Certs> {
    let mut params = rcgen::CertificateParams::new(vec![
        "127.0.0.1".to_string(),
        "localhost".to_string(),
        sni.to_string(),
    ])?;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
    let key_pair = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ca_path = dir.join("ca.pem");
    std::fs::write(&cert_path, cert.pem().as_bytes())?;
    write_key_0600(&key_path, &key_pair.serialize_pem())?;
    std::fs::write(&ca_path, cert.pem().as_bytes())?;
    Ok(Certs {
        cert: cert_path,
        key: key_path,
        ca: ca_path,
    })
}

/// Write a private key with mode 0600 (the gateway's strict TLS-key-permission
/// check rejects group/other-readable keys and exits before binding).
fn write_key_0600(path: &Path, pem: &str) -> anyhow::Result<()> {
    std::fs::write(path, pem)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// A short `[runtime]` block — small drain budget so the soak's per-scenario
/// SIGTERM teardown is quick, and the security caps left at their defaults so
/// the soak OBSERVES them bounding state under flood.
fn runtime_block() -> &'static str {
    "[runtime]\ndrain_timeout_ms = 5000\nreadiness_settle_ms = 100\n\n"
}

fn observability_block(metrics: SocketAddr) -> String {
    format!("[observability]\nmetrics_bind = \"{metrics}\"\n")
}

/// `h1` front (plain TCP HTTP/1.1) → backend speaking `backend_proto`
/// (`h1`/`tcp` for an H1 backend, `h2` for an H2 backend). Covers the
/// H1→H1 and H1→H2 cells.
#[must_use]
pub fn h1_front(
    listener: SocketAddr,
    backend: SocketAddr,
    backend_proto: &str,
    metrics: SocketAddr,
) -> String {
    format!(
        "{rt}[[listeners]]\naddress = \"{listener}\"\nprotocol = \"h1\"\n\n\
         [[listeners.backends]]\naddress = \"{backend}\"\nprotocol = \"{backend_proto}\"\nweight = 1\n\n\
         {obs}",
        rt = runtime_block(),
        obs = observability_block(metrics),
    )
}

/// `h1` front (plain TCP HTTP/1.1) with a `[listeners.websocket]` block ENABLED
/// → an H1 WebSocket backend. The S27 sc8_ws_h1 scenario: the binary wires
/// `with_websocket` on the H1 path (`build_h1_proxy`), so a WS upgrade arriving
/// here is intercepted, the gateway dials the backend, and the long-lived
/// `WsProxy::proxy_frames` relay runs. The block carries the canonical knobs
/// (idle/read-frame/ping caps) so the soak OBSERVES them bounding state.
///
/// `idle_timeout_seconds` is the WS idle close (1001) — kept generous so the
/// sustained echo clients stay up and the churn clients control their own
/// open→close cadence (we are proving connection RECLAIM on clean close, not
/// idle-reap). The backend speaks plain HTTP/1.1 (`backend_proto = "h1"`).
#[must_use]
pub fn h1_front_ws(
    listener: SocketAddr,
    backend: SocketAddr,
    metrics: SocketAddr,
    idle_timeout_seconds: u64,
    read_frame_timeout_seconds: u64,
) -> String {
    format!(
        "{rt}[[listeners]]\naddress = \"{listener}\"\nprotocol = \"h1\"\n\n\
         [listeners.websocket]\n\
         enabled = true\n\
         idle_timeout_seconds = {idle}\n\
         read_frame_timeout_seconds = {rft}\n\n\
         [[listeners.backends]]\naddress = \"{backend}\"\nprotocol = \"h1\"\nweight = 1\n\n\
         {obs}",
        rt = runtime_block(),
        idle = idle_timeout_seconds,
        rft = read_frame_timeout_seconds,
        obs = observability_block(metrics),
    )
}

/// `h1s` front (TLS, ALPN advertises `h2` then `http/1.1`) with a
/// `[listeners.websocket]` block ENABLED **and `h2_extended_connect = true`** →
/// an H1 WebSocket backend. The S27 sc8b_ws_h2 scenario: an H2 client drives WS
/// via RFC 8441 extended CONNECT; the gateway (`build_h2_proxy`) advertises
/// `SETTINGS_ENABLE_CONNECT_PROTOCOL`, intercepts the extended CONNECT, and runs
/// the same relay onto an H1 WS backend. `h2_extended_connect` is OFF by default
/// (CF-S27-2), so the soak must explicitly opt in to exercise the H2 path.
#[must_use]
pub fn h1s_front_ws(
    listener: SocketAddr,
    backend: SocketAddr,
    metrics: SocketAddr,
    certs: &Certs,
    idle_timeout_seconds: u64,
    read_frame_timeout_seconds: u64,
) -> String {
    format!(
        "{rt}[[listeners]]\naddress = \"{listener}\"\nprotocol = \"h1s\"\n\n\
         [listeners.tls]\ncert_path = \"{cert}\"\nkey_path = \"{key}\"\n\n\
         [listeners.websocket]\n\
         enabled = true\n\
         h2_extended_connect = true\n\
         idle_timeout_seconds = {idle}\n\
         read_frame_timeout_seconds = {rft}\n\n\
         [[listeners.backends]]\naddress = \"{backend}\"\nprotocol = \"h1\"\nweight = 1\n\n\
         {obs}",
        rt = runtime_block(),
        cert = certs.cert.display(),
        key = certs.key.display(),
        idle = idle_timeout_seconds,
        rft = read_frame_timeout_seconds,
        obs = observability_block(metrics),
    )
}

/// `h1s` front (TLS, ALPN advertises `h2` then `http/1.1` — an H2 client
/// negotiates HTTP/2) → backend speaking `backend_proto`. Covers the H2→H1 and
/// H2→H2 cells.
#[must_use]
pub fn h1s_front(
    listener: SocketAddr,
    backend: SocketAddr,
    backend_proto: &str,
    metrics: SocketAddr,
    certs: &Certs,
) -> String {
    format!(
        "{rt}[[listeners]]\naddress = \"{listener}\"\nprotocol = \"h1s\"\n\n\
         [listeners.tls]\ncert_path = \"{cert}\"\nkey_path = \"{key}\"\n\n\
         [[listeners.backends]]\naddress = \"{backend}\"\nprotocol = \"{backend_proto}\"\nweight = 1\n\n\
         {obs}",
        rt = runtime_block(),
        cert = certs.cert.display(),
        key = certs.key.display(),
        obs = observability_block(metrics),
    )
}

/// `quic` front in Mode B (terminate + re-originate) → a QUIC backend at
/// `backend`. `front_certs` terminate the client TLS; `backend_ca` is the trust
/// anchor the gateway uses to `verify_peer` the upstream QUIC backend.
#[must_use]
pub fn quic_mode_b(
    listener: SocketAddr,
    backend: SocketAddr,
    backend_sni: &str,
    metrics: SocketAddr,
    front_certs: &Certs,
    retry_secret: &Path,
    backend_ca: &Path,
    dgram_queue_cap: usize,
    max_relay_streams: usize,
) -> String {
    format!(
        "{rt}[[listeners]]\naddress = \"{listener}\"\nprotocol = \"quic\"\n\n\
         [listeners.quic]\ncert_path = \"{cert}\"\nkey_path = \"{key}\"\nretry_secret_path = \"{retry}\"\n\n\
         [listeners.quic.raw_proxy]\n\
         backend_addr = \"{backend}\"\n\
         sni = \"{backend_sni}\"\n\
         backend_ca_path = \"{ca}\"\n\
         dgram_queue_cap = {dgram_queue_cap}\n\
         max_relay_streams = {max_relay_streams}\n\n\
         {obs}",
        rt = runtime_block(),
        cert = front_certs.cert.display(),
        key = front_certs.key.display(),
        retry = retry_secret.display(),
        ca = backend_ca.display(),
        obs = observability_block(metrics),
    )
}

/// `quic` front in **H3-terminate** mode (the default QUIC datapath — no
/// `[listeners.quic.raw_proxy]` block, so `raw_quic_backend = None` and the
/// listener terminates client QUIC + speaks HTTP/3 via `quiche::h3`; R3).
///
/// ## F-S26-1 — this front is BACKEND-LESS in the production binary
///
/// The shipped `expressgateway` binary NEVER wires an HTTP backend onto a
/// `protocol = "quic"` listener: `spawn_quic` → `quic_listener_params_from_config`
/// does not call `with_backends`/`with_h3_backend`/`with_h2_backend`, and the
/// listener loop ignores `[[listeners.backends]]` on the QUIC path. So a real
/// H3 request to this front reaches `conn_actor::poll_h3` with no pool/backends
/// and the stream is dropped ("no backends available for H3 request"). The full
/// H3→{H1,H2,H3} relay + the §7.1 content-length truncation guard are
/// library/harness-reachable only (covered by the e2e harnesses + Phase-3 R13
/// bursts, NOT by this soak). The soak therefore deliberately emits NO backend
/// block — adding one would test a path the binary cannot enter and silently
/// mislead the verdict. What this front DOES exercise end-to-end (and what the
/// soak drives): the migrated `quiche::h3` ingress (handshake + control/QPACK
/// streams + HEADERS/DATA decode + the request-body cap/backpressure), the
/// inline-400 DECODED egress (a bad-`:authority` request → `send_response`/
/// `send_body` 400 "bad request", a true request→response round-trip), F-MD-4
/// RST/STOP_SENDING mapping, and the no-backend stream-drop path.
///
/// `front_certs` terminate the client TLS (ALPN `h3` — matches the listener's
/// `H3_ALPN_PROTOS`); the client trusts them. No DATAGRAM support (R3: only
/// `with_raw_backend` flips that on).
#[must_use]
pub fn quic_h3_terminate(
    listener: SocketAddr,
    metrics: SocketAddr,
    front_certs: &Certs,
    retry_secret: &Path,
) -> String {
    format!(
        "{rt}[[listeners]]\naddress = \"{listener}\"\nprotocol = \"quic\"\n\n\
         [listeners.quic]\ncert_path = \"{cert}\"\nkey_path = \"{key}\"\nretry_secret_path = \"{retry}\"\n\n\
         {obs}",
        rt = runtime_block(),
        cert = front_certs.cert.display(),
        key = front_certs.key.display(),
        retry = retry_secret.display(),
        obs = observability_block(metrics),
    )
}

/// Mode A QUIC passthrough — a top-level `[passthrough]` block routing flows to
/// `backend`. TLS is end-to-end client↔backend; the gateway never decrypts.
///
/// `mint_retry = false` is emitted unconditionally: the soak drives REAL
/// application streams end-to-end through Mode A, and with `mint_retry = true`
/// the LB-minted Retry triggers CF-S15-PASSTHROUGH-RETRY-ODCID (the backend
/// rejects the post-Retry `original_destination_connection_id`, so the client
/// is granted 0 streams). `false` forwards the Initial verbatim so the
/// end-to-end handshake completes and streams flow. `flow_idle_timeout_ms` is
/// the F-S20-2 idle-flow reaper window (short for the soak so reclamation is
/// visible within the run; the product default is 60 s).
#[must_use]
pub fn passthrough_mode_a(
    bind: SocketAddr,
    backend: SocketAddr,
    metrics: SocketAddr,
    retry_secret: &Path,
    max_quic_connections: usize,
    flow_idle_timeout_ms: u64,
) -> String {
    format!(
        "{rt}[passthrough]\n\
         bind_addr = \"{bind}\"\n\
         backends = [\"{backend}\"]\n\
         retry_secret_path = \"{retry}\"\n\
         max_quic_connections = {max_quic_connections}\n\
         mint_retry = false\n\
         flow_idle_timeout_ms = {idle_ms}\n\n\
         {obs}",
        rt = runtime_block(),
        retry = retry_secret.display(),
        idle_ms = flow_idle_timeout_ms,
        obs = observability_block(metrics),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
    }

    #[test]
    fn certs_generate_and_key_is_0600() {
        let dir = std::env::temp_dir().join(format!("lb-soak-cert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let certs = generate_certs(&dir, "soak.test").expect("gen certs");
        assert!(certs.cert.is_file() && certs.key.is_file() && certs.ca.is_file());
        let pem = std::fs::read_to_string(&certs.cert).unwrap();
        assert!(pem.contains("BEGIN CERTIFICATE"), "cert PEM present");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&certs.key).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "key must be 0600, got {:o}",
                mode & 0o777
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn h1_front_toml_shape() {
        let toml = h1_front(addr(8080), addr(3000), "h1", addr(9090));
        assert!(toml.contains("protocol = \"h1\""));
        assert!(toml.contains("address = \"127.0.0.1:8080\""));
        assert!(toml.contains("[[listeners.backends]]"));
        assert!(toml.contains("metrics_bind = \"127.0.0.1:9090\""));
    }

    #[test]
    fn h1_front_h2_backend_marks_backend_proto() {
        let toml = h1_front(addr(8080), addr(3000), "h2", addr(9090));
        assert!(
            toml.contains("protocol = \"h2\""),
            "H1→H2 backend proto must be h2"
        );
    }

    #[test]
    fn quic_mode_b_has_raw_proxy_and_backend_ca() {
        let dir = std::env::temp_dir().join(format!("lb-soak-qb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let certs = generate_certs(&dir, "soak.test").unwrap();
        let retry = dir.join("retry.bin");
        let toml = quic_mode_b(
            addr(8443),
            addr(4443),
            "soak.test",
            addr(9090),
            &certs,
            &retry,
            &certs.ca,
            1024,
            256,
        );
        assert!(toml.contains("[listeners.quic.raw_proxy]"));
        assert!(toml.contains("backend_addr = \"127.0.0.1:4443\""));
        assert!(
            toml.contains("backend_ca_path ="),
            "Mode B must pin a backend CA"
        );
        assert!(toml.contains("max_relay_streams = 256"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quic_h3_terminate_has_quic_listener_no_raw_proxy_no_backend() {
        let dir = std::env::temp_dir().join(format!("lb-soak-h3t-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let certs = generate_certs(&dir, "soak-front").unwrap();
        let retry = dir.join("retry.bin");
        let toml = quic_h3_terminate(addr(8443), addr(9090), &certs, &retry);
        assert!(toml.contains("protocol = \"quic\""));
        assert!(toml.contains("[listeners.quic]"));
        assert!(toml.contains("retry_secret_path ="));
        // R3 / F-S26-1: a H3-terminate front must NOT carry a raw_proxy block
        // (that would flip it to Mode B) NOR a backend block (the binary
        // ignores it on the quic path — emitting one would mislead the soak).
        assert!(
            !toml.contains("[listeners.quic.raw_proxy]"),
            "H3-terminate must have no raw_proxy block (else it's Mode B)"
        );
        assert!(
            !toml.contains("[[listeners.backends]]"),
            "F-S26-1: the binary ignores backends on the quic path — emit none"
        );
        assert!(toml.contains("metrics_bind = \"127.0.0.1:9090\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn h1_front_ws_has_enabled_websocket_block_and_backend() {
        let toml = h1_front_ws(addr(8080), addr(3000), addr(9090), 120, 30);
        assert!(toml.contains("protocol = \"h1\""));
        assert!(
            toml.contains("[listeners.websocket]"),
            "WS soak front must carry a websocket block"
        );
        assert!(
            toml.contains("enabled = true"),
            "the websocket block must be enabled"
        );
        assert!(toml.contains("idle_timeout_seconds = 120"));
        assert!(toml.contains("read_frame_timeout_seconds = 30"));
        assert!(
            toml.contains("[[listeners.backends]]"),
            "WS front must have an H1 WS backend (the relay's far end)"
        );
        assert!(toml.contains("metrics_bind = \"127.0.0.1:9090\""));
    }

    #[test]
    fn h1s_front_ws_opts_in_h2_extended_connect() {
        let dir = std::env::temp_dir().join(format!("lb-soak-wsh2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let certs = generate_certs(&dir, "soak.test").unwrap();
        let toml = h1s_front_ws(addr(8443), addr(3000), addr(9090), &certs, 120, 30);
        assert!(toml.contains("protocol = \"h1s\""));
        assert!(toml.contains("[listeners.tls]"));
        assert!(toml.contains("[listeners.websocket]"));
        assert!(
            toml.contains("h2_extended_connect = true"),
            "WS-over-H2 soak must opt in to RFC 8441 extended CONNECT (off by default)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn passthrough_mode_a_is_listener_free() {
        let toml = passthrough_mode_a(
            addr(8444),
            addr(4444),
            addr(9090),
            Path::new("/tmp/r"),
            100_000,
            10_000,
        );
        assert!(toml.contains("[passthrough]"));
        assert!(toml.contains("bind_addr = \"127.0.0.1:8444\""));
        assert!(toml.contains("mint_retry = false"));
        assert!(toml.contains("flow_idle_timeout_ms = 10000"));
        assert!(
            !toml.contains("[[listeners]]"),
            "Mode A needs no listener block"
        );
    }
}
