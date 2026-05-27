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
    let quic_pool =
        QuicUpstreamPool::new(QuicPoolConfig::default(), upstream_pool_config_factory());
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
    /// F-S7-8 cluster 2 — count of HEADERS frames the client decoded on
    /// the response stream. `1` = response head only; `2` = the head
    /// PLUS a forwarded post-DATA trailing-HEADERS frame (used by
    /// CASE 9a to assert response trailers are FORWARDED). Additive,
    /// `Default`-initialised; the existing 7 cases never read it (a
    /// provable pure no-op — S78-G3).
    resp_headers_frames: usize,
    /// F-S7-8 cluster 2 — the trailer field names the client decoded
    /// from a second (post-DATA) HEADERS frame, if any. Lets CASE 9a
    /// assert the specific trailer arrived and CASE 9b assert a
    /// pseudo-header trailer was NEVER forwarded.
    resp_trailer_names: Vec<String>,
    /// CF-H3H3-HEAD — every `(name, value)` the client decoded from the
    /// FIRST (response-head) HEADERS frame, EXCLUDING `:status` (kept in
    /// `status`). Lets the full-header round-trip test assert regular
    /// headers (content-type / custom x-*) survive the H3→H3 response
    /// leg. Additive, `Default`-initialised; the existing cases never
    /// read it (a provable pure no-op — mirrors `resp_headers_frames` /
    /// `resp_trailer_names`).
    resp_head_pairs: Vec<(String, String)>,
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
    /// F-S7-8 cluster 4 — when set, omit the `:authority` pseudo-header
    /// from the request HEADERS entirely. With an absent `:authority`
    /// the gateway's `req.authority` is empty, so `h3_to_h3_stream_resp`
    /// substitutes the configured SNI for the upstream `:authority`
    /// (h3_bridge.rs :2876-2877 — the `if req.authority.is_empty()`
    /// TRUE branch). The request still succeeds end-to-end.
    omit_authority: bool,
    /// F-S7-8 cluster 4 — when `Some(k)`, after the client has read `k`
    /// response body bytes it STOP_SENDINGs (`Shutdown::Read`) the
    /// RESPONSE stream WITHOUT reading further. quiche surfaces this to
    /// the gateway as `Err(StreamStopped)`; the actor's
    /// `reap_client_cancelled_responses` drops the response receiver, so
    /// the bridge's NEXT `send!`/`send_progress!` returns
    /// `Err(RespAbort::ClientGone)` (h3_bridge.rs :2806 — the `send!`
    /// macro's `ClientGone` map). Used by CASE 12.
    stop_reading_resp_after: Option<usize>,
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
    // F-S7-8 cluster 4 — once the client has READ this many response
    // body bytes it STOP_SENDINGs the response stream (CASE 12). Latched
    // so it fires exactly once.
    let mut resp_stop_sent = false;

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
            // F-S7-8 cluster 4 — `omit_authority` drops `:authority`
            // entirely so the gateway substitutes the SNI for the
            // upstream authority (h3_bridge.rs :2876-2877).
            let mut headers = vec![
                (":method".to_string(), cfg.method.to_string()),
                (":scheme".to_string(), "https".to_string()),
            ];
            if !cfg.omit_authority {
                headers.push((":authority".to_string(), TEST_SNI.to_string()));
            }
            headers.push((":path".to_string(), cfg.path.to_string()));
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
                                let _ = conn.stream_shutdown(sid, quiche::Shutdown::Write, 0x10c);
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
        if conn.is_established() && stalling_until.is_none() && !resp_stop_sent {
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
                        out.resp_headers_frames += 1;
                        // The FIRST HEADERS frame is the response head;
                        // any SUBSEQUENT (post-DATA) HEADERS frame is a
                        // forwarded trailer section (F-S7-8 cluster 2).
                        let is_trailer = out.resp_headers_frames > 1;
                        if let Ok(h) = QpackDecoder::new().decode(&header_block) {
                            for (n, v) in h {
                                if is_trailer {
                                    out.resp_trailer_names.push(n.clone());
                                }
                                // CF-H3H3-HEAD: capture every non-status
                                // field of the response HEAD frame so the
                                // full-header round-trip test can assert
                                // regular headers survived.
                                if !is_trailer && n != ":status" {
                                    out.resp_head_pairs.push((n.clone(), v.clone()));
                                }
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
            // F-S7-8 cluster 4 — CASE 12 client-gone: once the client
            // has read `k` response body bytes, STOP_SENDING the
            // response stream and stop reading it. quiche surfaces this
            // to the gateway as a peer STOP_SENDING on the stream it is
            // writing; the actor reaps the response receiver and the
            // bridge's next `send!` returns `RespAbort::ClientGone`
            // (h3_bridge.rs :2806).
            if let Some(k) = cfg.stop_reading_resp_after {
                if !resp_stop_sent && !out.fin && out.body.len() >= k {
                    let _ = conn.stream_shutdown(sid, quiche::Shutdown::Read, 0x010c);
                    resp_stop_sent = true;
                }
            }
        }

        // CASE 12: after STOP_SENDING the response stream the client
        // intentionally stops reading; keep the connection alive (so the
        // STOP_SENDING is actually delivered and the gateway observes
        // it) for a bounded settle window, then exit. Do NOT break on
        // `out.fin`/`out.reset` here for that case — there is no clean
        // FIN to wait for.
        if resp_stop_sent {
            // Drive a few more I/O ticks so quiche flushes the
            // STOP_SENDING frame to the gateway, then fall through to
            // the normal park/recv below until the overall deadline.
        } else if out.fin || out.reset {
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
    /// F-S7-8 cluster 1 — after draining `after_bytes` of request DATA,
    /// the upstream `stream_shutdown(req_sid, Shutdown::Read, ..)` ⇒ a
    /// real STOP_SENDING on the wire. The gateway's next request-leg
    /// `stream_send` then returns `Err(StreamStopped)`, driving the
    /// HEADERS / request-DATA / request-FIN send-error arms
    /// (h3_bridge.rs :2928-2933, :3062-3074, :3458-3461) depending on
    /// WHEN the STOP_SENDING lands relative to the request leg.
    StopSendingMidRequest { after_bytes: usize },
    /// F-S7-8 cluster 2 — 200 + a small DATA body, then a post-DATA
    /// trailing-HEADERS frame (RFC 9114 §4.1) of the given kind, then
    /// FIN. Drives the response-trailer path (h3_bridge.rs :3284-3312).
    RespWithTrailers(TrailerKind),
    /// F-S7-8 cluster 3 — 200 + an UNKNOWN frame type (small declared
    /// length) interleaved before the DATA body, then FIN. Drives the
    /// `RecvState::InSkip` unknown-frame skip (h3_bridge.rs :3200-3209
    /// entry + :3349-3360 drain) then transparent resume.
    UnknownFrameThenResp,
    /// F-S7-8 cluster 3 — 200 then a hand-built frame header declaring
    /// a length `> DEFAULT_MAX_PAYLOAD_SIZE` (1 MiB) for the given
    /// frame type, with NO real payload. The gateway aborts on the
    /// DECLARED length at the header (`check_block_len`): `frame_type`
    /// `0x01` ⇒ the FRAME_HEADERS arm (h3_bridge.rs :3189-3193);
    /// any other ⇒ the unknown-frame arm (:3204-3207). Both map to
    /// `RespAbort::BadHead` — the client never gets a clean 200+FIN.
    OversizedBlock { frame_type: u64 },
    /// F-S7-8 cluster 4 — 200, then a ZERO-LENGTH DATA frame (`0x00
    /// 0x00`) emitted BEFORE the real body, then the body + FIN. Drives
    /// the gateway's empty-DATA fast-path (h3_bridge.rs :3214-3217:
    /// `RecvState::InData { remaining: 0 }` returns straight to
    /// `AwaitingHeader`, no spurious chunk). The gateway must skip the
    /// zero-length frame and still deliver the real body byte-identical
    /// with a clean FIN.
    EmptyDataThenResp,
    /// F-S7-8 cluster 4 — 200, then a DATA frame whose declared length
    /// is LARGER than the payload actually written, then a clean QUIC
    /// FIN. The gateway is mid-`InData` (remaining > 0) when the FIN
    /// lands, so it is NOT between frames ⇒ a premature EOF
    /// (h3_bridge.rs :3375-3383: `else { outcome = PrematureEof }`).
    /// The client MUST NOT receive a clean complete 200+FIN.
    HeadThenTruncatedData,
    /// CF-H3H3-HEAD — 200 + a response HEAD carrying REGULAR headers
    /// (content-type + a custom `x-eg-resp` header) ALONGSIDE
    /// `content-length`, then a small DATA body + clean FIN. Drives the
    /// full-header re-encode in the `Wire` sink's `on_head`
    /// (h3_bridge.rs `encode_h3_headers_frame_full`). The gateway MUST
    /// forward the full non-pseudo set to the H3 client — pre-fix it
    /// dropped everything but `:status` + `content-length`.
    RespWithHeaders,
}

/// F-S7-8 cluster 2 — the kind of post-DATA trailing-HEADERS frame the
/// upstream emits, selecting which response-trailer arm is driven.
#[derive(Clone)]
enum TrailerKind {
    /// One ordinary `(name, value)` trailer ⇒ the forward path
    /// (h3_bridge.rs :3284-3296 + :3297-3312): trailers are FORWARDED
    /// on the H3→H3 response leg (confirmed against src; distinct from
    /// J2 dropping REQUEST trailers — different direction).
    Valid,
    /// A trailer field whose name begins with `:` (a pseudo-header) ⇒
    /// the malformed-trailer reject arm (h3_bridge.rs :3290-3293):
    /// `RespAbort::BadHead`, never forwarded (RFC 9114 §4.3).
    PseudoHeader,
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
            // F-S7-8 cluster 1 — STOP_SENDING issued at most once.
            let mut req_stop_sent = false;
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
            let conn_deadline = tokio::time::Instant::now() + Duration::from_secs(120);

            while tokio::time::Instant::now() < conn_deadline {
                // Flush egress.
                loop {
                    match conn.send(&mut out_buf) {
                        Ok((m, info)) => {
                            let _ = sock.send_to(out_buf.get(..m).unwrap_or(&[]), info.to).await;
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
                    // F-S7-4: for StallReadThenEcho (case-4), hold off
                    // the stall window. `draining` is computed BEFORE
                    // the stream_recv drain so the drain itself can be
                    // gated on it — during the stall the upstream calls
                    // NEITHER `readable()` NOR `stream_recv`, so quiche
                    // stops extending the request stream's flow-control
                    // window and the gateway's J2 `stream_capacity`
                    // backpressure gate GENUINELY fires (a real
                    // transport-layer stall, not merely deferred
                    // decoding). All other modes take the `_ => true`
                    // arm ⇒ byte-identical behaviour.
                    let draining = match &mode {
                        UpstreamMode::StallReadThenEcho(d) => {
                            if stall_until.is_none() {
                                stall_until = Some(tokio::time::Instant::now() + *d);
                            }
                            stall_until
                                .map(|u| tokio::time::Instant::now() >= u)
                                .unwrap_or(true)
                        }
                        _ => true,
                    };

                    if draining {
                        let readable: Vec<u64> = conn.readable().collect();
                        for r in readable {
                            if r != req_sid {
                                continue;
                            }
                            let mut chunk = [0u8; 8192];
                            loop {
                                match conn.stream_recv(r, &mut chunk) {
                                    Ok((m, fin)) => {
                                        rx_tail.extend_from_slice(chunk.get(..m).unwrap_or(&[]));
                                        if fin {
                                            req_fin = true;
                                        }
                                    }
                                    Err(quiche::Error::Done) => break,
                                    Err(_) => break,
                                }
                            }
                        }

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

                // F-S7-8 cluster 1 — once `after_bytes` of request DATA
                // have been drained (counted on the decoded request
                // body), the upstream issues a real STOP_SENDING on the
                // request stream (`Shutdown::Read`). The gateway's next
                // request-leg `stream_send` then returns
                // `Err(StreamStopped)`. `after_bytes == 0` ⇒ the
                // STOP_SENDING is armed as soon as the request stream is
                // observed at all (the HEADERS / first request-DATA
                // write races it). The connection is otherwise driven
                // normally so quiche actually transmits the frame.
                if let UpstreamMode::StopSendingMidRequest { after_bytes } = &mode {
                    if conn.is_established() && !req_stop_sent {
                        let seen_bytes = body.len();
                        let stream_known = conn.readable().any(|s| s == req_sid)
                            || seen_bytes > 0
                            || headers_frames > 0;
                        if (*after_bytes == 0 && stream_known) || seen_bytes >= *after_bytes {
                            let _ = conn.stream_shutdown(req_sid, quiche::Shutdown::Read, 0x010c);
                            req_stop_sent = true;
                        }
                    }
                }

                // Emit the response once the request is in / FIN'd.
                if conn.is_established()
                    && !response_started
                    && (req_fin || matches!(mode, UpstreamMode::LargeResp(_)))
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
                                    .map(|u| tokio::time::Instant::now() >= u)
                                    .unwrap_or(false)
                        }
                        // F-S7-8 cluster 1 — the gateway aborts the
                        // exchange WITHOUT FIN once it sees the
                        // STOP_SENDING; this upstream never produces a
                        // response (it just idles until the conn closes).
                        UpstreamMode::StopSendingMidRequest { .. } => false,
                        // F-S7-8 clusters 2/3 — respond on the clean
                        // request FIN, exactly like `Echo` (the request
                        // leg is conformant; the adversarial behaviour
                        // is on the RESPONSE leg).
                        UpstreamMode::RespWithTrailers(_)
                        | UpstreamMode::UnknownFrameThenResp
                        | UpstreamMode::OversizedBlock { .. }
                        | UpstreamMode::EmptyDataThenResp
                        | UpstreamMode::HeadThenTruncatedData
                        | UpstreamMode::RespWithHeaders => req_fin,
                    };
                    if ready {
                        *seen.body.lock().unwrap() = body.clone();
                        seen.complete.store(req_fin, Ordering::SeqCst);
                        seen.headers_frames.store(headers_frames, Ordering::SeqCst);
                        seen.requests.fetch_add(1, Ordering::SeqCst);
                        response_started = true;
                    }
                }

                if response_started && !resp_done {
                    // Build the full response wire ONCE.
                    if !resp_built {
                        match &mode {
                            UpstreamMode::Echo | UpstreamMode::StallReadThenEcho(_) => {
                                let payload = if body.is_empty() {
                                    b"h3-empty".to_vec()
                                } else {
                                    body.clone()
                                };
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&data_frames(&payload));
                                resp_fin_on_drain = true;
                            }
                            UpstreamMode::LargeResp(b) => {
                                resp_wire = response_head(200, Some(b.len()));
                                resp_wire.extend_from_slice(&data_frames(b));
                                resp_fin_on_drain = true;
                            }
                            UpstreamMode::ResetMidResponse => {
                                // Declare a 1 MiB body, write only
                                // ~64 KiB, then RESET the response
                                // stream — the gateway must NEVER
                                // deliver a clean complete 200.
                                resp_wire = response_head(200, Some(1_048_576));
                                resp_wire.extend_from_slice(&data_frames(&vec![7u8; 64 * 1024]));
                                resp_fin_on_drain = false;
                                resp_reset_after_drain = true;
                            }
                            // F-S7-8 cluster 1 — never reached: `ready`
                            // is always false for this mode (the gateway
                            // aborts the request leg), so the response
                            // is never built. Arm present for
                            // exhaustiveness; emits nothing.
                            UpstreamMode::StopSendingMidRequest { .. } => {}
                            // F-S7-8 cluster 2 — 200 + small DATA body
                            // + a post-DATA trailing-HEADERS frame, then
                            // a clean FIN. The body is intentionally
                            // tiny (S78-G4): the response-trailer arm is
                            // size-independent.
                            UpstreamMode::RespWithTrailers(kind) => {
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&data_frames(b"h3-trail-body"));
                                let tf = match kind {
                                    TrailerKind::Valid => {
                                        trailers_frame(&[("x-resp-trailer", "v1")])
                                    }
                                    TrailerKind::PseudoHeader => {
                                        // A `:`-prefixed field in a
                                        // trailer section is malformed
                                        // (RFC 9114 §4.3) ⇒ the gateway
                                        // MUST reject, never forward it.
                                        trailers_frame(&[(":illegal", "x")])
                                    }
                                };
                                resp_wire.extend_from_slice(&tf);
                                resp_fin_on_drain = true;
                            }
                            // F-S7-8 cluster 3 — 200, then an UNKNOWN
                            // frame type (small payload) BEFORE the DATA
                            // body, then FIN. The gateway must skip the
                            // unknown frame (`InSkip`) and still deliver
                            // the body byte-identical with a clean FIN.
                            UpstreamMode::UnknownFrameThenResp => {
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&unknown_frame(
                                    0x21,
                                    b"reserved-frame-payload-skip-me",
                                ));
                                resp_wire.extend_from_slice(&data_frames(b"h3-skip-body"));
                                resp_fin_on_drain = true;
                            }
                            // F-S7-8 cluster 3 — 200, then a frame header
                            // declaring a length > 1 MiB with NO body.
                            // The gateway aborts on the declared length
                            // (`check_block_len`) — it never gets a
                            // clean complete 200. We FIN after the
                            // (tiny) bytes we wrote; the gateway has
                            // already aborted by then.
                            UpstreamMode::OversizedBlock { frame_type } => {
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&oversized_block_header(
                                    *frame_type,
                                    (1024 * 1024 + 1) as u64,
                                ));
                                resp_fin_on_drain = true;
                            }
                            // F-S7-8 cluster 4 — 200, then a ZERO-LENGTH
                            // DATA frame (type 0x00, len 0) BEFORE the
                            // real body, then the body + FIN. The empty
                            // DATA frame is the `0x00 0x00` two-byte
                            // header with NO payload.
                            UpstreamMode::EmptyDataThenResp => {
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&empty_data_frame());
                                resp_wire.extend_from_slice(&data_frames(b"h3-empty-data-body"));
                                resp_fin_on_drain = true;
                            }
                            // F-S7-8 cluster 4 — 200, then a DATA frame
                            // header declaring 4096 bytes but only 16
                            // bytes of payload, then a clean FIN. The
                            // gateway FINs while still mid-`InData`
                            // (remaining > 0) ⇒ PrematureEof.
                            UpstreamMode::HeadThenTruncatedData => {
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&truncated_data_frame(4096, 16));
                                resp_fin_on_drain = true;
                            }
                            // CF-H3H3-HEAD — 200 + a head carrying
                            // content-type + a custom x-eg-resp header
                            // ALONGSIDE content-length, then a small body
                            // + FIN. The gateway must forward the FULL
                            // non-pseudo set to the H3 client.
                            UpstreamMode::RespWithHeaders => {
                                let payload = b"h3-full-head-body";
                                resp_wire = response_head_with_headers(
                                    200,
                                    Some(payload.len()),
                                    &[
                                        ("content-type", "application/json"),
                                        ("x-eg-resp", "round-trip"),
                                    ],
                                );
                                resp_wire.extend_from_slice(&data_frames(payload));
                                resp_fin_on_drain = true;
                            }
                        }
                        resp_built = true;
                    }
                    // Push with partial-accept retry; FIN only when
                    // fully drained (Echo/Large) — never lose bytes.
                    while resp_off < resp_wire.len() {
                        let remaining = resp_wire.get(resp_off..).unwrap_or(&[]);
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
                            let _ = conn.stream_shutdown(req_sid, quiche::Shutdown::Write, 0x010c);
                        }
                        resp_done = true;
                    }
                }

                let to = conn.timeout().unwrap_or(Duration::from_millis(20));
                match tokio::time::timeout(
                    to.min(Duration::from_millis(25)),
                    sock.recv_from(&mut in_buf),
                )
                .await
                {
                    Ok(Ok((m, f))) => {
                        let slice = in_buf.get_mut(..m).unwrap_or(&mut []);
                        let _ = conn.recv(slice, quiche::RecvInfo { from: f, to: local });
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

/// CF-H3H3-HEAD — encode an H3 response HEADERS frame carrying
/// `:status`, an optional `content-length`, and the given REGULAR
/// (non-pseudo) headers. Lets a backend emit a full response head so
/// the full-header round-trip test can assert the gateway forwards the
/// whole set (not just `:status` + `content-length`).
fn response_head_with_headers(
    status: u16,
    content_length: Option<usize>,
    extra: &[(&str, &str)],
) -> Vec<u8> {
    let mut headers = vec![(":status".to_string(), status.to_string())];
    if let Some(n) = content_length {
        headers.push(("content-length".to_string(), n.to_string()));
    }
    for (n, v) in extra {
        headers.push(((*n).to_string(), (*v).to_string()));
    }
    let block = QpackEncoder::new().encode(&headers).unwrap();
    encode_frame(&H3Frame::Headers {
        header_block: block,
    })
    .unwrap()
    .to_vec()
}

/// F-S7-8 cluster 2 — encode a post-DATA trailing-HEADERS frame
/// (a `FRAME_HEADERS` frame whose QPACK block carries trailer fields).
/// Wire-identical to a normal HEADERS frame; the gateway distinguishes
/// it as a TRAILER purely positionally (it arrives after `sent_head`).
fn trailers_frame(fields: &[(&str, &str)]) -> Vec<u8> {
    let owned: Vec<(String, String)> = fields
        .iter()
        .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
        .collect();
    let block = QpackEncoder::new().encode(&owned).unwrap();
    encode_frame(&H3Frame::Headers {
        header_block: block,
    })
    .unwrap()
    .to_vec()
}

/// F-S7-8 cluster 3 — an UNKNOWN H3 frame (RFC 9114 §7.2.8 reserved
/// type) with a small `payload`. The gateway MUST skip it transparently
/// (`RecvState::InSkip`) and resume parsing the following frames.
fn unknown_frame(frame_type: u64, payload: &[u8]) -> Vec<u8> {
    encode_frame(&H3Frame::Unknown {
        frame_type,
        payload: Bytes::copy_from_slice(payload),
    })
    .unwrap()
    .to_vec()
}

/// F-S7-8 cluster 3 — a hand-built frame header `varint(frame_type) ||
/// varint(declared_len)` with NO payload bytes. `declared_len` is
/// chosen `> DEFAULT_MAX_PAYLOAD_SIZE` (1 MiB) so the gateway's
/// `check_block_len` rejects it on the DECLARED length the moment the
/// header parses — it never waits for (and we never send) the body.
/// `frame_type 0x01` exercises the FRAME_HEADERS over-cap arm; any
/// other type exercises the unknown-frame over-cap arm.
fn oversized_block_header(frame_type: u64, declared_len: u64) -> Vec<u8> {
    use bytes::BytesMut;
    let mut buf = BytesMut::new();
    lb_h3::encode_varint(&mut buf, frame_type).unwrap();
    lb_h3::encode_varint(&mut buf, declared_len).unwrap();
    buf.to_vec()
}

/// F-S7-8 cluster 4 — a ZERO-LENGTH H3 DATA frame: the two-byte header
/// `varint(FRAME_DATA=0x00) || varint(len=0)` with NO payload. The
/// gateway must treat this as a no-op (`RecvState::InData { remaining:
/// 0 }`) and emit no spurious body chunk.
fn empty_data_frame() -> Vec<u8> {
    use bytes::BytesMut;
    let mut buf = BytesMut::new();
    lb_h3::encode_varint(&mut buf, 0x00).unwrap(); // FRAME_DATA
    lb_h3::encode_varint(&mut buf, 0).unwrap(); // length 0
    buf.to_vec()
}

/// F-S7-8 cluster 4 — a DATA frame HEADER declaring `declared_len`
/// bytes followed by only `actual_len` (< declared) real payload bytes.
/// Used with a subsequent clean FIN so the gateway sees the stream end
/// while still mid-`InData` (a premature EOF).
fn truncated_data_frame(declared_len: u64, actual_len: usize) -> Vec<u8> {
    use bytes::BytesMut;
    let mut buf = BytesMut::new();
    lb_h3::encode_varint(&mut buf, 0x00).unwrap(); // FRAME_DATA
    lb_h3::encode_varint(&mut buf, declared_len).unwrap();
    let mut out = buf.to_vec();
    out.extend(std::iter::repeat_n(0x5Au8, actual_len));
    out
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
            omit_authority: false,
            stop_reading_resp_after: None,
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
            omit_authority: false,
            stop_reading_resp_after: None,
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
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::LargeResp(Arc::new(body.clone())),
        seen,
    )
    .await;
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
            omit_authority: false,
            stop_reading_resp_after: None,
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
    let ceiling = retained_ceiling(
        H3_BODY_CHANNEL_DEPTH,
        H3_BODY_CHUNK_MAX,
        MAX_FRAME_HEADER_BYTES,
    );
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
            omit_authority: false,
            stop_reading_resp_after: None,
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
    assert!(
        got == payload,
        "4 MiB request body byte-identical at backend"
    );
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
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::LargeResp(Arc::new(body.clone())),
        seen,
    )
    .await;
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
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(100),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let retained = MAX_RETAINED_RESP_BYTES.load(Ordering::SeqCst);
    bh.abort();

    assert!(
        out.fin,
        "body must complete after resume (causal chain held)"
    );
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
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::ResetMidResponse, seen).await;
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
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(40),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    // A mid-body upstream reset must NEVER be delivered to the client
    // as a clean COMPLETE 200 (FIN with the full declared 1 MiB body).
    let delivered_complete_200 = out.status == Some(200) && out.fin && out.body.len() >= 1_048_576;
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
            omit_authority: false,
            stop_reading_resp_after: None,
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

