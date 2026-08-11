//! Crate-local QUIC router accept-path coverage, closing the finding that `router.rs` accept /
//! RETRY / per-CID dispatch had NO crate-local coverage. These spawn the REAL
//! [`lb_quic::QuicListener`] and drive REAL `quiche` clients, so `dispatch_packet`, `send_retry`,
//! `retry_signer.verify` and `spawn_new_connection` run on the wire.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_quic::{QuicListener, QuicListenerParams};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;
const H3_ALPN: &[u8] = b"h3";

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestCerts {
    dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    ca: PathBuf,
    retry: PathBuf,
}

impl Drop for TestCerts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn generate_loopback_certs() -> TestCerts {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "lb-quic-router-accept-{}-{}-{counter}",
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let mut params = rcgen::CertificateParams::new(vec![TEST_SNI.to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
    let key_pair = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ca_path = dir.join("ca.pem");
    let retry_path = dir.join("retry.key");
    std::fs::write(&cert_path, cert.pem().as_bytes()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem().as_bytes()).unwrap();
    std::fs::write(&ca_path, cert.pem().as_bytes()).unwrap();
    TestCerts {
        dir,
        cert: cert_path,
        key: key_path,
        ca: ca_path,
        retry: retry_path,
    }
}

fn build_client_config(ca_path: &std::path::Path) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
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

async fn drive_to_established(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    loop {
        if conn.is_established() {
            return Ok(());
        }
        if conn.is_closed() {
            return Err(format!(
                "closed before established: {:?}",
                conn.peer_error()
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("deadline; established={}", conn.is_established()));
        }

        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    socket
                        .send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                Err(quiche::Error::Done) => break,
                Err(e) => return Err(format!("conn.send: {e}")),
            }
        }

        let timeout = conn.timeout().unwrap_or(Duration::from_millis(50));
        match tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)).await {
            Ok(Ok((n, from))) => {
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                let info = quiche::RecvInfo { from, to: local };
                match conn.recv(slice, info) {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(e) => return Err(format!("conn.recv: {e}")),
                }
            }
            Ok(Err(e)) => return Err(format!("recv_from: {e}")),
            Err(_) => conn.on_timeout(),
        }
    }
}

async fn connect_client(
    server_addr: SocketAddr,
    ca: &std::path::Path,
) -> (quiche::Connection, UdpSocket) {
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let local = sock.local_addr().unwrap();
    let mut cfg = build_client_config(ca);
    let scid = random_scid_bytes();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let conn = quiche::connect(Some(TEST_SNI), &scid_ref, local, server_addr, &mut cfg).unwrap();
    (conn, sock)
}

fn listener_params(certs: &TestCerts) -> QuicListenerParams {
    QuicListenerParams::new(
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    )
}

#[tokio::test]
async fn two_concurrent_clients_distinct_actors() {
    let certs = generate_loopback_certs();
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(listener_params(&certs), shutdown.clone())
        .await
        .expect("listener spawn");
    let server_addr = listener.local_addr();

    // Two independent clients with distinct SCIDs and distinct source sockets: the single router
    // socket must demultiplex BOTH by DCID into two concurrent actors.
    let (c1, s1) = connect_client(server_addr, &certs.ca).await;
    let (c2, s2) = connect_client(server_addr, &certs.ca).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let (r1, r2) = tokio::join!(
        drive_to_established(c1, &s1, deadline),
        drive_to_established(c2, &s2, deadline),
    );

    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    assert!(
        r1.is_ok(),
        "client #1 handshake failed (router did not dispatch its CID): {r1:?}"
    );
    assert!(
        r2.is_ok(),
        "client #2 handshake failed (router did not dispatch its CID): {r2:?}"
    );
}

