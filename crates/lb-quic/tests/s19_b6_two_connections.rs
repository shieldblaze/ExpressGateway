//! SESSION 19 / Mode B — B6 AUTHORITATIVE TWO-CONNECTIONS security proof
//! (author ≠ verifier; this is the verifier's dedicated structural proof).
//!
//! The headline Mode-B security property: the LB **terminates** the client
//! QUIC connection and **re-originates** a SEPARATE QUIC connection to the
//! backend. It is NOT a CID bridge / packet forwarder — it holds TWO
//! genuinely distinct `quiche::Connection` objects, each with its own SCID
//! and therefore its own independent TLS key schedule. A bridge would carry
//! the client's connection (and its keys) straight through to the backend.
//!
//! Topology (real wire, mirrors `s16_b2_stream_relay_smoke.rs` /
//! `s19_b4_datagram_verify.rs`):
//!
//!   real quiche CLIENT  ⇄  Mode B actor (`run_raw_proxy_actor_for_test`)
//!                          ⇄  real quiche BACKEND server
//!
//! ## What is proven here (by mechanism, not assertion)
//!
//! 1. `client_scid != upstream_scid` — the LB sampled an INDEPENDENT SCID
//!    when re-originating upstream. Distinct SCIDs ⇒ distinct connections.
//! 2. `client_trace_id != upstream_trace_id` — quiche derives `trace_id`
//!    per `Connection` object, so two distinct trace ids prove two distinct
//!    objects (independent key schedules, recovery state, stream tables).
//! 3. `negotiated_alpn` is MIRRORED upstream (the upstream pool factory's
//!    default ALPN is deliberately WRONG; only ALPN mirroring lets the
//!    upstream handshake succeed against the h3-only backend).
//! 4. **LOAD-BEARING independence witness**: the BACKEND records the SCID it
//!    observed on the inbound Initial — independently of the actor's
//!    `RawProxyOutcome`. We assert that backend-observed SCID (a) equals the
//!    actor's reported `upstream_scid` (same upstream connection) and (b) is
//!    NOT the CLIENT's chosen SCID nor a byte-prefix derivation of it. A
//!    bridge would make the backend see the CLIENT's SCID; a re-origination
//!    makes it see a freshly random one. This is the assertion that FAILS on
//!    a bridge and PASSES on Mode B.
//!
//! ## Structural 1:1 (by construction — code citation)
//!
//! Independently of the wire test, the architecture CANNOT hold fewer than
//! two connections:
//! * the CLIENT-facing connection is created by `quiche::accept_with_retry`
//!   in the router — `crates/lb-quic/src/router.rs:351` — and handed to the
//!   actor as `ActorParams.conn`;
//! * the UPSTREAM connection is created by `QuicUpstreamPool::dial_dedicated`
//!   — `crates/lb-io/src/quic_pool.rs:412` — which `quiche::connect`s a
//!   brand-new `Connection` on its OWN UDP socket (un-pooled, owned solely
//!   by this actor). The actor calls it at
//!   `crates/lb-quic/src/raw_proxy.rs:287`.
//!
//! Both objects then live side-by-side in `run_raw_proxy_actor_inner`
//! (`params.conn` and `upstream.conn`) and are pumped by `run_dual_pump`.
//! Two separate `quiche::Connection` allocations owned by one actor ⇒ the
//! datapath is structurally 1:1 (one client conn : one upstream conn) and
//! cannot collapse to a single bridged connection.
//!
//! Driven with `--features test-gauges` so `run_raw_proxy_actor_for_test`
//! (gated `#[cfg(any(test, feature = "test-gauges"))]`) is reachable.

#![cfg(feature = "test-gauges")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::quic_pool::{QuicPoolConfig, QuicUpstreamPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::RawBackend;
use lb_quic::conn_actor::{ActorParams, InboundPacket};
use lb_quic::raw_proxy::run_raw_proxy_actor_for_test;

const TEST_SNI: &str = "expressgateway.test";
const H3_ALPN: &[u8] = b"h3";
const MAX_UDP: usize = 65_535;
const HANDSHAKE_BUDGET: Duration = Duration::from_secs(5);

// ─────────────────────────────────────────────────────────────────────
// Cert plumbing (mirrors s16_b1_two_connections.rs).
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
        "lb-quic-s19-b6-2conn-{}-{nanos}-{seq}",
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