/// Case 7 R13 (b)+(c) — isolation-burst + non-vacuity for the
/// mid-request-RESET smuggling guard. The single-shot test above is
/// R13 (a) (in-gate); this adds (b) a ≥50-iteration burst on a
/// SINGLE-THREADED runtime — the scheduling configuration that exposes
/// timing-dependent request-smuggling races (memory
/// `parallel-gate-masks-smuggle`; `h3h3-fmd4-no-r13-bc` flagged this
/// gap) — and (c) a LOAD-BEARING non-vacuity positive: a CLEAN POST
/// (full body + FIN) must be counted by the backend as a cleanly-ended
/// request, so the burst's "the count never moves" assertion is not
/// vacuously true.
///
/// SIGNAL: `BackendSeen::requests` is incremented ONLY when the upstream
/// observes a cleanly-ended (FIN) request (the `if ready { .. }` block,
/// `ready = req_fin` for `Echo`). A mid-request RESET never reaches it.
/// So a smuggled truncated request — one relayed to the backend as
/// cleanly-ended — would increment `requests`. The clean control moves
/// it 0→1 (non-vacuity); the burst of truncated requests must leave it
/// at 1 (no smuggle). `complete` (latched by the clean control) is
/// asserted too as a secondary check.
///
/// NOTE FOR THE VERIFIER: the LOAD-BEARING MUTATION proof (R13 (c)
/// proper) is yours — H3→H3 has no pre-fix bug to revert, so flip the
/// connector's request-abort arm (the `stream_shutdown(Write,
/// H3_REQUEST_CANCELLED)` + no-FIN + non-reusable in the request leg)
/// to a clean FIN and confirm THIS burst then FAILS (a truncated
/// request gets counted). This test provides the burst + non-vacuity
/// the mutation acts on.
#[tokio::test(flavor = "current_thread")]
async fn h3h3_e2e_client_reset_midrequest_burst_current_thread() {
    const ITERS: usize = 60; // ≥50 per R13 (b)
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::Echo, seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    // Per-iteration body: much smaller than the single-shot 2 MiB so
    // the ITERS-deep burst is tractable, but still a genuine mid-body
    // abort (reset after 32 KiB of a 128 KiB upload — the smuggle race
    // is about aborting before FIN, not body size).
    let payload = binary_body(128 * 1024);
    let chunks: Vec<Vec<u8>> = payload.chunks(16 * 1024).map(<[u8]>::to_vec).collect();

    // (c) NON-VACUITY control — a CLEAN POST (full body + FIN). The
    // backend MUST count it as a cleanly-ended request.
    let clean = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/clean",
            req_body: chunks.clone(),
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(30),
    )
    .await;
    assert!(
        clean.status == Some(200) && clean.fin,
        "non-vacuity: a clean POST must yield 200 + FIN (status={:?} fin={})",
        clean.status,
        clean.fin
    );
    let baseline = seen.requests.load(Ordering::SeqCst);
    assert!(
        baseline >= 1 && seen.complete.load(Ordering::SeqCst),
        "LOAD-BEARING control: a clean upload must be counted complete by the \
         backend before the burst (requests={baseline}, complete={})",
        seen.complete.load(Ordering::SeqCst)
    );

    // (b) BURST — ITERS mid-request RESETs. NONE may reach the backend
    // as a cleanly-ended request (the `requests` count must not move),
    // and none may yield a clean 200+FIN to the client. Fired with
    // BOUNDED CONCURRENCY (`IN_FLIGHT`-wide) on the single-threaded
    // runtime: concurrent in-flight aborts contending on ONE scheduler
    // is a STRONGER smuggling-race probe than back-to-back sequential
    // requests (the `parallel-gate-masks-smuggle` configuration), and it
    // amortises the per-request QUIC-handshake + abort-settle cost so an
    // ITERS-deep burst stays tractable. The window is bounded so the
    // shared gateway/backend is not swamped into spurious failures.
    const IN_FLIGHT: usize = 8;
    let ca = certs.ca.clone();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut set: tokio::task::JoinSet<ClientOut> = tokio::task::JoinSet::new();
            let mut launched = 0usize;
            let mut finished = 0usize;
            let spawn_one = |set: &mut tokio::task::JoinSet<ClientOut>| {
                let ca = ca.clone();
                let chunks = chunks.clone();
                set.spawn_local(async move {
                    drive_h3(
                        gw,
                        &ca,
                        DriveCfg {
                            method: "POST",
                            path: "/abort",
                            req_body: chunks,
                            req_trailers: vec![],
                            stall_after: None,
                            stall_for: Duration::ZERO,
                            reset_after_req_bytes: Some(32 * 1024),
                            omit_authority: false,
                            stop_reading_resp_after: None,
                        },
                        Duration::from_secs(30),
                    )
                    .await
                });
            };
            while launched < IN_FLIGHT.min(ITERS) {
                spawn_one(&mut set);
                launched += 1;
            }
            while let Some(joined) = set.join_next().await {
                let out = joined.expect("burst task panicked");
                finished += 1;
                assert!(
                    !(out.status == Some(200) && out.fin),
                    "SMUGGLING (burst iter {finished}/{ITERS}): a mid-request-aborted \
                     request yielded a clean 200+FIN to the client"
                );
                if launched < ITERS {
                    spawn_one(&mut set);
                    launched += 1;
                }
            }
        })
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after_burst = seen.requests.load(Ordering::SeqCst);
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    eprintln!("H3H3_CASE7_BURST iters={ITERS} baseline={baseline} after_burst={after_burst}");
    assert_eq!(
        after_burst, baseline,
        "SMUGGLING under burst: {ITERS} mid-request RESETs moved the backend's \
         cleanly-ended-request count {baseline}→{after_burst} — at least one \
         truncated request was relayed to the upstream as complete (F-MD-4 race)"
    );
}

