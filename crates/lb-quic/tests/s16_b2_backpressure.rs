//! Mode B — the R8 BOUNDED-WINDOW / BACKPRESSURE proof: when the BACKEND stops
//! reading a stream while the client keeps sending, the relay must NOT buffer
//! the client's send without bound — it must propagate backpressure so the
//! CLIENT itself stalls, and on resume the full payload must arrive intact.
//!
//! Mechanism: the backend stops `stream_recv` ⇒ the upstream send window fills
//! ⇒ the relay's `c2u` pending reaches `STREAM_RELAY_WINDOW` ⇒ the relay stops
//! calling `client.stream_recv` ⇒ quiche stops extending the client's
//! `MAX_STREAM_DATA` ⇒ the client's `stream_send` stalls once its credit is
//! spent. Retained bytes do NOT grow with the (much larger) total payload.
//!
//! LOAD-BEARING assertions (timing-robust): (1) while the backend refuses to
//! read it echoes ZERO bytes — the relay honours the destination's flow
//! control; (2) the round-trip does NOT complete during the stall, so the
//! transfer is genuinely GATED, not buffered through; (3) on resume the ENTIRE
//! payload is echoed back BYTE-IDENTICAL, so nothing was dropped or reordered.
//!
//! Honest scope: a black-box test cannot read the LB's in-process
//! `half.pending.len()`, and the client's `stream_send` cursor is NOT a tight
//! in-flight proxy (quiche buffers locally beyond the peer's window, and under
//! CPU starvation that inflates the cursor — the CF-SATURATION-1 class). So the
//! cursor ceiling is a LOOSE secondary witness that only falsifies a gross
//! buffer-everything relay; the exact 256 KiB bound is the `pump_dir` read gate,
//! confirmed by code-read.
//!
//! Driven with `--features test-gauges`.

#![cfg(feature = "test-gauges")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
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
/// The stalled stream relayed end-to-end.
const STREAM_ID: u64 = 0;

/// Total payload the client WANTS to send on the stalled stream — much larger
/// than (client credit + relay window), so an unbounded relay would march the
/// client's cursor to the full payload while a bounded one stalls it early.
const PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// Stream-0 send credit the LB grants the CLIENT. Small and fixed, so the
/// client's stall ceiling is a constant independent of `PAYLOAD_LEN`.
const LB_GRANT_TO_CLIENT: usize = 128 * 1024;

/// Per-stream + connection receive credit the BACKEND advertises. Keeping it
/// small bounds what a STALLED backend can hold unread, so data accumulates only
/// in `client grant + STREAM_RELAY_WINDOW + BACKEND_RECV_WINDOW` — a small
/// constant. With a multi-MiB backend window the client's plateau would drift up
/// toward it and become scheduling-dependent under gate saturation.
const BACKEND_RECV_WINDOW: usize = 128 * 1024;

/// GENEROUS secondary sanity ceiling for the client's `stream_send` cursor
/// during the stall. The cursor counts bytes accepted into quiche's LOCAL send
/// buffer, which exceeds the peer's advertised window and inflates under CPU
/// starvation — so this is a LOOSE witness whose only job is to falsify a gross
/// "the LB drained the entire payload into its own memory" relay. The SOUND
/// proof is the load-bearing pair (backend echoed 0 + transfer not complete
/// while stalled) plus full byte-identical completeness on resume.
const STALL_CEILING: usize = 3 * 1024 * 1024;

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
        "lb-quic-s16-b2-bp-{}-{nanos}-{seq}",
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

fn make_payload(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0x55AA_55AA);
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        out.push((state >> 33) as u8);
    }
    out
}

/// CLIENT-facing SERVER config. CRITICAL: grants only `LB_GRANT_TO_CLIENT` of
/// per-stream credit so the stall ceiling is a small constant, while
/// `initial_max_data` stays generous — the PER-STREAM backpressure must be the
/// binding constraint, not a coincidental connection-level cap.
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
    cfg.set_initial_max_stream_data_bidi_local(LB_GRANT_TO_CLIENT as u64);
    cfg.set_initial_max_stream_data_bidi_remote(LB_GRANT_TO_CLIENT as u64);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

