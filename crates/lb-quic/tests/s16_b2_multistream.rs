//! SESSION 16 / Mode B — B2 VERIFIER's MULTI-STREAM byte-identical wire
//! proof (the B2 headline bar, plan §5 "multiple bidi streams + datagrams;
//! binary byte-identical").
//!
//! Author ≠ verifier: the builder's self-check
//! (`s16_b2_stream_relay_smoke.rs`) only proves ONE small (4 KiB) bidi
//! stream round-trips. THIS test stands up the same real wire path —
//!
//!   real quiche CLIENT  ⇄  Mode B actor (`run_raw_proxy_actor_for_test`)
//!                          ⇄  real quiche ECHO backend
//!
//! — but opens MULTIPLE concurrent client bidi streams, each carrying a
//! DISTINCT multi-KiB BINARY payload, and asserts that every stream's
//! echoed bytes arrive at the client BYTE-IDENTICAL and each FINs cleanly.
//!
//! ## What makes this load-bearing (beyond the smoke test)
//!
//! * **Concurrency / no cross-talk**: 5 streams in flight at once. The
//!   identity stream-ID relay must keep each stream's bytes on its own
//!   stream — a bug that merged/swapped per-stream pending buffers would
//!   make at least one payload mismatch.
//! * **Multi-turn / multi-packet**: payloads are deliberately sized so at
//!   least one EXCEEDS [`STREAM_RELAY_WINDOW`] (256 KiB) — that stream
//!   CANNOT be carried in a single relay turn (the read gate caps pending
//!   at the window), so it forces the backpressure carry-over path
//!   (pending tail held, FIN deferred until fully drained) to run for real
//!   on the happy path. Several payloads also far exceed one UDP packet.
//! * **Distinct binary content**: each stream gets a different
//!   pseudo-random byte stream (different seed), so a correct length but
//!   wrong-bytes relay (e.g. zero-fill, stale-buffer reuse) is caught.
//! * **Clean FIN per stream**: the client reads each stream to FIN; a
//!   relay that lost or mis-ordered a FIN would hang (caught by the budget)
//!   or deliver short (caught by the length assert).
//!
//! Driven with `--features test-gauges` so `run_raw_proxy_actor_for_test`
//! (gated `#[cfg(any(test, feature = "test-gauges"))]`) is reachable.

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
/// Generous: the largest payload (>256 KiB) must traverse
/// client→LB→backend→LB→client over many relay turns at the 2 ms tick.
const RELAY_BUDGET: Duration = Duration::from_secs(25);

// ─────────────────────────────────────────────────────────────────────
// Cert plumbing (mirrors s16_b2_stream_relay_smoke.rs).
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
        "lb-quic-s16-b2-multi-{}-{nanos}-{seq}",
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

/// A deterministic, per-stream-DISTINCT pseudo-random binary payload. The
/// `seed` differs per stream so two streams of the same length still carry
/// different bytes (catches a cross-stream buffer mix-up). A simple LCG —
/// not crypto, just a cheap full-range byte spread that is not all-ASCII.
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

