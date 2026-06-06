//! SESSION 16 / Mode B — B2 VERIFIER's R8 BOUNDED-WINDOW / BACKPRESSURE
//! proof (plan §3: the bounded per-stream window is the memory-safety
//! mechanism; a slow backend pauses client reads via end-to-end
//! backpressure).
//!
//! Author ≠ verifier. The builder's smoke test proves the happy path; the
//! multistream test proves multi-turn carry. THIS test proves the mechanism
//! that makes the relay memory-SAFE under an adversarial/slow peer: when the
//! BACKEND stops reading a stream while the client keeps sending, the relay
//! must NOT buffer the client's send without bound — it must propagate
//! backpressure so the CLIENT itself stalls, and on resume the full payload
//! must arrive intact.
//!
//!   real quiche CLIENT  ⇄  Mode B actor  ⇄  real quiche backend that
//!                                            STALLS reading stream 0
//!
//! ## The mechanism (plan §3 / `raw_proxy.rs` `pump_dir` "Backpressure")
//!
//! Backend stops `stream_recv` on stream 0  ⇒  the upstream stream-0 send
//! window (LB-as-client → backend) fills  ⇒  the relay's `c2u` pending for
//! stream 0 reaches `STREAM_RELAY_WINDOW` (256 KiB)  ⇒  the relay stops
//! calling `client.stream_recv(0)`  ⇒  quiche stops extending the client's
//! `MAX_STREAM_DATA` for stream 0  ⇒  the CLIENT's `stream_send` for stream
//! 0 stalls (short write / `Done`) once its granted credit is spent. The
//! LB's retained bytes for the stream are bounded by the relay window + the
//! two quiche connections' own per-stream buffers — they do NOT grow with
//! the (much larger) total payload.
//!
//! ## What this test ASSERTS (black-box, from outside the LB)
//!
//! LOAD-BEARING (timing-robust):
//! 1. **NOTHING TRAVERSES TO A STALLED DESTINATION**: while the backend
//!    refuses to read, it echoes ZERO bytes back. The relay honours the
//!    destination's flow control instead of pushing past it.
//! 2. **NO FALSE COMPLETION WHILE STALLED**: the client never receives its
//!    echo + FIN back during the stall — the round-trip does NOT complete.
//!    The transfer is genuinely GATED on the destination, not buffered
//!    through or fabricated.
//! 3. **NO LOSS / NO REORDER ON RESUME**: once the backend resumes reading,
//!    the client finishes and the ENTIRE 4 MiB payload is echoed back
//!    BYTE-IDENTICAL. The backpressure stall dropped/scrambled nothing (the
//!    pending tail is held in order, FIN deferred until drained).
//!
//! SECONDARY (loose) witness:
//! 4. The client did not get its WHOLE payload drained into the LB while the
//!    backend was stalled (cursor < a generous ceiling, well below 4 MiB) —
//!    falsifies a gross "buffer-everything" relay.
//!
//! ## Honest scope (what it proves vs. doesn't)
//!
//! A black-box test CANNOT directly read the LB's in-process
//! `half.pending.len()` to assert `<= 256 KiB`, and the client's
//! `stream_send` cursor is NOT a tight in-flight proxy (quiche buffers
//! locally beyond the peer's window; under parallel-gate CPU starvation that
//! local buffer inflates the cursor, so a tight cursor ceiling is
//! scheduling-fragile — the S11 CF-SATURATION-1 class). So the LOAD-BEARING
//! proof here is the destination-gating pair (1)+(2) plus completeness (3):
//! a slow backend pauses the flow end-to-end and nothing is lost on resume.
//! The exact internal 256 KiB bound is the builder's `STREAM_RELAY_WINDOW`
//! constant + the `pump_dir` read gate
//! (`while half.pending.len() < STREAM_RELAY_WINDOW`), which the verifier
//! confirms by CODE-READ. Observable gating + internal gate ⇒ the bound is
//! real and load-bearing.
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

/// Total payload the client WANTS to send on the stalled stream. Made much
/// larger than (client stream-0 credit + relay window) so an unbounded relay
/// would have to march the client's cursor all the way to 4 MiB, while a
/// correctly-bounded relay stalls it at a small ceiling.
const PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// The LB-as-server grants the CLIENT this much stream-0 send credit
/// (`initial_max_stream_data_bidi_remote`). Small + fixed so the client's
/// stall ceiling is a small constant independent of `PAYLOAD_LEN`.
const LB_GRANT_TO_CLIENT: usize = 128 * 1024;