// ─────────────────────────────────────────────────────────────────────
// §4  F-S7-8 — adversarial coverage-remediation cases (test asset
//     only; same real wire: front quiche H3 client → QuicListener →
//     router → conn_actor poll_h3 LIVE h3_backend → h3_to_h3_stream_resp
//     → genuine quiche::accept upstream). Each case drives a specific
//     verifier-identified uncovered ERROR/EDGE arm of
//     `h3_to_h3_stream_resp` over the real wire — no stub, no #[ignore].
// ─────────────────────────────────────────────────────────────────────

/// Cluster 1 / CASE 8 — the upstream STOP_SENDINGs the REQUEST stream
/// AFTER ~64 KiB of request DATA has been forwarded. The gateway's next
/// request-DATA `stream_send` returns `Err(StreamStopped)`, driving the
/// request-DATA send-error arm (h3_bridge.rs :3062-3074:
/// `stream_shutdown(Write, H3_REQUEST_CANCELLED)` → `UpstreamReset` →
/// break). The client MUST NOT get a clean 200+FIN, and the backend
/// MUST NOT have seen a cleanly-ended request.
#[tokio::test]
async fn h3h3_e2e_upstream_stop_sending_mid_request_data_aborts_no_fin() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::StopSendingMidRequest {
            after_bytes: 64 * 1024,
        },
        seen.clone(),
    )
    .await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    // ~512 KiB binary body so request DATA is genuinely in flight when
    // the STOP_SENDING lands (small enough — S78-G4 — but multi-chunk).
    let payload = binary_body(512 * 1024);
    let chunks: Vec<Vec<u8>> = payload.chunks(32 * 1024).map(<[u8]>::to_vec).collect();

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/stop-mid-data",
            req_body: chunks,
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(40),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let backend_saw_complete = seen.complete.load(Ordering::SeqCst);
    bh.abort();

    assert!(
        !(out.status == Some(200) && out.fin),
        "an upstream STOP_SENDING mid request-DATA must NOT yield a \
         clean 200 FIN (status={:?} fin={})",
        out.status,
        out.fin
    );
    assert!(
        !backend_saw_complete,
        "the upstream must NEVER observe a cleanly-ended request after \
         it STOP_SENDINGs the request stream mid-DATA"
    );
}

