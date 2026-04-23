//! Pillar 3b.3c-1 end-to-end tests for the QUIC listener seam.
//!
//! These tests exercise [`lb_quic::QuicListener`] — the binary-side
//! seam that binds a UDP socket, loads (or generates) a retry secret,
//! builds a [`quiche::Config`], and drives a single-connection accept
//! loop. They prove the seam is wired correctly **end-to-end**: a real
//! quiche client dials the listener's bound port and the handshake
//! reaches `is_established() == true`.
//!
//! What these tests validate:
//!
//! * UDP bind + observable `local_addr`.
//! * Retry-secret file auto-generation on first boot (mode 0600 on
//!   Unix).
//! * Cert + key PEM loading through quiche's BoringSSL context.
//! * The accept task handshakes against a real quiche client (not a
//!   fake / in-process loopback reuse of `QuicEndpoint`).
//! * Cancellation of the shutdown token cleanly terminates the accept
//!   task.
//!
//! What these tests do NOT validate (Pillar 3b.3c-2):
//!
//! * Stateless-retry wire handling via [`lb_quic::RetryTokenSigner`].
//! * 0-RTT replay defense via [`lb_quic::ZeroRttReplayGuard`].
//! * H3 request parsing / backend forwarding / response streaming.
//! * Concurrent connections on the same socket via an
//!   `InboundPacketRouter`.
//!
//! System `curl 8.5.0` on Ubuntu 24.04 ships without HTTP/3 support,
//! so the moral equivalent of
//! `curl --http3 https://expressgateway.test:PORT/ --resolve
//! expressgateway.test:PORT:127.0.0.1 --cacert test-ca.pem` is this
//! in-process quiche client. A subprocess-`curl` variant should be
//! added once a CI image ships curl built against quiche or ngtcp2.

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_quic::{QuicListener, QuicListenerParams};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const LB_QUIC_ALPN: &[u8] = b"lb-quic";
const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestCerts {
    _dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    ca: PathBuf,
    retry: PathBuf,
}

impl Drop for TestCerts {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self._dir);
    }
}

fn generate_loopback_certs() -> TestCerts {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "lb-quic-listener-e2e-{}-{}-{counter}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).unwrap();

    let mut params = rcgen::CertificateParams::new(vec![TEST_SNI.to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
    let key_pair = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ca_path = dir.join("ca.pem");
    // Retry secret path — deliberately does NOT exist, so the listener
    // generates and persists a fresh 32-byte file on spawn.
    let retry_path = dir.join("retry.key");
    fs::write(&cert_path, cert_pem.as_bytes()).unwrap();
    fs::write(&key_path, key_pem.as_bytes()).unwrap();
    fs::write(&ca_path, cert_pem.as_bytes()).unwrap();
    TestCerts {
        _dir: dir,
        cert: cert_path,
        key: key_path,
        ca: ca_path,
        retry: retry_path,
    }
}

fn build_client_config(ca_path: &std::path::Path) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[LB_QUIC_ALPN]).unwrap();
    cfg.load_verify_locations_from_file(ca_path.to_str().unwrap())
        .unwrap();
    // The listener cert is self-signed with DNS SAN = expressgateway.test
    // and `serverAuth` EKU. BoringSSL's hostname verifier is satisfied by
    // matching SNI; leaving `verify_peer(true)` proves the chain builds.
    cfg.verify_peer(true);
    cfg.set_max_idle_timeout(5_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(4);
    cfg.set_initial_max_streams_uni(4);
    cfg.set_disable_active_migration(true);
    cfg
}

fn random_scid_bytes() -> [u8; quiche::MAX_CONN_ID_LEN] {
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new().fill(&mut scid).unwrap();
    scid
}

/// Drive a client-side quiche::Connection until `is_established()`
/// or the deadline elapses. Returns Ok(()) on successful handshake.
async fn drive_client_to_established(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "handshake deadline elapsed, is_established={}",
                conn.is_established()
            ));
        }
        if conn.is_established() {
            return Ok(());
        }
        if conn.is_closed() {
            return Err(format!(
                "connection closed before established: {:?}",
                conn.peer_error()
            ));
        }

        // Flush outbound.
        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    let bytes = out_buf.get(..n).unwrap_or(&[]);
                    if let Err(e) = socket.send_to(bytes, info.to).await {
                        return Err(format!("send_to failed: {e}"));
                    }
                }
                Err(quiche::Error::Done) => break,
                Err(e) => return Err(format!("conn.send: {e}")),
            }
        }

        let timeout = conn.timeout().unwrap_or(Duration::from_millis(50));
        let local = socket.local_addr().map_err(|e| e.to_string())?;
        tokio::select! {
            r = tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)) => {
                match r {
                    Ok(Ok((n, from))) => {
                        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                        let info = quiche::RecvInfo { from, to: local };
                        match conn.recv(slice, info) {
                            Ok(_) | Err(quiche::Error::Done) => {}
                            Err(e) => return Err(format!("conn.recv: {e}")),
                        }
                    }
                    Ok(Err(e)) => return Err(format!("recv_from: {e}")),
                    Err(_elapsed) => {
                        conn.on_timeout();
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn quic_listener_accepts_handshake_smoke() {
    let certs = generate_loopback_certs();

    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    );
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone())
        .await
        .expect("listener bind failed");

    // Retry secret file must now exist (the listener generated it on
    // first boot because the path did not pre-exist).
    let secret = fs::read(&certs.retry).expect("retry secret file");
    assert_eq!(secret.len(), 32, "retry secret must be 32 bytes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&certs.retry).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "retry secret file mode must be 0600");
    }

    let server_addr = listener.local_addr();
    assert!(server_addr.ip().is_loopback());

    // Client: plain quiche::Connection over a loopback UDP socket.
    let client_sock = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    let client_local = client_sock.local_addr().unwrap();

    let mut client_cfg = build_client_config(&certs.ca);
    let scid = random_scid_bytes();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let conn = quiche::connect(
        Some(TEST_SNI),
        &scid_ref,
        client_local,
        server_addr,
        &mut client_cfg,
    )
    .expect("quiche::connect");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let result = drive_client_to_established(conn, &client_sock, deadline).await;

    // Shut the listener down cleanly before asserting so a failed
    // handshake does not leave a runaway task.
    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    result.expect("client side handshake did not reach is_established()");
}

#[tokio::test]
async fn quic_listener_shuts_down_on_cancellation_token() {
    let certs = generate_loopback_certs();
    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    );
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone())
        .await
        .expect("listener bind");

    // Let the accept loop park on its first recv.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Cancel and await graceful exit.
    let handle = listener.shutdown();
    let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        joined.is_ok(),
        "listener task did not exit within 2s of shutdown.cancel()"
    );
}
