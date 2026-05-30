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

/// Mode A QUIC passthrough — a top-level `[passthrough]` block routing flows to
/// `backend`. TLS is end-to-end client↔backend; the gateway never decrypts.
#[must_use]
pub fn passthrough_mode_a(
    bind: SocketAddr,
    backend: SocketAddr,
    metrics: SocketAddr,
    retry_secret: &Path,
    max_quic_connections: usize,
) -> String {
    format!(
        "{rt}[passthrough]\n\
         bind_addr = \"{bind}\"\n\
         backends = [\"{backend}\"]\n\
         retry_secret_path = \"{retry}\"\n\
         max_quic_connections = {max_quic_connections}\n\n\
         {obs}",
        rt = runtime_block(),
        retry = retry_secret.display(),
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
    fn passthrough_mode_a_is_listener_free() {
        let toml = passthrough_mode_a(
            addr(8444),
            addr(4444),
            addr(9090),
            Path::new("/tmp/r"),
            100_000,
        );
        assert!(toml.contains("[passthrough]"));
        assert!(toml.contains("bind_addr = \"127.0.0.1:8444\""));
        assert!(
            !toml.contains("[[listeners]]"),
            "Mode A needs no listener block"
        );
    }
}
