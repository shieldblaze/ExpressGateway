//! Mode B — the F-MD-4 bar for raw cancellation propagation (R13 a+b+c): real quiche client ⇄
//! Mode B actor ⇄ real quiche backend. Covers a forward reset with a DISTINCT code plus a
//! sibling-stream proof that the cancellation is STREAM-level and not a connection teardown, a
//! reverse (backend) reset, a client STOP_SENDING, and a `#[should_panic]` control proving the
//! no-clean-FIN discriminator actually fires.
//!
//! quiche gotcha: `stream_finished()` returns `true` for an UNKNOWN/collected stream, and a
//! correctly-reset stream IS collected — so it FALSE-POSITIVES a clean end. The ONLY clean-FIN
//! witness used here is `stream_recv` returning `fin == true`.

#![cfg(feature = "test-gauges")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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

/// Client-initiated bidi stream that is reset mid-upload (forward test).
const FWD_STREAM: u64 = 0;
/// A SECOND client bidi stream opened AFTER the forward reset, to prove the relay is alive.
const SIBLING_STREAM: u64 = 4;
/// A DISTINCT, non-trivial value, so a stray default-0 reset or a copy of the smoke fixture
/// cannot pass.
const FWD_RESET_CODE: u64 = 0xCAFE;
/// Reverse direction: code the BACKEND puts on its RESET_STREAM.
const REV_RESET_CODE: u64 = 0xD00D;
/// Client STOP_SENDING code (forward stop test).
const STOP_CODE: u64 = 0x5701;
/// Sentinel "no StreamReset code observed yet".
const NO_RESET: u64 = u64::MAX;
/// Partial body sent BEFORE a reset (multi-packet, no FIN).
const PARTIAL_LEN: usize = 24 * 1024;
const SIBLING_PAYLOAD: &[u8] = b"sibling-stream-survives-the-reset-0123456789";

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
        "lb-quic-s16-b3-verify-{}-{nanos}-{seq}",
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

fn make_payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 37 + 13) % 256) as u8).collect()
}

