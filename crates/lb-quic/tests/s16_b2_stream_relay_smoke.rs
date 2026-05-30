//! SESSION 16 / Mode B — B2 MINIMAL smoke test (author's self-check).
//!
//! Scope is deliberately narrow (author ≠ verifier): this proves that
//! the B2 bidirectional raw-STREAM relay carries ONE bidi stream's small
//! binary payload byte-identically through the full wire path —
//!
//!   real quiche CLIENT  ⇄  Mode B actor (`run_raw_proxy_actor_for_test`)
//!                          ⇄  real quiche ECHO backend
//!
//! — i.e. client→LB→backend→LB→client. It does NOT author the full
//! multi-stream proof, the bounded-window/backpressure proof, or the
//! cancellation (reset/stop) proof — those are the VERIFIER's (plan §5 /
//! increments B2, B3, B5).
//!
//! The mechanism under test: the client opens bidi stream 0, sends a
//! 4 KiB pseudo-random payload + FIN. The actor's `relay_streams` copies
//! it client→upstream (identity stream-ID 0). The backend echoes every
//! received byte back on the SAME stream 0 + FIN. The actor relays it
//! upstream→client. The client reads stream 0 to FIN and the bytes MUST
//! equal what it sent.
//!
//! Driven with `--features test-gauges` so the `run_raw_proxy_actor_for_test`
//! hook (gated `#[cfg(any(test, feature = "test-gauges"))]`) is reachable.

#![cfg(feature = "test-gauges")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
const RELAY_BUDGET: Duration = Duration::from_secs(8);
/// The client bidi stream relayed end-to-end (stream 0 = first
/// client-initiated bidi stream; identity-mapped both legs).
const STREAM_ID: u64 = 0;

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
        "lb-quic-s16-b2-relay-{}-{nanos}-{seq}",
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

/// A deterministic pseudo-random binary payload (not all-ASCII; exercises
/// the relay on raw bytes, not text).
fn make_payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + 7) % 256) as u8).collect()
}

/// CLIENT-facing SERVER config (the LB-as-server leg). Serves the
/// loopback cert; advertises `h3`.
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

/// The pool's per-dial CLIENT config factory (LB → backend leg). Installs
/// a deliberately-wrong ALPN so the actor must MIRROR the client's `h3`.
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

/// A throwaway BACKEND quiche server that accepts ONE connection and
/// ECHOes any received STREAM bytes back on the SAME stream id. When it
/// sees FIN on a stream it has fully echoed, it sends FIN back. This is
/// the far end of the relay: client→LB→**backend (echo)**→LB→client.
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
        // Per-stream: bytes still queued to echo back, and whether the
        // peer FIN was seen (so we FIN once the queue drains).
        let mut echo_pending: std::collections::HashMap<u64, (Vec<u8>, bool, bool)> =
            std::collections::HashMap::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            // 1) Read any readable streams into the per-stream echo queue.
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
                // 2) Drain each echo queue back onto the same stream.
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
                        // FIN back once the peer FIN'd and the queue is empty.
                        if e.1 && e.0.is_empty() && !e.2 && c.stream_send(sid, &[], true).is_ok() {
                            e.2 = true;
                        }
                    }
                }
                // 3) Flush outbound.
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
                .unwrap_or(Duration::from_millis(5));
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

/// THE B2 self-check: one bidi stream's binary payload survives
/// client→LB→backend(echo)→LB→client byte-identically.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s16_b2_one_bidi_stream_round_trips_byte_identical() {
    let certs = generate_loopback_certs();

    // 1) Real echo backend.
    let backend_addr = spawn_echo_backend(&certs);

    // 2) Shared LB listener socket (the "server" leg).
    let lb_socket = Arc::new(
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap(),
    );
    let lb_local = lb_socket.local_addr().unwrap();

    // 3) Real downstream CLIENT.
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

    // 4) Inline-drive the client⇄LB legs to established (round8 pattern).
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

    // 5) The client opens bidi stream 0 and sends the payload + FIN
    //    BEFORE handing off — so the bytes are buffered in quiche and the
    //    relay forwards them once the upstream leg is up. (A short send is
    //    fine: the client-driver below keeps flushing.)
    let payload = make_payload(4096);
    let sent = client_conn
        .stream_send(STREAM_ID, &payload, true)
        .expect("client stream_send");
    assert_eq!(
        sent,
        payload.len(),
        "fixture: whole payload fits the window"
    );
    flush(&mut client_conn, &client_socket, &mut out).await;

    // 6) Forwarder: drain the shared LB socket into the actor's inbound
    //    mpsc (the router's job in production).
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

    // 7) Client driver: keep the client live (flush + recv) and collect
    //    the echoed bytes off stream 0 until FIN. Returns the received
    //    payload via a oneshot.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
    let client_cancel = cancel.clone();
    let client_driver = tokio::spawn(async move {
        let mut out = vec![0u8; MAX_UDP];
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut recv_buf = vec![0u8; MAX_UDP];
        let mut received: Vec<u8> = Vec::new();
        let mut got_fin = false;
        let mut done_tx = Some(done_tx);
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
                Duration::from_millis(10),
            )
            .await;
            // Pull echoed bytes off stream 0.
            if !got_fin {
                loop {
                    match client_conn.stream_recv(STREAM_ID, &mut recv_buf) {
                        Ok((n, fin)) => {
                            received.extend_from_slice(recv_buf.get(..n).unwrap_or(&[]));
                            if fin {
                                got_fin = true;
                                if let Some(tx) = done_tx.take() {
                                    let _ = tx.send(std::mem::take(&mut received));
                                }
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
            }
        }
    });

    // 8) The Mode B backend.
    let pool = QuicUpstreamPool::new(
        QuicPoolConfig::default(),
        upstream_config_factory(certs.ca.clone()),
    );
    let raw_backend = RawBackend {
        pool,
        addr: backend_addr,
        sni: TEST_SNI.to_string(),
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
    };

    // 9) Run the actor; wait for the echoed payload, then cancel.
    let actor = tokio::spawn(run_raw_proxy_actor_for_test(params));

    let received = tokio::time::timeout(RELAY_BUDGET, done_rx)
        .await
        .expect("client must receive the echoed payload before the budget")
        .expect("client driver must deliver the received bytes");

    // ── THE ASSERTION: byte-identical round-trip through the relay. ──
    assert_eq!(
        received.len(),
        payload.len(),
        "echoed payload length must match what the client sent"
    );
    assert_eq!(
        received, payload,
        "echoed payload must be byte-identical after \
         client→LB→backend→LB→client raw-stream relay"
    );

    // Tidy up.
    cancel.cancel();
    forwarder.abort();
    let _ = client_driver.await;
    let _ = tokio::time::timeout(Duration::from_secs(5), actor).await;
}