/// Cluster 1 / CASE 8b — the upstream STOP_SENDINGs the REQUEST stream
/// as early as possible (`after_bytes = 0`, armed the instant the
/// request stream is observed). The fault lands on the HEADERS / first
/// request-DATA write, driving the HEADERS send-error arm
/// (h3_bridge.rs :2928-2933: `set_reusable(false)` → `RespEvent::Reset`
/// → `return Err(UpstreamReset)`) and/or the early request-DATA
/// send-error arm. Either way the client never gets a clean 200+FIN.
#[tokio::test]
async fn h3h3_e2e_upstream_stop_sending_immediately_aborts_no_fin() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::StopSendingMidRequest { after_bytes: 0 },
        seen.clone(),
    )
    .await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    // A multi-chunk body so a request leg genuinely exists for the
    // STOP_SENDING to abort (kept small — S78-G4).
    let payload = binary_body(256 * 1024);
    let chunks: Vec<Vec<u8>> = payload.chunks(32 * 1024).map(<[u8]>::to_vec).collect();

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/stop-immediate",
            req_body: chunks,
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(40),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let backend_saw_complete = seen.complete.load(Ordering::SeqCst);
    bh.abort();

    assert!(
        !(out.status == Some(200) && out.fin),
        "an immediate upstream STOP_SENDING must NOT yield a clean 200 \
         FIN (status={:?} fin={})",
        out.status,
        out.fin
    );
    assert!(
        !backend_saw_complete,
        "the upstream must NEVER observe a cleanly-ended request when \
         it STOP_SENDINGs the request stream immediately"
    );
}

