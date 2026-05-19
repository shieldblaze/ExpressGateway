//! SESSION 7 — H3 → H3 R8 streaming verification suite (real wire).
//!
//! Path under test: real quiche H3 client → production `QuicListener`
//! → `router` → `conn_actor::poll_h3` (the LIVE J3 `h3_backend`
//! branch) → `h3_to_h3_stream_resp` → a REAL quiche `accept` H3
//! upstream (genuine QUIC endpoint over a real `UdpSocket` speaking
//! the `lb_h3` codec — NOT an in-process stub, NOTHING below quiche
//! hand-rolled; the real-quiche-accept pump pattern is extended from
//! `round8_h3_authority_enforced.rs` / `h3_graceful_close.rs`).
//!
//! `h3_to_h3_stream_resp` is crate-internal (J3 lead ruling) and is
//! NEVER called directly: the suite drives ONLY the public
//! `QuicListener` + `QuicUpstreamPool` surface, so every assertion is
//! a genuine front-listener → router → bridge → real-backend wire
//! result.
//!
//! This is the H3→H1/H3→H2 BUILT bar applied to H3→H3:
//!
//! * **cond 1** — `h3h3_e2e_request_body_byte_identical_at_backend`:
//!   a NON-EMPTY BINARY (non-UTF-8) request body of ≥1 MiB arrives
//!   BYTE-IDENTICAL at the real H3 backend (proves J2's dropped-
//!   request-body fix on the wire). The same case asserts request
//!   TRAILERS are DROPPED on the H3→H3 leg (parity H3→H1 P1-C /
//!   H3→H2 A3) — the backend sees the body fully FIN-framed WITHOUT
//!   a post-DATA trailing-HEADERS frame: a lossless RFC-acceptable
//!   downgrade, NOT silent loss.
//! * **cond 2** — non-vacuous live-gauge memory proofs BOTH
//!   directions under a stalled peer
//!   (`h3h3_e2e_response_memory_bounded_through_stalled_client`,
//!   `h3h3_e2e_request_memory_bounded_through_stalled_backend`) plus
//!   a backpressure proof
//!   (`h3h3_e2e_backpressure_stalled_client_pauses_upstream_read`).
//! * **cond 3** — every gauge case references the feature-gated
//!   `lb_quic::h3_bridge::MAX_RETAINED_{RESP,BODY}_BYTES` statics, so
//!   the suite ONLY compiles those proofs under
//!   `cargo test -p lb-quic --features test-gauges` (a run without
//!   the flag fails to compile them — an invalid gate, by design,
//!   exactly as for `h3_h2_stream_e2e.rs`).
//!
//! BINDING case 7 —
//! `h3h3_e2e_client_reset_midrequest_rsts_upstream_no_truncated_request`:
//! the H3 client RESETs MID request body; the real H3 backend NEVER
//! observes a silently-truncated request presented as complete.
//!
//! No `#[ignore]` anywhere. The real H3 upstream is a genuine
//! `quiche::accept` endpoint — no mock, no stub.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use lb_io::quic_pool::{QuicPoolConfig, QuicUpstreamPool};
use lb_quic::{QuicListener, QuicListenerParams};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use lb_h3::{H3Frame, QpackDecoder, QpackEncoder, decode_frame, encode_frame};

const TEST_SNI: &str = "expressgateway.test";
const H3_ALPN: &[u8] = b"h3";
const H3_ALPN_PROTOS: &[&[u8]] = &[b"h3", b"h3-29"];
const MAX_UDP: usize = 65_535;

// ─────────────────────────────────────────────────────────────────────
// §1  Cert + listener + client harness (mirrors h3_h2_stream_e2e).
// ─────────────────────────────────────────────────────────────────────

/// The §1.5 C5 sound ceiling, IDENTICAL formula to
/// `h3_h2_stream_e2e::retained_ceiling` (`4 × depth×(chunk+hdr)`).
/// `test ceiling == gauge bound`.
fn retained_ceiling(depth: usize, chunk_max: usize, frame_hdr_max: usize) -> usize {
    4 * (depth * (chunk_max + frame_hdr_max))
}

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestCerts {
    _dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    ca: PathBuf,
    retry: PathBuf,
}
impl Drop for TestCerts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

fn generate_loopback_certs() -> TestCerts {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "lb-quic-h3h3-stream-{}-{}-{counter}",
        std::process::id(),
        nanos
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
    let retry_path = dir.join("retry.key");
    std::fs::write(&cert_path, cert.pem().as_bytes()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem().as_bytes()).unwrap();
    std::fs::write(&ca_path, cert.pem().as_bytes()).unwrap();
    TestCerts {
        _dir: dir,
        cert: cert_path,
        key: key_path,
        ca: ca_path,
        retry: retry_path,
    }
}