/// The BACKEND advertises only this much per-stream + connection receive
/// credit. Keeping it small means a STALLED backend (not reading) can hold
/// at most this many unread bytes in its receive buffer — so the place data
/// accumulates is bounded to `LB_GRANT_TO_CLIENT (client→LB) +
/// STREAM_RELAY_WINDOW (in the LB) + BACKEND_RECV_WINDOW (LB→backend)`,
/// a small constant. Without this (a multi-MiB backend window) the client's
/// plateau would drift up toward that window and become scheduling-
/// dependent under parallel-gate saturation (the S11 CF-SATURATION-1 class).
const BACKEND_RECV_WINDOW: usize = 128 * 1024;

/// GENEROUS, secondary sanity ceiling for the client's `stream_send` cursor
/// while the backend is stalled. NOTE: the cursor counts bytes accepted into
/// quiche's LOCAL send buffer, which exceeds the peer's advertised
/// MAX_STREAM_DATA and inflates under parallel-gate CPU starvation (observed
/// up to ~1.6 MiB in isolation it settles ~0.6 MiB). So this is a LOOSE
/// witness, not the load-bearing bound — its only job is to falsify a gross
/// "the LB drained the client's entire payload into its own memory" relay
/// (whose cursor would reach the full 4 MiB). 3 MiB leaves wide margin over
/// the saturation-inflated steady state yet is still well below the 4 MiB
/// payload. The SOUND backpressure proof is the load-bearing pair: backend
/// echoed 0 + transfer-not-complete while stalled, then full byte-identical
/// completeness on resume (PHASE B).
const STALL_CEILING: usize = 3 * 1024 * 1024;

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

/// CLIENT-facing SERVER config. CRITICAL: grants the client only
/// `LB_GRANT_TO_CLIENT` of per-stream send credit so the client's stall
/// ceiling is a small constant. `initial_max_data` (connection-level) is
/// generous so the CONNECTION-level window is not the throttle — we want the
/// PER-STREAM backpressure (the relay-window mechanism) to be the binding
/// constraint, not a coincidental conn-level cap.
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

/// The BACKEND's SERVER config — presents the loopback cert, speaks `h3`,
/// but advertises only a SMALL receive window (`BACKEND_RECV_WINDOW` per
/// stream + 2× that at the connection level). A stalled backend (not
/// reading) can therefore hold at most ~`BACKEND_RECV_WINDOW` of unread
/// data, bounding the client's stall plateau to a small constant. Distinct
/// from `lb_server_config` (the client⇄LB leg, which keeps a generous conn
/// window so the per-stream `LB_GRANT_TO_CLIENT` is the binding throttle
/// there, not a conn cap).
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

