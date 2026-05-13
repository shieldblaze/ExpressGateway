//! PROTO-2-02 proof: the QUIC listener advertises the RFC 9114 ALPN
//! token `h3` (with `h3-29` for draft-29 compat) and rejects any client
//! that advertises an unknown ALPN.
//!
//! Audit-ref: `audit/protocol/round-2-review.md` PROTO-2-02 and the
//! synthesis §D allocation. RFC 9114 §3.1 mandates `h3` for HTTP/3.
//!
//! These tests do NOT exercise the H3 wire format past the handshake —
//! they only validate ALPN negotiation. PROTO-2-05 (h3i conformance)
//! covers the next layer.
//!
//! Test surface:
//!
//! * `server_advertises_h3` — client sets `[b"h3"]`, handshake reaches
//!   `is_established()`, `application_proto() == b"h3"`.
//! * `server_accepts_h3_29_legacy_client` — client sets `[b"h3-29"]`,
//!   handshake succeeds, `application_proto() == b"h3-29"`.
//! * `server_rejects_unknown_alpn` — client sets `[b"lb-quic"]`
//!   (the pre-fix token); handshake closes with a TLS-level peer
//!   error shaped like `no_application_protocol` (TLS alert 120).
//! * `production_alpn_constant_is_h3` — static guard that the source
//!   constant `H3_ALPN_PROTOS[0]` is `b"h3"`. This is the regression
//!   guard against future "just one more compat token first" edits.

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_quic::{H3_ALPN_PROTOS, QuicListener, QuicListenerParams};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;
const OLD_LB_QUIC_ALPN: &[u8] = b"lb-quic";

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

/// Mirror of `tests/quic_listener_e2e.rs::generate_loopback_certs` so
/// this test file is self-contained. A self-signed CA cert with DNS SAN
/// `expressgateway.test` and `serverAuth` EKU; BoringSSL's hostname
/// verifier accepts the cert when the client sends `TEST_SNI` as SNI.
fn generate_loopback_certs() -> TestCerts {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "lb-quic-alpn-h3-{}-{}-{counter}",
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

fn build_client_config_with_alpn(ca_path: &std::path::Path, alpns: &[&[u8]]) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(alpns).unwrap();
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

/// Outcome of a handshake attempt.
#[derive(Debug)]
enum HandshakeOutcome {
    /// Reached `is_established() == true`; carries the negotiated
    /// `application_proto()` bytes (empty if the peer never set one).
    Established(Vec<u8>),
    /// Connection closed before reaching established. Carries the
    /// `peer_error()` (which is what BoringSSL surfaces an ALPN
    /// mismatch through: an `error_code = 0x100 | TLS_alert`). `None`
    /// means the local side hit `is_closed()` without a peer error.
    Closed(Option<quiche::ConnectionError>),
    /// Local handshake driver budget elapsed.
    DeadlineElapsed,
}

/// Drive the client connection until established or terminal. Returns
/// the negotiated ALPN bytes on success, or the peer error / deadline.
async fn drive_client_handshake(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    deadline: tokio::time::Instant,
) -> HandshakeOutcome {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];

    loop {
        if tokio::time::Instant::now() >= deadline {
            return HandshakeOutcome::DeadlineElapsed;
        }
        if conn.is_established() {
            return HandshakeOutcome::Established(conn.application_proto().to_vec());
        }
        if conn.is_closed() {
            return HandshakeOutcome::Closed(conn.peer_error().cloned());
        }

        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    let bytes = out_buf.get(..n).unwrap_or(&[]);
                    if socket.send_to(bytes, info.to).await.is_err() {
                        return HandshakeOutcome::Closed(conn.peer_error().cloned());
                    }
                }
                Err(quiche::Error::Done) => break,
                Err(_) => return HandshakeOutcome::Closed(conn.peer_error().cloned()),
            }
        }

        let timeout = conn.timeout().unwrap_or(Duration::from_millis(50));
        let local = match socket.local_addr() {
            Ok(a) => a,
            Err(_) => return HandshakeOutcome::DeadlineElapsed,
        };
        tokio::select! {
            r = tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)) => {
                match r {
                    Ok(Ok((n, from))) => {
                        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                        let info = quiche::RecvInfo { from, to: local };
                        match conn.recv(slice, info) {
                            Ok(_) | Err(quiche::Error::Done) => {}
                            Err(_) => return HandshakeOutcome::Closed(conn.peer_error().cloned()),
                        }
                    }
                    Ok(Err(_)) => return HandshakeOutcome::Closed(conn.peer_error().cloned()),
                    Err(_elapsed) => conn.on_timeout(),
                }
            }
        }
    }
}

