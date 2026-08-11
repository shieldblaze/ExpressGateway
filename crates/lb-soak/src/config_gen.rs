//! Generate the gateway's TOML config + TLS material for each soak datapath. A wrong key silently disables the datapath under test, so the block shapes are pinned here and validated by the
//! apparatus smoke run before any long soak. Mode B is `[listeners.quic.raw_proxy]` and its `backend_ca_path` is MANDATORY for a self-signed backend (the gateway always `verify_peer`s, so
//! omitting it makes the dial fail and the soak would test a dead path); Mode A is the top-level `[passthrough]` block.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

pub struct Certs {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub ca: PathBuf,
}

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

/// Write a private key with mode 0600 (the gateway's strict TLS-key-permission check rejects group/other-readable keys and exits before binding).
fn write_key_0600(path: &Path, pem: &str) -> anyhow::Result<()> {
    std::fs::write(path, pem)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn runtime_block() -> &'static str {
    "[runtime]\ndrain_timeout_ms = 5000\nreadiness_settle_ms = 100\n\n"
}

fn observability_block(metrics: SocketAddr) -> String {
    format!("[observability]\nmetrics_bind = \"{metrics}\"\n")
}

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

/// `h1` front with `[listeners.websocket]` ENABLED -> an H1 WebSocket backend (sc8_ws_h1). `idle_timeout_seconds` is generous on purpose: we are proving connection RECLAIM on clean close, not idle-reap, so the sustained echo clients must stay up.
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

/// `h1s` front with `[listeners.websocket]` ENABLED **and `h2_extended_connect = true`** -> an H1 WebSocket backend (sc8b_ws_h2, RFC 8441). The knob is OFF by default (CF-S27-2), so the soak must explicitly opt in.
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

/// `quic` front in **H3-terminate** mode (no `[listeners.quic.raw_proxy]`, so `raw_quic_backend = None`; R3). This scenario deliberately emits NO backend block, so what it exercises end to
/// end is the `quiche::h3` ingress, the inline-400 DECODED egress, F-MD-4 RST/STOP_SENDING mapping, and the no-backend stream-drop path. The full H3->{H1,H2,H3} relay is covered by the e2e harnesses.
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

/// `quic` H3-terminate front with `h3_extended_connect = true` -> an H1 WebSocket backend (sc8c_ws_h3, RFC 9220). Same long-lived-relay leak class as sc8_ws_h1, over quiche.
#[must_use]
pub fn quic_h3_terminate_ws(
    listener: SocketAddr,
    backend: SocketAddr,
    metrics: SocketAddr,
    front_certs: &Certs,
    retry_secret: &Path,
    idle_timeout_seconds: u64,
    read_frame_timeout_seconds: u64,
) -> String {
    format!(
        "{rt}[[listeners]]\naddress = \"{listener}\"\nprotocol = \"quic\"\n\n\
         [listeners.quic]\ncert_path = \"{cert}\"\nkey_path = \"{key}\"\nretry_secret_path = \"{retry}\"\n\n\
         [listeners.websocket]\n\
         enabled = true\n\
         h3_extended_connect = true\n\
         idle_timeout_seconds = {idle}\n\
         read_frame_timeout_seconds = {rft}\n\n\
         [[listeners.backends]]\naddress = \"{backend}\"\nprotocol = \"h1\"\nweight = 1\n\n\
         {obs}",
        rt = runtime_block(),
        cert = front_certs.cert.display(),
        key = front_certs.key.display(),
        retry = retry_secret.display(),
        idle = idle_timeout_seconds,
        rft = read_frame_timeout_seconds,
        obs = observability_block(metrics),
    )
}

/// `quic` H3-terminate front -> an HTTP/2 gRPC origin (sc9_grpc_h3). Leak-class signal: per-RPC stream open/close plus the response-trailer terminal cleanup (the `drain_resp_channels` path F-S29-1 corrected) under sustained churn — `fds`, RSS/VmHWM, and panic=0.
#[must_use]
pub fn quic_h3_terminate_h2(
    listener: SocketAddr,
    backend: SocketAddr,
    metrics: SocketAddr,
    front_certs: &Certs,
    retry_secret: &Path,
) -> String {
    format!(
        "{rt}[[listeners]]\naddress = \"{listener}\"\nprotocol = \"quic\"\n\n\
         [listeners.quic]\ncert_path = \"{cert}\"\nkey_path = \"{key}\"\nretry_secret_path = \"{retry}\"\n\n\
         [[listeners.backends]]\naddress = \"{backend}\"\nprotocol = \"h2\"\nweight = 1\n\n\
         {obs}",
        rt = runtime_block(),
        cert = front_certs.cert.display(),
        key = front_certs.key.display(),
        retry = retry_secret.display(),
        obs = observability_block(metrics),
    )
}

/// Mode A QUIC passthrough — a top-level `[passthrough]` block; TLS is end-to-end and the gateway never decrypts. `mint_retry = false` is emitted unconditionally: with `true`, the LB-minted
/// Retry trips CF-S15-PASSTHROUGH-RETRY-ODCID and the client is granted 0 streams, so the soak would drive a dead path. `flow_idle_timeout_ms` is the F-S20-2 reaper window, shortened so reclamation is visible within the run.
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
        // R3 / F-S26-1: an H3-terminate front must carry NEITHER a raw_proxy block (that flips it to Mode B) NOR a backend block.
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
