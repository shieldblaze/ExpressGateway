//! S1-A (task B.1) — crate-local QUIC listener lifecycle coverage.
//!
//! Closes the `audit/h3-program/s1-inventory.md` finding that
//! `crates/lb-quic/src/listener.rs` had **0% coverage from the crate's
//! own test suite** (it was exercised only by repo-root `tests/*.rs`
//! integration binaries that do NOT gate `cargo test -p lb-quic`).
//!
//! Every test here spawns the REAL [`lb_quic::QuicListener`] (real UDP
//! bind, real `load_or_generate_retry_secret`, real `build_server_config`
//! cert/ALPN/flow-control, real `router::spawn`) and drives a REAL
//! `quiche` client over loopback UDP. The helper shapes (rcgen
//! self-signed cert with SAN `expressgateway.test` + serverAuth EKU,
//! CA-trusting client config with `verify_peer(true)`, the
//! handshake-pump driver) mirror the proven repo-root
//! `tests/quic_listener_e2e.rs` so this is the same wire path, just
//! gated inside the crate.
//!
//! Assertions are real: a single-actor / mis-bind / wrong-ALPN /
//! retry-secret-permission / shutdown-hang regression in
//! `listener.rs` fails at least one test here.

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_quic::{QuicListener, QuicListenerParams};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;
/// ALPN the production listener advertises (RFC 9114 §3.1).
const H3_ALPN: &[u8] = b"h3";
/// Legacy ALPN the listener also accepts (`H3_ALPN_PROTOS = [h3, h3-29]`).
const H3_29_ALPN: &[u8] = b"h3-29";
/// An ALPN the listener does NOT advertise — must fail negotiation.
const UNKNOWN_ALPN: &[u8] = b"hq-interop";

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// rcgen-generated loopback cert/key + a (non-existent) retry-secret
/// path so the listener exercises the *generate* branch of
/// `load_or_generate_retry_secret`. Dropping removes the temp dir.
struct TestCerts {
    dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    ca: PathBuf,
    retry: PathBuf,
}

impl Drop for TestCerts {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn generate_loopback_certs() -> TestCerts {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "lb-quic-listener-lifecycle-{}-{}-{counter}",
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
    // Deliberately does NOT exist: forces the listener's retry-secret
    // *generate* branch (mode 0600 write) on first spawn.
    let retry_path = dir.join("retry.key");
    fs::write(&cert_path, cert_pem.as_bytes()).unwrap();
    fs::write(&key_path, key_pem.as_bytes()).unwrap();
    fs::write(&ca_path, cert_pem.as_bytes()).unwrap();
    TestCerts {
        dir,
        cert: cert_path,
        key: key_path,
        ca: ca_path,
        retry: retry_path,
    }
}

/// Build a client config trusting the rcgen CA, offering `alpn`.
fn build_client_config(ca_path: &std::path::Path, alpn: &[u8]) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[alpn]).unwrap();
    cfg.load_verify_locations_from_file(ca_path.to_str().unwrap())
        .unwrap();
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

/// Outcome of driving a client handshake to a terminal state.
#[derive(Debug, PartialEq, Eq)]
enum HandshakeOutcome {
    Established,
    ClosedBeforeEstablished,
    DeadlineNotEstablished,
}

/// Drive a client `quiche::Connection` until it is established, it
/// closes, or the deadline elapses. Returns which terminal state was
/// reached so callers can assert BOTH positive (h3/h3-29) and negative
/// (unknown ALPN must NOT establish) outcomes.
async fn drive_client(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    deadline: tokio::time::Instant,
) -> HandshakeOutcome {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let local = socket.local_addr().unwrap();

    loop {
        if conn.is_established() {
            return HandshakeOutcome::Established;
        }
        if conn.is_closed() {
            return HandshakeOutcome::ClosedBeforeEstablished;
        }
        if tokio::time::Instant::now() >= deadline {
            return HandshakeOutcome::DeadlineNotEstablished;
        }

        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    let _ = socket.send_to(out_buf.get(..n).unwrap_or(&[]), info.to).await;
                }
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }

        let timeout = conn.timeout().unwrap_or(Duration::from_millis(50));
        match tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)).await {
            Ok(Ok((n, from))) => {
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                let info = quiche::RecvInfo { from, to: local };
                let _ = conn.recv(slice, info);
            }
            Ok(Err(_)) | Err(_) => conn.on_timeout(),
        }
    }
}

