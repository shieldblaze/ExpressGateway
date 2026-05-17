//! PROTO-2-11 proof — the per-connection actor's graceful-shutdown
//! helper (`graceful_h3_shutdown`) must emit an application-layer
//! `CONNECTION_CLOSE` frame carrying `H3_NO_ERROR = 0x0100` when the
//! listener's cancellation token fires.
//!
//! Coverage strategy:
//!
//! 1. Build a server + client `quiche::Connection` with a self-signed
//!    cert (rcgen), the production HTTP/3 ALPN tokens, and the
//!    in-tree `lb-quic`'s `LB_QUIC_TEST_SNI` so the handshake reaches
//!    `is_established()` end-to-end.
//! 2. Pump packets between two `127.0.0.1` UDP sockets until
//!    establishment.
//! 3. Call the exported [`lb_quic::graceful_h3_shutdown`] on the
//!    server-side `quiche::Connection`.
//! 4. Drive the client until its `peer_error()` becomes `Some`.
//! 5. Assert `is_app == true` and `error_code == 0x0100` —
//!    the bytes that left the server's UDP socket parsed back into a
//!    proper application-layer CLOSE on the peer.
//!
//! The drive logic is intentionally minimal: no streams are opened,
//! no H3 messages exchanged. PROTO-2-11 only cares that the CLOSE
//! emission and pump-until-closed semantics are wired up correctly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;

use lb_quic::{H3_NO_ERROR, graceful_h3_shutdown};

const MAX_UDP: usize = 65_535;
/// SNI advertised by the loopback client. Mirrors `lb_quic`'s internal
/// `LB_QUIC_TEST_SNI` constant; the cert's SAN must match.
const TEST_SNI: &str = "expressgateway.test";
/// Production ALPN tokens (RFC 9114 §3.1). Mirrors
/// `lb_quic::H3_ALPN_PROTOS` so the handshake exercises the same
/// negotiation path the listener uses in production.
const H3_ALPN_PROTOS: &[&[u8]] = &[b"h3", b"h3-29"];
/// Upper bound on the in-process handshake pump. 2 s is well beyond
/// any realistic loopback handshake while keeping test runtime bounded.
const HANDSHAKE_BUDGET: Duration = Duration::from_secs(2);
/// Upper bound on how long the client will drive after the server has
/// shut down before we declare the test a failure.
const POST_CLOSE_BUDGET: Duration = Duration::from_secs(1);

/// RAII guard around a tempfile we wrote ourselves. We avoid the
/// `tempfile` crate to stay within the existing dependency budget
/// (audit/deps-added.md). The guard unlinks on drop.
struct CertTempFile(PathBuf);

impl Drop for CertTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Write a self-signed cert + key to disk and return paths usable by
/// `quiche::Config::load_*_from_pem_file`. The cert's SAN includes
/// [`TEST_SNI`] so `BoringSSL`'s hostname verifier accepts it.
fn write_test_cert() -> (CertTempFile, CertTempFile) {
    let generated =
        rcgen::generate_simple_self_signed(vec![TEST_SNI.to_string()]).expect("rcgen self-signed");
    let cert_pem = generated.cert.pem();
    let key_pem = generated.key_pair.serialize_pem();

    let dir = std::env::temp_dir();
    let subsec = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    // F-COR-8 (foundation audit, auditor-4 LATENT note): the prior
    // nonce was `pid * K + subsec_nanos`. `std::process::id()` is
    // constant across every `#[test]` in this binary, so the instant
    // a second `#[test]` is added two tests running in parallel could
    // land on the same `subsec_nanos()` (or collide through the
    // wrapping mul/add) and therefore the SAME cert path — one test's
    // `CertTempFile::drop` then `remove_file`s the cert another test
    // is still loading (the round8 parallel-flake class). A
    // process-global monotonic counter makes every cert path unique
    // by construction. Mirrors round8_h3_authority_enforced.rs:75-81.
    use std::sync::atomic::{AtomicU64, Ordering};
    static CERT_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = CERT_SEQ.fetch_add(1, Ordering::Relaxed);
    let nonce = std::process::id()
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(subsec);
    let cert_path = dir.join(format!("lb-quic-proto-2-11-cert-{nonce}-{seq}.pem"));
    let key_path = dir.join(format!("lb-quic-proto-2-11-key-{nonce}-{seq}.pem"));
    std::fs::write(&cert_path, cert_pem).expect("write cert");
    std::fs::write(&key_path, key_pem).expect("write key");
    (CertTempFile(cert_path), CertTempFile(key_path))
}