/// CLIENT-facing SERVER config (the LB-as-server leg). Serves the loopback
/// cert; advertises `h3`. Note: NO `enable_early_data` — the Mode-B client-
/// facing server cannot issue early-data tickets nor accept 0-RTT (the
/// dedicated 0-RTT proof is in `s19_b6_zero_rtt_rejection.rs`).
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
    cfg.set_initial_max_streams_bidi(4);
    cfg.set_initial_max_streams_uni(4);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

/// The real CLIENT (downstream) config — verifies the LB's cert.
fn client_config(certs: &TestCerts) -> quiche::Config {
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
    cfg.set_initial_max_streams_bidi(4);
    cfg.set_initial_max_streams_uni(4);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

/// The pool's per-dial CLIENT config factory (LB → backend re-origination
/// leg). Installs a DELIBERATELY-WRONG ALPN so the test proves the actor
/// MIRRORS the client's negotiated `h3` onto the dedicated dial (without
/// the override the backend — which only speaks `h3` — would TLS-fail).
fn upstream_config_factory(
    ca: PathBuf,
) -> Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> {
    Arc::new(move || {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
        cfg.set_application_protos(&[b"mode-b-factory-default"])?;
        cfg.load_verify_locations_from_file(ca.to_str().ok_or(quiche::Error::TlsFail)?)
            .map_err(|_| quiche::Error::TlsFail)?;
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(10_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(4);
        cfg.set_initial_max_streams_uni(4);
        cfg.set_disable_active_migration(true);
        cfg.enable_dgram(true, 1024, 1024);
        Ok(cfg)
    })
}

/// A throwaway BACKEND quiche server that accepts ONE connection, drives it
/// to established, and RECORDS the SCID it observed on the inbound Initial
/// header (= the SCID the LB-as-client chose for the upstream connection).
/// That recorded value is the load-bearing independence witness: a bridge
/// would make the backend see the CLIENT's SCID.
fn spawn_backend_recording_scid(certs: &TestCerts) -> (SocketAddr, Arc<Mutex<Option<Vec<u8>>>>) {
    let std_sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    std_sock.set_nonblocking(true).unwrap();
    let addr = std_sock.local_addr().unwrap();
    let observed: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let observed_task = Arc::clone(&observed);
    let mut config = lb_server_config(certs);

    tokio::spawn(async move {
        let socket = UdpSocket::from_std(std_sock).unwrap();
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];
        let mut conn: Option<quiche::Connection> = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            if let Some(c) = conn.as_mut() {
                loop {
                    match c.send(&mut out_buf) {
                        Ok((n, info)) => {
                            let _ = socket
                                .send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                                .await;
                        }
                        Err(quiche::Error::Done) => break,
                        Err(_) => break,
                    }
                }
            }
            let timeout = conn
                .as_ref()
                .and_then(quiche::Connection::timeout)
                .unwrap_or(Duration::from_millis(50));
            match tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((n, from))) => {
                    if conn.is_none() {
                        // Record the SCID the LB chose as client (the
                        // upstream connection's SCID) straight off the
                        // wire — BEFORE accept, independently of the
                        // RawProxyOutcome the actor returns.
                        if let Ok(hdr) = quiche::Header::from_slice(
                            in_buf.get_mut(..n).unwrap_or(&mut []),
                            quiche::MAX_CONN_ID_LEN,
                        ) {
                            if let Ok(mut g) = observed_task.lock() {
                                *g = Some(hdr.scid.to_vec());
                            }
                        }
                        let scid = random_scid();
                        let scid_ref = quiche::ConnectionId::from_ref(&scid);
                        match quiche::accept(&scid_ref, None, addr, from, &mut config) {
                            Ok(c) => conn = Some(c),
                            Err(_) => continue,
                        }
                    }
                    if let Some(c) = conn.as_mut() {
                        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                        let info = quiche::RecvInfo { from, to: addr };
                        let _ = c.recv(slice, info);
                    }
                }
                Ok(Err(_)) | Err(_) => {
                    if let Some(c) = conn.as_mut() {
                        c.on_timeout();
                    }
                }
            }
        }
    });

    (addr, observed)
}

