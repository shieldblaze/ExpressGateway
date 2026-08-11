//! Mode B — the METRICS NON-VACUITY proof (the stub trap): a metric that NEVER moves is
//! registered for show and not wired through the datapath. Drives a REAL Mode-B relay with a REAL
//! [`QuicModeBMetrics`] and asserts each `quic_modeb_*` handle MOVES: `connections_total`
//! increments and the `connections` gauge returns to 0 (proving the `ActiveConnGuard` RAII
//! dec-on-drop is live); `datagrams_dropped_total` increments under flood at a stalled backend;
//! `streams_active` is SAMPLED live mid-transfer, since it is set per-pass and returns to 0.

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
use lb_observability::{MetricsRegistry, QuicModeBMetrics};
use lb_quic::RawBackend;
use lb_quic::conn_actor::{ActorParams, InboundPacket};
use lb_quic::raw_proxy::run_raw_proxy_actor_for_test;

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
        "lb-quic-s19-b6-metrics-{}-{nanos}-{seq}",
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
    cfg.set_initial_max_streams_bidi(64);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

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
    cfg.set_initial_max_streams_bidi(64);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

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
        cfg.set_initial_max_streams_bidi(64);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        cfg.enable_dgram(true, 1024, 1024);
        Ok(cfg)
    })
}

/// ECHOes STREAM bytes back on the same stream id, FINing once drained.
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
        let mut echo_pending: std::collections::HashMap<u64, (Vec<u8>, bool, bool)> =
            std::collections::HashMap::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(40);

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

