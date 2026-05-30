//! SESSION 19 / Mode B — B6 0-RTT REJECTION security proof
//! (author ≠ verifier; this is the verifier's independent proof).
//!
//! Owner ruling: Mode B rejects client 0-RTT / early data in v1. This file
//! PROVES that — by construction AND on the wire — rather than asserting it.
//!
//! ## BY CONSTRUCTION (cited; not re-checked at runtime)
//!
//! `enable_early_data()` is NEVER called on any client-facing server config:
//! * `crates/lb-quic/src/listener.rs:426` `build_server_config` (the
//!   production client-facing config) builds a `quiche::Config` and never
//!   calls `enable_early_data` (verified absent from all non-test source).
//!
//! A quiche SERVER that never calls `enable_early_data()` leaves
//! `max_early_data_size = 0`, so it issues session tickets WITHOUT the
//! early-data marker and cannot accept 0-RTT — early data is impossible by
//! construction. (`ZeroRttReplayGuard`, `router.rs:227`, remains
//! defence-in-depth against retry-token replay; it is NOT removed.)
//!
//! ## ON THE WIRE (this test — quiche 0.28 mechanism)
//!
//! The LB-as-server here is built by `lb_server_config`, which — exactly
//! like production `build_server_config` — does NOT call
//! `enable_early_data`. A real client that DOES enable early data attempts
//! 0-RTT against it:
//!
//! 1. Connection #1 completes a FULL 1-RTT handshake against the LB server
//!    and captures `client.session()` (the resumption ticket the LB issued).
//! 2. Connection #2 (fresh client, `enable_early_data()` ON) calls
//!    `set_session(captured)` BEFORE any packet, then — before
//!    `is_established()` — attempts to `stream_send` early data.
//! 3. ASSERTED ON THE WIRE:
//!    * the resuming client NEVER reports `is_in_early_data() == true`
//!      (the LB's ticket carries no early-data capability, so BoringSSL never
//!      opens the 0-RTT epoch on the client) — i.e. NO 0-RTT is offered/acted
//!      on before handshake completion;
//!    * the connection completes via FULL 1-RTT (`is_established()`), and
//!      `is_resumed()` confirms the ticket WAS used for resumption (so the
//!      ticket path is genuinely exercised — this is not a vacuous "no
//!      ticket" pass);
//!    * the early bytes the client queued are delivered to the peer ONLY
//!      after the handshake completes — never as 0-RTT.
//!
//! Honest scope note: the wire assertion is made against an LB-server config
//! that is constructed identically to production `build_server_config`
//! w.r.t. the early-data property (no `enable_early_data`). The by-
//! construction citation is what binds the production path; this test
//! demonstrates the resulting wire behaviour end-to-end with a willing 0-RTT
//! client.
//!
//! Driven with `--features test-gauges` for parity with the other B6 proofs
//! (this particular test does not need the actor hook, but stays in-family).

#![cfg(feature = "test-gauges")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::net::UdpSocket;

const TEST_SNI: &str = "expressgateway.test";
const H3_ALPN: &[u8] = b"h3";
const MAX_UDP: usize = 65_535;
const HANDSHAKE_BUDGET: Duration = Duration::from_secs(6);
/// The client bidi stream the resuming client tries to send "early" on.
const EARLY_STREAM_ID: u64 = 0;
/// A FIXED session-ticket key shared by both LB-server configs so a ticket
/// issued on connection #1 is decryptable on connection #2 — i.e. resumption
/// genuinely happens. (quiche auto-rotates a per-`Config` key otherwise, so a
/// fresh `Config` cannot decrypt a prior connection's ticket; the same is
/// true of production's per-accept config factory. Pinning the key here lets
/// the proof EXERCISE the resumption path so "no early data" is non-vacuous.
/// BoringSSL ticket key is 48 bytes.)
const SESSION_TICKET_KEY: [u8; 48] = [0xa5; 48];

// ─────────────────────────────────────────────────────────────────────
// Cert plumbing.
// ─────────────────────────────────────────────────────────────────────

static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