/// Cluster 1 / CASE 8c — the upstream STOP_SENDINGs the REQUEST stream
/// AFTER it has drained the FULL request body. The fault then lands on
/// the gateway's terminal request-stream FIN write, driving the
/// request-FIN send-error arm (h3_bridge.rs :3458-3461:
/// `UpstreamReset` → break). Contingency (S78-G5): if the FIN-race
/// proves timing-flaky over the real wire this still deterministically
/// drives a request-leg send-error (the body fully buffered upstream
/// but the request never cleanly FIN-completed at the gateway); the
/// binding assertion (no clean 200+FIN, request never seen complete)
/// holds for EITHER landing point — reported as such.
#[tokio::test]
async fn h3h3_e2e_upstream_stop_sending_at_request_fin_aborts_no_fin() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    // ~256 KiB body; STOP_SENDING armed once the upstream has drained
    // ALL of it, so the gateway's terminal FIN write races the fault.
    let payload = binary_body(256 * 1024);
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::StopSendingMidRequest {
            after_bytes: payload.len(),
        },
        seen.clone(),
    )
    .await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let chunks: Vec<Vec<u8>> = payload.chunks(32 * 1024).map(<[u8]>::to_vec).collect();
    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/stop-at-fin",
            req_body: chunks,
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(40),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let backend_saw_complete = seen.complete.load(Ordering::SeqCst);
    bh.abort();

    assert!(
        !(out.status == Some(200) && out.fin),
        "an upstream STOP_SENDING at request-FIN must NOT yield a clean \
         200 FIN (status={:?} fin={})",
        out.status,
        out.fin
    );
    assert!(
        !backend_saw_complete,
        "the upstream must NEVER record a cleanly-ended request when it \
         STOP_SENDINGs the request stream at the FIN boundary"
    );
}

