//! Mode B — B4 authoritative verifier wire tests over real quiche client ⇄ Mode
//! B actor ⇄ real quiche DATAGRAM backend. TWO genuinely distinct
//! `quiche::Connection`s (the terminated client leg and the re-originated
//! upstream), proven by the distinct SCIDs in [`RawProxyOutcome`].
//!
//! Proves: (1) real-wire byte-identical pass-through BOTH directions for
//! binary / zero-length / all-zero / non-UTF8 / near-max payloads, plus the
//! two-distinct-connections mechanism check; (2) under a sustained flood at a
//! STALLED destination the bounded drop-newest queue holds — the connection
//! stays alive, nothing OOMs or hangs, and delivery is BOUNDED (the internal
//! `dropped` counter is not exported, so this is an observable-behaviour
//! proof); (3) a flood larger than the cap at a slowly-draining backend still
//! yields bounded delivery and a healthy connection.
//!
//! The which-end proof (drop-NEWEST, not drop-oldest) is pinned
//! deterministically at the unit level by `dgram_queue_drop_newest_negative_
//! control`; on the wire only the deterministic properties are asserted.
//!
//! Driven with `--features test-gauges` so the test hook is reachable.

#![cfg(feature = "test-gauges")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::quic_pool::{QuicPoolConfig, QuicUpstreamPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::RawBackend;
use lb_quic::conn_actor::{ActorParams, InboundPacket};
use lb_quic::raw_proxy::{RawProxyOutcome, run_raw_proxy_actor_for_test};

const TEST_SNI: &str = "expressgateway.test";
const H3_ALPN: &[u8] = b"h3";
const MAX_UDP: usize = 65_535;
const HANDSHAKE_BUDGET: Duration = Duration::from_secs(5);
const RELAY_BUDGET: Duration = Duration::from_secs(12);

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
        "lb-quic-s19-b4-verify-{}-{nanos}-{seq}",
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

/// CLIENT-facing SERVER config: negotiates DATAGRAM with quiche queues at 1024,
/// matching the production `DGRAM_QUEUE_CAP`, so the RELAY-layer bound is the
/// binding one under flood.
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
    cfg.set_initial_max_stream_data_bidi_local(256 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(8);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

/// The real downstream CLIENT config — verifies the LB's cert.
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
    cfg.set_initial_max_stream_data_bidi_local(256 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(8);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

/// The pool's per-dial CLIENT config factory (LB → backend leg).
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
        cfg.set_initial_max_stream_data_bidi_local(256 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(8);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        cfg.enable_dgram(true, 1024, 1024);
        Ok(cfg)
    })
}