struct TestCerts {
    dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    ca: PathBuf,
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
    let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "lb-quic-s19-b6-0rtt-{}-{nanos}-{seq}",
        std::process::id()
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
    std::fs::write(&cert_path, cert.pem().as_bytes()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem().as_bytes()).unwrap();
    std::fs::write(&ca_path, cert.pem().as_bytes()).unwrap();
    TestCerts {
        dir,
        cert: cert_path,
        key: key_path,
        ca: ca_path,
    }
}

fn random_scid() -> [u8; quiche::MAX_CONN_ID_LEN] {
    use ring::rand::SecureRandom;
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    ring::rand::SystemRandom::new().fill(&mut scid).unwrap();
    scid
}

/// CLIENT-facing SERVER config — constructed IDENTICALLY to production
/// `build_server_config` (`crates/lb-quic/src/listener.rs:426`) w.r.t. the
/// early-data property: it loads the cert/key + ALPN + transport params and
/// — crucially — NEVER calls `enable_early_data()`. This is the LB-as-server
/// the 0-RTT attempt is made against.
fn lb_server_config(certs: &TestCerts) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
        .unwrap();
    cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
        .unwrap();
    cfg.set_max_idle_timeout(10_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(8);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    // Pin the session-ticket key so connection #2 can decrypt connection #1's
    // ticket and resumption genuinely occurs (see SESSION_TICKET_KEY docs).
    cfg.set_ticket_key(&SESSION_TICKET_KEY).unwrap();
    // NOTE: deliberately NO `cfg.enable_early_data()` — mirrors production
    // `build_server_config` (listener.rs). With early data disabled the
    // server issues 1-RTT-resumption tickets only (max_early_data_size = 0).
    cfg
}

/// The real CLIENT config. Verifies the LB cert AND — to make the 0-RTT
/// attempt a genuine one — ENABLES early data on the client. A willing 0-RTT
/// client is the adversary here: the proof is that even with the client
/// asking, the LB-as-server (no early data) gives it no 0-RTT.
fn client_config_early_data(certs: &TestCerts) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_verify_locations_from_file(certs.ca.to_str().unwrap())
        .unwrap();
    cfg.verify_peer(true);
    cfg.set_max_idle_timeout(10_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(8);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    // The CLIENT enables early data — it WANTS to send 0-RTT. The LB-server
    // must still refuse to act on any early data.
    cfg.enable_early_data();
    cfg
}

async fn flush(conn: &mut quiche::Connection, socket: &UdpSocket, out: &mut [u8]) {
    loop {
        match conn.send(out) {
            Ok((n, info)) => {
                let _ = socket.send_to(out.get(..n).unwrap_or(&[]), info.to).await;
            }
            Err(quiche::Error::Done) => break,
            Err(e) => panic!("conn.send: {e:?}"),
        }
    }
}

async fn try_recv_one(
    conn: &mut quiche::Connection,
    socket: &UdpSocket,
    local: SocketAddr,
    in_buf: &mut [u8],
    wait: Duration,
) {
    if let Ok(Ok((n, from))) = tokio::time::timeout(wait, socket.recv_from(in_buf)).await {
        let info = quiche::RecvInfo { from, to: local };
        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
        let _ = conn.recv(slice, info);
    }
}

/// Drive a client⇄server pair to BOTH established, watching the client for any
/// `is_in_early_data() == true` (records it if seen). Returns `true` once both
/// are established. `early_seen` is set if the client ever entered the 0-RTT
/// epoch (which would mean the server accepted/enabled early data).
#[allow(clippy::too_many_arguments)]
async fn drive_to_established(
    client_conn: &mut quiche::Connection,
    client_socket: &UdpSocket,
    client_local: SocketAddr,
    server_conn: &mut quiche::Connection,
    server_socket: &UdpSocket,
    server_local: SocketAddr,
    early_seen: &mut bool,
    server_early_recv: &mut bool,
) {
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let deadline = tokio::time::Instant::now() + HANDSHAKE_BUDGET;
    while !(server_conn.is_established() && client_conn.is_established()) {
        if tokio::time::Instant::now() > deadline {
            panic!("handshake did not establish within budget");
        }
        // If the client EVER enters the early-data epoch, the server must
        // have advertised/accepted early data — a 0-RTT acceptance.
        if client_conn.is_in_early_data() {
            *early_seen = true;
        }
        // If the server EVER reports readable streams BEFORE it is
        // established, it has accepted early (0-RTT) application data.
        if !server_conn.is_established() && server_conn.is_readable() {
            *server_early_recv = true;
        }
        flush(client_conn, client_socket, &mut out).await;
        flush(server_conn, server_socket, &mut out).await;
        try_recv_one(
            server_conn,
            server_socket,
            server_local,
            &mut in_buf,
            Duration::from_millis(20),
        )
        .await;
        try_recv_one(
            client_conn,
            client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(20),
        )
        .await;
    }
}