/// Build a quiche::Config wired for a single H3 endpoint role.
fn build_config(server: bool, cert_path: &str, key_path: &str) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).expect("Config::new");
    cfg.set_application_protos(H3_ALPN_PROTOS).expect("alpn");
    cfg.set_max_idle_timeout(5_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(10 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(1024 * 1024);
    cfg.set_initial_max_stream_data_uni(1024 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(16);
    cfg.set_disable_active_migration(true);
    if server {
        cfg.load_cert_chain_from_pem_file(cert_path)
            .expect("load cert");
        cfg.load_priv_key_from_pem_file(key_path).expect("load key");
    } else {
        cfg.load_verify_locations_from_file(cert_path)
            .expect("trust cert");
        // The server cert is self-signed and used directly as the trust
        // anchor; turn peer verification on so BoringSSL still
        // exercises the verify path, mirroring the production loopback
        // rig.
        cfg.verify_peer(true);
    }
    cfg
}

/// 16-byte deterministic SCID — enough for one-conn loopback.
fn scid_bytes() -> [u8; quiche::MAX_CONN_ID_LEN] {
    let mut b = [0u8; quiche::MAX_CONN_ID_LEN];
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    for (i, byte) in b.iter_mut().enumerate() {
        *byte = nanos
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add(u32::try_from(i).unwrap_or(0))
            .to_le_bytes()[i % 4];
    }
    b
}

/// Push every queued packet on `conn` to `socket`. Returns when quiche
/// reports `Done`.
async fn flush(conn: &mut quiche::Connection, socket: &UdpSocket, out: &mut [u8]) {
    loop {
        match conn.send(out) {
            Ok((n, info)) => {
                socket
                    .send_to(out.get(..n).unwrap_or(&[]), info.to)
                    .await
                    .expect("send_to");
            }
            Err(quiche::Error::Done) => break,
            Err(e) => panic!("conn.send: {e:?}"),
        }
    }
}

/// Try to receive one packet within `wait` and feed it into `conn`.
async fn try_recv_one(
    conn: &mut quiche::Connection,
    socket: &UdpSocket,
    local: SocketAddr,
    in_buf: &mut [u8],
    wait: Duration,
) -> bool {
    match tokio::time::timeout(wait, socket.recv_from(in_buf)).await {
        Ok(Ok((n, from))) => {
            let info = quiche::RecvInfo { from, to: local };
            let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
            match conn.recv(slice, info) {
                Ok(_) | Err(quiche::Error::Done) => true,
                Err(e) => panic!("conn.recv: {e:?}"),
            }
        }
        // Timeout or socket error: nothing to feed.
        Ok(Err(_)) | Err(_) => false,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn test_h3_connection_close_emitted_on_cancel() {
    // ----- cert + sockets -----
    let (cert_file, key_file) = write_test_cert();
    let cert_path = cert_file.0.to_str().unwrap().to_string();
    let key_path = key_file.0.to_str().unwrap().to_string();

    let server_socket = Arc::new(
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .expect("bind server"),
    );
    let server_local = server_socket.local_addr().expect("server local");
    let client_socket = Arc::new(
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .expect("bind client"),
    );
    let client_local = client_socket.local_addr().expect("client local");

    // ----- configs + connections -----
    let mut server_cfg = build_config(true, &cert_path, &key_path);
    let mut client_cfg = build_config(false, &cert_path, &key_path);

    let s_scid_bytes = scid_bytes();
    let s_scid = quiche::ConnectionId::from_ref(&s_scid_bytes);
    let c_scid_bytes = scid_bytes();
    let c_scid = quiche::ConnectionId::from_ref(&c_scid_bytes);

    let mut server_conn =
        quiche::accept(&s_scid, None, server_local, client_local, &mut server_cfg)
            .expect("quiche::accept");
    let mut client_conn = quiche::connect(
        Some(TEST_SNI),
        &c_scid,
        client_local,
        server_local,
        &mut client_cfg,
    )
    .expect("quiche::connect");

    // ----- handshake pump -----
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let deadline = tokio::time::Instant::now() + HANDSHAKE_BUDGET;
    while !(server_conn.is_established() && client_conn.is_established()) {
        if tokio::time::Instant::now() > deadline {
            panic!(
                "handshake did not establish in {HANDSHAKE_BUDGET:?} \
                 (server_est={}, client_est={})",
                server_conn.is_established(),
                client_conn.is_established(),
            );
        }
        flush(&mut client_conn, &client_socket, &mut out).await;
        flush(&mut server_conn, &server_socket, &mut out).await;
        // Tiny waits so each side has a chance to read what the other
        // just wrote — 5 ms keeps the loop tight on loopback.
        try_recv_one(
            &mut server_conn,
            &server_socket,
            server_local,
            &mut in_buf,
            Duration::from_millis(50),
        )
        .await;
        try_recv_one(
            &mut client_conn,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(50),
        )
        .await;
    }
    assert!(server_conn.is_established(), "server should be established");
    assert!(client_conn.is_established(), "client should be established");

    // ----- THE SUBJECT UNDER TEST -----
    //
    // graceful_h3_shutdown must:
    //   1. call conn.close(true, 0x0100, b"shutdown") on the server, and
    //   2. pump conn.send / on_timeout until quiche reports closed.
    //
    // The server-side socket carries the resulting outbound
    // CONNECTION_CLOSE frame into the loopback path.
    graceful_h3_shutdown(&mut server_conn, &server_socket, &mut out).await;
    assert!(
        server_conn.is_closed() || server_conn.is_draining(),
        "server should be closed or draining after graceful_h3_shutdown"
    );

    // ----- drive the client until peer_error() arrives -----
    let post_deadline = tokio::time::Instant::now() + POST_CLOSE_BUDGET;
    while client_conn.peer_error().is_none() {
        if tokio::time::Instant::now() > post_deadline {
            panic!(
                "client did not observe peer CONNECTION_CLOSE within \
                 {POST_CLOSE_BUDGET:?}",
            );
        }
        try_recv_one(
            &mut client_conn,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(50),
        )
        .await;
        flush(&mut client_conn, &client_socket, &mut out).await;
    }

    let pe = client_conn
        .peer_error()
        .expect("peer_error must be Some after loop exit");
    assert!(
        pe.is_app,
        "PROTO-2-11: peer error must be application-layer (is_app=true), got {pe:?}"
    );
    assert_eq!(
        pe.error_code, H3_NO_ERROR,
        "PROTO-2-11: app_error must equal H3_NO_ERROR (0x0100), got 0x{:x}",
        pe.error_code
    );
    assert_eq!(
        H3_NO_ERROR, 0x0100,
        "exported H3_NO_ERROR constant must encode RFC 9114 §8.1"
    );
}
