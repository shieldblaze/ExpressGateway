//! SESSION 16 / Mode B — B3 INDEPENDENT VERIFIER: the F-MD-4 bar for raw
//! cancellation propagation (plan §2.3, R13 a+b+c).
//!
//! Author ≠ verifier. The builder's `s16_b3_reset_propagation_smoke.rs`
//! proves the *positive* forward leg (client RESET → backend sees
//! `StreamReset(0xBEEF)`). THIS file is the load-bearing negative-control
//! suite the bar demands:
//!
//!   real quiche CLIENT  ⇄  Mode B actor (`run_raw_proxy_actor_for_test`)
//!                          ⇄  real quiche BACKEND
//!
//! It adds, beyond the smoke test:
//!   1. **Forward reset** with a DISTINCT code (`0xCAFE`) AND a connection-
//!      stays-up proof: after the reset, a SECOND client stream relays
//!      end-to-end byte-identical, proving the cancellation is STREAM-level
//!      RESET_STREAM, NOT the whole connection being torn down (the
//!      reset-vs-connection-teardown masking hazard from memory
//!      [[h1h3-fmd4-teardown-not-reset]] / [[h3h3-fmd4-no-r13-bc]]).
//!   2. **Reverse reset**: the BACKEND resets a backend-initiated stream
//!      mid-response → the CLIENT observes `StreamReset(code)` with the exact
//!      code, never a clean FIN.
//!   3. **Client STOP_SENDING** → the backend's `stream_send` surfaces the
//!      propagated `StreamStopped(code)`.
//!   4. A `#[should_panic]` control proving the no-clean-FIN discriminator
//!      actually FIRES on a genuine FIN (so assertion (1)/(2) are not
//!      vacuously true).
//!
//! quiche-0.28 gotcha (already bit B2): `stream_finished()` returns `true`
//! for an UNKNOWN/collected stream — a correctly-reset stream is collected,
//! so `stream_finished` would FALSE-positive a clean end. The ONLY clean-FIN
//! witness used here is `stream_recv` returning `fin == true`.
//!
//! Driven with `--features test-gauges` so `run_raw_proxy_actor_for_test`
//! (gated `#[cfg(any(test, feature = "test-gauges"))]`) is reachable.

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
/// A SECOND client-initiated bidi stream opened AFTER the forward reset to
/// prove the connection/relay is still alive (per-stream reset, not conn
/// teardown). Client-initiated bidi ⇒ id 4.
const SIBLING_STREAM: u64 = 4;
/// Application error code the client puts on its forward RESET_STREAM. A
/// DISTINCT, non-trivial value (different from the smoke test's 0xBEEF) so a
/// stray default-0 reset or a copy of the smoke fixture cannot pass.
const FWD_RESET_CODE: u64 = 0xCAFE;
/// Reverse direction: code the BACKEND puts on its RESET_STREAM.
const REV_RESET_CODE: u64 = 0xD00D;
/// Client STOP_SENDING code (forward stop test).
const STOP_CODE: u64 = 0x5701;
/// Sentinel "no StreamReset code observed yet".
const NO_RESET: u64 = u64::MAX;
/// Partial body sent BEFORE a reset (multi-packet, no FIN).
const PARTIAL_LEN: usize = 24 * 1024;
/// Sibling-stream payload (small, single-shot, FIN).
const SIBLING_PAYLOAD: &[u8] = b"sibling-stream-survives-the-reset-0123456789";

// ─────────────────────────────────────────────────────────────────────
// Cert plumbing (mirrors s16_b3_reset_propagation_smoke.rs).
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

/// Pool dial config factory (LB → backend). Wrong ALPN on purpose so the
/// actor must MIRROR the client's `h3`.
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