/// STALLS (never reads its datagrams) so the LB→backend `dgram_send` back-pressures and the
/// relay's bounded queue drops-newest, while keeping the connection alive.
fn spawn_stalled_dgram_backend(certs: &TestCerts) -> SocketAddr {
    let std_sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    std_sock.set_nonblocking(true).unwrap();
    let addr = std_sock.local_addr().unwrap();
    let mut config = lb_server_config(certs);

    tokio::spawn(async move {
        let socket = UdpSocket::from_std(std_sock).unwrap();
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];
        let mut conn: Option<quiche::Connection> = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(40);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            if let Some(c) = conn.as_mut() {
                // Deliberately NEVER drain dgram_recv: the recv queue fills, back-pressuring
                // the LB→backend leg so the relay's c2u queue saturates and drops-newest.
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

struct Rig {
    client_conn: quiche::Connection,
    client_socket: Arc<UdpSocket>,
    client_local: SocketAddr,
    cancel: CancellationToken,
    forwarder: tokio::task::JoinHandle<()>,
    actor: tokio::task::JoinHandle<std::io::Result<lb_quic::raw_proxy::RawProxyOutcome>>,
    _certs: TestCerts,
}

async fn build_rig(certs: TestCerts, backend_addr: SocketAddr, metrics: QuicModeBMetrics) -> Rig {
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
        quic_modeb_metrics: Some(metrics),
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

async fn teardown(rig: Rig) {
    rig.cancel.cancel();
    rig.forwarder.abort();
    let _ = tokio::time::timeout(Duration::from_secs(6), rig.actor).await;
}

/// CHECK 3a — `connections_total >= 1`, the `connections` gauge reads 1 WHILE the relay is live
/// (sampled via a clone), and it is back to 0 AFTER the actor returns (`ActiveConnGuard` Drop ran).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_total_increments_and_gauge_returns_to_zero() {
    let certs = generate_loopback_certs();
    let backend_addr = spawn_echo_backend(&certs);

    let reg = MetricsRegistry::new();
    let metrics = QuicModeBMetrics::register(&reg).expect("register");
    // Arc-backed prometheus handles: these clones observe the same value the actor bumps.
    let connections = metrics.connections.clone();
    let connections_total = metrics.connections_total.clone();

    assert_eq!(connections.get(), 0, "gauge starts 0");
    assert_eq!(connections_total.get(), 0, "counter starts 0");

    let mut rig = build_rig(certs, backend_addr, metrics).await;

    // A small round-trip first, so both legs are known up — the actor bumps `connections_total`
    // only AFTER the upstream dial succeeds.
    let payload = b"mode-b-metrics-probe".to_vec();
    rig.client_conn
        .stream_send(0, &payload, true)
        .expect("client stream_send");
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut recv_buf = vec![0u8; MAX_UDP];
    let mut echoed: Vec<u8> = Vec::new();
    let mut gauge_seen_one = false;
    let deadline = tokio::time::Instant::now() + RELAY_BUDGET;
    while echoed.len() < payload.len() && tokio::time::Instant::now() < deadline {
        flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;
        try_recv_one(
            &mut rig.client_conn,
            &rig.client_socket,
            rig.client_local,
            &mut in_buf,
            Duration::from_millis(10),
        )
        .await;
        if connections.get() >= 1 {
            gauge_seen_one = true;
        }
        loop {
            match rig.client_conn.stream_recv(0, &mut recv_buf) {
                Ok((n, _fin)) => echoed.extend_from_slice(recv_buf.get(..n).unwrap_or(&[])),
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }
    }

    assert_eq!(
        echoed, payload,
        "stream round-trip must complete (both legs established)"
    );
    assert!(
        connections_total.get() >= 1,
        "quic_modeb_connections_total must increment to >= 1 once the relay \
         established (observed {})",
        connections_total.get()
    );
    assert!(
        gauge_seen_one,
        "quic_modeb_connections gauge must read >= 1 WHILE the relay is live"
    );

    teardown(rig).await;
    let drop_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while connections.get() != 0 && tokio::time::Instant::now() < drop_deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        connections.get(),
        0,
        "quic_modeb_connections gauge MUST return to 0 after the actor exits \
         (the ActiveConnGuard RAII dec-on-drop is live, not a leak)"
    );
    // The cumulative counter does NOT decrement.
    assert!(
        connections_total.get() >= 1,
        "connections_total is cumulative — stays >= 1 after the relay ends"
    );
}

/// CHECK 3b — flood client→upstream datagrams at a STALLED backend so the bounded drop-newest
/// queue overflows: `datagrams_dropped_total` must increment strictly > 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn datagrams_dropped_total_increments_under_flood() {
    let certs = generate_loopback_certs();
    let backend_addr = spawn_stalled_dgram_backend(&certs);

    let reg = MetricsRegistry::new();
    let metrics = QuicModeBMetrics::register(&reg).expect("register");
    let dropped = metrics.datagrams_dropped_total.clone();
    assert_eq!(dropped.get(), 0, "counter starts 0");

    let mut rig = build_rig(certs, backend_addr, metrics).await;

    let payload: Vec<u8> = (0..1_000usize).map(|i| (i % 251) as u8).collect();
    const FLOOD: usize = 50_000;
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut sent_ok = 0usize;
    let deadline = tokio::time::Instant::now() + RELAY_BUDGET;

    for _ in 0..FLOOD {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        match rig.client_conn.dgram_send(&payload) {
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
            if dropped.get() > 0 {
                break;
            }
        }
    }

    let drop_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while dropped.get() == 0 && tokio::time::Instant::now() < drop_deadline {
        flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;
        try_recv_one(
            &mut rig.client_conn,
            &rig.client_socket,
            rig.client_local,
            &mut in_buf,
            Duration::from_millis(5),
        )
        .await;
    }

    let observed = dropped.get();
    teardown(rig).await;

    assert!(
        sent_ok > 1024,
        "must have flooded past a relay-cap worth ({sent_ok})"
    );
    assert!(
        observed > 0,
        "quic_modeb_datagrams_dropped_total MUST increment > 0 under a flood \
         at a stalled backend (the bounded drop-newest queue overflowed and \
         the per-pass delta is surfaced); observed {observed}"
    );
    eprintln!("datagrams_dropped_total observed = {observed} (sent_ok={sent_ok})");
}

/// CHECK 3c — several concurrent bidi streams held mid-flight long enough to sample
/// `streams_active` non-zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streams_active_set_nonzero_during_multistream_transfer() {
    let certs = generate_loopback_certs();
    let backend_addr = spawn_echo_backend(&certs);

    let reg = MetricsRegistry::new();
    let metrics = QuicModeBMetrics::register(&reg).expect("register");
    let streams_active = metrics.streams_active.clone();
    assert_eq!(streams_active.get(), 0, "gauge starts 0");

    let mut rig = build_rig(certs, backend_addr, metrics).await;

    // Payloads large enough not to clear in a single pass, so the relay table holds multiple
    // entries concurrently and the gauge reads > 0.
    const N: u64 = 8;
    let payload: Vec<u8> = (0..32 * 1024usize)
        .map(|i| ((i * 31 + 7) % 256) as u8)
        .collect();
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut recv_buf = vec![0u8; MAX_UDP];

    for k in 0..N {
        let sid = k * 4; // client-initiated bidi stream ids
        let _ = rig.client_conn.stream_send(sid, &payload, true);
    }
    flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;

    let mut max_seen: i64 = 0;
    let mut got: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let deadline = tokio::time::Instant::now() + RELAY_BUDGET;
    let expected_total = (N as usize) * payload.len();
    loop {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        flush(&mut rig.client_conn, &rig.client_socket, &mut out).await;
        try_recv_one(
            &mut rig.client_conn,
            &rig.client_socket,
            rig.client_local,
            &mut in_buf,
            Duration::from_millis(2),
        )
        .await;
        let g = streams_active.get();
        if g > max_seen {
            max_seen = g;
        }
        let readable: Vec<u64> = rig.client_conn.readable().collect();
        for sid in readable {
            loop {
                match rig.client_conn.stream_recv(sid, &mut recv_buf) {
                    Ok((n, _fin)) => *got.entry(sid).or_insert(0) += n,
                    Err(quiche::Error::Done) => break,
                    Err(_) => break,
                }
            }
        }
        let total: usize = got.values().sum();
        if total >= expected_total {
            break;
        }
    }

    // Sample once more BEFORE teardown, in case the last drive turn drained the table.
    let pre_teardown = streams_active.get();
    teardown(rig).await;

    // The load-bearing claim is that the gauge READ non-zero while streams were live. Unlike
    // `connections`, `streams_active` has NO RAII reset — it is a per-pass `set()` — so the
    // post-teardown value is whatever the final pass wrote and is not asserted.
    assert!(
        max_seen >= 1,
        "quic_modeb_streams_active MUST read >= 1 during a multi-stream \
         transfer (the relay-table gauge is wired through the per-pass \
         aggregate); max observed = {max_seen}, pre_teardown = {pre_teardown}"
    );
    eprintln!("streams_active max observed = {max_seen} (pre_teardown = {pre_teardown})");
}
