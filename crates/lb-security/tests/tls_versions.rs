//! PROTO-2-14 — `tls13_only` proof tests. The shape is a live TLS 1.2-only client against a
//! `tls13_only = true` listener, because a faked ClientHello would need a whole TLS 1.2 client.
//! `build_server_config_with_policy` shadows the unchanged `build_server_config` shim so the rest
//! of the codebase does not see a rename.

use std::sync::Arc;
use std::time::Duration;

use lb_security::{TicketRotator, build_server_config_with_policy};
use parking_lot::Mutex;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

fn fresh_rotator() -> Arc<Mutex<TicketRotator>> {
    Arc::new(Mutex::new(
        TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap(),
    ))
}

fn self_signed() -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_der: Vec<u8> = generated.cert.der().to_vec();
    let key_der: Vec<u8> = generated.signing_key.serialize_der();
    let chain = vec![CertificateDer::from(cert_der)];
    let key = PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(key_der));
    (chain, key)
}

#[test]
fn default_config_lists_tls12_and_tls13() {
    let (chain, key) = self_signed();
    let cfg = build_server_config_with_policy(fresh_rotator(), chain, key, &[], false)
        .expect("default config builds");
    // The negotiated version set is not readable off a built `ServerConfig`, so a successful
    // `with_safe_default_protocol_versions` is the only available proxy for "1.2 and 1.3 wired".
    let _ = cfg;
}

#[test]
fn tls13_only_config_builds_without_tls12() {
    let (chain, key) = self_signed();
    let cfg = build_server_config_with_policy(fresh_rotator(), chain, key, &[], true)
        .expect("tls13_only config builds");
    // Construction cannot show the rejection — that happens at handshake time, which
    // `test_tls13_only_rejects_tls12` covers.
    let _ = cfg;
}

#[tokio::test]
async fn test_tls13_only_rejects_tls12() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use tokio::net::TcpStream;
    use tokio_rustls::TlsAcceptor;
    use tokio_rustls::TlsConnector;

    let (chain, key) = self_signed();
    let server_cfg =
        build_server_config_with_policy(fresh_rotator(), chain.clone(), key, &[], true)
            .expect("tls13_only server cfg");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let acceptor = TlsAcceptor::from(server_cfg);
    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let result = acceptor.accept(stream).await;
        result.err().is_some()
    });

    let mut root = rustls::RootCertStore::empty();
    for c in &chain {
        root.add(c.clone()).unwrap();
    }
    let client_cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS12])
    .expect("tls12-only client cfg builds")
    .with_root_certificates(root)
    .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_cfg));

    let sock = TcpStream::connect(addr).await.unwrap();
    let server_name = rustls_pki_types::ServerName::try_from("localhost").unwrap();
    let outcome = connector.connect(server_name, sock).await;
    assert!(
        outcome.is_err(),
        "PROTO-2-14: TLS 1.2 client must fail against tls13_only server, got {outcome:?}"
    );
    let server_rejected = server_task.await.unwrap();
    assert!(server_rejected, "server side must also surface the failure");
    let _ = AsyncWriteExt::shutdown(&mut tokio::io::stdout()).await; // keep tokio happy on CI
}

// ---------------------------------------------------------------------------
// S47-TLS-01 — the tests above prove the POLICY. They do not prove it is WIRED.
//
// Every test above drives `build_server_config_with_policy` directly. Production never called it
// with `true`: `main.rs` builds its listeners through `TlsConfigBundle::load_from_paths*`, which
// hard-coded `with_safe_default_protocol_versions()`. So `[runtime.tls] tls13_only = true` booted
// cleanly, was documented in CONFIG.md, features.md and SECURITY.md, and every `tls`/`h1s`
// listener went on negotiating TLS 1.2. The knob was inert and these tests could not see it.
//
// The test below closes that gap by going through the loader the binary actually uses.
// ---------------------------------------------------------------------------

fn write_self_signed_pem(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, generated.cert.pem()).unwrap();
    std::fs::write(&key_path, generated.signing_key.serialize_pem()).unwrap();
    (cert_path, key_path)
}

/// Drive a real TLS 1.2-only client at `server_config`; `true` if the handshake completed.
async fn tls12_client_succeeds_against(server_config: Arc<rustls::ServerConfig>) -> bool {
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(server_config);
    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        acceptor.accept(stream).await.is_ok()
    });

    // Trust anything: this test is about the negotiated VERSION, not certificate validation, and
    // a self-signed leaf would otherwise fail for the wrong reason and make the assertion vacuous.
    let client_cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS12])
    .expect("tls12-only client cfg builds")
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCert))
    .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_cfg));
    let sock = TcpStream::connect(addr).await.unwrap();
    let name = rustls_pki_types::ServerName::try_from("localhost").unwrap();
    let client_ok = connector.connect(name, sock).await.is_ok();
    let server_ok = server_task.await.unwrap_or(false);
    client_ok && server_ok
}

/// Test-only verifier: accepts any certificate. Scoped to this file; never compiled into the crate.
#[derive(Debug)]
struct AcceptAnyServerCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[tokio::test]
async fn s47_tls13_only_is_honoured_by_the_bundle_loader_the_binary_uses() {
    let dir = std::env::temp_dir().join(format!(
        "eg-s47-tls13-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (cert_path, key_path) = write_self_signed_pem(&dir);

    // The path `main.rs::build_tls_bundle` takes, with the policy ON.
    let restricted = lb_security::TlsConfigBundle::load_from_paths_with_policy(
        &cert_path,
        &key_path,
        &[],
        lb_security::DEFAULT_MAX_CHAIN_DEPTH,
        None,
        true,
    )
    .expect("tls13_only bundle loads");
    assert!(
        !tls12_client_succeeds_against(Arc::clone(&restricted.server_config)).await,
        "S47-TLS-01: with tls13_only = true a TLS 1.2 client must be refused by a bundle built \
         through the loader the binary uses"
    );

    // CONTROL — the same loader with the policy OFF must still accept TLS 1.2. Without this the
    // assertion above would pass just as well if the handshake were failing for an unrelated
    // reason (bad cert, wrong SNI, a broken fixture), which is precisely how the original gap
    // hid: a test that cannot distinguish "policy applied" from "nothing works".
    let permissive = lb_security::TlsConfigBundle::load_from_paths_with_policy(
        &cert_path,
        &key_path,
        &[],
        lb_security::DEFAULT_MAX_CHAIN_DEPTH,
        None,
        false,
    )
    .expect("default bundle loads");
    assert!(
        tls12_client_succeeds_against(Arc::clone(&permissive.server_config)).await,
        "control: with tls13_only = false a TLS 1.2 client must still succeed — otherwise the \
         assertion above proves nothing"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