/// CLIENT-facing SERVER config (the LB-as-server leg). Generous flow
/// control so the CLIENT can buffer whole payloads; the relay window
/// (256 KiB) is the bound under test, not these.
fn lb_server_config(certs: &TestCerts) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
        .unwrap();
    cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
        .unwrap();
    cfg.set_max_idle_timeout(20_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(16 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(2 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(2 * 1024 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(32);
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
    cfg.set_max_idle_timeout(20_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(16 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(2 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(2 * 1024 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(32);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

/// The pool's per-dial CLIENT config factory (LB → backend leg). Installs
/// a deliberately-wrong ALPN so the actor must MIRROR the client's `h3`.
/// Generous flow control so the backend grants the LB-as-client room.
fn upstream_config_factory(
    ca: PathBuf,
) -> Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> {
    Arc::new(move || {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
        cfg.set_application_protos(&[b"mode-b-factory-default"])?;
        cfg.load_verify_locations_from_file(ca.to_str().ok_or(quiche::Error::TlsFail)?)
            .map_err(|_| quiche::Error::TlsFail)?;
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(20_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(16 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(2 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(2 * 1024 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(32);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        cfg.enable_dgram(true, 1024, 1024);
        Ok(cfg)
    })
}

/// A throwaway BACKEND quiche server that accepts ONE connection and
/// ECHOes any received STREAM bytes back on the SAME stream id, FINing
/// each stream once it has fully echoed the peer FIN. Far end of the
/// relay: client→LB→**backend (echo)**→LB→client. (Same shape as the
/// smoke test's backend; carries N concurrent streams.)
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
        // Per-stream: (bytes queued to echo, peer-FIN-seen, our-FIN-sent).
        let mut echo_pending: HashMap<u64, (Vec<u8>, bool, bool)> = HashMap::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            if let Some(c) = conn.as_mut() {
                // 1) Read readable streams into the per-stream echo queue.
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

/// THE B2 headline verify: N concurrent bidi streams, each a distinct
/// multi-KiB binary payload (one >256 KiB relay window), all round-trip
/// client→LB→backend(echo)→LB→client byte-identically + clean FIN.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s16_b2_multistream_byte_identical_round_trip() {
    let certs = generate_loopback_certs();

    // Stream plan: (client bidi stream id, payload length). Stream ids are
    // client-initiated bidi = 0,4,8,12,16. Lengths chosen to span: small
    // multi-packet, ~window, and one OVER the 256 KiB relay window so it
    // needs many relay turns (the multi-turn backpressure carry path).
    let plan: Vec<(u64, usize)> = vec![
        (0, 9_000),    // > 1 packet (~1350 B), small
        (4, 60_000),   // many packets
        (8, 200_000),  // approaching the 256 KiB window
        (12, 400_000), // > 256 KiB window — MUST take multiple relay turns
        (16, 130_000), // distinct mid-size
    ];

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

    // 4) Inline-drive the client⇄LB legs to established.
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

    // 5) Build the distinct payloads and OPEN every stream up front. Each
    //    stream_send may be a SHORT write (payload > current send window);
    //    the client driver below keeps flushing AND keeps sending the
    //    unsent tail until the whole payload + FIN is on the wire. This is
    //    what forces the relay's bounded-window multi-turn carry for the
    //    >256 KiB stream.
    let payloads: HashMap<u64, Vec<u8>> = plan
        .iter()
        .map(|&(sid, len)| (sid, make_payload(sid.wrapping_add(1), len)))
        .collect();

    // Sanity: every payload is distinct and the big one exceeds the window.
    assert!(
        plan.iter().any(|&(_, len)| len > 256 * 1024),
        "fixture: at least one payload must exceed the 256 KiB relay window"
    );

    // Per-stream send cursor (how many bytes of the payload are queued into
    // quiche so far) and whether the FIN has been queued.
    let mut send_cursor: HashMap<u64, usize> = plan.iter().map(|&(sid, _)| (sid, 0usize)).collect();
    let mut fin_queued: HashMap<u64, bool> = plan.iter().map(|&(sid, _)| (sid, false)).collect();

    // Kick off the first send for each stream before handing off.
    for &(sid, _) in &plan {
        let payload = payloads.get(&sid).unwrap();
        match client_conn.stream_send(sid, payload, true) {
            Ok(n) => {
                send_cursor.insert(sid, n);
                if n == payload.len() {
                    fin_queued.insert(sid, true);
                }
            }
            Err(quiche::Error::Done) => {}
            Err(e) => panic!("initial client stream_send(sid={sid}): {e:?}"),
        }
    }
    flush(&mut client_conn, &client_socket, &mut out).await;

    // 6) Forwarder: drain the shared LB socket into the actor's inbound
    //    mpsc (the router's job in production).
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

    // 7) Client driver: keep the client live, KEEP SENDING the unsent tail
    //    of each payload (+ FIN) as the send window opens, and collect the
    //    echoed bytes per stream until every stream FINs. Returns the
    //    per-stream received bytes via a oneshot.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<HashMap<u64, Vec<u8>>>();
    let client_cancel = cancel.clone();
    let plan_for_driver = plan.clone();
    let payloads_for_driver = payloads.clone();
    let client_driver = tokio::spawn(async move {
        let mut out = vec![0u8; MAX_UDP];
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut recv_buf = vec![0u8; MAX_UDP];
        let mut received: HashMap<u64, Vec<u8>> = plan_for_driver
            .iter()
            .map(|&(sid, _)| (sid, Vec::new()))
            .collect();
        let mut got_fin: HashMap<u64, bool> = plan_for_driver
            .iter()
            .map(|&(sid, _)| (sid, false))
            .collect();
        let mut done_tx = Some(done_tx);
        loop {
            if client_cancel.is_cancelled() || client_conn.is_closed() {
                break;
            }

            // (a) Push any unsent payload tail + FIN for each stream.
            for &(sid, _) in &plan_for_driver {
                let payload = payloads_for_driver.get(&sid).unwrap();
                let cursor = *send_cursor.get(&sid).unwrap();
                if cursor < payload.len() {
                    let tail = payload.get(cursor..).unwrap_or(&[]);
                    match client_conn.stream_send(sid, tail, true) {
                        Ok(n) => {
                            let nc = cursor + n;
                            send_cursor.insert(sid, nc);
                            if nc == payload.len() {
                                fin_queued.insert(sid, true);
                            }
                        }
                        Err(quiche::Error::Done) => {}
                        Err(_) => {}
                    }
                } else if !*fin_queued.get(&sid).unwrap() {
                    // All bytes queued but FIN was not (cursor reached len
                    // via a non-FIN send) — send the standalone FIN.
                    if client_conn.stream_send(sid, &[], true).is_ok() {
                        fin_queued.insert(sid, true);
                    }
                }
            }

            flush(&mut client_conn, &client_socket, &mut out).await;
            try_recv_one(
                &mut client_conn,
                &client_socket,
                client_local,
                &mut in_buf,
                Duration::from_millis(5),
            )
            .await;

            // (b) Pull echoed bytes off every readable stream.
            let readable: Vec<u64> = client_conn.readable().collect();
            for sid in readable {
                if *got_fin.get(&sid).unwrap_or(&true) {
                    continue;
                }
                loop {
                    match client_conn.stream_recv(sid, &mut recv_buf) {
                        Ok((n, fin)) => {
                            if let Some(v) = received.get_mut(&sid) {
                                v.extend_from_slice(recv_buf.get(..n).unwrap_or(&[]));
                            }
                            if fin {
                                got_fin.insert(sid, true);
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

            // (c) Done when every stream has FIN'd.
            if got_fin.values().all(|&f| f) {
                if let Some(tx) = done_tx.take() {
                    let _ = tx.send(std::mem::take(&mut received));
                }
                break;
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
    };

    // 9) Run the actor; wait for all echoed payloads, then cancel.
    let actor = tokio::spawn(run_raw_proxy_actor_for_test(params));

    let received = tokio::time::timeout(RELAY_BUDGET, done_rx)
        .await
        .expect("client must receive every echoed payload before the budget")
        .expect("client driver must deliver the received bytes");

    // ── THE ASSERTIONS: every stream byte-identical + cleanly FIN'd. ──
    for &(sid, len) in &plan {
        let got = received
            .get(&sid)
            .unwrap_or_else(|| panic!("no bytes for stream {sid}"));
        let want = payloads.get(&sid).unwrap();
        assert_eq!(
            got.len(),
            len,
            "stream {sid}: echoed length {} != sent length {len} \
             (a FIN was lost or the transfer was truncated)",
            got.len()
        );
        assert_eq!(
            got, want,
            "stream {sid}: echoed bytes are NOT byte-identical to what the \
             client sent (cross-stream mix-up, reorder, or corruption in \
             the raw-stream relay)"
        );
    }
    eprintln!(
        "s16_b2_multistream: {} streams round-tripped byte-identical; \
         sizes = {:?}",
        plan.len(),
        plan.iter().map(|&(_, l)| l).collect::<Vec<_>>()
    );

    // Tidy up.
    cancel.cancel();
    forwarder.abort();
    let _ = client_driver.await;
    let _ = tokio::time::timeout(Duration::from_secs(5), actor).await;
}