/// Shared per-stream observations recorded by the backend.
#[derive(Clone)]
struct BackendObs {
    /// Bytes received on FWD_STREAM.
    fwd_bytes: Arc<AtomicUsize>,
    /// `StreamReset` code seen on FWD_STREAM (first wins; NO_RESET = none).
    fwd_reset_code: Arc<AtomicU64>,
    /// Whether a CLEAN FIN was ever observed on FWD_STREAM (must stay false).
    fwd_saw_fin: Arc<AtomicBool>,
    /// Bytes received on SIBLING_STREAM (the backend echoes them back; the
    /// client side does the byte-identity check, so we only count here).
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

/// Spawn the standard control backend used by the FORWARD reset test. It:
///  * records bytes / clean-FIN / reset-code on `FWD_STREAM`;
///  * for `SIBLING_STREAM`, accumulates received bytes and ECHOes them back
///    (so the client can prove a post-reset stream round-trips cleanly).
///
/// No `stream_finished()` witness — see the file header re: the quiche-0.28
/// collected-stream gotcha. The only clean-FIN witness is `fin == true` from
/// `stream_recv`.
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
        // sibling echo queue: bytes pending + peer-FIN-seen + our-FIN-sent.
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
                // Echo the sibling stream back so the client sees it complete.
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
                // Flush outbound.
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

/// Build + drive a client⇄LB⇄backend Mode B world to the point where both the
/// client⇄LB legs are established and the actor is running. Returns the live
/// handles the test bodies drive. Kept as one helper because all three tests
/// share the identical bring-up.
struct World {
    cancel: CancellationToken,
    actor: tokio::task::JoinHandle<std::io::Result<lb_quic::RawProxyOutcome>>,
    forwarder: tokio::task::JoinHandle<()>,
    client_socket: Arc<UdpSocket>,
    client_local: SocketAddr,
    // The client connection is owned by the test body (driven inline).
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

    // Forwarder: shared LB socket → actor inbound mpsc (the router's job).
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

// ─────────────────────────────────────────────────────────────────────
// R13(c) — FORWARD: client RESET → backend StreamReset(code), no clean FIN,
// connection stays up (sibling stream survives).
// ─────────────────────────────────────────────────────────────────────

/// **THE HEADLINE.** A client RESET_STREAM mid-upload is propagated to the
/// backend as a STREAM-level RESET_STREAM carrying the EXACT code; the backend
/// never sees a clean FIN on that stream; AND the connection stays fully alive
/// — a SECOND client stream opened after the reset round-trips byte-identical.
///
/// This is load-bearing in three independent ways:
///  * **reset-with-code**: asserts `fwd_reset_code == FWD_RESET_CODE` — fails
///    if the relay dropped the half (B2: backend would keep getting `Done`,
///    code stays `NO_RESET`) OR forwarded the wrong code.
///  * **no-clean-FIN**: asserts `!fwd_saw_fin` — fails if the relay ever
///    synthesised `stream_send(.., fin=true)` (the F-MD-4 smuggling bug).
///  * **per-stream, not conn-teardown**: asserts the sibling stream completed
///    byte-identical — fails if the cancellation tore the whole connection
///    down (which would also kill the sibling), proving the reset is scoped to
///    the one stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forward_client_reset_propagates_with_code_conn_stays_up() {
    let certs = generate_loopback_certs();
    let obs = BackendObs::new();
    let backend_addr = spawn_forward_backend(&certs, obs.clone());
    let mut w = bring_up(&certs, backend_addr).await;

    // Move the client conn out so we can drive it inline here.
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

    // 1) Partial upload on FWD_STREAM, NO fin.
    let payload = make_payload(PARTIAL_LEN);
    client.stream_send(FWD_STREAM, &payload, false).unwrap();
    flush(&mut client, &client_socket, &mut out).await;

    // 2) Wait until the backend has received SOME of those bytes (the reset
    //    must land MID-transfer, after the mirror stream exists).
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

    // 3) RESET_STREAM the forward stream with the distinct code, and keep
    //    pumping so the RESET frame actually leaves quiche.
    client
        .stream_shutdown(FWD_STREAM, quiche::Shutdown::Write, FWD_RESET_CODE)
        .unwrap();

    // 4) Simultaneously OPEN the sibling stream (after the reset) and drive
    //    both: wait for (a) backend observes the reset code, AND (b) the
    //    sibling round-trips back to the client with a clean FIN.
    client
        .stream_send(SIBLING_STREAM, SIBLING_PAYLOAD, true)
        .unwrap();

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

        // Pull sibling echo bytes.
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

    // ── ASSERTIONS ──
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
    // Connection-stays-up / per-stream proof: the sibling stream relayed
    // end-to-end byte-identical AFTER the reset.
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