/// Front quiche H3 CLIENT config (mirrors h3_h2_stream_e2e exactly —
/// generous conn-level data so the per-stream window governs).
fn build_client_config(ca_path: &std::path::Path) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_verify_locations_from_file(ca_path.to_str().unwrap())
        .unwrap();
    cfg.verify_peer(true);
    cfg.set_max_idle_timeout(30_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(8 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(8 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(8 * 1024 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(8);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg
}

/// The SERVER config for the real quiche::accept H3 upstream. Serves
/// the loopback cert. Generous conn-level + per-stream windows so the
/// memory/backpressure proofs stall on the GATEWAY's bounded channel
/// (the thing under test), NOT on the upstream's own QUIC window.
fn build_upstream_server_config(certs: &TestCerts) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(H3_ALPN_PROTOS).unwrap();
    cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
        .unwrap();
    cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
        .unwrap();
    cfg.set_max_idle_timeout(30_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(16 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(16 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(16 * 1024 * 1024);
    cfg.set_initial_max_stream_data_uni(1024 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(16);
    cfg.set_disable_active_migration(true);
    cfg
}

/// The pool's per-dial CLIENT config factory (proxy → upstream leg).
/// Generous windows so the request/response memory proofs are
/// governed by the gateway's bounded in-flight channel, not this leg.
fn upstream_pool_config_factory()
-> Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> {
    Arc::new(|| {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
        cfg.set_application_protos(H3_ALPN_PROTOS)?;
        // The upstream presents the loopback cert; this leg is the
        // proxy dialing its own backend — verify off (parity with the
        // pool's production dummy_config_factory test usage).
        cfg.verify_peer(false);
        cfg.set_max_idle_timeout(30_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(16 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(16 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(16 * 1024 * 1024);
        cfg.set_initial_max_stream_data_uni(1024 * 1024);
        cfg.set_initial_max_streams_bidi(16);
        cfg.set_initial_max_streams_uni(16);
        cfg.set_disable_active_migration(true);
        Ok(cfg)
    })
}

fn random_scid() -> [u8; quiche::MAX_CONN_ID_LEN] {
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new().fill(&mut scid).unwrap();
    scid
}

/// Pseudo-random NON-UTF-8 body of `n` bytes (deterministic; same
/// generator as h3_h2_stream_e2e so the bar is identical).
fn binary_body(n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n);
    let mut s: u32 = 0x9E37_79B9;
    for i in 0..n {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        v.push((((s >> 24) as u8) | 0x80).wrapping_add(i as u8));
    }
    v
}

async fn start_h3_listener_h3(
    certs: &TestCerts,
    backend: SocketAddr,
) -> (QuicListener, SocketAddr, CancellationToken) {
    let quic_pool = QuicUpstreamPool::new(
        QuicPoolConfig::default(),
        upstream_pool_config_factory(),
    );
    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    )
    .with_h3_backend(quic_pool, backend, TEST_SNI);
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone()).await.unwrap();
    let addr = listener.local_addr();
    (listener, addr, shutdown)
}

/// What the driven H3 client observed (identical shape to
/// h3_h2_stream_e2e::ClientOut).
#[derive(Default)]
struct ClientOut {
    status: Option<u16>,
    body: Vec<u8>,
    content_length: Option<usize>,
    fin: bool,
    reset: bool,
}

/// Drive ONE H3 request on stream 0 (verbatim shape from
/// h3_h2_stream_e2e::drive_h3 — the front-client behaviour under test
/// is identical; only the upstream differs). `req_trailers`, when
/// non-empty, are sent as a post-DATA trailing-HEADERS frame so the
/// trailers-dropped parity can be asserted at the backend.
#[allow(clippy::struct_excessive_bools)]
struct DriveCfg {
    method: &'static str,
    path: &'static str,
    req_body: Vec<Vec<u8>>,
    req_trailers: Vec<(String, String)>,
    stall_after: Option<usize>,
    stall_for: Duration,
    reset_after_req_bytes: Option<usize>,
}

async fn drive_h3(
    gateway: SocketAddr,
    ca: &std::path::Path,
    cfg: DriveCfg,
    overall: Duration,
) -> ClientOut {
    let sock = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    let local = sock.local_addr().unwrap();
    let mut ccfg = build_client_config(ca);
    let scid = random_scid();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::connect(Some(TEST_SNI), &scid_ref, local, gateway, &mut ccfg).unwrap();

    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let sid: u64 = 0;
    let mut head_sent = false;
    let mut tx_wire: Vec<u8> = Vec::new();
    let mut tx_off = 0usize;
    let mut data_start = 0usize;
    let mut req_done = false;
    let mut did_reset = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut out = ClientOut::default();
    let mut stalling_until: Option<tokio::time::Instant> = None;
    let mut stalled_once = false;

    let deadline = tokio::time::Instant::now() + overall;
    while tokio::time::Instant::now() < deadline {
        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    let _ = sock.send_to(out_buf.get(..n).unwrap_or(&[]), info.to).await;
                }
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }

        if conn.is_established() && !head_sent {
            let encoder = QpackEncoder::new();
            let headers = vec![
                (":method".to_string(), cfg.method.to_string()),
                (":scheme".to_string(), "https".to_string()),
                (":authority".to_string(), TEST_SNI.to_string()),
                (":path".to_string(), cfg.path.to_string()),
            ];
            let block = encoder.encode(&headers).unwrap();
            tx_wire.extend_from_slice(
                &encode_frame(&H3Frame::Headers {
                    header_block: block,
                })
                .unwrap(),
            );
            data_start = tx_wire.len();
            for chunk in &cfg.req_body {
                tx_wire.extend_from_slice(
                    &encode_frame(&H3Frame::Data {
                        payload: Bytes::from(chunk.clone()),
                    })
                    .unwrap(),
                );
            }
            // Optional post-DATA trailing-HEADERS frame (RFC 9114
            // §4.1) so the test can prove the H3→H3 leg DROPS request
            // trailers (parity H3→H1 P1-C / H3→H2 A3).
            if !cfg.req_trailers.is_empty() {
                let tblock = QpackEncoder::new().encode(&cfg.req_trailers).unwrap();
                tx_wire.extend_from_slice(
                    &encode_frame(&H3Frame::Headers {
                        header_block: tblock,
                    })
                    .unwrap(),
                );
            }
            head_sent = true;
        }

        if head_sent && !req_done && !did_reset {
            if let Some(k) = cfg.reset_after_req_bytes {
                let body_sent = tx_off.saturating_sub(data_start);
                if body_sent >= k {
                    let _ = conn.stream_shutdown(sid, quiche::Shutdown::Write, 0x10c);
                    did_reset = true;
                    req_done = true;
                }
            }
            while !req_done && !did_reset && tx_off < tx_wire.len() {
                let remaining = tx_wire.get(tx_off..).unwrap_or(&[]);
                match conn.stream_send(sid, remaining, true) {
                    Ok(0) => break,
                    Ok(n) => {
                        tx_off += n;
                        if tx_off >= tx_wire.len() {
                            req_done = true;
                        } else if let Some(k) = cfg.reset_after_req_bytes {
                            if tx_off.saturating_sub(data_start) >= k {
                                let _ =
                                    conn.stream_shutdown(sid, quiche::Shutdown::Write, 0x10c);
                                did_reset = true;
                                req_done = true;
                            }
                        }
                    }
                    Err(quiche::Error::Done) => break,
                    Err(_) => break,
                }
            }
        }

        let now = tokio::time::Instant::now();
        if let Some(until) = stalling_until {
            if now >= until {
                stalling_until = None;
            }
        }
        if conn.is_established() && stalling_until.is_none() {
            let readable: Vec<u64> = conn.readable().collect();
            for r in readable {
                if r != sid {
                    continue;
                }
                let mut chunk = [0u8; 8192];
                loop {
                    match conn.stream_recv(r, &mut chunk) {
                        Ok((n, fin)) => {
                            rx_tail.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                            if fin {
                                out.fin = true;
                            }
                        }
                        Err(quiche::Error::Done) => break,
                        Err(_) => {
                            out.reset = true;
                            break;
                        }
                    }
                }
            }
            loop {
                match decode_frame(&rx_tail, 1 << 20) {
                    Ok((H3Frame::Headers { header_block }, c)) => {
                        rx_tail.drain(..c);
                        if let Ok(h) = QpackDecoder::new().decode(&header_block) {
                            for (n, v) in h {
                                if n == ":status" {
                                    out.status = v.parse().ok();
                                } else if n == "content-length" {
                                    out.content_length = v.parse().ok();
                                }
                            }
                        }
                    }
                    Ok((H3Frame::Data { payload }, c)) => {
                        rx_tail.drain(..c);
                        out.body.extend_from_slice(&payload);
                    }
                    Ok((_other, c)) => {
                        rx_tail.drain(..c);
                    }
                    Err(_) => break,
                }
            }
            if let Some(n) = cfg.stall_after {
                if !stalled_once
                    && stalling_until.is_none()
                    && !out.fin
                    && out.body.len() >= n
                    && cfg.stall_for > Duration::ZERO
                {
                    stalling_until = Some(now + cfg.stall_for);
                    stalled_once = true;
                }
            }
        }

        if out.fin || out.reset {
            break;
        }

        let to = conn.timeout().unwrap_or(Duration::from_millis(20));
        match tokio::time::timeout(
            to.min(Duration::from_millis(25)),
            sock.recv_from(&mut in_buf),
        )
        .await
        {
            Ok(Ok((n, from))) => {
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                let _ = conn.recv(slice, quiche::RecvInfo { from, to: local });
            }
            Ok(Err(_)) | Err(_) => conn.on_timeout(),
        }
        for _ in 0..64 {
            match sock.try_recv_from(&mut in_buf) {
                Ok((n, from)) => {
                    let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                    let _ = conn.recv(slice, quiche::RecvInfo { from, to: local });
                }
                Err(_) => break,
            }
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// §2  Real quiche::accept H3 upstream (genuine endpoint, real socket,
//     lb_h3 codec — extends the round8 / h3_graceful_close pattern).
// ─────────────────────────────────────────────────────────────────────

/// What the upstream observed about the request it received.
#[derive(Clone, Default)]
struct BackendSeen {
    /// Full request body bytes received (concatenated DATA payloads).
    body: Arc<Mutex<Vec<u8>>>,
    /// True iff the request stream ended with a clean FIN (a
    /// truncated/aborted request leaves this false — case 7).
    complete: Arc<AtomicBool>,
    /// Count of request HEADERS frames seen on the request stream
    /// (1 = no trailers forwarded; 2 = a post-DATA trailing-HEADERS
    /// frame WAS forwarded — used to assert trailers are DROPPED).
    headers_frames: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
}

/// How the upstream should respond.
#[derive(Clone)]
enum UpstreamMode {
    /// 200 + the received request body echoed back verbatim
    /// (bodyless ⇒ a fixed sentinel body).
    Echo,
    /// 200 + a fixed `body` (response-direction memory/backpressure).
    LargeResp(Arc<Vec<u8>>),
    /// Stall ~`delay` BEFORE reading the request body, then 200 +
    /// echo (request-direction stalled-backend memory proof).
    StallReadThenEcho(Duration),
    /// 200 + declared content-length far larger than the bytes
    /// actually written, then RESET the response stream mid-body
    /// (response-splitting guard — the client must never get a clean
    /// complete 200).
    ResetMidResponse,
}

/// Spawn a genuine quiche `accept` H3 upstream on a real `UdpSocket`.
/// One accepted connection per pooled dial (the gateway pool dials a
/// fresh conn per request and `set_reusable(false)`s it — S-2). Per
/// request stream: decode the HEADERS + DATA frames via `lb_h3`,
/// capture the body + whether it FIN'd cleanly + the HEADERS-frame
/// count, then emit the configured response. NOTHING below quiche is
/// hand-rolled.
async fn spawn_h3_upstream(
    certs: &TestCerts,
    mode: UpstreamMode,
    seen: BackendSeen,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let sock = Arc::new(
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap(),
    );
    let addr = sock.local_addr().unwrap();
    let mut server_cfg = build_upstream_server_config(certs);

    let h = tokio::spawn(async move {
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];
        // Accept loop: one quiche conn at a time (serially — the pool
        // opens a fresh conn per request, single-stream, non-reusable).
        loop {
            // Wait for an Initial.
            let (n, from) = match sock.recv_from(&mut in_buf).await {
                Ok(v) => v,
                Err(_) => return,
            };
            let local = sock.local_addr().unwrap();
            let hdr = match quiche::Header::from_slice(
                in_buf.get_mut(..n).unwrap_or(&mut []),
                quiche::MAX_CONN_ID_LEN,
            ) {
                Ok(h) => h,
                Err(_) => continue,
            };
            if hdr.ty != quiche::Type::Initial {
                continue;
            }
            let scid = random_scid();
            let scid_ref = quiche::ConnectionId::from_ref(&scid);
            let mut conn = match quiche::accept(&scid_ref, None, local, from, &mut server_cfg) {
                Ok(c) => c,
                Err(_) => continue,
            };
            // Feed the Initial we already read.
            let _ = conn.recv(
                in_buf.get_mut(..n).unwrap_or(&mut []),
                quiche::RecvInfo { from, to: local },
            );

            let seen = seen.clone();
            let mode = mode.clone();
            // Per-connection request/response state.
            let req_sid: u64 = 0;
            let mut rx_tail: Vec<u8> = Vec::new();
            let mut body: Vec<u8> = Vec::new();
            let mut headers_frames = 0usize;
            let mut req_fin = false;
            let mut response_started = false;
            let mut stall_until: Option<tokio::time::Instant> = None;
            // The full response wire, built ONCE when the request is
            // ready, then pushed with partial-accept retry (mirrors
            // `drive_h3`'s tx_wire/tx_off discipline — a bare
            // `stream_send` ignoring the returned count loses bytes).
            let mut resp_wire: Vec<u8> = Vec::new();
            let mut resp_off = 0usize;
            let mut resp_built = false;
            let mut resp_fin_on_drain = true;
            let mut resp_reset_after_drain = false;
            let mut resp_done = false;
            let conn_deadline =
                tokio::time::Instant::now() + Duration::from_secs(120);

            while tokio::time::Instant::now() < conn_deadline {
                // Flush egress.
                loop {
                    match conn.send(&mut out_buf) {
                        Ok((m, info)) => {
                            let _ = sock
                                .send_to(out_buf.get(..m).unwrap_or(&[]), info.to)
                                .await;
                        }
                        Err(quiche::Error::Done) => break,
                        Err(_) => break,
                    }
                }
                if conn.is_closed() {
                    break;
                }

                // Drain request stream bytes.
                if conn.is_established() {
                    let readable: Vec<u64> = conn.readable().collect();
                    for r in readable {
                        if r != req_sid {
                            continue;
                        }
                        let mut chunk = [0u8; 8192];
                        loop {
                            match conn.stream_recv(r, &mut chunk) {
                                Ok((m, fin)) => {
                                    rx_tail.extend_from_slice(
                                        chunk.get(..m).unwrap_or(&[]),
                                    );
                                    if fin {
                                        req_fin = true;
                                    }
                                }
                                Err(quiche::Error::Done) => break,
                                Err(_) => break,
                            }
                        }
                    }

                    // For the stalled-backend mode, hold off decoding
                    // (so we do not drain the request stream) until
                    // the stall window elapses — this is what forces
                    // the gateway's request pump to backpressure.
                    let draining = match &mode {
                        UpstreamMode::StallReadThenEcho(d) => {
                            if stall_until.is_none() {
                                stall_until =
                                    Some(tokio::time::Instant::now() + *d);
                            }
                            stall_until
                                .map(|u| tokio::time::Instant::now() >= u)
                                .unwrap_or(true)
                        }
                        _ => true,
                    };

                    if draining {
                        loop {
                            match decode_frame(&rx_tail, 1 << 20) {
                                Ok((H3Frame::Headers { .. }, c)) => {
                                    rx_tail.drain(..c);
                                    headers_frames += 1;
                                }
                                Ok((H3Frame::Data { payload }, c)) => {
                                    rx_tail.drain(..c);
                                    body.extend_from_slice(&payload);
                                }
                                Ok((_other, c)) => {
                                    rx_tail.drain(..c);
                                }
                                Err(_) => break,
                            }
                        }
                    }
                }

                // Emit the response once the request is in / FIN'd.
                if conn.is_established()
                    && !response_started
                    && (req_fin
                        || matches!(mode, UpstreamMode::LargeResp(_)))
                {
                    // For Echo/Stall we wait for the clean request FIN
                    // so the captured body + complete flag are final.
                    let ready = match &mode {
                        UpstreamMode::LargeResp(_) => true,
                        UpstreamMode::ResetMidResponse => req_fin,
                        UpstreamMode::Echo => req_fin,
                        UpstreamMode::StallReadThenEcho(_) => {
                            req_fin
                                && stall_until
                                    .map(|u| {
                                        tokio::time::Instant::now() >= u
                                    })
                                    .unwrap_or(false)
                        }
                    };
                    if ready {
                        *seen.body.lock().unwrap() = body.clone();
                        seen.complete.store(req_fin, Ordering::SeqCst);
                        seen.headers_frames
                            .store(headers_frames, Ordering::SeqCst);
                        seen.requests.fetch_add(1, Ordering::SeqCst);
                        response_started = true;
                    }
                }

                if response_started && !resp_done {
                    // Build the full response wire ONCE.
                    if !resp_built {
                        match &mode {
                            UpstreamMode::Echo
                            | UpstreamMode::StallReadThenEcho(_) => {
                                let payload = if body.is_empty() {
                                    b"h3-empty".to_vec()
                                } else {
                                    body.clone()
                                };
                                resp_wire = response_head(200, None);
                                resp_wire
                                    .extend_from_slice(&data_frames(&payload));
                                resp_fin_on_drain = true;
                            }
                            UpstreamMode::LargeResp(b) => {
                                resp_wire =
                                    response_head(200, Some(b.len()));
                                resp_wire.extend_from_slice(&data_frames(b));
                                resp_fin_on_drain = true;
                            }
                            UpstreamMode::ResetMidResponse => {
                                // Declare a 1 MiB body, write only
                                // ~64 KiB, then RESET the response
                                // stream — the gateway must NEVER
                                // deliver a clean complete 200.
                                resp_wire =
                                    response_head(200, Some(1_048_576));
                                resp_wire.extend_from_slice(&data_frames(
                                    &vec![7u8; 64 * 1024],
                                ));
                                resp_fin_on_drain = false;
                                resp_reset_after_drain = true;
                            }
                        }
                        resp_built = true;
                    }
                    // Push with partial-accept retry; FIN only when
                    // fully drained (Echo/Large) — never lose bytes.
                    while resp_off < resp_wire.len() {
                        let remaining =
                            resp_wire.get(resp_off..).unwrap_or(&[]);
                        let last = true;
                        let fin = resp_fin_on_drain && last;
                        match conn.stream_send(req_sid, remaining, fin) {
                            Ok(0) => break,
                            Ok(n) => {
                                resp_off += n;
                            }
                            Err(quiche::Error::Done) => break,
                            Err(_) => break,
                        }
                    }
                    if resp_off >= resp_wire.len() {
                        if resp_reset_after_drain {
                            let _ = conn.stream_shutdown(
                                req_sid,
                                quiche::Shutdown::Write,
                                0x010c,
                            );
                        }
                        resp_done = true;
                    }
                }

                let to =
                    conn.timeout().unwrap_or(Duration::from_millis(20));
                match tokio::time::timeout(
                    to.min(Duration::from_millis(25)),
                    sock.recv_from(&mut in_buf),
                )
                .await
                {
                    Ok(Ok((m, f))) => {
                        let slice =
                            in_buf.get_mut(..m).unwrap_or(&mut []);
                        let _ = conn.recv(
                            slice,
                            quiche::RecvInfo { from: f, to: local },
                        );
                    }
                    Ok(Err(_)) | Err(_) => conn.on_timeout(),
                }
            }
            // Connection finished; loop to accept the next pooled dial.
        }
    });
    (addr, h)
}

/// Encode an H3 response HEADERS frame (status [+ content-length]).
fn response_head(status: u16, content_length: Option<usize>) -> Vec<u8> {
    let mut headers = vec![(":status".to_string(), status.to_string())];
    if let Some(n) = content_length {
        headers.push(("content-length".to_string(), n.to_string()));
    }
    let block = QpackEncoder::new().encode(&headers).unwrap();
    encode_frame(&H3Frame::Headers {
        header_block: block,
    })
    .unwrap()
    .to_vec()
}

/// Encode `payload` as ≤16 KiB H3 DATA frames.
fn data_frames(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if payload.is_empty() {
        return out;
    }
    for slice in payload.chunks(16 * 1024) {
        out.extend_from_slice(
            &encode_frame(&H3Frame::Data {
                payload: Bytes::copy_from_slice(slice),
            })
            .unwrap(),
        );
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// §3  Cases (mirror h3_h2_stream_e2e's 7, 1:1).
// ─────────────────────────────────────────────────────────────────────

/// Case 1 — liveness floor: bodyless GET, 200 + sentinel body,
/// clean FIN (the no-regression anchor: the only case the deleted
/// buffered round-trip ever did; round8 also proves it live post-J3).
#[tokio::test]
async fn h3h3_e2e_get_response_byte_identical() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::Echo, seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/hello",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
        },
        Duration::from_secs(25),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    assert_eq!(out.status, Some(200), "H3→H3 GET must return 200");
    assert!(out.fin, "clean FIN expected");
    assert_eq!(out.body, b"h3-empty", "bodyless GET ⇒ backend sentinel");
}

/// Case 2 (BINDING cond 1) — a ≥1 MiB NON-UTF-8 BINARY request body
/// arrives BYTE-IDENTICAL at the real H3 backend (proves J2's
/// dropped-request-body fix on the wire), AND request trailers are
/// DROPPED on the H3→H3 leg (parity H3→H1 P1-C / H3→H2 A3): the
/// backend sees exactly ONE HEADERS frame (no post-DATA trailing-
/// HEADERS), the body fully FIN-framed — lossless, NOT silent loss.
#[tokio::test]
async fn h3h3_e2e_request_body_byte_identical_at_backend() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::Echo, seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let payload = binary_body(1024 * 1024 + 777);
    let chunks: Vec<Vec<u8>> = payload.chunks(48 * 1024).map(<[u8]>::to_vec).collect();

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/upload",
            req_body: chunks,
            req_trailers: vec![("x-req-trailer".to_string(), "v1".to_string())],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
        },
        Duration::from_secs(60),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let got = seen.body.lock().unwrap().clone();
    let hframes = seen.headers_frames.load(Ordering::SeqCst);
    let complete = seen.complete.load(Ordering::SeqCst);
    bh.abort();

    assert_eq!(out.status, Some(200), "POST must succeed");
    assert!(out.fin, "clean FIN expected");
    assert!(complete, "backend must see a CLEANLY-ENDED request body");
    assert_eq!(
        got.len(),
        payload.len(),
        "backend received {} bytes, sent {}",
        got.len(),
        payload.len()
    );
    assert!(
        got == payload,
        "request body must arrive BYTE-IDENTICAL at the H3 backend \
         (J2 dropped-request-body fix proven on the wire)"
    );
    assert_eq!(out.body, payload, "echoed response body must match");
    // Trailers-dropped parity: the client sent HEADERS + DATA* +
    // trailing-HEADERS; the H3→H3 leg DROPS request trailers, so the
    // backend must observe EXACTLY ONE HEADERS frame (the head) — the
    // body is fully framed by the FIN, a lossless RFC-acceptable
    // downgrade, NOT silent loss.
    assert_eq!(
        hframes, 1,
        "request trailers MUST be dropped on the H3→H3 leg (backend \
         saw {hframes} HEADERS frames; expected exactly 1 — the head)"
    );
}

/// Case 3 (BINDING cond 2, response direction) — a 4 MiB H3 response
/// streams through a STALLED H3 client; the live
/// `MAX_RETAINED_RESP_BYTES` gauge stays ≤ the C5 ceiling (≪ body),
/// then the client resumes and the body arrives byte-identical with a
/// clean FIN (non-vacuous: bound + liveness).
#[cfg(feature = "test-gauges")]
#[tokio::test]
async fn h3h3_e2e_response_memory_bounded_through_stalled_client() {
    use lb_quic::h3_bridge::{
        H3_FRAME_HDR_MAX, H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, MAX_RETAINED_RESP_BYTES,
    };

    MAX_RETAINED_RESP_BYTES.store(0, Ordering::SeqCst);
    let ceiling = retained_ceiling(H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, H3_FRAME_HDR_MAX);
    assert_eq!(ceiling, 262_656, "C5 RESP ceiling authoritative value");

    let body = binary_body(4 * 1024 * 1024);
    assert!(
        ceiling * 8 <= body.len(),
        "non-vacuous: ceiling ({ceiling}) must be ≪ body ({}) ≥8×",
        body.len()
    );

    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) =
        spawn_h3_upstream(&certs, UpstreamMode::LargeResp(Arc::new(body.clone())), seen).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/big",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: Some(256 * 1024),
            stall_for: Duration::from_secs(2),
            reset_after_req_bytes: None,
        },
        Duration::from_secs(75),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let retained = MAX_RETAINED_RESP_BYTES.load(Ordering::SeqCst);
    bh.abort();

    assert_eq!(out.status, Some(200));
    assert!(out.fin, "client must resume and see a clean FIN (liveness)");
    assert_eq!(out.body, body, "4 MiB body byte-identical after resume");
    assert!(
        retained <= ceiling,
        "RESP retained {retained} must stay ≤ C5 ceiling {ceiling} \
         (body was {} — proxy is NOT whole-body buffering)",
        body.len()
    );
    assert!(retained > 0, "gauge must be live (non-vacuous)");
}

/// Case 4 (BINDING cond 2, request direction) — a 4 MiB binary
/// request body while the H3 backend STALLS reading it; the live
/// `MAX_RETAINED_BODY_BYTES` gauge stays ≤ the C5 ceiling (≪ body),
/// then the backend unblocks and the body is byte-identical.
#[cfg(feature = "test-gauges")]
#[tokio::test]
async fn h3h3_e2e_request_memory_bounded_through_stalled_backend() {
    use lb_quic::conn_actor::H3_BODY_CHANNEL_DEPTH;
    use lb_quic::h3_bridge::{H3_BODY_CHUNK_MAX, MAX_FRAME_HEADER_BYTES, MAX_RETAINED_BODY_BYTES};

    MAX_RETAINED_BODY_BYTES.store(0, Ordering::SeqCst);
    let ceiling = retained_ceiling(H3_BODY_CHANNEL_DEPTH, H3_BODY_CHUNK_MAX, MAX_FRAME_HEADER_BYTES);
    assert_eq!(ceiling, 262_656, "C5 REQ ceiling authoritative value");

    let payload = binary_body(4 * 1024 * 1024);
    assert!(
        ceiling * 8 <= payload.len(),
        "non-vacuous: ceiling ({ceiling}) ≪ body ({}) ≥8×",
        payload.len()
    );

    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::StallReadThenEcho(Duration::from_millis(1500)),
        seen.clone(),
    )
    .await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let chunks: Vec<Vec<u8>> = payload.chunks(48 * 1024).map(<[u8]>::to_vec).collect();
    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/slow-upload",
            req_body: chunks,
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
        },
        Duration::from_secs(75),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let retained = MAX_RETAINED_BODY_BYTES.load(Ordering::SeqCst);
    let got = seen.body.lock().unwrap().clone();
    bh.abort();

    assert_eq!(out.status, Some(200), "request must complete after unblock");
    assert!(got == payload, "4 MiB request body byte-identical at backend");
    assert_eq!(out.body, payload, "echoed response body byte-identical");
    assert!(
        retained <= ceiling,
        "REQ retained {retained} must stay ≤ C5 ceiling {ceiling} \
         while the body was {} (request pump NOT whole-body buffering)",
        payload.len()
    );
    assert!(retained > 0, "gauge must be live (non-vacuous)");
}

