//! SESSION 16 / Mode B — B3 BUILDER smoke: a client RESET_STREAM is
//! PROPAGATED to the backend as a real RESET_STREAM carrying the SAME app
//! code (the F-MD-4 analog *positive* leg; plan §2.3 + the `pump_dir`
//! reset/stop arms in `crates/lb-quic/src/raw_proxy.rs`).
//!
//! Author ≠ verifier. This is the MINIMAL builder self-check that B3
//! actually PROPAGATES the cancellation (not merely drops it, which the B2
//! `s16_b2_reset_not_fin.rs` negative-control already covers). The full
//! R13 (a)+(b)+(c) — in-gate run, ≥50-iter isolation burst, and the
//! load-bearing negative control — is the VERIFIER's bar.
//!
//!   real quiche CLIENT  ⇄  Mode B actor  ⇄  real quiche BACKEND (records)
//!
//! The client opens bidi stream 0, sends a PARTIAL body (no FIN), waits
//! until the backend has actually received some of those bytes (so the
//! upstream mirror stream 0 genuinely exists and is mid-transfer), then
//! RESET_STREAMs stream 0 with a specific app code. The backend records:
//!   * `observed_reset_code` — the code carried by the `Err(StreamReset)`
//!     it gets from `stream_recv(0)` (proves the RESET_STREAM was relayed
//!     onward AND the code was preserved); and
//!   * `saw_clean_fin` — whether it EVER saw a genuine FIN frame on
//!     stream 0 (`stream_recv` returning `fin == true`). Must stay false:
//!     the B2 smuggling guard is kept under B3. NOTE: we deliberately do
//!     NOT use `stream_finished()` as a witness — in quiche 0.28 it returns
//!     `true` for an *unknown* stream, and a RESET stream is collected and
//!     becomes unknown, so `stream_finished()` would falsely report a clean
//!     end on a stream that was correctly reset. Only the real FIN-frame
//!     signal distinguishes a smuggled clean-FIN from a propagated reset.
//!
//! ## Why this proves PROPAGATION, not just a drop
//!
//! A relay that merely dropped the half (B2) would leave the backend's
//! stream 0 hanging — `stream_recv(0)` would keep returning `Done`, never
//! `StreamReset`. Only the B3 `dst.stream_shutdown(0, Shutdown::Write,
//! code)` makes the backend observe a STREAM-LEVEL RESET_STREAM with the
//! code. We assert the backend saw `StreamReset(RESET_CODE)` — a
//! stream-level reset with the propagated code, NOT a stream dying from a
//! connection close (see the verifier note in the report re: F-MD-4
//! reset-vs-connection-teardown masking).
//!
//! Driven with `--features test-gauges` so the
//! `run_raw_proxy_actor_for_test` hook (gated
//! `#[cfg(any(test, feature = "test-gauges"))]`) is reachable.

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
/// The client bidi stream that is reset mid-transfer (identity-mapped both
/// legs).
const STREAM_ID: u64 = 0;
/// Application error code the client puts on its RESET_STREAM. B3 MUST
/// propagate this EXACT value to the backend — that is the load-bearing
/// assertion. A non-zero, non-trivial value so a stray default-`0` reset
/// cannot pass by coincidence.
const RESET_CODE: u64 = 0xBEEF;
/// Sentinel for "the backend has not observed any StreamReset code yet"
/// (a real propagated code is `RESET_CODE`, well below this).
const NO_RESET: u64 = u64::MAX;
/// Partial body the client sends BEFORE the reset (multi-packet, no FIN).
const PARTIAL_LEN: usize = 24 * 1024;

// ─────────────────────────────────────────────────────────────────────
// Cert plumbing (mirrors s16_b2_reset_not_fin.rs).
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
        "lb-quic-s16-b3-resetprop-{}-{nanos}-{seq}",
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

/// A deterministic binary payload (not all-ASCII).
fn make_payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 37 + 13) % 256) as u8).collect()
}

/// CLIENT-facing SERVER config (the LB-as-server leg). Generous windows so
/// the partial body is admitted; the reset PROPAGATION is what's tested.
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