    // Put the client back so teardown owns a real conn (cosmetic).
    w.client_conn = client;
    teardown(w).await;
}

// ─────────────────────────────────────────────────────────────────────
// R13(c) — REVERSE: backend RESET → client StreamReset(code), no clean FIN.
// ─────────────────────────────────────────────────────────────────────

/// The BACKEND resets a backend-initiated bidi stream mid-response; the CLIENT
/// must observe `StreamReset(REV_RESET_CODE)` (propagated by the relay's u2c
/// direction) and never a clean FIN.
///
/// Stream choice: a *backend-initiated* bidi stream is server-initiated id 1
/// on the backend's connection. Under the identity map it surfaces to the
/// client as the same id 1 (the LB is server to the client, so a peer-of-LB
/// server stream id stays consistent). We let the backend open it, push a
/// partial response, then RESET it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reverse_backend_reset_propagates_with_code_to_client() {
    let certs = generate_loopback_certs();

    // A backend that opens stream 1 (server-initiated bidi), pushes a partial
    // response without FIN, then RESET_STREAMs it with REV_RESET_CODE once the
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
                    // Drain anything the client/relay sends (e.g. the request
                    // that triggers the actor to start relaying), so the conn
                    // progresses; we don't care about its content.
                    let readable: Vec<u64> = c.readable().collect();
                    for sid in readable {
                        while let Ok((_, fin)) = c.stream_recv(sid, &mut rd) {
                            if fin {
                                break;
                            }
                        }
                    }
                    // Open the server-initiated stream + push partial response.
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
                    // When told, RESET the response stream with the code.
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

    // The client opens stream 0 with a FIN'd request so the actor begins
    // relaying and the backend's connection is driven (the backend opens its
    // response stream only after established + it sees client traffic).
    client.stream_send(0, b"GET /", true).unwrap();
    flush(&mut client, &client_socket, &mut out).await;

    // Wait until the client has received SOME bytes of the backend's response
    // on REV_STREAM (mirror exists, mid-response), then fire the reset.
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

    // Drive until the client observes the propagated StreamReset code.
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

// ─────────────────────────────────────────────────────────────────────
// R13(c) — STOP_SENDING: client STOP_SENDING → backend stream_send stops.
// ─────────────────────────────────────────────────────────────────────

/// The client STOP_SENDINGs the response (read) side of a stream. The relay
/// must propagate a STOP_SENDING toward the backend so the backend's
/// `stream_send` on that stream eventually returns `Err(StreamStopped(code))`,
/// and must never let the client observe a clean FIN. We assert the backend
/// surfaces the propagated code AND no clean FIN reaches the client.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_stop_sending_propagates_to_backend() {
    let certs = generate_loopback_certs();

    // Backend opens stream 1, streams a response continuously, and records the
    // FIRST StreamStopped code it gets on stream 1 from its own `stream_send`.
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
                    // Continuously try to push response bytes on REV_STREAM.
                    // Once the client's STOP_SENDING is propagated, this turns
                    // into StreamStopped(code).
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

    // Wait until the client has received SOME response bytes on REV_STREAM,
    // then STOP_SENDING that read side.
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

    // STOP_SENDING the read side of REV_STREAM (Shutdown::Read ⇒ STOP_SENDING).
    client
        .stream_shutdown(REV_STREAM, quiche::Shutdown::Read, STOP_CODE)
        .unwrap();

    // Drive until the backend's stream_send surfaces the propagated stop code.
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
        // Keep reading so we'd catch any (forbidden) clean FIN.
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

// ─────────────────────────────────────────────────────────────────────
// Discriminator self-test: prove the no-clean-FIN witness actually fires.
// ─────────────────────────────────────────────────────────────────────

/// LOAD-BEARING META-TEST. If a genuine clean FIN is delivered, the same
/// `stream_recv` fin-scan the forward/reverse tests rely on MUST trip. This
/// guards against the witness being vacuously satisfiable (e.g. if a future
/// refactor made `stream_recv` never surface fin). A real client→backend FIN
/// is relayed by Mode B on the happy path; we assert the scan observes it.
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

    // A COMPLETE upload WITH fin on the forward stream — the relay delivers a
    // legitimate clean FIN to the backend.
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