async fn flush(conn: &mut quiche::Connection, socket: &UdpSocket, out: &mut [u8]) {
    loop {
        match conn.send(out) {
            Ok((n, info)) => {
                let _ = socket.send_to(out.get(..n).unwrap_or(&[]), info.to).await;
            }
            Err(quiche::Error::Done) => break,
            Err(e) => panic!("client conn.send: {e:?}"),
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

/// THE B6 two-connections security proof: real client ⇄ Mode B actor ⇄ real
/// backend, asserting two distinct quiche connections by mechanism (distinct
/// SCIDs + trace ids + the backend's own independence witness).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s19_b6_mode_b_holds_two_distinct_connections_not_a_bridge() {
    let certs = generate_loopback_certs();

    // 1) Real backend that records the SCID the LB dials it with.
    let (backend_addr, observed_upstream_scid) = spawn_backend_recording_scid(&certs);

    // 2) The shared LB listener socket (the "server" leg).
    let lb_socket = Arc::new(
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap(),
    );
    let lb_local = lb_socket.local_addr().unwrap();

    // 3) The real downstream CLIENT.
    let client_socket = Arc::new(
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap(),
    );
    let client_local = client_socket.local_addr().unwrap();

    let mut server_cfg = lb_server_config(&certs);
    let mut client_cfg = client_config(&certs);

    let s_scid = random_scid();
    let s_scid_ref = quiche::ConnectionId::from_ref(&s_scid);
    let c_scid = random_scid();
    let c_scid_ref = quiche::ConnectionId::from_ref(&c_scid);

    let mut server_conn =
        quiche::accept(&s_scid_ref, None, lb_local, client_local, &mut server_cfg).unwrap();
    let mut client_conn = quiche::connect(
        Some(TEST_SNI),
        &c_scid_ref,
        client_local,
        lb_local,
        &mut client_cfg,
    )
    .unwrap();

    // The client's CHOSEN SCID — what a bridge would forward to the backend.
    let client_chosen_scid = client_conn.source_id().as_ref().to_vec();

    // 4) Inline-drive BOTH legs to established (round8 pattern).
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let deadline = tokio::time::Instant::now() + HANDSHAKE_BUDGET;
    while !(server_conn.is_established() && client_conn.is_established()) {
        if tokio::time::Instant::now() > deadline {
            panic!("client⇄LB handshake did not establish");
        }
        flush(&mut client_conn, &client_socket, &mut out).await;
        flush(&mut server_conn, &lb_socket, &mut out).await;
        try_recv_one(
            &mut server_conn,
            &lb_socket,
            lb_local,
            &mut in_buf,
            Duration::from_millis(20),
        )
        .await;
        try_recv_one(
            &mut client_conn,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(20),
        )
        .await;
    }
    assert_eq!(
        client_conn.application_proto(),
        H3_ALPN,
        "fixture: client must negotiate h3 with the LB"
    );

    // 5) Forwarder: shared LB socket → actor inbound mpsc (router stand-in).
    let (tx, rx) = mpsc::channel::<InboundPacket>(64);
    let cancel = CancellationToken::new();
    let fwd_socket = Arc::clone(&lb_socket);
    let fwd_cancel = cancel.clone();
    let forwarder = tokio::spawn(async move {
        let mut buf = vec![0u8; MAX_UDP];
        loop {
            tokio::select! {
                () = fwd_cancel.cancelled() => break,
                r = fwd_socket.recv_from(&mut buf) => {
                    if let Ok((n, from)) = r {
                        let pkt = InboundPacket {
                            data: buf.get(..n).unwrap_or(&[]).to_vec(),
                            from,
                            to: lb_local,
                        };
                        if tx.send(pkt).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    });

    // 6) Client keep-alive driver (so neither leg idles out mid-dial).
    let client_cancel = cancel.clone();
    let client_driver = tokio::spawn(async move {
        let mut out = vec![0u8; MAX_UDP];
        let mut in_buf = vec![0u8; MAX_UDP];
        loop {
            if client_cancel.is_cancelled() || client_conn.is_closed() {
                break;
            }
            flush(&mut client_conn, &client_socket, &mut out).await;
            try_recv_one(
                &mut client_conn,
                &client_socket,
                client_local,
                &mut in_buf,
                Duration::from_millis(20),
            )
            .await;
        }
    });

    // 7) The Mode B re-origination backend.
    let pool = QuicUpstreamPool::new(
        QuicPoolConfig::default(),
        upstream_config_factory(certs.ca.clone()),
    );
    let raw_backend = RawBackend {
        pool,
        addr: backend_addr,
        sni: TEST_SNI.to_string(),
        dgram_queue_cap: lb_quic::DGRAM_QUEUE_CAP,
        max_relay_streams: lb_quic::MAX_RELAY_STREAMS,
    };
    let runtime = lb_io::Runtime::new();
    let tcp_pool = TcpPool::new(PoolConfig::default(), BackendSockOpts::default(), runtime);
    let params = ActorParams {
        conn: server_conn,
        socket: Arc::clone(&lb_socket),
        inbound: rx,
        cancel: cancel.clone(),
        pool: tcp_pool,
        backends: Arc::new(Vec::new()),
        h3_backend: None,
        h2_backend: None,
        raw_quic_backend: Some(raw_backend),
        quic_modeb_metrics: None,
        // SESSION 27 WS-over-H3 Stage A: Mode-B tests never H3-terminate.
        ws_enabled: false,
        ws_relay_launcher: None,
        max_requests_per_h3_connection: 0,
        h3_recycle_metrics: None,
    };

    // 8) Cancel shortly after both legs are up so the actor returns its
    //    RawProxyOutcome (graceful close → Ok(outcome)).
    let cancel_for_timer = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        cancel_for_timer.cancel();
    });

    // 9) Drive the actor via the test hook and capture the outcome.
    let outcome = tokio::time::timeout(
        Duration::from_secs(15),
        run_raw_proxy_actor_for_test(params),
    )
    .await
    .expect("Mode B actor must not hang")
    .expect("Mode B actor must establish both legs and return RawProxyOutcome");

    cancel.cancel();
    forwarder.abort();
    let _ = client_driver.await;

    // ── MECHANISM ASSERTIONS ──────────────────────────────────────────
    eprintln!("client_scid          = {:02x?}", outcome.client_scid);
    eprintln!("upstream_scid        = {:02x?}", outcome.upstream_scid);
    eprintln!("client_trace_id      = {}", outcome.client_trace_id);
    eprintln!("upstream_trace_id    = {}", outcome.upstream_trace_id);
    eprintln!(
        "negotiated_alpn      = {:?}",
        String::from_utf8_lossy(&outcome.negotiated_alpn)
    );
    eprintln!("client_chosen_scid   = {client_chosen_scid:02x?}");

    // (1) Distinct SCIDs — the LB chose an independent SCID upstream.
    assert_ne!(
        outcome.client_scid, outcome.upstream_scid,
        "two-connections proof: client SCID and upstream SCID MUST differ \
         (a bridge would reuse/derive one CID ⇒ shared key schedule)"
    );

    // (2) Distinct quiche::Connection objects — distinct trace_ids.
    assert_ne!(
        outcome.client_trace_id, outcome.upstream_trace_id,
        "two-connections proof: client and upstream MUST be distinct \
         quiche::Connection objects (distinct trace_ids ⇒ independent \
         key schedules / recovery / stream tables)"
    );

    // (3) ALPN mirrored upstream (and NOT the factory default).
    assert_eq!(
        outcome.negotiated_alpn,
        H3_ALPN,
        "ALPN mirroring: the actor must mirror the client's negotiated `h3` \
         upstream; got {:?}",
        String::from_utf8_lossy(&outcome.negotiated_alpn)
    );

    // (4) LOAD-BEARING independence — the BACKEND's own witness.
    let backend_saw = observed_upstream_scid.lock().unwrap().clone().expect(
        "backend must have observed an inbound Initial (the LB \
             re-originated a real upstream connection)",
    );
    eprintln!("backend_observed_scid= {backend_saw:02x?}");

    assert_eq!(
        backend_saw, outcome.upstream_scid,
        "the SCID the backend observed on the inbound Initial MUST equal the \
         actor's reported upstream SCID (same upstream connection)"
    );
    // A bridge would forward the CLIENT's connection — the backend would see
    // the CLIENT's chosen SCID. Re-origination samples a fresh one.
    assert_ne!(
        backend_saw, client_chosen_scid,
        "two-connections proof (LOAD-BEARING): the backend MUST NOT see the \
         CLIENT's SCID — a Mode B re-origination dials with a freshly sampled \
         SCID; equality here would mean the client's connection was bridged"
    );
    // And the upstream SCID must not be the LB-as-server SCID either.
    assert_ne!(
        backend_saw, outcome.client_scid,
        "the upstream SCID must be independent of the LB's client-facing \
         (server) SCID"
    );

    // (5) Independence is not a trivial coincidence: the upstream SCID is not
    //     a truncation/prefix-derivation of the client's chosen SCID.
    let common_prefix = outcome
        .upstream_scid
        .iter()
        .zip(client_chosen_scid.iter())
        .take_while(|(a, b)| a == b)
        .count();
    assert!(
        common_prefix < outcome.upstream_scid.len(),
        "upstream SCID must not be byte-identical to the client SCID"
    );
    assert!(
        common_prefix <= 2,
        "upstream SCID shares a {common_prefix}-byte prefix with the client \
         SCID — suspicious of a derive-from-client bridge (expected \
         independent randomness)"
    );
}
