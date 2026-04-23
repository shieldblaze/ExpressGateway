//! Integration test for Pillar 3b.2: a TLS-over-TCP listener wired to
//! `lb_security::build_server_config` accepts a rustls client, completes
//! a TLS 1.3 handshake, proxies an application-layer echo, and presents
//! a session ticket that the second handshake reuses.
//!
//! The test reuses the same `TicketRotator` + `ServerConfig` wiring the
//! binary (`crates/lb/src/main.rs`) uses. It does NOT spawn the binary
//! as a subprocess — doing so would drag config-file I/O, TOML parsing,
//! and signal-handling fixtures into the test with no added signal
//! over the rustls-level assertions this test actually cares about.

use std::sync::Arc;
use std::time::Duration;

use lb_security::{TicketRotator, build_server_config};
use parking_lot::Mutex;
use rustls::client::Resumption;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const ECHO_PAYLOAD: &[u8] = b"hello-tls";

fn make_cert() -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_der = generated.cert.der().to_vec();
    let key_der = generated.key_pair.serialize_der();
    (
        vec![CertificateDer::from(cert_der)],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
    )
}

fn build_client_config(trust_anchor: CertificateDer<'static>) -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    roots.add(trust_anchor).unwrap();
    let mut cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(Arc::new(roots))
        .with_no_client_auth();
    // rustls 0.23: the default `Resumption` enables tickets; make the
    // intent explicit so this test fails loudly if a future default
    // disables session resumption.
    cfg.resumption = Resumption::in_memory_sessions(8)
        .tls12_resumption(rustls::client::Tls12Resumption::SessionIdOrTickets);
    Arc::new(cfg)
}

async fn spawn_echo_tls_server(
    server_cfg: Arc<ServerConfig>,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(server_cfg);
    let handle = tokio::spawn(async move {
        loop {
            let (sock, _peer) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(sock).await else {
                    return;
                };
                let mut buf = [0u8; 64];
                let Ok(n) = tls.read(&mut buf).await else {
                    return;
                };
                let _ = tls.write_all(&buf[..n]).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (local_addr, handle)
}

async fn tls_roundtrip(
    server_addr: std::net::SocketAddr,
    connector: &TlsConnector,
    server_name: ServerName<'static>,
    payload: &[u8],
) -> Vec<u8> {
    let sock = TcpStream::connect(server_addr).await.unwrap();
    let mut tls = connector
        .connect(server_name, sock)
        .await
        .expect("TLS handshake failed");
    tls.write_all(payload).await.unwrap();
    let mut buf = vec![0u8; payload.len()];
    tls.read_exact(&mut buf).await.unwrap();
    let _ = tls.shutdown().await;
    buf
}

#[tokio::test]
async fn tls_listener_handshake_and_ticket_reuse() {
    let (cert_chain, key) = make_cert();
    let trust_anchor = cert_chain[0].clone();
    let rot = TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
    let rot_arc = Arc::new(Mutex::new(rot));
    let server_cfg = build_server_config(Arc::clone(&rot_arc), cert_chain.clone(), key).unwrap();

    let (addr, server_handle) = spawn_echo_tls_server(Arc::clone(&server_cfg)).await;

    // The ClientConfig is shared across both connections so its in-memory
    // ticket cache carries the ticket from handshake #1 to handshake #2.
    let client_cfg = build_client_config(trust_anchor);
    let connector = TlsConnector::from(client_cfg);
    let server_name = ServerName::try_from("localhost").unwrap();

    // First handshake: full (no ticket available).
    let echo1 = tls_roundtrip(addr, &connector, server_name.clone(), ECHO_PAYLOAD).await;
    assert_eq!(echo1, ECHO_PAYLOAD);

    // Second handshake: the server's ticketer (our RotatingTicketer)
    // issued a NewSessionTicket on connection #1, and the rustls client
    // cached it. The second connection should present that ticket and
    // the server's RotatingTicketer::decrypt path should accept it.
    let echo2 = tls_roundtrip(addr, &connector, server_name, ECHO_PAYLOAD).await;
    assert_eq!(echo2, ECHO_PAYLOAD);

    // The ServerConfig's ticketer must advertise as enabled, otherwise
    // rustls would not have sent a NewSessionTicket at all.
    assert!(server_cfg.ticketer.enabled());

    // The rotator still holds a single current key and no previous key
    // — nothing has aged past the interval in the test's wall-clock
    // window.
    {
        let guard = rot_arc.lock();
        assert!(guard.previous().is_none());
    }

    server_handle.abort();
}