async fn connect_client(
    server_addr: SocketAddr,
    ca: &std::path::Path,
    alpn: &[u8],
) -> (quiche::Connection, UdpSocket) {
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let local = sock.local_addr().unwrap();
    let mut cfg = build_client_config(ca, alpn);
    let scid = random_scid_bytes();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let conn = quiche::connect(Some(TEST_SNI), &scid_ref, local, server_addr, &mut cfg).unwrap();
    (conn, sock)
}

fn spawn_listener_params(certs: &TestCerts) -> QuicListenerParams {
    QuicListenerParams::new(
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    )
}

// ---------------------------------------------------------------------
// 1. UDP bind + observable local_addr
// ---------------------------------------------------------------------
#[tokio::test]
async fn udp_bind_and_local_addr_observable() {
    let certs = generate_loopback_certs();
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(spawn_listener_params(&certs), shutdown.clone())
        .await
        .expect("listener must bind on 127.0.0.1:0");

    let addr = listener.local_addr();
    assert!(addr.ip().is_loopback(), "bound addr must be loopback");
    assert_ne!(addr.port(), 0, "OS must assign a concrete port");

    // A second UDP bind to the SAME concrete port must fail — proves
    // the listener actually holds the socket (regression: a no-op
    // spawn that never binds would let this succeed).
    let rebind = UdpSocket::bind(addr).await;
    assert!(
        rebind.is_err(),
        "listener did not actually own the UDP port {addr}"
    );

    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

// ---------------------------------------------------------------------
// 2. Retry secret: auto-generate at 0600, then reload identical secret
// ---------------------------------------------------------------------
#[tokio::test]
async fn retry_secret_autogenerated_0600_then_reloaded() {
    let certs = generate_loopback_certs();
    assert!(
        !certs.retry.exists(),
        "fixture: retry path must not pre-exist so the generate branch runs"
    );

    // --- first spawn: generate branch ---
    let shutdown1 = CancellationToken::new();
    let l1 = QuicListener::spawn(spawn_listener_params(&certs), shutdown1.clone())
        .await
        .expect("first spawn (retry-secret generate)");

    let secret_bytes = fs::read(&certs.retry).expect("retry secret file must now exist");
    assert_eq!(
        secret_bytes.len(),
        32,
        "generated retry secret must be exactly 32 bytes"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&certs.retry).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "retry secret file mode must be 0600");
    }

    // A token minted by listener #1's signer, bound to a fixed peer.
    let signer1 = l1.retry_signer();
    let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
    let odcid = b"original-dcid-16";
    let token = signer1.mint(peer, odcid);

    let h1 = l1.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), h1).await;

    // --- second spawn on the SAME retry path: reload branch ---
    let shutdown2 = CancellationToken::new();
    let l2 = QuicListener::spawn(spawn_listener_params(&certs), shutdown2.clone())
        .await
        .expect("second spawn (retry-secret reload)");

    let reread = fs::read(&certs.retry).expect("retry file still present");
    assert_eq!(
        reread, secret_bytes,
        "reload must NOT rewrite/rotate the persisted secret"
    );

    // Decisive reload assertion: the reloaded signer must verify a
    // token minted by the FIRST listener's signer. This only holds if
    // the 32-byte secret was loaded back identically — a regression
    // that regenerates instead of reloading fails here.
    let signer2 = l2.retry_signer();
    let verified = signer2.verify(&token, peer, std::time::Instant::now());
    assert!(
        verified.is_ok(),
        "reloaded retry signer must verify a token minted before reload \
         (secret was not reloaded identically): {verified:?}"
    );
    assert_eq!(
        verified.unwrap(),
        odcid.to_vec(),
        "verified token must recover the original ODCID"
    );

    let h2 = l2.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), h2).await;
}