/// Case 5 (BINDING cond 2, backpressure) — a stalled H3 client must
/// pause the H3 upstream read: retained ≤ ceiling for a body ≫
/// ceiling, AND the body still completes byte-identical after resume
/// (the response-direction causal chain held without drop/corruption).
#[cfg(feature = "test-gauges")]
#[tokio::test]
async fn h3h3_e2e_backpressure_stalled_client_pauses_upstream_read() {
    use lb_quic::h3_bridge::{
        H3_FRAME_HDR_MAX, H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, MAX_RETAINED_RESP_BYTES,
    };
    MAX_RETAINED_RESP_BYTES.store(0, Ordering::SeqCst);
    let ceiling = retained_ceiling(H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, H3_FRAME_HDR_MAX);

    let body = binary_body(8 * 1024 * 1024);
    assert!(ceiling * 16 <= body.len(), "non-vacuous ≥16×");

    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) =
        spawn_h3_upstream(&certs, UpstreamMode::LargeResp(Arc::new(body.clone())), seen).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/huge",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: Some(256 * 1024),
            stall_for: Duration::from_secs(3),
            reset_after_req_bytes: None,
        },
        Duration::from_secs(100),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let retained = MAX_RETAINED_RESP_BYTES.load(Ordering::SeqCst);
    bh.abort();

    assert!(out.fin, "body must complete after resume (causal chain held)");
    assert_eq!(out.body, body, "8 MiB byte-identical (no drop/corruption)");
    assert!(
        retained <= ceiling,
        "stalled client MUST pause the H3 upstream read: retained \
         {retained} ≤ ceiling {ceiling} for an 8 MiB body"
    );
}

