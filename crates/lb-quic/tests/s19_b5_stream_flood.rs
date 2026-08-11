//! Mode B — B5 STREAM-FLOOD eviction-under-load proof (the builder's seed; the verifier owns the
//! authoritative burst). The per-connection relay STREAM table must stay BOUNDED while the
//! connection carries a TOTAL stream count far larger than the negotiated concurrent grant.
//!
//! Load-bearing: WITHOUT `relay_streams`'s `streams.retain`, every finished state lingers, the
//! table grows with the TOTAL count and crosses `MAX_RELAY_STREAMS`, so later streams are refused
//! and the client hangs (caught by the budget) — or without the cap, grows unbounded.

#![cfg(feature = "test-gauges")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
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
const RELAY_BUDGET: Duration = Duration::from_secs(60);

/// Chosen FAR above both the negotiated concurrent grant and the relay-table ceiling: only
/// reclamation keeps the table bounded across this many.
const TOTAL_STREAMS: u64 = 400;

/// Small, so the CONCURRENT count (hence the steady-state table size) is tiny while the TOTAL is
/// large — that gap is the whole point.
const CONCURRENCY: u64 = 4;

/// Tiny: this test is about stream COUNT / table lifetime, not per-stream volume.
const PAYLOAD_LEN: usize = 64;


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
        "lb-quic-s19-b5-flood-{}-{nanos}-{seq}",
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

/// Distinct per-stream payload so a cross-stream mix-up is caught.
fn make_payload(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0x1234_5678);
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        out.push((state >> 33) as u8);
    }
    out
}