/// The real downstream CLIENT config — verifies the LB's cert.
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

/// The pool's per-dial CLIENT config factory (LB → backend leg). Installs a
/// deliberately-wrong ALPN so the actor must MIRROR the client's `h3`.
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

/// A throwaway BACKEND that accepts ONE connection and, on stream 0,
/// records: (a) bytes received; (b) whether it ever observed a genuine
/// CLEAN end — `stream_recv` returning `fin == true` — which must stay
/// false; and (c) the application error code carried by any
/// `Err(StreamReset)` on stream 0 — the B3 witness. Storing the code (not
/// just a bool) lets the caller assert the EXACT propagated value,
/// distinguishing a real relayed RESET_STREAM from a coincidental
/// default-`0` reset or a connection teardown (which would NOT surface as a
/// stream-level `StreamReset`).
///
/// We deliberately do NOT use `stream_finished()` as a clean-end witness:
/// quiche 0.28 `stream_finished()` returns `true` for an UNKNOWN stream
/// (`lib.rs` `None => return true`), and a stream that has been RESET is
/// collected and becomes unknown — so once B3 correctly propagates the
/// reset, `stream_finished(0)` flips to `true` and would FALSELY report a
/// clean end on a correctly-reset stream. The real FIN-frame signal
/// (`fin == true` from `stream_recv`) is the only reliable smuggling
/// witness.
fn spawn_recording_backend(
    certs: &TestCerts,
    recv_bytes: Arc<AtomicUsize>,
    saw_clean_fin: Arc<AtomicBool>,
    observed_reset_code: Arc<AtomicU64>,
) -> SocketAddr {
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
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            if let Some(c) = conn.as_mut() {
                // Drain readable streams; record bytes, any CLEAN end, and
                // any RESET on stream 0 (the B3 witness — capture its code).
                let readable: Vec<u64> = c.readable().collect();
                for sid in readable {
                    loop {
                        match c.stream_recv(sid, &mut rd) {
                            Ok((n, fin)) => {
                                if sid == STREAM_ID {
                                    recv_bytes.fetch_add(n, Ordering::Relaxed);
                                    if fin {
                                        saw_clean_fin.store(true, Ordering::Relaxed);
                                    }
                                }
                                if fin || n == 0 {
                                    break;
                                }
                            }
                            // THE B3 WITNESS: a relayed RESET_STREAM surfaces
                            // here as a stream-level StreamReset carrying the
                            // propagated app code. Record it (first wins).
                            Err(quiche::Error::StreamReset(code)) => {
                                if sid == STREAM_ID {
                                    let _ = observed_reset_code.compare_exchange(
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
                    // NB: no `stream_finished()` witness here — see the
                    // fn-level note. It returns `true` for an unknown/
                    // collected stream, which a correctly-reset stream
                    // becomes, so it would false-positive the smuggling
                    // assertion. The `fin == true` branch above is the
                    // genuine clean-FIN signal.
                }
                // Flush outbound (ACKs / flow-control updates).
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

/// THE B3 self-check: a client RESET_STREAM mid-transfer is PROPAGATED to
/// the backend as a stream-level RESET_STREAM carrying the SAME app code,
/// and the backend still never sees a clean FIN (smuggling guard kept).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s16_b3_client_reset_propagates_with_code_to_backend() {
    let certs = generate_loopback_certs();

    let recv_bytes = Arc::new(AtomicUsize::new(0));
    let saw_clean_fin = Arc::new(AtomicBool::new(false));
    let observed_reset_code = Arc::new(AtomicU64::new(NO_RESET));

    let backend_addr = spawn_recording_backend(
        &certs,
        Arc::clone(&recv_bytes),
        Arc::clone(&saw_clean_fin),
        Arc::clone(&observed_reset_code),
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

    // Drive client⇄LB to established.
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

    // Send a PARTIAL body (no FIN).
    let payload = make_payload(PARTIAL_LEN);
    let _ = client_conn.stream_send(STREAM_ID, &payload, false).unwrap();
    flush(&mut client_conn, &client_socket, &mut out).await;

    // Forwarder: drain the shared LB socket into the actor's inbound mpsc.
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

    // Client driver: keep the client live; RESET stream 0 when told to and
    // then KEEP PUMPING so the RESET_STREAM frame actually reaches the LB
    // (a shutdown without a subsequent flush would never leave quiche).
    let do_reset = Arc::new(AtomicBool::new(false));
    let did_reset = Arc::new(AtomicBool::new(false));
    let client_cancel = cancel.clone();
    let do_reset_drv = Arc::clone(&do_reset);
    let did_reset_drv = Arc::clone(&did_reset);
    let client_driver = tokio::spawn(async move {
        let mut out = vec![0u8; MAX_UDP];
        let mut in_buf = vec![0u8; MAX_UDP];
        loop {
            if client_cancel.is_cancelled() || client_conn.is_closed() {
                break;
            }
            if do_reset_drv.load(Ordering::Relaxed) && !did_reset_drv.load(Ordering::Relaxed) {
                // Shutdown::Write ⇒ RESET_STREAM (per quiche API notes).
                let _ = client_conn.stream_shutdown(STREAM_ID, quiche::Shutdown::Write, RESET_CODE);
                did_reset_drv.store(true, Ordering::Relaxed);
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
        }
    });

    // The Mode B actor.
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

    // Wait until the backend has actually received SOME of the partial body
    // (so the upstream mirror stream 0 exists), then trigger the reset.
    let wait_recv = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if recv_bytes.load(Ordering::Relaxed) > 0 {
            break;
        }
        if tokio::time::Instant::now() >= wait_recv {
            panic!(
                "backend never received any of the partial body — cannot \
                 stage the mid-transfer reset (relay did not forward)"
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let bytes_before_reset = recv_bytes.load(Ordering::Relaxed);
    eprintln!("reset-propagation: backend received {bytes_before_reset} bytes before reset");

    // Fire the reset.
    do_reset.store(true, Ordering::Relaxed);

    // Wait for the backend to observe the PROPAGATED RESET_STREAM on
    // stream 0 (carrying the code). The relay must
    // `dst.stream_shutdown(0, Shutdown::Write, code)`; the backend's
    // `stream_recv(0)` then returns `Err(StreamReset(code))`.
    let observe = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < observe {
        if observed_reset_code.load(Ordering::Relaxed) != NO_RESET {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // ── THE B3 ASSERTIONS ──
    let seen = observed_reset_code.load(Ordering::Relaxed);
    assert_ne!(
        seen, NO_RESET,
        "B3 PROPAGATION MISSING: the backend never observed a stream-level \
         RESET_STREAM on stream 0 after the client reset it — the relay \
         dropped the half without propagating (B2 behaviour). B3 must call \
         dst.stream_shutdown(sid, Shutdown::Write, code)."
    );
    assert_eq!(
        seen, RESET_CODE,
        "B3 CODE NOT PRESERVED: the backend saw a RESET_STREAM but with code \
         {seen:#x}, not the client's {RESET_CODE:#x}. The propagated reset \
         must carry the SAME application error code."
    );
    // Smuggling guard (B2, kept under B3): never a clean FIN on a truncated
    // transfer.
    assert!(
        !saw_clean_fin.load(Ordering::Relaxed),
        "F-MD-4 SMUGGLING: the backend observed a CLEAN FIN on stream 0 after \
         a mid-transfer reset — a truncated transfer presented as complete. \
         B3 must propagate a RESET, NEVER a clean FIN."
    );
    assert!(
        did_reset.load(Ordering::Relaxed),
        "fixture: the client must have issued the RESET_STREAM"
    );
    eprintln!(
        "reset-propagation: VERIFIED — backend observed RESET_STREAM \
         code={seen:#x} (== client {RESET_CODE:#x}) on stream 0, no clean FIN"
    );

    // Tidy up.
    cancel.cancel();
    forwarder.abort();
    let _ = client_driver.await;
    let _ = tokio::time::timeout(Duration::from_secs(5), actor).await;
}