/// THE B6 0-RTT-rejection wire proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s19_b6_lb_server_rejects_client_zero_rtt_early_data() {
    let certs = generate_loopback_certs();

    // Two UDP sockets: the LB-as-server leg and the client leg.
    let server_socket = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    let server_local = server_socket.local_addr().unwrap();
    let client_socket = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    let client_local = client_socket.local_addr().unwrap();

    // ── Connection #1: full 1-RTT handshake; capture the LB's session ──
    let mut server_cfg = lb_server_config(&certs);
    let mut client_cfg = client_config_early_data(&certs);

    let s_scid = random_scid();
    let s_scid_ref = quiche::ConnectionId::from_ref(&s_scid);
    let c_scid = random_scid();
    let c_scid_ref = quiche::ConnectionId::from_ref(&c_scid);

    let mut server1 = quiche::accept(
        &s_scid_ref,
        None,
        server_local,
        client_local,
        &mut server_cfg,
    )
    .unwrap();
    let mut client1 = quiche::connect(
        Some(TEST_SNI),
        &c_scid_ref,
        client_local,
        server_local,
        &mut client_cfg,
    )
    .unwrap();

    let mut early_seen_1 = false;
    let mut server_early_1 = false;
    drive_to_established(
        &mut client1,
        &client_socket,
        client_local,
        &mut server1,
        &server_socket,
        server_local,
        &mut early_seen_1,
        &mut server_early_1,
    )
    .await;
    assert_eq!(client1.application_proto(), H3_ALPN);

    // Pump a few extra turns so the server's post-handshake NewSessionTicket
    // reaches the client and `session()` returns the resumption blob.
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut captured: Option<Vec<u8>> = None;
    let ticket_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while captured.is_none() && tokio::time::Instant::now() < ticket_deadline {
        flush(&mut server1, &server_socket, &mut out).await;
        flush(&mut client1, &client_socket, &mut out).await;
        try_recv_one(
            &mut client1,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(20),
        )
        .await;
        try_recv_one(
            &mut server1,
            &server_socket,
            server_local,
            &mut in_buf,
            Duration::from_millis(20),
        )
        .await;
        if let Some(s) = client1.session() {
            captured = Some(s.to_vec());
        }
    }
    let session = captured.expect(
        "the LB server must issue a session ticket (1-RTT resumption is \
         supported; only EARLY DATA is not) — required to drive the 0-RTT attempt",
    );
    // Connection #1 never entered early data (it was a fresh, non-resumed
    // handshake) — sanity only.
    assert!(!early_seen_1, "fresh conn #1 must not be in early data");

    // ── Connection #2: resume + ATTEMPT 0-RTT early data ──
    let mut server_cfg2 = lb_server_config(&certs);
    let mut client_cfg2 = client_config_early_data(&certs);

    let s_scid2 = random_scid();
    let s_scid2_ref = quiche::ConnectionId::from_ref(&s_scid2);
    let c_scid2 = random_scid();
    let c_scid2_ref = quiche::ConnectionId::from_ref(&c_scid2);

    let mut server2 = quiche::accept(
        &s_scid2_ref,
        None,
        server_local,
        client_local,
        &mut server_cfg2,
    )
    .unwrap();
    let mut client2 = quiche::connect(
        Some(TEST_SNI),
        &c_scid2_ref,
        client_local,
        server_local,
        &mut client_cfg2,
    )
    .unwrap();

    // Offer the captured session for resumption — MUST be before any packet.
    client2
        .set_session(&session)
        .expect("set_session must accept the LB-issued ticket");

    // ATTEMPT to send early (0-RTT) data BEFORE the handshake completes. If
    // the LB had enabled early data, `is_in_early_data()` would open and this
    // payload would ride a 0-RTT packet. Against a no-early-data LB it can
    // only be sent after 1-RTT establishment.
    let early_payload = b"ZERO-RTT-EARLY-DATA-ATTEMPT".to_vec();
    assert!(
        !client2.is_established(),
        "fixture: must attempt the early send BEFORE establishment"
    );
    // The stream_send may be buffered by quiche regardless of epoch; the
    // load-bearing question is WHICH epoch carries it (0-RTT vs 1-RTT), which
    // we observe via is_in_early_data() during the handshake drive.
    let _ = client2.stream_send(EARLY_STREAM_ID, &early_payload, true);

    // Drive #2 to established, watching for ANY early-data epoch on either end.
    let mut early_seen_2 = false;
    let mut server_early_2 = false;
    drive_to_established(
        &mut client2,
        &client_socket,
        client_local,
        &mut server2,
        &server_socket,
        server_local,
        &mut early_seen_2,
        &mut server_early_2,
    )
    .await;

    // ── WIRE ASSERTIONS ───────────────────────────────────────────────
    eprintln!(
        "conn#2: is_resumed={} is_in_early_data(final)={} early_seen_during_hs={} server_early_recv={}",
        client2.is_resumed(),
        client2.is_in_early_data(),
        early_seen_2,
        server_early_2,
    );

    // (1) The resuming client NEVER entered the 0-RTT epoch — the LB issued a
    //     ticket WITHOUT early-data capability, so BoringSSL never opened
    //     0-RTT. No early data was offered/acted on before the handshake.
    assert!(
        !early_seen_2,
        "0-RTT REJECTION: the resuming client MUST NOT enter is_in_early_data() \
         at any point — the LB server never enabled early data, so no 0-RTT \
         epoch exists; entering it would mean the LB accepted 0-RTT"
    );
    assert!(
        !client2.is_in_early_data(),
        "0-RTT REJECTION: client must not be in early data after the drive"
    );

    // (2) The SERVER never surfaced readable application data before it was
    //     established — i.e. it never acted on early (0-RTT) data.
    assert!(
        !server_early_2,
        "0-RTT REJECTION: the LB server MUST NOT have any readable stream data \
         before its handshake completes (it never acted on early data)"
    );

    // (3) The connection still completed via FULL 1-RTT, AND it genuinely used
    //     the ticket (is_resumed) — so this is not a vacuous "no ticket" pass:
    //     a real resumption happened, and it was 1-RTT, not 0-RTT.
    assert!(
        client2.is_established() && server2.is_established(),
        "the connection must complete via full 1-RTT"
    );
    assert!(
        client2.is_resumed(),
        "the ticket MUST have been used for (1-RTT) resumption — proving the \
         0-RTT-capable path was genuinely exercised and still refused early data"
    );

    // (4) Only AFTER establishment can the early bytes reach the peer — never
    //     as 0-RTT. Drain the server post-establishment and confirm it now
    //     receives the bytes the client queued "early".
    let mut rd = vec![0u8; MAX_UDP];
    let mut server_got: Vec<u8> = Vec::new();
    let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while server_got.len() < early_payload.len() && tokio::time::Instant::now() < drain_deadline {
        flush(&mut client2, &client_socket, &mut out).await;
        flush(&mut server2, &server_socket, &mut out).await;
        try_recv_one(
            &mut server2,
            &server_socket,
            server_local,
            &mut in_buf,
            Duration::from_millis(20),
        )
        .await;
        try_recv_one(
            &mut client2,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(20),
        )
        .await;
        let readable: Vec<u64> = server2.readable().collect();
        for sid in readable {
            loop {
                match server2.stream_recv(sid, &mut rd) {
                    Ok((n, _fin)) => server_got.extend_from_slice(rd.get(..n).unwrap_or(&[])),
                    Err(quiche::Error::Done) => break,
                    Err(_) => break,
                }
            }
        }
    }
    // The data the client queued "early" is delivered post-1-RTT (verbatim),
    // proving it was NOT dropped — it simply waited for the full handshake.
    assert_eq!(
        server_got, early_payload,
        "the bytes the client queued early are delivered to the server ONLY \
         after the 1-RTT handshake (never as 0-RTT), byte-identical"
    );
}