/// Case 6 — the H3 backend RESETs mid response body ⇒ the H3 client
/// must NEVER get a clean complete 200 (response-splitting guard:
/// `RespAbort::UpstreamReset` → actor RESET_STREAMs, never FIN).
#[tokio::test]
async fn h3h3_e2e_upstream_reset_midbody_resets_client_no_fin() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) =
        spawn_h3_upstream(&certs, UpstreamMode::ResetMidResponse, seen).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/broken",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
        },
        Duration::from_secs(40),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    // A mid-body upstream reset must NEVER be delivered to the client
    // as a clean COMPLETE 200 (FIN with the full declared 1 MiB body).
    let delivered_complete_200 =
        out.status == Some(200) && out.fin && out.body.len() >= 1_048_576;
    assert!(
        !delivered_complete_200,
        "mid-body upstream reset MUST NOT yield a clean complete 200 \
         (status={:?} fin={} body_len={} declared=1048576)",
        out.status,
        out.fin,
        out.body.len()
    );
    if out.status == Some(200) && out.fin {
        assert!(
            out.body.len() < 1_048_576,
            "a 200+FIN must not carry the full declared body after a \
             mid-body upstream reset"
        );
    }
}

/// Case 7 (BINDING — request-side smuggling-parity) — the H3 client
/// RESETs MID request body. J2's mid-body abort
/// (`stream_shutdown(Write, H3_REQUEST_CANCELLED)` + no FIN +
/// non-reusable) means the real H3 backend NEVER observes a silently-
/// truncated request presented as a cleanly-ended (complete) one.
#[tokio::test]
async fn h3h3_e2e_client_reset_midrequest_rsts_upstream_no_truncated_request() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::Echo, seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let payload = binary_body(2 * 1024 * 1024);
    let chunks: Vec<Vec<u8>> = payload.chunks(32 * 1024).map(<[u8]>::to_vec).collect();

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/abort",
            req_body: chunks,
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: Some(256 * 1024),
        },
        Duration::from_secs(40),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let backend_saw_complete = seen.complete.load(Ordering::SeqCst);
    let backend_body_len = seen.body.lock().unwrap().len();
    bh.abort();

    assert!(
        !backend_saw_complete,
        "H3 backend must NEVER see a mid-request-aborted body as a \
         cleanly-ended (complete) request — got complete={backend_saw_complete}, \
         backend_body_len={backend_body_len} (intended {})",
        payload.len()
    );
    assert!(
        !(out.status == Some(200) && out.fin),
        "an aborted request must not yield a clean 200 FIN to the client"
    );
}