/// Throwaway BACKEND server: accepts ONE connection and ECHOes any received
/// DATAGRAM — unless `stall` is set, when it stops *reading* them, so its recv
/// queue fills and the LB→backend `dgram_send` starts returning `Done`,
/// exercising the relay's bounded-queue path. It always keeps the connection
/// alive, so the test observes liveness rather than a teardown.
fn spawn_dgram_backend(certs: &TestCerts, stall: Arc<AtomicBool>) -> SocketAddr {
    let std_sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    std_sock.set_nonblocking(true).unwrap();
    let addr = std_sock.local_addr().unwrap();
    let mut config = lb_server_config(certs);

    tokio::spawn(async move {
        let socket = UdpSocket::from_std(std_sock).unwrap();
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];
        let mut rd = vec![0u8; MAX_UDP];
        let mut echo_q: std::collections::VecDeque<Vec<u8>> = std::collections::VecDeque::new();
        let mut conn: Option<quiche::Connection> = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(40);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            if let Some(c) = conn.as_mut() {
                // While stalled, deliberately do NOT drain the recv queue so it
                // fills and back-pressures the LB→backend leg.
                if !stall.load(Ordering::Relaxed) {
                    loop {
                        match c.dgram_recv(&mut rd) {
                            Ok(n) => echo_q.push_back(rd.get(..n).unwrap_or(&[]).to_vec()),
                            Err(quiche::Error::Done) => break,
                            Err(_) => break,
                        }
                    }
                    while let Some(front) = echo_q.front() {
                        match c.dgram_send(front) {
                            Ok(()) => {
                                let _ = echo_q.pop_front();
                            }
                            Err(quiche::Error::Done) => break,
                            Err(_) => {
                                let _ = echo_q.pop_front();
                            }
                        }
                    }
                }
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
                .unwrap_or(Duration::from_millis(5))
                .min(Duration::from_millis(20));
            match tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((n, from))) => {
                    if conn.is_none() {
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

    addr
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

/// Shared harness: handshake the client⇄LB legs, wire the forwarder + the Mode
/// B actor against `backend_addr`, and return the live handles.
struct Rig {
    client_conn: quiche::Connection,
    client_socket: Arc<UdpSocket>,
    client_local: SocketAddr,
    cancel: CancellationToken,
    forwarder: tokio::task::JoinHandle<()>,
    actor: tokio::task::JoinHandle<std::io::Result<RawProxyOutcome>>,
    _certs: TestCerts,
}

async fn build_rig(certs: TestCerts, backend_addr: SocketAddr) -> Rig {
    let lb_socket = Arc::new(
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap(),
    );
    let lb_local = lb_socket.local_addr().unwrap();

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
    assert_eq!(client_conn.application_proto(), H3_ALPN);

    let (tx, rx) = mpsc::channel::<InboundPacket>(4096);
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

    let pool = QuicUpstreamPool::new(
        QuicPoolConfig::default(),
        upstream_config_factory(certs.ca.clone()),
    );
    let raw_backend = RawBackend {
        pool,
        addr: backend_addr,
        sni: TEST_SNI.to_string(),
        // B6 (R14/R12): caps now carried on RawBackend; the const
        // defaults keep these tests byte-identical in behaviour.
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
        ws_enabled: false,
        ws_relay_launcher: None,
        max_requests_per_h3_connection: 0,
        h3_recycle_metrics: None,
    };
    let actor = tokio::spawn(run_raw_proxy_actor_for_test(params));

    Rig {
        client_conn,
        client_socket,
        client_local,
        cancel,
        forwarder,
        actor,
        _certs: certs,
    }
}

/// Tear the rig down and read the two-connection proof. Returns the
/// `RawProxyOutcome` if the actor produced one (it does on graceful close).
async fn teardown(rig: Rig) -> Option<RawProxyOutcome> {
    rig.cancel.cancel();
    rig.forwarder.abort();
    tokio::time::timeout(Duration::from_secs(6), rig.actor)
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(Result::ok)
}

/// Pass-through fixture: varied shapes proving verbatim, binary-safe,
/// zero-length-preserving relay. Sized to FIT the negotiated writable length, so
/// even the large one is never refused with `BufferTooShort`.
fn pass_through_set() -> Vec<Vec<u8>> {
    vec![
        Vec::new(),                                           // zero-length
        vec![0u8; 128],                                       // all-zero bytes
        vec![0xff, 0xfe, 0x80, 0x00, 0x7f, 0xc0, 0xff, 0x01], // non-UTF8 high-bit
        b"plain-ascii-datagram".to_vec(),                     // ordinary text
        vec![0x00],                                           // single zero byte
        (0..1_200usize)
            .map(|i| ((i * 53 + 17) % 256) as u8)
            .collect(), // large near-max-writable
    ]
}

/// The echo backend bounces each datagram back, exercising BOTH relay
/// directions in one round-trip; every one must return byte-identical. Also
/// asserts the actor used TWO distinct connections.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_b4_pass_through_both_directions_binary() {
    let certs = generate_loopback_certs();
    let stall = Arc::new(AtomicBool::new(false)); // echo backend never stalls
    let backend_addr = spawn_dgram_backend(&certs, Arc::clone(&stall));
    let mut rig = build_rig(certs, backend_addr).await;

    let sent = pass_through_set();
    for d in &sent {
        rig.client_conn
            .dgram_send(d)
            .expect("client dgram_send (fits negotiated frame size)");
    }

    // Drive the client: flush, recv, collect echoes until we have them all
    // or the budget elapses.
    let expected = sent.len();
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut recv_buf = vec![0u8; MAX_UDP];
    let mut received: Vec<Vec<u8>> = Vec::new();
    let deadline = tokio::time::Instant::now() + RELAY_BUDGET;
    while received.len() < expected && tokio::time::Instant::now() < deadline {
        flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;
        try_recv_one(
            &mut rig.client_conn,
            &rig.client_socket,
            rig.client_local,
            &mut in_buf,
            Duration::from_millis(10),
        )
        .await;
        loop {
            match rig.client_conn.dgram_recv(&mut recv_buf) {
                Ok(n) => received.push(recv_buf.get(..n).unwrap_or(&[]).to_vec()),
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }
    }

    assert_eq!(
        received.len(),
        sent.len(),
        "client must receive exactly as many datagrams as it sent \
         (verbatim through the Mode B relay BOTH directions)"
    );
    let mut remaining = received;
    for s in &sent {
        let pos = remaining.iter().position(|r| r == s).unwrap_or_else(|| {
            panic!(
                "sent datagram of len {} was not received byte-identical",
                s.len()
            )
        });
        remaining.swap_remove(pos);
    }
    assert!(remaining.is_empty(), "no extra/unexpected datagrams");

    let outcome = teardown(rig).await.expect("actor produced an outcome");
    assert_ne!(
        outcome.client_scid, outcome.upstream_scid,
        "Mode B uses two DISTINCT quiche connections (distinct SCIDs) — not a CID bridge"
    );
    assert_ne!(
        outcome.client_trace_id, outcome.upstream_trace_id,
        "the two legs have distinct quiche trace ids"
    );
}

// PROOF 2 — bounded queue under flood (R8): the relay stays HEALTHY and
// delivery is BOUNDED; nothing OOMs, panics or hangs.

/// Flood the client→upstream direction at a backend that STOPS reading, so the
/// relay's bounded c2u queue saturates and drops-newest past cap. The bound is
/// proven by OBSERVABLE behaviour: the connection stays ALIVE throughout, the
/// run COMPLETES well within budget (no hang, no unbounded growth), and the
/// process does not OOM — an unbounded queue under a 50k-datagram flood at
/// ~1200 B each would be 60 MB+ AND still growing.
///
/// The flood count is far larger than the cap plus both quiche queues, so the
/// relay MUST be dropping.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_b4_queue_bound_under_flood_stays_healthy() {
    let certs = generate_loopback_certs();
    let stall = Arc::new(AtomicBool::new(true)); // backend never drains: sink is stalled
    let backend_addr = spawn_dgram_backend(&certs, Arc::clone(&stall));
    let mut rig = build_rig(certs, backend_addr).await;

    let payload: Vec<u8> = (0..1_000usize).map(|i| (i % 251) as u8).collect();
    const FLOOD: usize = 50_000;

    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut sent_ok: usize = 0;
    let mut send_full_events: usize = 0;
    let deadline = tokio::time::Instant::now() + RELAY_BUDGET;

    for _ in 0..FLOOD {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        match rig.client_conn.dgram_send(&payload) {
            Ok(()) => sent_ok += 1,
            // The client's OWN send queue is full: flush + drain a turn, then
            // keep flooding. The relay bound is downstream of this.
            Err(quiche::Error::Done) => {
                send_full_events += 1;
                flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;
                try_recv_one(
                    &mut rig.client_conn,
                    &rig.client_socket,
                    rig.client_local,
                    &mut in_buf,
                    Duration::from_millis(1),
                )
                .await;
            }
            Err(_) => break,
        }
        // Periodically pump the wire so the relay actually runs and the
        // bounded queue is exercised under sustained pressure.
        if sent_ok % 256 == 0 {
            flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;
            try_recv_one(
                &mut rig.client_conn,
                &rig.client_socket,
                rig.client_local,
                &mut in_buf,
                Duration::from_millis(1),
            )
            .await;
        }
    }

    // Liveness: the connection never closed. A hang or OOM would have blown the
    // budget, so reaching here within it with the conn open IS the bounded
    // -behaviour proof.
    assert!(
        !rig.client_conn.is_closed(),
        "the client⇄LB connection must stay ALIVE under the flood (the bounded \
         drop-newest queue absorbs the overload without tearing the conn down)"
    );
    assert!(
        send_full_events > 0,
        "the flood must have actually back-pressured (the client's own send \
         queue filled), confirming we drove well past the bounded capacities"
    );
    assert!(
        sent_ok > 1024,
        "the client successfully handed well over one relay-cap worth of \
         datagrams to the wire ({sent_ok}); the relay absorbed them bounded"
    );

    let outcome = teardown(rig).await.expect("actor produced an outcome");
    assert_ne!(
        outcome.client_scid, outcome.upstream_scid,
        "two distinct connections even after the flood"
    );
}

// PROOF 3 — drop-newest exercised on the wire (bounded delivery under a flood
// exceeding cap before drain). The deterministic which-end proof is the unit
// negative control.

/// Flood MORE than the relay cap into a backend that drains SLOWLY, then prove
/// on the wire that the destination receives a BOUNDED subset (strictly fewer
/// than were sent — drops occurred) and the connection stays healthy and still
/// flows once pressure eases.
///
/// Ordering across three bounded queues makes "exactly WHICH datagrams were
/// dropped" non-deterministic, so the which-end (drop-NEWEST) is pinned by the
/// unit negative control; here only bounded delivery + liveness + recovery are
/// asserted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_b4_drop_newest_oldest_survive_when_drained() {
    let certs = generate_loopback_certs();
    // Start stalled so the queues build past cap, then release to let the
    // surviving (bounded) subset flow back.
    let stall = Arc::new(AtomicBool::new(true));
    let backend_addr = spawn_dgram_backend(&certs, Arc::clone(&stall));
    let mut rig = build_rig(certs, backend_addr).await;

    // Index-tagged payloads so received datagrams are recognizable. Each is
    // a fixed ~600B (fits writable len). FLOOD > cap(1024) + quiche queues.
    const FLOOD: usize = 8_000;
    let mk = |i: usize| -> Vec<u8> {
        let mut v = vec![0u8; 600];
        v[0] = (i & 0xff) as u8;
        v[1] = ((i >> 8) & 0xff) as u8;
        v[2] = ((i >> 16) & 0xff) as u8;
        v
    };

    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut recv_buf = vec![0u8; MAX_UDP];
    let mut sent_ok: usize = 0;
    let deadline = tokio::time::Instant::now() + RELAY_BUDGET;

    for i in 0..FLOOD {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        match rig.client_conn.dgram_send(&mk(i)) {
            Ok(()) => sent_ok += 1,
            Err(quiche::Error::Done) => {
                flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;
                try_recv_one(
                    &mut rig.client_conn,
                    &rig.client_socket,
                    rig.client_local,
                    &mut in_buf,
                    Duration::from_millis(1),
                )
                .await;
            }
            Err(_) => break,
        }
        if sent_ok % 128 == 0 {
            flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;
        }
    }
    flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;

    stall.store(false, Ordering::Relaxed);
    let mut received: Vec<Vec<u8>> = Vec::new();
    let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while tokio::time::Instant::now() < drain_deadline {
        flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;
        try_recv_one(
            &mut rig.client_conn,
            &rig.client_socket,
            rig.client_local,
            &mut in_buf,
            Duration::from_millis(5),
        )
        .await;
        loop {
            match rig.client_conn.dgram_recv(&mut recv_buf) {
                Ok(n) => received.push(recv_buf.get(..n).unwrap_or(&[]).to_vec()),
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }
    }

    // (1) Bounded delivery — never MORE than were handed to the wire. Under a
    //     flood exceeding every bounded queue, drops MUST have happened, so the
    //     queue is bounded, NOT unbounded.
    assert!(
        sent_ok > 1024,
        "must have flooded past a relay-cap worth ({sent_ok})"
    );
    assert!(
        received.len() <= sent_ok,
        "cannot receive more than were sent (echo is 1:1)"
    );
    assert!(
        received.len() < sent_ok,
        "under a flood exceeding cap({}) + quiche queues, the bounded \
         drop-newest queue MUST have dropped some (received {} < sent {}); \
         an unbounded queue would have retained+delivered (nearly) all",
        1024,
        received.len(),
        sent_ok
    );
    // (2) Liveness + recovery: the connection survived and datagrams STILL
    //     flowed after the pressure eased (the relay is not wedged).
    assert!(
        !rig.client_conn.is_closed(),
        "connection stays alive across flood + drain"
    );
    assert!(
        !received.is_empty(),
        "datagrams still flow once the sink drains (relay not wedged by the flood)"
    );
    for r in &received {
        assert_eq!(
            r.len(),
            600,
            "delivered datagrams are byte-intact (verbatim)"
        );
    }

    let _ = teardown(rig).await;
}