/// CF-H3H3-HEAD — the upstream sends 200 with content-type +
/// content-length + a custom `x-eg-resp` header, then a small DATA body
/// + clean FIN. The H3→H3 streaming response leg MUST forward the FULL
/// non-pseudo response header set to the H3 client — pre-fix it dropped
/// everything but `:status` + `content-length`
/// (`encode_h3_headers_frame(status, declared_len)`); post-fix the
/// `Wire` sink re-encodes the whole set via
/// `encode_h3_headers_frame_full`. LOAD-BEARING: this asserts
/// content-type AND x-eg-resp arrive at the client — it FAILS on the
/// old lossy projection (neither would be present) and PASSES with the
/// full-header re-encode. Body byte-identity + 200 + clean FIN confirm
/// the head change does not perturb the body/FIN framing.
#[tokio::test]
async fn h3h3_e2e_full_response_headers_round_trip() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::RespWithHeaders, seen).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/full-headers",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(25),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    assert_eq!(out.status, Some(200), "full-header response must be 200");
    assert!(out.fin, "clean FIN expected after full-header response");
    assert_eq!(
        out.body, b"h3-full-head-body",
        "response body must be byte-identical alongside the full header set"
    );
    // The two REGULAR headers the backend emitted must both round-trip
    // to the H3 client — the load-bearing assertions (pre-fix the
    // gateway dropped both; only :status + content-length survived).
    let has = |name: &str, val: &str| {
        out.resp_head_pairs
            .iter()
            .any(|(n, v)| n == name && v == val)
    };
    assert!(
        has("content-type", "application/json"),
        "content-type MUST round-trip H3→H3 (CF-H3H3-HEAD); got head pairs {:?}",
        out.resp_head_pairs
    );
    assert!(
        has("x-eg-resp", "round-trip"),
        "a custom response header MUST round-trip H3→H3 (CF-H3H3-HEAD); got head pairs {:?}",
        out.resp_head_pairs
    );
    // content-length still rides through as a regular header (it lands
    // in both `content_length` and `resp_head_pairs`).
    assert_eq!(
        out.content_length,
        Some(b"h3-full-head-body".len()),
        "content-length must still be forwarded"
    );
}