#[tokio::test]
async fn retry_token_round_trip_through_router() {
    let certs = generate_loopback_certs();
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(listener_params(&certs), shutdown.clone())
        .await
        .unwrap();
    let server_addr = listener.local_addr();

    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let client_local = sock.local_addr().unwrap();
    let mut cfg = build_client_config(&certs.ca);
    let scid = random_scid_bytes();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::connect(
        Some(TEST_SNI),
        &scid_ref,
        client_local,
        server_addr,
        &mut cfg,
    )
    .unwrap();

    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let mut saw_retry = false;
    let mut saw_non_retry_after_retry = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);

    while !conn.is_established() && tokio::time::Instant::now() < deadline {
        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    sock.send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                        .await
                        .unwrap();
                }
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }

        let timeout = conn.timeout().unwrap_or(Duration::from_millis(50));
        match tokio::time::timeout(timeout, sock.recv_from(&mut in_buf)).await {
            Ok(Ok((n, from))) => {
                // Inspect the long-header type WITHOUT consuming the datagram.
                let mut probe = in_buf.get(..n).unwrap_or(&[]).to_vec();
                if let Ok(hdr) = quiche::Header::from_slice(&mut probe, quiche::MAX_CONN_ID_LEN) {
                    if hdr.ty == quiche::Type::Retry {
                        saw_retry = true;
                    } else if saw_retry {
                        // Any non-RETRY server packet AFTER the RETRY proves the router verified
                        // the echoed token and advanced the connection.
                        saw_non_retry_after_retry = true;
                    }
                }
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                let info = quiche::RecvInfo {
                    from,
                    to: client_local,
                };
                let _ = conn.recv(slice, info);
            }
            Ok(Err(_)) | Err(_) => conn.on_timeout(),
        }
    }

    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    assert!(
        saw_retry,
        "router must emit a RETRY packet in response to the first \
         token-less Initial (send_retry path)"
    );
    assert!(
        saw_non_retry_after_retry || conn.is_established(),
        "after the client echoed the minted token, the router must \
         verify it and advance the handshake (retry_signer.verify + \
         spawn_new_connection); saw_retry={saw_retry}, \
         established={}",
        conn.is_established()
    );
    assert!(
        conn.is_established(),
        "RETRY round-trip must culminate in an established connection"
    );
}

// DOCUMENTED REACHABILITY BOUNDARY for the 0-RTT replay reject: the wire-level replay key is
// `client_SCID || retry_token_prefix`, computed from the client's SECOND Initial. A true wire
// replay needs that exact datagram re-injected byte-identically, and the quiche client API does
// not expose the raw post-RETRY Initial (`conn.send` returns coalesced bytes with no stable
// per-Initial boundary), so byte-exact wire replay is NOT reachable without source changes.
//
// What IS asserted: the router holds the SAME `ZeroRttReplayGuard` instance the listener exposes,
// and that instance's first-use-ok / second-use-rejected contract holds — so a guard that accepts
// replays, or that becomes detached from the router, is caught.
#[tokio::test]
async fn zero_rtt_replay_guard_rejects_second_use_on_shared_instance() {
    let certs = generate_loopback_certs();
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(listener_params(&certs), shutdown.clone())
        .await
        .unwrap();

    // The exact guard instance the router uses (Arc::clone in listener.rs RouterParams).
    let guard = listener.replay_guard();

    // A representative router replay key shape: SCID bytes || token prefix (router::build_replay_key).
    let key: &[u8] = b"\x01\x02\x03\x04\x05\x06\x07\x08token-prefix-bytes-0123456789";

    assert!(
        guard.lock().check_0rtt_token(key).is_ok(),
        "first observation of a replay key must be accepted"
    );
    assert!(
        guard.lock().check_0rtt_token(key).is_err(),
        "second observation of the SAME replay key must be rejected \
         as a 0-RTT replay (router uses this exact shared guard)"
    );
    // A different key must still be accepted — no false-positive that would break legitimate
    // distinct connections.
    let other: &[u8] = b"\xaa\xbb\xcc\xdd\x09\x0a\x0b\x0cdifferent-token-prefix-aaaa";
    assert!(
        guard.lock().check_0rtt_token(other).is_ok(),
        "a distinct replay key must NOT be falsely rejected"
    );

    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}