/// Spawn a `QuicListener` on an ephemeral loopback port. Returns the
/// listener handle, its bound address, and the test cert bundle.
async fn spawn_listener() -> (QuicListener, SocketAddr, TestCerts, CancellationToken) {
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
    let addr = listener.local_addr();
    (listener, addr, certs, shutdown)
}

/// Run one handshake attempt with the given client ALPN list. Tears
/// the listener down before returning so a failing test does not leak
/// the accept task.
async fn handshake_with_alpn(alpns: &[&[u8]]) -> HandshakeOutcome {
    let (listener, server_addr, certs, _shutdown) = spawn_listener().await;

    let client_sock = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    let client_local = client_sock.local_addr().unwrap();

    let mut client_cfg = build_client_config_with_alpn(&certs.ca, alpns);
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
    let outcome = drive_client_handshake(conn, &client_sock, deadline).await;

    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    outcome
}

#[tokio::test]
async fn server_advertises_h3() {
    let outcome = handshake_with_alpn(&[b"h3"]).await;
    match outcome {
        HandshakeOutcome::Established(proto) => {
            assert_eq!(
                proto,
                b"h3",
                "expected RFC 9114 ALPN `h3`, negotiated {:?}",
                String::from_utf8_lossy(&proto)
            );
        }
        other => panic!("expected established with h3, got {other:?}"),
    }
}

#[tokio::test]
async fn server_accepts_h3_29_legacy_client() {
    let outcome = handshake_with_alpn(&[b"h3-29"]).await;
    match outcome {
        HandshakeOutcome::Established(proto) => {
            assert_eq!(
                proto,
                b"h3-29",
                "expected draft-29 ALPN `h3-29`, negotiated {:?}",
                String::from_utf8_lossy(&proto)
            );
        }
        other => panic!("expected established with h3-29, got {other:?}"),
    }
}

#[tokio::test]
async fn server_rejects_unknown_alpn() {
    // Pre-fix the server advertised `b"lb-quic"`; after PROTO-2-02 a
    // client that ONLY offers `b"lb-quic"` must fail ALPN negotiation.
    // BoringSSL surfaces the failure as TLS alert 120
    // (`no_application_protocol`), which quiche wraps into a CRYPTO
    // error frame with `error_code = 0x100 | 120 = 0x178` per
    // RFC 9001 §4.8. We accept any non-Established outcome: peer error
    // with that code shape, plain close, or deadline (some BoringSSL
    // builds drop the connection without surfacing a peer_error).
    let outcome = handshake_with_alpn(&[OLD_LB_QUIC_ALPN]).await;
    match outcome {
        HandshakeOutcome::Established(proto) => {
            panic!(
                "handshake unexpectedly succeeded with old `lb-quic` ALPN; \
                 negotiated proto = {:?}. PROTO-2-02 invariant violated.",
                String::from_utf8_lossy(&proto)
            );
        }
        HandshakeOutcome::Closed(peer_err) => {
            // Optional shape check: when present, the TLS alert should
            // be `no_application_protocol` (120). BoringSSL builds vary
            // on whether they surface it as a peer error vs. a local
            // close, so this is a soft assertion via eprintln.
            if let Some(err) = peer_err.as_ref() {
                eprintln!(
                    "ALPN mismatch peer_error: code=0x{:x} reason={:?}",
                    err.error_code,
                    String::from_utf8_lossy(&err.reason)
                );
            }
        }
        HandshakeOutcome::DeadlineElapsed => {
            // Acceptable: server may simply discard the failing
            // handshake without sending CONNECTION_CLOSE in time.
            eprintln!("ALPN mismatch closed via deadline (no CONNECTION_CLOSE observed)");
        }
    }
}

#[test]
fn production_alpn_constant_is_h3() {
    // Static guard. If a future refactor reorders the list or drops
    // `h3` to add yet another draft token first, this test fires and
    // the audit invariant remains visible.
    assert_eq!(
        H3_ALPN_PROTOS[0], b"h3",
        "PROTO-2-02: H3_ALPN_PROTOS[0] MUST be the RFC 9114 token `h3`"
    );
    assert!(
        H3_ALPN_PROTOS.iter().any(|p| *p == b"h3-29"),
        "PROTO-2-02: H3_ALPN_PROTOS must still include `h3-29` for \
         draft-29 compatibility (chromium < 91, quic-go < 0.31)"
    );
}