// ---------------------------------------------------------------------
// 3a. ALPN `h3` is accepted -> handshake reaches is_established()
// ---------------------------------------------------------------------
#[tokio::test]
async fn alpn_h3_accepted_handshake_established() {
    let certs = generate_loopback_certs();
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(spawn_listener_params(&certs), shutdown.clone())
        .await
        .unwrap();
    let server_addr = listener.local_addr();

    let (conn, sock) = connect_client(server_addr, &certs.ca, H3_ALPN).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let outcome = drive_client(conn, &sock, deadline).await;

    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    assert_eq!(
        outcome,
        HandshakeOutcome::Established,
        "client offering ALPN `h3` must complete the handshake"
    );
}

// ---------------------------------------------------------------------
// 3b. ALPN `h3-29` (legacy) is accepted -> is_established()
// ---------------------------------------------------------------------
#[tokio::test]
async fn alpn_h3_29_legacy_accepted_handshake_established() {
    let certs = generate_loopback_certs();
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(spawn_listener_params(&certs), shutdown.clone())
        .await
        .unwrap();
    let server_addr = listener.local_addr();

    let (conn, sock) = connect_client(server_addr, &certs.ca, H3_29_ALPN).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let outcome = drive_client(conn, &sock, deadline).await;

    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    assert_eq!(
        outcome,
        HandshakeOutcome::Established,
        "client offering legacy ALPN `h3-29` must complete the handshake"
    );
}

// ---------------------------------------------------------------------
// 3c. Unknown ALPN is rejected -> handshake must NOT establish
// ---------------------------------------------------------------------
#[tokio::test]
async fn alpn_unknown_rejected_handshake_fails() {
    let certs = generate_loopback_certs();
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(spawn_listener_params(&certs), shutdown.clone())
        .await
        .unwrap();
    let server_addr = listener.local_addr();

    let (conn, sock) = connect_client(server_addr, &certs.ca, UNKNOWN_ALPN).await;
    // Shorter deadline: a correct server tears the TLS handshake down
    // on no-ALPN-overlap (no_application_protocol); we must observe a
    // NON-established terminal state.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let outcome = drive_client(conn, &sock, deadline).await;

    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    assert_ne!(
        outcome,
        HandshakeOutcome::Established,
        "client offering an unadvertised ALPN ({}) MUST NOT reach \
         is_established() — ALPN negotiation must fail",
        String::from_utf8_lossy(UNKNOWN_ALPN)
    );
}

// ---------------------------------------------------------------------
// 4. shutdown() cancels the router and the join completes cleanly
// ---------------------------------------------------------------------
#[tokio::test]
async fn shutdown_cancels_join_cleanly() {
    let certs = generate_loopback_certs();
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(spawn_listener_params(&certs), shutdown.clone())
        .await
        .unwrap();
    let addr = listener.local_addr();

    // shutdown() cancels the token and returns the listener task join
    // handle. It MUST resolve well within the budget — a regression
    // where the router select! does not honour `cancel.cancelled()`
    // would hang here and the timeout would fire.
    let handle = listener.shutdown();
    let joined = tokio::time::timeout(Duration::from_secs(3), handle).await;
    assert!(
        joined.is_ok(),
        "listener task did not join within 3s after shutdown() — \
         cancellation not honoured by the router loop"
    );
    joined
        .unwrap()
        .expect("listener task panicked instead of clean exit");

    // After a clean drain the port is released — rebinding now
    // succeeds (proves the socket was actually dropped, not leaked).
    let rebound = UdpSocket::bind(addr).await;
    assert!(
        rebound.is_ok(),
        "UDP port {addr} still held after clean shutdown — socket leaked"
    );
}