/// The real downstream CLIENT config.
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
    cfg.set_initial_max_stream_data_bidi_local(2 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(2 * 1024 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

/// The BACKEND's SERVER config: advertises only a SMALL receive window, so a
/// stalled backend holds at most `BACKEND_RECV_WINDOW` unread and the client's
/// stall plateau stays a small constant. Distinct from `lb_server_config`, where
/// the conn window is deliberately generous.
fn backend_config(certs: &TestCerts) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
        .unwrap();
    cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
        .unwrap();
    cfg.set_max_idle_timeout(30_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    // Small connection-level window (2× the per-stream window) so the
    // backend cannot absorb more than a small constant of unread data.
    cfg.set_initial_max_data((2 * BACKEND_RECV_WINDOW) as u64);
    cfg.set_initial_max_stream_data_bidi_local(BACKEND_RECV_WINDOW as u64);
    cfg.set_initial_max_stream_data_bidi_remote(BACKEND_RECV_WINDOW as u64);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

/// The backend dial config factory. The backend grants the LB-as-client a
/// generous per-stream window, which does NOT matter here: the binding throttle
/// is the backend refusing to READ, which backs up into the relay's window.
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
        cfg.set_initial_max_stream_data_bidi_local(2 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(2 * 1024 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(16);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        cfg.enable_dgram(true, 1024, 1024);
        Ok(cfg)
    })
}

/// A BACKEND that, while `reading_enabled` is FALSE, does NOT call
/// `stream_recv` — it still pumps the connection so handshake/ACKs proceed, but
/// leaves stream data unread so its receive window stays closed. Flipped TRUE it
/// drains + echoes everything and FINs.
fn spawn_stalling_echo_backend(
    certs: &TestCerts,
    reading_enabled: Arc<AtomicBool>,
    total_echoed: Arc<AtomicUsize>,
) -> SocketAddr {
    let std_sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    std_sock.set_nonblocking(true).unwrap();
    let addr = std_sock.local_addr().unwrap();
    // Use the SMALL-window backend config so unread data cannot pile up in the
    // backend's own receive buffer, keeping the client's stall plateau a small,
    // scheduling-STABLE constant rather than drifting toward a multi-MiB buffer.
    let mut config = backend_config(certs);

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
                // While disabled, deliberately leave readable data un-read so
                // the backend's flow-control window stays shut.
                if reading_enabled.load(Ordering::Relaxed) {
                    let readable: Vec<u64> = c.readable().collect();
                    for sid in readable {
                        loop {
                            match c.stream_recv(sid, &mut rd) {
                                Ok((n, fin)) => {
                                    let e = echo_pending.entry(sid).or_insert((
                                        Vec::new(),
                                        false,
                                        false,
                                    ));
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
                                total_echoed.fetch_add(acc, Ordering::Relaxed);
                                e.0.drain(..acc.min(e.0.len()));
                            }
                            if e.1
                                && e.0.is_empty()
                                && !e.2
                                && c.stream_send(sid, &[], true).is_ok()
                            {
                                e.2 = true;
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

/// THE B2 backpressure verify: a stalled backend throttles the client
/// (bounded, payload-independent), and on resume the full payload arrives
/// byte-identical.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s16_b2_backpressure_client_throttled_then_complete_on_resume() {
    let certs = generate_loopback_certs();

    let reading_enabled = Arc::new(AtomicBool::new(false));
    let total_echoed = Arc::new(AtomicUsize::new(0));

    let backend_addr = spawn_stalling_echo_backend(
        &certs,
        Arc::clone(&reading_enabled),
        Arc::clone(&total_echoed),
    );

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

    let payload = make_payload(0xBEEF, PAYLOAD_LEN);

    // `transfer_complete` flips true ONLY when the client has read the FULL
    // echo back + FIN — the load-bearing completion witness, which must stay
    // false while the backend is stalled.
    let sent_cursor = Arc::new(AtomicUsize::new(0));
    let fin_queued = Arc::new(AtomicBool::new(false));
    let transfer_complete = Arc::new(AtomicBool::new(false));

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

    // Client driver: relentlessly push the unsent tail + FIN, keep the conn
    // live, and collect echoed bytes until FIN.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
    let client_cancel = cancel.clone();
    let payload_for_driver = payload.clone();
    let sent_cursor_drv = Arc::clone(&sent_cursor);
    let fin_queued_drv = Arc::clone(&fin_queued);
    let transfer_complete_drv = Arc::clone(&transfer_complete);
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
            let cursor = sent_cursor_drv.load(Ordering::Relaxed);
            if cursor < payload_for_driver.len() {
                let tail = payload_for_driver.get(cursor..).unwrap_or(&[]);
                match client_conn.stream_send(STREAM_ID, tail, true) {
                    Ok(n) => {
                        let nc = cursor + n;
                        sent_cursor_drv.store(nc, Ordering::Relaxed);
                        if nc == payload_for_driver.len() {
                            fin_queued_drv.store(true, Ordering::Relaxed);
                        }
                    }
                    Err(quiche::Error::Done) => {}
                    Err(_) => {}
                }
            } else if !fin_queued_drv.load(Ordering::Relaxed)
                && client_conn.stream_send(STREAM_ID, &[], true).is_ok()
            {
                fin_queued_drv.store(true, Ordering::Relaxed);
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

            if !got_fin {
                loop {
                    match client_conn.stream_recv(STREAM_ID, &mut recv_buf) {
                        Ok((n, fin)) => {
                            received.extend_from_slice(recv_buf.get(..n).unwrap_or(&[]));
                            if fin {
                                got_fin = true;
                                transfer_complete_drv.store(true, Ordering::Relaxed);
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

    // PHASE A: backend STALLED, held for a fixed settle window. We do NOT try
    // to detect a precise "plateau instant": the cursor counts bytes accepted
    // into quiche's LOCAL send buffer, so under CPU starvation it keeps inching
    // up while the actor is descheduled, making an instantaneous ceiling
    // scheduling-FRAGILE. The sound, timing-robust signals are asserted below.
    let settle = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < settle {
        // Bail early if the transfer (wrongly) completes while stalled — we
        // want to catch that as a failure, not wait out the whole window.
        if transfer_complete.load(Ordering::Relaxed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let queued_while_stalled = sent_cursor.load(Ordering::Relaxed);
    let echoed_during_stall = total_echoed.load(Ordering::Relaxed);
    let complete_while_stalled = transfer_complete.load(Ordering::Relaxed);
    eprintln!(
        "backpressure PHASE A: client queued {queued_while_stalled} bytes into \
         quiche; backend echoed {echoed_during_stall} bytes while stalled; \
         transfer_complete_while_stalled={complete_while_stalled}; \
         payload = {PAYLOAD_LEN}, LB grant to client = {LB_GRANT_TO_CLIENT}, \
         relay window = 262144, backend recv window = {BACKEND_RECV_WINDOW}, \
         cursor sanity ceiling = {STALL_CEILING}"
    );

    // ASSERT 1a (LOAD-BEARING): while the backend refuses to read, NOTHING
    // traverses back to it. A relay ignoring the destination's flow control
    // would have pushed data the backend could echo.
    assert!(
        echoed_during_stall == 0,
        "backend echoed {echoed_during_stall} bytes while it was NOT reading \
         — the relay pushed past the stalled destination instead of \
         honouring its flow control"
    );

    // ASSERT 1b (LOAD-BEARING): the round-trip did NOT complete while stalled.
    // A relay that fabricated a clean end, or buffered and echoed locally,
    // would wrongly complete here. With PHASE B completeness this proves the
    // transfer is GATED on the destination, not buffered through.
    assert!(
        !complete_while_stalled,
        "the transfer COMPLETED while the backend was stalled — the relay \
         must not deliver a complete round-trip when the destination has \
         read nothing (it is buffering/fabricating instead of back-pressuring)"
    );

    // ASSERT 1c (secondary, GENEROUS): the client did not get its WHOLE payload
    // drained. An unbounded buffer-everything relay would pull all of it into LB
    // memory. The cursor is a LOOSE proxy (local send-buffering inflates it
    // under saturation), so the ceiling has wide margin — its only role is to
    // falsify a gross "drained everything" relay.
    assert!(
        queued_while_stalled < STALL_CEILING,
        "client queued {queued_while_stalled} bytes while the backend was \
         stalled — at/above the generous {STALL_CEILING}-byte sanity ceiling, \
         approaching the {PAYLOAD_LEN}-byte payload, suggesting the LB drained \
         the client unboundedly rather than back-pressuring"
    );
    assert!(
        queued_while_stalled > 0,
        "the client queued zero bytes — the relay never accepted any data \
         (mis-configured fixture, not a backpressure proof)"
    );

    reading_enabled.store(true, Ordering::Relaxed);

    // Generous budget: the small backend window makes the resumed transfer
    // advance in stop-and-wait steps, each costing scheduling time under gate
    // saturation. Far above the real need — bump timeouts, don't weaken.
    let received = tokio::time::timeout(Duration::from_secs(90), done_rx)
        .await
        .expect("after resume, the client must receive the full echoed payload")
        .expect("client driver must deliver the received bytes");

    // ASSERT 2: NO LOSS / NO REORDER. The entire payload round-tripped
    // byte-identical despite the mid-transfer backpressure stall.
    assert_eq!(
        received.len(),
        payload.len(),
        "after resume the echoed length {} != sent length {} — backpressure \
         dropped or truncated data",
        received.len(),
        payload.len()
    );
    assert_eq!(
        received, payload,
        "after resume the echoed bytes are NOT byte-identical — the \
         backpressure carry-over reordered or corrupted the stream"
    );
    eprintln!(
        "backpressure PHASE B: full {PAYLOAD_LEN}-byte payload round-tripped \
         byte-identical after resume (no loss, no reorder)"
    );

    cancel.cancel();
    forwarder.abort();
    let _ = client_driver.await;
    let _ = tokio::time::timeout(Duration::from_secs(5), actor).await;
}