/// Cluster 2 / CASE 9a — the upstream sends 200 + a small DATA body +
/// a VALID post-DATA trailing-HEADERS frame, then a clean FIN. Drives
/// the response-trailer FORWARD path (h3_bridge.rs :3284-3296 +
/// :3297-3312; src-confirmed to forward response trailers — distinct
/// from J2 dropping REQUEST trailers). The client MUST get 200, the
/// body byte-identical, a clean FIN, AND a SECOND (trailing) HEADERS
/// frame carrying the forwarded trailer.
#[tokio::test]
async fn h3h3_e2e_response_trailers_forwarded() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::RespWithTrailers(TrailerKind::Valid),
        seen,
    )
    .await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/with-trailers",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(25),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    assert_eq!(out.status, Some(200), "trailer response must be 200");
    assert!(out.fin, "clean FIN expected after forwarded trailers");
    assert_eq!(
        out.body, b"h3-trail-body",
        "response body byte-identical alongside forwarded trailers"
    );
    assert_eq!(
        out.resp_headers_frames, 2,
        "the client must see TWO HEADERS frames: the response head + a \
         FORWARDED post-DATA trailing-HEADERS frame (got {})",
        out.resp_headers_frames
    );
    assert!(
        out.resp_trailer_names.iter().any(|n| n == "x-resp-trailer"),
        "the forwarded trailer field must be present on the H3→H3 \
         response leg (got names {:?})",
        out.resp_trailer_names
    );
}

/// Cluster 2 / CASE 9b — the upstream sends 200 + DATA + a MALFORMED
/// trailing-HEADERS frame containing a `:`-prefixed pseudo-header.
/// Drives the trailer pseudo-header reject arm (h3_bridge.rs
/// :3290-3293: `RespAbort::BadHead` → `RespEvent::Reset`). The pseudo
/// trailer MUST NEVER be forwarded and the client MUST NOT get a clean
/// complete 200+FIN.
#[tokio::test]
async fn h3h3_e2e_response_pseudo_header_trailer_rejected() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::RespWithTrailers(TrailerKind::PseudoHeader),
        seen,
    )
    .await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/bad-trailers",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(25),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    assert!(
        !out.resp_trailer_names.iter().any(|n| n.starts_with(':')),
        "a pseudo-header trailer MUST NEVER be forwarded to the client \
         (got trailer names {:?})",
        out.resp_trailer_names
    );
    assert!(
        !(out.status == Some(200) && out.fin && !out.body.is_empty()),
        "a malformed (pseudo-header) trailer section must abort the \
         response, never a clean complete 200+FIN (status={:?} \
         fin={} body_len={})",
        out.status,
        out.fin,
        out.body.len()
    );
}

/// Cluster 3 / CASE 10a — the upstream sends 200, then an UNKNOWN H3
/// frame type (RFC 9114 §7.2.8 reserved) BEFORE the DATA body, then a
/// clean FIN. Drives the unknown-frame skip path (h3_bridge.rs
/// :3200-3209 `InSkip` entry + :3349-3360 incremental drain). The
/// gateway MUST transparently skip the unknown frame and still deliver
/// the body byte-identical with a clean FIN (non-vacuous: the skip
/// resumed correctly).
#[tokio::test]
async fn h3h3_e2e_unknown_response_frame_skipped_transparently() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::UnknownFrameThenResp, seen).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/unknown-frame",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(25),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    assert_eq!(out.status, Some(200), "200 expected past the skipped frame");
    assert!(
        out.fin,
        "clean FIN expected — the unknown frame was skipped and parsing \
         resumed (InSkip drain non-vacuous)"
    );
    assert_eq!(
        out.body, b"h3-skip-body",
        "body byte-identical after a transparently-skipped unknown frame"
    );
}

/// Cluster 3 / CASE 10b — the upstream sends 200, then a HEADERS-typed
/// (`0x01`) frame header declaring a length `> DEFAULT_MAX_PAYLOAD_SIZE`
/// (1 MiB) with NO payload. Drives the FRAME_HEADERS over-cap arm
/// (h3_bridge.rs :3189-3193: `check_block_len` → `RespAbort::BadHead`
/// → break). The client MUST NOT get a clean complete 200+FIN.
#[tokio::test]
async fn h3h3_e2e_oversized_headers_block_rejected() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::OversizedBlock { frame_type: 0x01 },
        seen,
    )
    .await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/oversized-headers",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(25),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    assert!(
        !(out.status == Some(200) && out.fin),
        "an oversized declared HEADERS block must abort (BadHead), \
         never a clean complete 200+FIN (status={:?} fin={})",
        out.status,
        out.fin
    );
}

/// Cluster 3 / CASE 10c — as 10b but the oversized declared length is
/// on an UNKNOWN frame type (`0x21`), driving the OTHER `check_block_len`
/// call-site: the unknown-frame over-cap arm (h3_bridge.rs :3204-3207:
/// `check_block_len` → `RespAbort::BadHead` → break) — a distinct line
/// region from 10b's FRAME_HEADERS arm. The client MUST NOT get a clean
/// complete 200+FIN.
#[tokio::test]
async fn h3h3_e2e_oversized_unknown_frame_rejected() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::OversizedBlock { frame_type: 0x21 },
        seen,
    )
    .await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/oversized-unknown",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(25),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    assert!(
        !(out.status == Some(200) && out.fin),
        "an oversized declared UNKNOWN-frame length must abort \
         (BadHead), never a clean complete 200+FIN (status={:?} \
         fin={})",
        out.status,
        out.fin
    );
}

// ─────────────────────────────────────────────────────────────────────
// §5  F-S7-8 cluster 4 — further targeted real-wire coverage of the
//     session error/edge arms (test asset only; same real wire).
// ─────────────────────────────────────────────────────────────────────