fn lb_server_config(certs: &TestCerts) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
        .unwrap();
    cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
        .unwrap();
    cfg.set_max_idle_timeout(15_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(4 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(512 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(512 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(16);
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
    cfg.set_max_idle_timeout(15_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(4 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(512 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(512 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

/// Pool dial config factory (LB → backend). Wrong ALPN on purpose so the actor must MIRROR
/// the client's `h3`.
fn upstream_config_factory(
    ca: PathBuf,
) -> Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> {
    Arc::new(move || {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
        cfg.set_application_protos(&[b"mode-b-factory-default"])?;
        cfg.load_verify_locations_from_file(ca.to_str().ok_or(quiche::Error::TlsFail)?)
            .map_err(|_| quiche::Error::TlsFail)?;
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(15_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(4 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(512 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(512 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(16);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        cfg.enable_dgram(true, 1024, 1024);
        Ok(cfg)
    })
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

#[derive(Clone)]
struct BackendObs {
    fwd_bytes: Arc<AtomicUsize>,
    /// `StreamReset` code seen on FWD_STREAM (first wins; NO_RESET = none).
    fwd_reset_code: Arc<AtomicU64>,
    /// Whether a CLEAN FIN was ever observed on FWD_STREAM (must stay false).
    fwd_saw_fin: Arc<AtomicBool>,
    /// Bytes received on SIBLING_STREAM; the client side does the byte-identity check.
    sibling_bytes: Arc<AtomicUsize>,
}

impl BackendObs {
    fn new() -> Self {
        Self {
            fwd_bytes: Arc::new(AtomicUsize::new(0)),
            fwd_reset_code: Arc::new(AtomicU64::new(NO_RESET)),
            fwd_saw_fin: Arc::new(AtomicBool::new(false)),
            sibling_bytes: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// The FORWARD-reset control backend: records bytes / clean-FIN / reset-code on `FWD_STREAM`
/// and echoes `SIBLING_STREAM`. No `stream_finished()` witness — see the collected-stream gotcha.
fn spawn_forward_backend(certs: &TestCerts, obs: BackendObs) -> SocketAddr {
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
        let mut sib_pending: Vec<u8> = Vec::new();
        let mut sib_peer_fin = false;
        let mut sib_fin_sent = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

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
                                if sid == FWD_STREAM {
                                    obs.fwd_bytes.fetch_add(n, Ordering::Relaxed);
                                    if fin {
                                        obs.fwd_saw_fin.store(true, Ordering::Relaxed);
                                    }
                                } else if sid == SIBLING_STREAM {
                                    obs.sibling_bytes.fetch_add(n, Ordering::Relaxed);
                                    sib_pending.extend_from_slice(rd.get(..n).unwrap_or(&[]));
                                    if fin {
                                        sib_peer_fin = true;
                                    }
                                }
                                if fin || n == 0 {
                                    break;
                                }
                            }
                            // THE forward witness: relayed RESET_STREAM with code.
                            Err(quiche::Error::StreamReset(code)) => {
                                if sid == FWD_STREAM {
                                    let _ = obs.fwd_reset_code.compare_exchange(
                                        NO_RESET,
                                        code,
                                        Ordering::Relaxed,
                                        Ordering::Relaxed,
                                    );
                                }
                                break;
                            }
                            Err(quiche::Error::Done) => break,
                            Err(_) => break,
                        }
                    }
                }
                if !sib_pending.is_empty() {
                    let mut acc = 0usize;
                    while acc < sib_pending.len() {
                        let chunk = sib_pending.get(acc..).unwrap_or(&[]);
                        match c.stream_send(SIBLING_STREAM, chunk, false) {
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
                        sib_pending.drain(..acc.min(sib_pending.len()));
                    }
                }
                if sib_peer_fin
                    && sib_pending.is_empty()
                    && !sib_fin_sent
                    && c.stream_send(SIBLING_STREAM, &[], true).is_ok()
                {
                    sib_fin_sent = true;
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
                .min(Duration::from_millis(5));
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

/// Bring a client⇄LB⇄backend Mode B world up to both legs established; all three tests share it.
struct World {
    cancel: CancellationToken,
    actor: tokio::task::JoinHandle<std::io::Result<lb_quic::RawProxyOutcome>>,
    forwarder: tokio::task::JoinHandle<()>,
    client_socket: Arc<UdpSocket>,
    client_local: SocketAddr,
    client_conn: quiche::Connection,
}

async fn bring_up(certs: &TestCerts, backend_addr: SocketAddr) -> World {
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

    let mut server_cfg = lb_server_config(certs);
    let mut client_cfg = client_config(certs);

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

    let (tx, rx) = mpsc::channel::<InboundPacket>(256);
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
        quic_modeb_metrics: None,
        ws_enabled: false,
        ws_relay_launcher: None,
        max_requests_per_h3_connection: 0,
        h3_recycle_metrics: None,
    };
    let actor = tokio::spawn(run_raw_proxy_actor_for_test(params));

    World {
        cancel,
        actor,
        forwarder,
        client_socket,
        client_local,
        client_conn,
    }
}

async fn teardown(w: World) {
    w.cancel.cancel();
    w.forwarder.abort();
    let _ = tokio::time::timeout(Duration::from_secs(5), w.actor).await;
}

/// **THE HEADLINE**, load-bearing three independent ways: **reset-with-code** fails if the relay
/// dropped the half (the backend would keep getting `Done`) or forwarded the wrong code;
/// **no-clean-FIN** fails if the relay ever synthesised `fin = true` (the F-MD-4 smuggling bug);
/// **per-stream, not conn-teardown** fails if the cancellation tore the connection down.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forward_client_reset_propagates_with_code_conn_stays_up() {
    let certs = generate_loopback_certs();
    let obs = BackendObs::new();
    let backend_addr = spawn_forward_backend(&certs, obs.clone());
    let mut w = bring_up(&certs, backend_addr).await;

    let mut client = std::mem::replace(
        &mut w.client_conn,
        quiche::accept(
            &quiche::ConnectionId::from_ref(&random_scid()),
            None,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:2".parse().unwrap(),
            &mut lb_server_config(&certs),
        )
        .unwrap(),
    );
    let client_socket = Arc::clone(&w.client_socket);
    let client_local = w.client_local;
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];

    let payload = make_payload(PARTIAL_LEN);
    client.stream_send(FWD_STREAM, &payload, false).unwrap();
    flush(&mut client, &client_socket, &mut out).await;

    // 2) Wait until the backend has received SOME bytes — the reset must land MID-transfer,
    //    after the mirror stream exists.
    let wait_recv = tokio::time::Instant::now() + Duration::from_secs(10);
    while obs.fwd_bytes.load(Ordering::Relaxed) == 0 {
        if tokio::time::Instant::now() >= wait_recv {
            panic!("backend never received the partial upload (relay did not forward)");
        }
        flush(&mut client, &client_socket, &mut out).await;
        try_recv_one(
            &mut client,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(20),
        )
        .await;
    }
    let before = obs.fwd_bytes.load(Ordering::Relaxed);
    eprintln!("forward: backend received {before} bytes before the reset");

    // 3) RESET_STREAM with the distinct code, and keep pumping so the RESET frame actually
    //    leaves quiche.
    client
        .stream_shutdown(FWD_STREAM, quiche::Shutdown::Write, FWD_RESET_CODE)
        .unwrap();

    // Right after the RESET_STREAM quiche can transiently return `Done` here (stream grant /
    // connection flow control not yet available) and can also SHORT-WRITE, so pump and retry
    // until the whole sibling payload is queued.
    let mut sibling_sent = 0usize;
    let queue_by = tokio::time::Instant::now() + Duration::from_secs(10);
    while sibling_sent < SIBLING_PAYLOAD.len() {
        if tokio::time::Instant::now() >= queue_by {
            panic!(
                "could not queue the sibling payload within 10s (queued {sibling_sent} of {} \
                 bytes) — client-local stream_send kept returning Done after the reset",
                SIBLING_PAYLOAD.len()
            );
        }
        match client.stream_send(SIBLING_STREAM, &SIBLING_PAYLOAD[sibling_sent..], true) {
            Ok(n) => sibling_sent += n,
            Err(quiche::Error::Done) => {}
            Err(e) => panic!("sibling stream_send failed unexpectedly: {e:?}"),
        }
        flush(&mut client, &client_socket, &mut out).await;
        try_recv_one(
            &mut client,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(5),
        )
        .await;
    }

    let mut sibling_recv: Vec<u8> = Vec::new();
    let mut sibling_done = false;
    let mut recv_buf = vec![0u8; MAX_UDP];
    let observe = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        flush(&mut client, &client_socket, &mut out).await;
        try_recv_one(
            &mut client,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(5),
        )
        .await;

        if !sibling_done {
            let readable: Vec<u64> = client.readable().collect();
            for sid in readable {
                if sid != SIBLING_STREAM {
                    continue;
                }
                loop {
                    match client.stream_recv(SIBLING_STREAM, &mut recv_buf) {
                        Ok((n, fin)) => {
                            sibling_recv.extend_from_slice(recv_buf.get(..n).unwrap_or(&[]));
                            if fin {
                                sibling_done = true;
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

        let reset_seen = obs.fwd_reset_code.load(Ordering::Relaxed) != NO_RESET;
        if reset_seen && sibling_done {
            break;
        }
        if tokio::time::Instant::now() >= observe {
            break;
        }
    }

    let code = obs.fwd_reset_code.load(Ordering::Relaxed);
    assert_ne!(
        code, NO_RESET,
        "B3 PROPAGATION MISSING: backend never observed a stream-level \
         RESET_STREAM on the forward stream — the relay dropped the half \
         (B2 behaviour) instead of dst.stream_shutdown(sid, Write, code)."
    );
    assert_eq!(
        code, FWD_RESET_CODE,
        "B3 CODE NOT PRESERVED: backend saw RESET code {code:#x}, expected \
         the client's {FWD_RESET_CODE:#x}."
    );
    assert!(
        !obs.fwd_saw_fin.load(Ordering::Relaxed),
        "F-MD-4 SMUGGLING: backend saw a CLEAN FIN on the forward stream after \
         a mid-transfer reset — truncated transfer presented as complete."
    );
    // Connection-stays-up / per-stream proof: the sibling relayed byte-identical AFTER the reset.
    assert!(
        sibling_done,
        "the sibling stream never completed — the reset appears to have torn \
         down the whole CONNECTION (connection-teardown masking), not just the \
         one stream. A per-stream RESET_STREAM must leave siblings alive."
    );
    assert_eq!(
        sibling_recv, SIBLING_PAYLOAD,
        "the sibling stream's echoed bytes are not byte-identical — the relay \
         is unhealthy after the reset."
    );
    eprintln!(
        "forward: VERIFIED — backend StreamReset code={code:#x}, no clean FIN, \
         sibling stream round-tripped {} bytes (connection stayed up)",
        sibling_recv.len()
    );

    w.client_conn = client;
    teardown(w).await;
}

/// The BACKEND resets a backend-initiated bidi stream mid-response; the CLIENT must observe
/// `StreamReset(REV_RESET_CODE)` and never a clean FIN. A server-initiated bidi stream is id 1 on
/// the backend's connection and surfaces to the client as the same id, the LB being its server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reverse_backend_reset_propagates_with_code_to_client() {
    let certs = generate_loopback_certs();

    // Opens stream 1, pushes a partial response without FIN, then RESET_STREAMs it once the
    // relay has carried some bytes.
    let backend_sent = Arc::new(AtomicUsize::new(0));
    let client_recv_seen = Arc::new(AtomicUsize::new(0)); // set by test via shared? no — client side
    let do_reset = Arc::new(AtomicBool::new(false));
    let did_reset = Arc::new(AtomicBool::new(false));
    let backend_sent_b = Arc::clone(&backend_sent);
    let do_reset_b = Arc::clone(&do_reset);
    let did_reset_b = Arc::clone(&did_reset);
    let _ = &client_recv_seen;

    const REV_STREAM: u64 = 1; // server-initiated bidi
    const REV_PARTIAL: usize = 24 * 1024;

    let std_sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    std_sock.set_nonblocking(true).unwrap();
    let backend_addr = std_sock.local_addr().unwrap();
    let mut backend_cfg = lb_server_config(&certs);

    tokio::spawn(async move {
        let socket = UdpSocket::from_std(std_sock).unwrap();
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];
        let mut rd = vec![0u8; MAX_UDP];
        let mut conn: Option<quiche::Connection> = None;
        let mut opened = false;
        let mut sent = 0usize;
        let payload = make_payload(REV_PARTIAL);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            if let Some(c) = conn.as_mut() {
                if c.is_established() {
                    let readable: Vec<u64> = c.readable().collect();
                    for sid in readable {
                        while let Ok((_, fin)) = c.stream_recv(sid, &mut rd) {
                            if fin {
                                break;
                            }
                        }
                    }
                    if !opened {
                        match c.stream_send(REV_STREAM, &payload, false) {
                            Ok(n) => {
                                sent += n;
                                opened = true;
                                backend_sent_b.store(sent, Ordering::Relaxed);
                            }
                            Err(quiche::Error::Done) => {}
                            Err(_) => {}
                        }
                    } else if sent < payload.len() && !did_reset_b.load(Ordering::Relaxed) {
                        let tail = payload.get(sent..).unwrap_or(&[]);
                        if let Ok(n) = c.stream_send(REV_STREAM, tail, false) {
                            sent += n;
                            backend_sent_b.store(sent, Ordering::Relaxed);
                        }
                    }
                    if do_reset_b.load(Ordering::Relaxed) && !did_reset_b.load(Ordering::Relaxed) {
                        let _ =
                            c.stream_shutdown(REV_STREAM, quiche::Shutdown::Write, REV_RESET_CODE);
                        did_reset_b.store(true, Ordering::Relaxed);
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
                .min(Duration::from_millis(5));
            match tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((n, from))) => {
                    if conn.is_none() {
                        let scid = random_scid();
                        let scid_ref = quiche::ConnectionId::from_ref(&scid);
                        match quiche::accept(&scid_ref, None, backend_addr, from, &mut backend_cfg)
                        {
                            Ok(c) => conn = Some(c),
                            Err(_) => continue,
                        }
                    }
                    if let Some(c) = conn.as_mut() {
                        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                        let info = quiche::RecvInfo {
                            from,
                            to: backend_addr,
                        };
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

    let mut w = bring_up(&certs, backend_addr).await;
    let mut client = std::mem::replace(
        &mut w.client_conn,
        quiche::accept(
            &quiche::ConnectionId::from_ref(&random_scid()),
            None,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:2".parse().unwrap(),
            &mut lb_server_config(&certs),
        )
        .unwrap(),
    );
    let client_socket = Arc::clone(&w.client_socket);
    let client_local = w.client_local;
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut recv_buf = vec![0u8; MAX_UDP];

    // A FIN'd request on stream 0 starts the actor relaying — the backend opens its response
    // stream only after it sees client traffic.
    client.stream_send(0, b"GET /", true).unwrap();
    flush(&mut client, &client_socket, &mut out).await;

    // Fire the reset only once the client has received SOME response bytes on REV_STREAM,
    // so the mirror exists and it lands mid-response.
    let mut client_rev_bytes = 0usize;
    let mut client_saw_fin = false;
    let mut client_reset_code: u64 = NO_RESET;
    let wait_recv = tokio::time::Instant::now() + Duration::from_secs(12);
    while client_rev_bytes == 0 {
        if tokio::time::Instant::now() >= wait_recv {
            panic!(
                "client never received any backend response on the reverse \
                 stream — cannot stage a mid-response reset (backend_sent={})",
                backend_sent.load(Ordering::Relaxed)
            );
        }
        flush(&mut client, &client_socket, &mut out).await;
        try_recv_one(
            &mut client,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(10),
        )
        .await;
        let readable: Vec<u64> = client.readable().collect();
        for sid in readable {
            if sid != REV_STREAM {
                continue;
            }
            match client.stream_recv(REV_STREAM, &mut recv_buf) {
                Ok((n, fin)) => {
                    client_rev_bytes += n;
                    if fin {
                        client_saw_fin = true;
                    }
                }
                Err(quiche::Error::StreamReset(c)) => client_reset_code = c,
                Err(_) => {}
            }
        }
    }
    eprintln!("reverse: client received {client_rev_bytes} response bytes before reset");

    do_reset.store(true, Ordering::Relaxed);

    let observe = tokio::time::Instant::now() + Duration::from_secs(8);
    while client_reset_code == NO_RESET {
        if tokio::time::Instant::now() >= observe {
            break;
        }
        flush(&mut client, &client_socket, &mut out).await;
        try_recv_one(
            &mut client,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(10),
        )
        .await;
        let readable: Vec<u64> = client.readable().collect();
        for sid in readable {
            if sid != REV_STREAM {
                continue;
            }
            loop {
                match client.stream_recv(REV_STREAM, &mut recv_buf) {
                    Ok((_, fin)) => {
                        if fin {
                            client_saw_fin = true;
                            break;
                        }
                    }
                    Err(quiche::Error::StreamReset(c)) => {
                        client_reset_code = c;
                        break;
                    }
                    Err(quiche::Error::Done) => break,
                    Err(_) => break,
                }
            }
        }
    }

    assert!(
        did_reset.load(Ordering::Relaxed),
        "fixture: the backend must have issued its RESET_STREAM"
    );
    assert_ne!(
        client_reset_code, NO_RESET,
        "REVERSE PROPAGATION MISSING: the client never observed a stream-level \
         RESET_STREAM from the backend — the relay's u2c reset arm did not \
         propagate (dst=client stream_shutdown Write code)."
    );
    assert_eq!(
        client_reset_code, REV_RESET_CODE,
        "REVERSE CODE NOT PRESERVED: client saw RESET {client_reset_code:#x}, \
         expected the backend's {REV_RESET_CODE:#x}."
    );
    assert!(
        !client_saw_fin,
        "F-MD-4 SMUGGLING (reverse): the client saw a clean FIN on the reset \
         response stream — truncated response presented as complete."
    );
    eprintln!("reverse: VERIFIED — client StreamReset code={client_reset_code:#x}, no clean FIN");

    w.client_conn = client;
    teardown(w).await;
}

/// A client STOP_SENDING must propagate toward the backend, so the backend's `stream_send`
/// returns `Err(StreamStopped(code))` with that code and the client never observes a clean FIN.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_stop_sending_propagates_to_backend() {
    let certs = generate_loopback_certs();

    // Records the FIRST StreamStopped code its own `stream_send` gets on stream 1.
    let stop_code_seen = Arc::new(AtomicU64::new(NO_RESET));
    let backend_started = Arc::new(AtomicBool::new(false));
    let stop_code_b = Arc::clone(&stop_code_seen);
    let started_b = Arc::clone(&backend_started);

    const REV_STREAM: u64 = 1;

    let std_sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    std_sock.set_nonblocking(true).unwrap();
    let backend_addr = std_sock.local_addr().unwrap();
    let mut backend_cfg = lb_server_config(&certs);

    tokio::spawn(async move {
        let socket = UdpSocket::from_std(std_sock).unwrap();
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];
        let mut rd = vec![0u8; MAX_UDP];
        let mut conn: Option<quiche::Connection> = None;
        let chunk = make_payload(8 * 1024);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            if let Some(c) = conn.as_mut() {
                if c.is_established() {
                    let readable: Vec<u64> = c.readable().collect();
                    for sid in readable {
                        while let Ok((_, fin)) = c.stream_recv(sid, &mut rd) {
                            if fin {
                                break;
                            }
                        }
                    }
                    // Keep pushing; once the STOP_SENDING is propagated this becomes
                    // StreamStopped(code).
                    match c.stream_send(REV_STREAM, &chunk, false) {
                        Ok(n) => {
                            if n > 0 {
                                started_b.store(true, Ordering::Relaxed);
                            }
                        }
                        Err(quiche::Error::Done) => {}
                        Err(quiche::Error::StreamStopped(code)) => {
                            let _ = stop_code_b.compare_exchange(
                                NO_RESET,
                                code,
                                Ordering::Relaxed,
                                Ordering::Relaxed,
                            );
                        }
                        Err(_) => {}
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
                .min(Duration::from_millis(5));
            match tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((n, from))) => {
                    if conn.is_none() {
                        let scid = random_scid();
                        let scid_ref = quiche::ConnectionId::from_ref(&scid);
                        match quiche::accept(&scid_ref, None, backend_addr, from, &mut backend_cfg)
                        {
                            Ok(c) => conn = Some(c),
                            Err(_) => continue,
                        }
                    }
                    if let Some(c) = conn.as_mut() {
                        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                        let info = quiche::RecvInfo {
                            from,
                            to: backend_addr,
                        };
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

    let mut w = bring_up(&certs, backend_addr).await;
    let mut client = std::mem::replace(
        &mut w.client_conn,
        quiche::accept(
            &quiche::ConnectionId::from_ref(&random_scid()),
            None,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:2".parse().unwrap(),
            &mut lb_server_config(&certs),
        )
        .unwrap(),
    );
    let client_socket = Arc::clone(&w.client_socket);
    let client_local = w.client_local;
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut recv_buf = vec![0u8; MAX_UDP];

    client.stream_send(0, b"GET /", true).unwrap();
    flush(&mut client, &client_socket, &mut out).await;

    // STOP_SENDING only after some response bytes arrived, so the read side exists.
    let mut got_some = false;
    let mut client_saw_fin = false;
    let wait_recv = tokio::time::Instant::now() + Duration::from_secs(12);
    while !got_some {
        if tokio::time::Instant::now() >= wait_recv {
            panic!(
                "client never received response bytes on the stop stream \
                 (backend_started={})",
                backend_started.load(Ordering::Relaxed)
            );
        }
        flush(&mut client, &client_socket, &mut out).await;
        try_recv_one(
            &mut client,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(10),
        )
        .await;
        let readable: Vec<u64> = client.readable().collect();
        for sid in readable {
            if sid != REV_STREAM {
                continue;
            }
            if let Ok((n, fin)) = client.stream_recv(REV_STREAM, &mut recv_buf) {
                if n > 0 {
                    got_some = true;
                }
                if fin {
                    client_saw_fin = true;
                }
            }
        }
    }

    client
        .stream_shutdown(REV_STREAM, quiche::Shutdown::Read, STOP_CODE)
        .unwrap();

    let observe = tokio::time::Instant::now() + Duration::from_secs(10);
    while stop_code_seen.load(Ordering::Relaxed) == NO_RESET {
        if tokio::time::Instant::now() >= observe {
            break;
        }
        flush(&mut client, &client_socket, &mut out).await;
        try_recv_one(
            &mut client,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(10),
        )
        .await;
        let readable: Vec<u64> = client.readable().collect();
        for sid in readable {
            if sid != REV_STREAM {
                continue;
            }
            loop {
                match client.stream_recv(REV_STREAM, &mut recv_buf) {
                    Ok((_, true)) => {
                        client_saw_fin = true;
                        break;
                    }
                    Ok((_, false)) => {}
                    Err(_) => break,
                }
            }
        }
    }

    let seen = stop_code_seen.load(Ordering::Relaxed);
    assert_ne!(
        seen, NO_RESET,
        "STOP_SENDING PROPAGATION MISSING: the backend's stream_send never \
         surfaced StreamStopped — the client's STOP_SENDING was not propagated \
         to the backend (relay should src.stream_shutdown(sid, Read, code))."
    );
    assert_eq!(
        seen, STOP_CODE,
        "STOP_SENDING CODE NOT PRESERVED: backend saw StreamStopped {seen:#x}, \
         expected the client's {STOP_CODE:#x}."
    );
    assert!(
        !client_saw_fin,
        "SMUGGLING: the client saw a clean FIN after STOP_SENDING."
    );
    eprintln!("stop_sending: VERIFIED — backend StreamStopped code={seen:#x}, no clean FIN");

    w.client_conn = client;
    teardown(w).await;
}

/// LOAD-BEARING META-TEST: on a genuine clean FIN the same `stream_recv` fin-scan the
/// forward/reverse tests rely on MUST trip — otherwise the witness is vacuously satisfiable
/// (e.g. if a refactor made `stream_recv` never surface fin).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discriminator_clean_fin_is_observable_on_happy_path() {
    let certs = generate_loopback_certs();
    let obs = BackendObs::new();
    let backend_addr = spawn_forward_backend(&certs, obs.clone());
    let mut w = bring_up(&certs, backend_addr).await;
    let mut client = std::mem::replace(
        &mut w.client_conn,
        quiche::accept(
            &quiche::ConnectionId::from_ref(&random_scid()),
            None,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:2".parse().unwrap(),
            &mut lb_server_config(&certs),
        )
        .unwrap(),
    );
    let client_socket = Arc::clone(&w.client_socket);
    let client_local = w.client_local;
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];

    // A COMPLETE upload WITH fin — the relay delivers a legitimate clean FIN to the backend.
    client
        .stream_send(FWD_STREAM, b"complete-with-fin", true)
        .unwrap();

    let observe = tokio::time::Instant::now() + Duration::from_secs(10);
    while !obs.fwd_saw_fin.load(Ordering::Relaxed) {
        if tokio::time::Instant::now() >= observe {
            break;
        }
        flush(&mut client, &client_socket, &mut out).await;
        try_recv_one(
            &mut client,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(10),
        )
        .await;
    }

    assert!(
        obs.fwd_saw_fin.load(Ordering::Relaxed),
        "the clean-FIN discriminator did NOT observe a FIN on a genuinely \
         completed stream — the no-clean-FIN witness used by the forward/reverse \
         tests would be vacuously satisfiable. (stream_recv fin-scan is broken.)"
    );
    assert_eq!(
        obs.fwd_reset_code.load(Ordering::Relaxed),
        NO_RESET,
        "no reset should be observed on a cleanly completed stream"
    );
    eprintln!("discriminator: VERIFIED — a genuine clean FIN IS observed (witness is live)");

    w.client_conn = client;
    teardown(w).await;
}