/// The backend dial config factory (wrong ALPN → actor mirrors `h3`). The
/// backend grants the LB-as-client a generous per-stream window; that does
/// NOT matter for the stall because the binding throttle is the backend
/// refusing to READ (so it stops sending MAX_STREAM_DATA increments once the
/// LB has sent its initial grant of bytes), backing up into the relay's
/// bounded window.
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
/// `stream_recv` (it still pumps the connection so the handshake / ACKs
/// proceed, but it leaves stream data unread so its receive window for the
/// stream stays closed). When flipped TRUE it drains + echoes everything,
/// FINing once drained. `total_echoed` lets the test observe progress.
fn spawn_stalling_echo_backend(
    certs: &TestCerts,
    reading_enabled: Arc<AtomicBool>,
    total_echoed: Arc<AtomicUsize>,
) -> SocketAddr {
    let std_sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    std_sock.set_nonblocking(true).unwrap();
    let addr = std_sock.local_addr().unwrap();
    // Use the SMALL-window backend config (not lb_server_config): the
    // backend advertises only `BACKEND_RECV_WINDOW` of per-stream receive
    // credit so unread data cannot pile up in the backend's own receive
    // buffer. This makes the client's stall plateau a SMALL, scheduling-
    // STABLE constant (client grant + LB relay window + backend window)
    // rather than drifting up toward a multi-MiB backend buffer under
    // parallel-gate saturation. See `backend_config` docs.
    let mut config = backend_config(certs);

    tokio::spawn(async move {
        let socket = UdpSocket::from_std(std_sock).unwrap();
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];
        let mut rd = vec![0u8; MAX_UDP];
        let mut conn: Option<quiche::Connection> = None;
        // Per-stream: (bytes queued to echo, peer-FIN-seen, our-FIN-sent).
        let mut echo_pending: HashMap<u64, (Vec<u8>, bool, bool)> = HashMap::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            if let Some(c) = conn.as_mut() {
                // Only READ streams when reading is enabled. While disabled
                // we deliberately leave readable stream data un-read so the
                // backend's flow-control window for the stream stays shut.
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
                    // Echo back.
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
                // Always flush outbound (ACKs, MAX_DATA, echo bytes).
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
            // Short timeout so we keep pumping ACKs even while "stalled".
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

    // 1) Backend that starts STALLED (not reading stream data).
    let backend_addr = spawn_stalling_echo_backend(
        &certs,
        Arc::clone(&reading_enabled),
        Arc::clone(&total_echoed),
    );

    // 2) Shared LB listener socket.
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

    // 4) Drive client⇄LB to established.
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

    // 5) Shared cursor for how many payload bytes the client has queued into
    //    quiche (sent). The client driver advances it; the test reads it to
    //    observe the stall. `transfer_complete` flips true only when the
    //    client has read the FULL echo back + FIN — the load-bearing
    //    completion witness (must stay false while the backend is stalled).
    let sent_cursor = Arc::new(AtomicUsize::new(0));
    let fin_queued = Arc::new(AtomicBool::new(false));
    let transfer_complete = Arc::new(AtomicBool::new(false));

    // 6) Forwarder.
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

    // 7) Client driver: relentlessly try to push the unsent payload tail +
    //    FIN (advancing `sent_cursor`), keep the conn live, and collect
    //    echoed bytes until FIN. Returns received bytes via oneshot.
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
            // Push unsent tail + FIN.
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

    // 8) The Mode B actor.
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
        // SESSION 27 WS-over-H3 Stage A: Mode-B tests never H3-terminate.
        ws_enabled: false,
        ws_relay_launcher: None,
        max_requests_per_h3_connection: 0,
        h3_recycle_metrics: None,
    };
    let actor = tokio::spawn(run_raw_proxy_actor_for_test(params));

    // ── PHASE A: backend STALLED. Hold the stall for a fixed settle window
    // and observe. We do NOT try to detect a precise "plateau instant": the
    // client `stream_send` cursor counts bytes accepted into quiche's LOCAL
    // send buffer (which exceeds the peer's advertised MAX_STREAM_DATA), so
    // under parallel-gate CPU starvation the cursor keeps inching up while
    // the actor is descheduled — a plateau-at-an-instant ceiling is
    // therefore scheduling-FRAGILE (S11 CF-SATURATION-1). Instead we assert
    // the SOUND, timing-robust signals below.
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
    // traverses the relay back to it — the backend echoed zero bytes. A relay
    // that ignored the destination's flow control would have pushed data the
    // backend could echo. (The backend's small recv window + not-reading is
    // the stall; the relay correctly stops at it.)
    assert!(
        echoed_during_stall == 0,
        "backend echoed {echoed_during_stall} bytes while it was NOT reading \
         — the relay pushed past the stalled destination instead of \
         honouring its flow control"
    );

    // ASSERT 1b (LOAD-BEARING): the round-trip did NOT complete while the
    // backend was stalled. The client never received its echo + FIN back.
    // A relay that fabricated a clean end (or buffered+echoed locally) would
    // wrongly complete here. Combined with PHASE B completeness, this proves
    // the transfer is genuinely GATED on the destination, not buffered
    // through.
    assert!(
        !complete_while_stalled,
        "the transfer COMPLETED while the backend was stalled — the relay \
         must not deliver a complete round-trip when the destination has \
         read nothing (it is buffering/fabricating instead of back-pressuring)"
    );

    // ASSERT 1c (secondary throttle witness, GENEROUS bound): the client did
    // not get its WHOLE payload accepted+drained. With a bounded relay the
    // client stalls once the LB stops extending its window; an UNBOUNDED
    // buffer-everything relay would drain the client's entire 4 MiB into LB
    // memory (cursor → PAYLOAD_LEN and the transfer would race to completion
    // even with the backend stalled, already caught by 1a/1b). The cursor is
    // a LOOSE proxy (quiche local send-buffering inflates it under
    // saturation, observed up to ~1.6 MiB), so the ceiling is set with wide
    // margin — its role is only to falsify a gross "drained everything"
    // relay, the sound proof is 1a+1b+PHASE B.
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

    // ── PHASE B: RESUME the backend; the full payload must complete. ──
    reading_enabled.store(true, Ordering::Relaxed);

    // Generous budget: the small backend receive window means the resumed
    // transfer advances in `BACKEND_RECV_WINDOW`-sized stop-and-wait steps,
    // and under parallel-gate saturation each step costs scheduling time.
    // 90 s is far above the real need (sub-second isolated) — "bump
    // timeouts, don't weaken" (S11 CF-SATURATION-1).
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

    // Tidy up.
    cancel.cancel();
    forwarder.abort();
    let _ = client_driver.await;
    let _ = tokio::time::timeout(Duration::from_secs(5), actor).await;
}