/// Cluster 4 / CASE 11 — the H3 backend address has NO live quiche
/// upstream listening, so the pool's fresh dial cannot complete the
/// handshake and `pool.acquire(addr, sni)` returns `Err` after the
/// dial deadline. Drives the pool-acquire-failure 502 path
/// (h3_bridge.rs :2856-2862: the `acquire` `Err(e)` arm → `inline(502)`
/// → return `Ok(())`) AND the `inline` helper SUCCESS path
/// (:2785-2788: `encode_h3_response` Ok → `Bytes` + `End`). The client
/// MUST receive a 502 with a clean FIN (a genuine inline error
/// response, NOT a dropped/hung stream).
#[tokio::test]
async fn h3h3_e2e_pool_acquire_failure_returns_502() {
    let certs = generate_loopback_certs();
    // A loopback UDP address with NOTHING bound: bind to grab a free
    // ephemeral port, capture it, then drop the socket so the port is
    // free again. The gateway's pooled dial sends Initials there and
    // never gets a handshake response ⇒ `acquire` fails at the dial
    // deadline. Deterministic; no long idle (the dial deadline bounds
    // it — see quic_pool::dial_new).
    let dead_backend = {
        let s =
            std::net::UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .unwrap();
        s.local_addr().unwrap()
        // `s` dropped here ⇒ port unbound, no responder.
    };

    let (listener, gw, sd) = start_h3_listener_h3(&certs, dead_backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/no-backend",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(20),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();

    assert_eq!(
        out.status,
        Some(502),
        "a failed pool acquire (no live backend) MUST yield an inline \
         502 (status={:?})",
        out.status
    );
    assert!(
        out.fin,
        "the inline 502 must be a CLEAN complete response (FIN), not a \
         hung/dropped stream"
    );
    assert_eq!(
        out.body, b"bad gateway",
        "the inline 502 body must be the gateway's `bad gateway` sentinel"
    );
}

/// Cluster 4 / CASE 12 — the H3 client reads the response head + a few
/// body bytes, then STOP_SENDINGs the RESPONSE stream and stops reading.
/// quiche surfaces this to the gateway as a peer STOP_SENDING on the
/// stream it is writing; the actor's `reap_client_cancelled_responses`
/// drops the response receiver, so the bridge's next
/// `send!`/`send_progress!` returns `Err(RespAbort::ClientGone)`
/// (h3_bridge.rs :2806 — the `send!` macro's `ClientGone` map). The
/// client MUST NOT receive a clean complete 200+FIN carrying the whole
/// body (the relay was cancelled mid-stream).
#[tokio::test]
async fn h3h3_e2e_client_stop_sending_response_maps_client_gone() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    // A multi-MiB response so the relay is genuinely in-flight across
    // many ticks when the client STOP_SENDINGs after the first 64 KiB.
    let body = binary_body(4 * 1024 * 1024);
    let (backend, bh) = spawn_h3_upstream(
        &certs,
        UpstreamMode::LargeResp(Arc::new(body.clone())),
        seen,
    )
    .await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/client-gone",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: Some(64 * 1024),
        },
        // Bounded settle window: long enough for the STOP_SENDING to
        // land and the gateway to attempt another relay send (→
        // ClientGone), short enough to keep the test snappy.
        Duration::from_secs(10),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    // The client cancelled mid-stream: it MUST NOT have received a
    // clean complete 200+FIN carrying the whole 4 MiB body.
    assert!(
        !(out.fin && out.body.len() >= body.len()),
        "a client STOP_SENDING mid-response must NOT yield a clean \
         complete delivery of the whole body (fin={} body_len={} of {})",
        out.fin,
        out.body.len(),
        body.len()
    );
}

/// Cluster 4 / CASE 13 — the H3 client omits the `:authority`
/// pseudo-header entirely. The gateway's `req.authority` is then empty,
/// so `h3_to_h3_stream_resp` substitutes the configured SNI for the
/// upstream request `:authority` (h3_bridge.rs :2876-2877 — the
/// `if req.authority.is_empty()` TRUE branch). The request must still
/// succeed end-to-end: 200, clean FIN, body byte-identical at the
/// backend (an absent authority is a legitimate handled case here).
#[tokio::test]
async fn h3h3_e2e_absent_authority_substitutes_sni() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::Echo, seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let payload = binary_body(40 * 1024);
    let chunks: Vec<Vec<u8>> = payload.chunks(16 * 1024).map(<[u8]>::to_vec).collect();

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/no-authority",
            req_body: chunks,
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: true,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(25),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let got = seen.body.lock().unwrap().clone();
    bh.abort();

    assert_eq!(
        out.status,
        Some(200),
        "an absent :authority must still succeed (SNI substituted \
         upstream); status={:?}",
        out.status
    );
    assert!(out.fin, "clean FIN expected for the absent-authority case");
    assert!(
        got == payload,
        "request body byte-identical at the backend even with the \
         SNI-substituted authority"
    );
    assert_eq!(out.body, payload, "echoed response body byte-identical");
}

/// Cluster 4 / CASE 14 — the upstream emits a ZERO-LENGTH DATA frame
/// (`0x00 0x00`) BEFORE the real body, then the body + a clean FIN.
/// Drives the gateway's empty-DATA fast-path (h3_bridge.rs :3214-3217:
/// `RecvState::InData { remaining: 0 }` returns straight to
/// `AwaitingHeader` with no spurious chunk). The gateway MUST skip the
/// zero-length frame and still deliver the real body byte-identical
/// with a clean FIN (non-vacuous: the empty frame was handled, not
/// mis-parsed, and the body resumed correctly).
#[tokio::test]
async fn h3h3_e2e_empty_data_frame_skipped_then_body() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::EmptyDataThenResp, seen).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/empty-data",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(25),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    assert_eq!(
        out.status,
        Some(200),
        "200 expected past the zero-length DATA frame"
    );
    assert!(
        out.fin,
        "clean FIN expected — the zero-length DATA frame was handled \
         and parsing resumed"
    );
    assert_eq!(
        out.body, b"h3-empty-data-body",
        "real body byte-identical after a skipped zero-length DATA frame"
    );
}

/// Cluster 4 / CASE 15 — the upstream sends 200, then a DATA frame
/// header declaring 4096 bytes but only 16 bytes of payload, then a
/// clean QUIC FIN. The gateway sees the upstream stream end while still
/// mid-`InData` (`remaining > 0`), i.e. NOT between frames, so it is a
/// premature EOF (h3_bridge.rs :3375-3383: the `else { outcome =
/// PrematureEof }` branch — never FIN a partial body, the response-
/// splitting / smuggling guard). The client MUST NOT receive a clean
/// complete 200+FIN.
#[tokio::test]
async fn h3h3_e2e_upstream_premature_eof_mid_data_no_clean_fin() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::HeadThenTruncatedData, seen).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/premature-eof",
            req_body: vec![],
            req_trailers: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
            omit_authority: false,
            stop_reading_resp_after: None,
        },
        Duration::from_secs(25),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    // An upstream FIN mid-DATA (declared 4096, only 16 sent) must NEVER
    // be presented to the client as a clean complete 200+FIN.
    assert!(
        !(out.status == Some(200) && out.fin),
        "a premature upstream EOF mid-DATA must NOT yield a clean \
         complete 200+FIN (status={:?} fin={} body_len={})",
        out.status,
        out.fin,
        out.body.len()
    );
}