/// CLIENT-facing SERVER config. The bidi grant deliberately MIRRORS production
/// `build_server_config`, so the concurrent ceiling under test is the real one.
fn lb_server_config(certs: &TestCerts) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
        .unwrap();
    cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
        .unwrap();
    cfg.set_max_idle_timeout(30_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(16 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(256 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    // Mirror production: a small concurrent grant, so the 400 total streams MUST be opened
    // sequentially (reclamation-bounded).
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(16);
    cfg.set_disable_active_migration(true);
    cfg
}

fn client_config(certs: &TestCerts) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_verify_locations_from_file(certs.ca.to_str().unwrap())
        .unwrap();
    cfg.verify_peer(true);
    cfg.set_max_idle_timeout(30_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(16 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(256 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(16);
    cfg.set_disable_active_migration(true);
    cfg
}

/// Grants the LB-as-client the SAME small bidi ceiling, so the relay must re-open/finish backend
/// streams sequentially too.
fn upstream_config_factory(
    ca: PathBuf,
) -> Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> {
    Arc::new(move || {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
        cfg.set_application_protos(&[b"mode-b-factory-default"])?;
        cfg.load_verify_locations_from_file(ca.to_str().ok_or(quiche::Error::TlsFail)?)
            .map_err(|_| quiche::Error::TlsFail)?;
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(30_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(16 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(256 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(16);
        cfg.set_initial_max_streams_uni(16);
        cfg.set_disable_active_migration(true);
        Ok(cfg)
    })
}

/// ECHOes STREAM bytes on the SAME stream id, FINing each once it has echoed the peer FIN, and
/// reclaiming its own finished-stream state so it too stays bounded.
fn spawn_echo_backend(certs: &TestCerts) -> SocketAddr {
    let std_sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    std_sock.set_nonblocking(true).unwrap();
    let addr = std_sock.local_addr().unwrap();
    let mut config = lb_server_config(certs);

    tokio::spawn(async move {
        let socket = UdpSocket::from_std(std_sock).unwrap();
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];
        let mut rd = vec![0u8; MAX_UDP];
        let mut conn: Option<quiche::Connection> = None;
        let mut echo_pending: HashMap<u64, (Vec<u8>, bool, bool)> = HashMap::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            if let Some(c) = conn.as_mut() {
                let readable: Vec<u64> = c.readable().collect();
                for sid in readable {
                    loop {
                        match c.stream_recv(sid, &mut rd) {
                            Ok((n, fin)) => {
                                let e =
                                    echo_pending
                                        .entry(sid)
                                        .or_insert((Vec::new(), false, false));
                                e.0.extend_from_slice(rd.get(..n).unwrap_or(&[]));
                                if fin {
                                    e.1 = true;
                                }
                                if fin || n == 0 {
                                    break;
                                }
                            }
                            Err(quiche::Error::Done) => break,
                            Err(_) => break,
                        }
                    }
                }
                let sids: Vec<u64> = echo_pending.keys().copied().collect();
                for sid in sids {
                    if let Some(e) = echo_pending.get_mut(&sid) {
                        let mut acc = 0usize;
                        while acc < e.0.len() {
                            let chunk = e.0.get(acc..).unwrap_or(&[]);
                            match c.stream_send(sid, chunk, false) {
                                Ok(0) | Err(quiche::Error::Done) => break,
                                Ok(n) => {
                                    acc += n;
                                    if n < chunk.len() {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        if acc > 0 {
                            e.0.drain(..acc.min(e.0.len()));
                        }
                        if e.1 && e.0.is_empty() && !e.2 && c.stream_send(sid, &[], true).is_ok() {
                            e.2 = true;
                        }
                    }
                }
                // Reclaim fully-echoed streams so the backend's own state is bounded too.
                echo_pending.retain(|_, e| !(e.1 && e.0.is_empty() && e.2));
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
                .unwrap_or(Duration::from_millis(2));
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

/// THE B5 stream-flood proof: `TOTAL_STREAMS` opened through a bounded concurrency window, all
/// round-tripping byte-identically. A reclamation-broken proxy would hang (table hits the cap and
/// refuses later streams) or OOM.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s19_b5_stream_flood_bounded_table_completes_all() {
    let certs = generate_loopback_certs();

    let backend_addr = spawn_echo_backend(&certs);

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

    let (tx, rx) = mpsc::channel::<InboundPacket>(512);
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

    // A stream counts as in-flight until its echo has fully FIN'd.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<u64>();
    let client_cancel = cancel.clone();
    let client_driver = tokio::spawn(async move {
        let mut out = vec![0u8; MAX_UDP];
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut recv_buf = vec![0u8; MAX_UDP];

        let mut next_index: u64 = 0;
        let mut inflight: HashMap<u64, (Vec<u8>, Vec<u8>)> = HashMap::new();
        let mut completed: u64 = 0;
        let mut mismatches: u64 = 0;
        let mut done_tx = Some(done_tx);

        loop {
            if client_cancel.is_cancelled() || client_conn.is_closed() {
                break;
            }

            while (inflight.len() as u64) < CONCURRENCY && next_index < TOTAL_STREAMS {
                let sid = next_index * 4; // client-initiated bidi ids
                let payload = make_payload(next_index.wrapping_add(1), PAYLOAD_LEN);
                match client_conn.stream_send(sid, &payload, true) {
                    Ok(_) => {
                        inflight.insert(sid, (payload, Vec::new()));
                        next_index += 1;
                    }
                    // Concurrent stream-grant exhausted for the moment: stop opening, let some
                    // complete and free credit, retry later.
                    Err(quiche::Error::StreamLimit) | Err(quiche::Error::Done) => break,
                    Err(e) => panic!("client stream_send(open sid): {e:?}"),
                }
            }

            flush(&mut client_conn, &client_socket, &mut out).await;
            try_recv_one(
                &mut client_conn,
                &client_socket,
                client_local,
                &mut in_buf,
                Duration::from_millis(3),
            )
            .await;

            let readable: Vec<u64> = client_conn.readable().collect();
            for sid in readable {
                let mut fin_seen = false;
                loop {
                    match client_conn.stream_recv(sid, &mut recv_buf) {
                        Ok((n, fin)) => {
                            if let Some(e) = inflight.get_mut(&sid) {
                                e.1.extend_from_slice(recv_buf.get(..n).unwrap_or(&[]));
                            }
                            if fin {
                                fin_seen = true;
                                break;
                            }
                            if n == 0 {
                                break;
                            }
                        }
                        Err(quiche::Error::Done) => break,
                        Err(_) => break,
                    }
                }
                if fin_seen {
                    if let Some((want, got)) = inflight.remove(&sid) {
                        if got == want {
                            completed += 1;
                        } else {
                            mismatches += 1;
                        }
                    }
                }
            }

            if next_index >= TOTAL_STREAMS && inflight.is_empty() {
                if let Some(tx) = done_tx.take() {
                    let _ = tx.send(if mismatches == 0 { completed } else { 0 });
                }
                break;
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

    let completed = tokio::time::timeout(RELAY_BUDGET, done_rx)
        .await
        .expect(
            "the proxy must complete ALL TOTAL_STREAMS within the budget — a hang here \
             means the relay table was NOT reclaimed (it hit the cap and refused later \
             streams) or the connection wedged",
        )
        .expect("client driver must report a completion count");

    assert_eq!(
        completed, TOTAL_STREAMS,
        "the proxy must relay ALL {TOTAL_STREAMS} sequential streams byte-identically \
         with a bounded relay table (reclamation evicts completed streams); a smaller \
         count means a stream was dropped, mismatched, or the table grew unbounded / \
         hit the cap and refused later streams"
    );
    eprintln!(
        "s19_b5_stream_flood: {TOTAL_STREAMS} total streams (concurrency {CONCURRENCY}, \
         grant 16, cap 256) all round-tripped byte-identical — relay table stayed bounded \
         by reclamation"
    );

    cancel.cancel();
    forwarder.abort();
    let _ = client_driver.await;
    let _ = tokio::time::timeout(Duration::from_secs(5), actor).await;
}
