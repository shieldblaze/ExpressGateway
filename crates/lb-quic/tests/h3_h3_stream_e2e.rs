//! H3 → H3 streaming suite on the real wire: a quiche H3 client → the production
//! `QuicListener` → `conn_actor::poll_h3` → `h3_to_h3_stream_resp` → a real
//! `quiche::accept` H3 upstream. Only the public listener + pool surface is driven.

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

use lb_h3_testcodec::{H3Frame, QpackEncoder, decode_frame, encode_frame};

const TEST_SNI: &str = "expressgateway.test";
const H3_ALPN: &[u8] = b"h3";

/// The migrated egress Huffman-encodes values; `lb_h3_testcodec::QpackDecoder` is raw-only.
#[allow(dead_code)]
fn decode_resp_qpack(header_block: &[u8]) -> Result<Vec<(String, String)>, String> {
    use quiche::h3::NameValue;
    let hdrs = quiche::h3::qpack::Decoder::new()
        .decode(header_block, u64::MAX)
        .map_err(|e| format!("qpack decode: {e:?}"))?;
    Ok(hdrs
        .iter()
        .map(|h| {
            (
                String::from_utf8_lossy(h.name()).into_owned(),
                String::from_utf8_lossy(h.value()).into_owned(),
            )
        })
        .collect())
}
const H3_ALPN_PROTOS: &[&[u8]] = &[b"h3", b"h3-29"];
const MAX_UDP: usize = 65_535;

/// The §1.5 C5 sound ceiling; the test ceiling must equal the gauge bound.
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

/// Front H3 CLIENT config: generous conn-level data so the per-stream window governs.
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

/// Upstream SERVER config: windows generous enough that the memory/backpressure proofs
/// stall on the GATEWAY's bounded channel, not on the upstream's own QUIC window.
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

/// The pool's per-dial CLIENT config (proxy → upstream leg); generous for the same reason.
fn upstream_pool_config_factory()
-> Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> {
    Arc::new(|| {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
        cfg.set_application_protos(H3_ALPN_PROTOS)?;
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

/// Deterministic pseudo-random NON-UTF-8 body of `n` bytes.
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

#[derive(Default)]
struct ClientOut {
    status: Option<u16>,
    body: Vec<u8>,
    content_length: Option<usize>,
    fin: bool,
    reset: bool,
    /// Response-stream HEADERS frames decoded: `1` = head only, `2` = head PLUS a trailer.
    resp_headers_frames: usize,
    resp_trailer_names: Vec<String>,
    /// Response-head `(name, value)` pairs except `:status` (the CF-H3H3-HEAD round-trip).
    resp_head_pairs: Vec<(String, String)>,
}

/// Drive ONE H3 request on stream 0; non-empty `req_trailers` ride as a post-DATA
/// trailing-HEADERS frame so the trailers-dropped parity can be asserted at the backend.
#[allow(clippy::struct_excessive_bools)]
struct DriveCfg {
    method: &'static str,
    path: &'static str,
    req_body: Vec<Vec<u8>>,
    req_trailers: Vec<(String, String)>,
    stall_after: Option<usize>,
    stall_for: Duration,
    reset_after_req_bytes: Option<usize>,
    omit_authority: bool,
    /// After `k` response body bytes, STOP_SENDING the response stream: quiche surfaces
    /// `Err(StreamStopped)`, the actor reaps the receiver, the bridge then sees `ClientGone`.
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
                        let is_trailer = out.resp_headers_frames > 1;
                        if let Ok(h) = decode_resp_qpack(&header_block) {
                            for (n, v) in h {
                                if is_trailer {
                                    out.resp_trailer_names.push(n.clone());
                                }
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
            if let Some(k) = cfg.stop_reading_resp_after {
                if !resp_stop_sent && !out.fin && out.body.len() >= k {
                    let _ = conn.stream_shutdown(sid, quiche::Shutdown::Read, 0x010c);
                    resp_stop_sent = true;
                }
            }
        }

        // CASE 12 stops reading, so there is no clean FIN to wait for: hold the connection
        // open a bounded settle window so the STOP_SENDING is actually delivered.
        if resp_stop_sent {
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

#[derive(Clone, Default)]
struct BackendSeen {
    body: Arc<Mutex<Vec<u8>>>,
    /// True iff the request stream ended with a clean FIN — the smuggling signal.
    complete: Arc<AtomicBool>,
    /// Request HEADERS frames: 1 = no trailers forwarded, 2 = a trailer section WAS.
    headers_frames: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
}

#[derive(Clone)]
enum UpstreamMode {
    /// 200 + the request body echoed back (bodyless ⇒ a fixed sentinel body).
    Echo,
    LargeResp(Arc<Vec<u8>>),
    StallReadThenEcho(Duration),
    /// 200 + a content-length far larger than the bytes written, then RESET mid-body.
    ResetMidResponse,
    /// After `after_bytes` of request DATA, a real STOP_SENDING. `after_bytes == 0` arms it
    /// the instant the stream is observed, so the HEADERS / first DATA write races it.
    StopSendingMidRequest {
        after_bytes: usize,
    },
    RespWithTrailers(TrailerKind),
    UnknownFrameThenResp,
    /// 200 then a frame header declaring over the 1 MiB cap with NO payload: the gateway must
    /// abort on the DECLARED length. `frame_type 0x01` drives the HEADERS arm, else unknown.
    OversizedBlock {
        frame_type: u64,
    },
    EmptyDataThenResp,
    /// 200 (no content-length), a DATA header declaring more than it writes, then a clean
    /// FIN — the documented quiche §7.1 frame-completeness gap (see CASE 15).
    HeadThenTruncatedData,
    /// As above but WITH `content-length: 4096`, so the truncation guard must catch it.
    HeadCLThenTruncatedData,
    /// 200 carrying REGULAR headers alongside `content-length` — pre-fix the gateway
    /// forwarded only `:status` + `content-length` (CF-H3H3-HEAD).
    RespWithHeaders,
}

#[derive(Clone)]
enum TrailerKind {
    /// One ordinary trailer ⇒ the FORWARD path. Response trailers ARE forwarded, unlike
    /// REQUEST trailers, which are dropped.
    Valid,
    /// A `:`-prefixed trailer name ⇒ the malformed-trailer reject arm (RFC 9114 §4.3).
    PseudoHeader,
}

/// One accepted connection per pooled dial: the gateway dials fresh per request and marks
/// the connection non-reusable.
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
        loop {
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
            let _ = conn.recv(
                in_buf.get_mut(..n).unwrap_or(&mut []),
                quiche::RecvInfo { from, to: local },
            );

            let seen = seen.clone();
            let mode = mode.clone();
            let req_sid: u64 = 0;
            let mut rx_tail: Vec<u8> = Vec::new();
            let mut body: Vec<u8> = Vec::new();
            let mut headers_frames = 0usize;
            let mut req_fin = false;
            let mut response_started = false;
            let mut req_stop_sent = false;
            let mut stall_until: Option<tokio::time::Instant> = None;
            // Partial-accept retry: a bare `stream_send` ignoring the returned count loses bytes.
            let mut resp_wire: Vec<u8> = Vec::new();
            let mut resp_off = 0usize;
            let mut resp_built = false;
            let mut resp_fin_on_drain = true;
            let mut resp_reset_after_drain = false;
            let mut resp_done = false;
            let conn_deadline = tokio::time::Instant::now() + Duration::from_secs(120);

            while tokio::time::Instant::now() < conn_deadline {
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

                if conn.is_established() {
                    // Computed BEFORE the drain so the drain is gated on it: during the stall
                    // the upstream calls neither `readable()` nor `stream_recv`, so quiche stops
                    // extending the window and the gateway's `stream_capacity` gate genuinely
                    // fires.
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

                // `after_bytes == 0` arms this the instant the stream is observed, so the
                // HEADERS / first request-DATA write races the STOP_SENDING.
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

                if conn.is_established()
                    && !response_started
                    && (req_fin || matches!(mode, UpstreamMode::LargeResp(_)))
                {
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
                        // The gateway aborts without FIN on the STOP_SENDING, so this
                        // upstream never responds.
                        UpstreamMode::StopSendingMidRequest { .. } => false,
                        // Respond on the clean request FIN: the adversarial behaviour is on
                        // the RESPONSE leg.
                        UpstreamMode::RespWithTrailers(_)
                        | UpstreamMode::UnknownFrameThenResp
                        | UpstreamMode::OversizedBlock { .. }
                        | UpstreamMode::EmptyDataThenResp
                        | UpstreamMode::HeadThenTruncatedData
                        | UpstreamMode::HeadCLThenTruncatedData
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
                                resp_wire = response_head(200, Some(1_048_576));
                                resp_wire.extend_from_slice(&data_frames(&vec![7u8; 64 * 1024]));
                                resp_fin_on_drain = false;
                                resp_reset_after_drain = true;
                            }
                            // Never reached: `ready` is always false for this mode.
                            UpstreamMode::StopSendingMidRequest { .. } => {}
                            UpstreamMode::RespWithTrailers(kind) => {
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&data_frames(b"h3-trail-body"));
                                let tf = match kind {
                                    TrailerKind::Valid => {
                                        trailers_frame(&[("x-resp-trailer", "v1")])
                                    }
                                    TrailerKind::PseudoHeader => {
                                        // A `:`-prefixed trailer field is malformed
                                        // (RFC 9114 §4.3).
                                        trailers_frame(&[(":illegal", "x")])
                                    }
                                };
                                resp_wire.extend_from_slice(&tf);
                                resp_fin_on_drain = true;
                            }
                            UpstreamMode::UnknownFrameThenResp => {
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&unknown_frame(
                                    0x21,
                                    b"reserved-frame-payload-skip-me",
                                ));
                                resp_wire.extend_from_slice(&data_frames(b"h3-skip-body"));
                                resp_fin_on_drain = true;
                            }
                            // The gateway aborts on the DECLARED length, so FIN after the
                            // tiny prefix — it has already aborted by then.
                            UpstreamMode::OversizedBlock { frame_type } => {
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&oversized_block_header(
                                    *frame_type,
                                    (1024 * 1024 + 1) as u64,
                                ));
                                resp_fin_on_drain = true;
                            }
                            UpstreamMode::EmptyDataThenResp => {
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&empty_data_frame());
                                resp_wire.extend_from_slice(&data_frames(b"h3-empty-data-body"));
                                resp_fin_on_drain = true;
                            }
                            UpstreamMode::HeadThenTruncatedData => {
                                resp_wire = response_head(200, None);
                                resp_wire.extend_from_slice(&truncated_data_frame(4096, 16));
                                resp_fin_on_drain = true;
                            }
                            UpstreamMode::HeadCLThenTruncatedData => {
                                resp_wire = response_head(200, Some(4096));
                                resp_wire.extend_from_slice(&truncated_data_frame(4096, 16));
                                resp_fin_on_drain = true;
                            }
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
                    // Partial-accept retry; FIN only when fully drained — never lose bytes.
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
        }
    });
    (addr, h)
}

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

/// Wire-identical to a normal HEADERS frame — the gateway distinguishes a trailer purely
/// POSITIONALLY (it arrives after the head).
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

/// An UNKNOWN H3 frame (RFC 9114 §7.2.8 reserved type) the gateway MUST skip and resume after.
fn unknown_frame(frame_type: u64, payload: &[u8]) -> Vec<u8> {
    encode_frame(&H3Frame::Unknown {
        frame_type,
        payload: Bytes::copy_from_slice(payload),
    })
    .unwrap()
    .to_vec()
}

/// `varint(type) || varint(len)` with NO payload, `len` over the 1 MiB cap. `frame_type 0x01`
/// exercises the HEADERS over-cap arm; any other type the unknown-frame arm.
fn oversized_block_header(frame_type: u64, declared_len: u64) -> Vec<u8> {
    use bytes::BytesMut;
    let mut buf = BytesMut::new();
    lb_h3_testcodec::encode_varint(&mut buf, frame_type).unwrap();
    lb_h3_testcodec::encode_varint(&mut buf, declared_len).unwrap();
    buf.to_vec()
}

fn empty_data_frame() -> Vec<u8> {
    use bytes::BytesMut;
    let mut buf = BytesMut::new();
    lb_h3_testcodec::encode_varint(&mut buf, 0x00).unwrap(); // FRAME_DATA
    lb_h3_testcodec::encode_varint(&mut buf, 0).unwrap(); // length 0
    buf.to_vec()
}

/// A DATA header declaring `declared_len` with only `actual_len` real bytes, so a subsequent
/// clean FIN lands mid-frame.
fn truncated_data_frame(declared_len: u64, actual_len: usize) -> Vec<u8> {
    use bytes::BytesMut;
    let mut buf = BytesMut::new();
    lb_h3_testcodec::encode_varint(&mut buf, 0x00).unwrap(); // FRAME_DATA
    lb_h3_testcodec::encode_varint(&mut buf, declared_len).unwrap();
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

/// Case 1 — liveness floor: bodyless GET, 200 + sentinel body, clean FIN.
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

/// Case 2 (BINDING cond 1) — a ≥1 MiB NON-UTF-8 request body arrives byte-identical at the
/// real H3 backend AND request trailers are DROPPED (backend sees exactly ONE HEADERS frame).
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
    // Trailers-dropped parity: the client sent a trailing-HEADERS frame, so the backend
    // must observe EXACTLY ONE.
    assert_eq!(
        hframes, 1,
        "request trailers MUST be dropped on the H3→H3 leg (backend \
         saw {hframes} HEADERS frames; expected exactly 1 — the head)"
    );
}

/// Case 3 (BINDING cond 2, response direction) — a 4 MiB response through a STALLED client
/// keeps `MAX_RETAINED_RESP_BYTES` ≤ the C5 ceiling, and resumes byte-identical (non-vacuous).
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

/// Case 4 (BINDING cond 2, request direction) — a 4 MiB request body against a STALLED backend
/// keeps `MAX_RETAINED_BODY_BYTES` ≤ the ceiling, and arrives byte-identical.
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

/// Case 5 (BINDING cond 2, backpressure) — a stalled client must pause the upstream read:
/// retained ≤ ceiling for a body ≫ ceiling, and the body still completes byte-identical.
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

/// Case 6 — a backend RESET mid response body must NEVER reach the client as a clean
/// complete 200 (response-splitting guard).
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

/// Case 6 R13 (b)+(c) for the F-MD-4 MIRROR: (b) a ≥50-iteration burst on a SINGLE-THREADED
/// runtime, the configuration that exposes quiche's timing-dependent `Finished`-vs-`Reset`
/// delivery for a RESET after the last DATA; (c) a clean GET control so (b) is non-vacuous.
///
/// VERIFIER mutation: flip the `Finished` arm's `was_reset` to always-false — this burst must FAIL.
#[tokio::test(flavor = "current_thread")]
async fn h3h3_e2e_upstream_reset_midresponse_burst_current_thread() {
    const ITERS: usize = 60; // ≥50 per R13 (b)
    let certs = generate_loopback_certs();

    // (c) NON-VACUITY control — a clean GET MUST yield a clean complete 200+FIN.
    {
        let seen = BackendSeen::default();
        let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::Echo, seen).await;
        let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;
        let clean = drive_h3(
            gw,
            &certs.ca,
            DriveCfg {
                method: "GET",
                path: "/clean",
                req_body: vec![],
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
        let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
        sd.cancel();
        bh.abort();
        assert!(
            clean.status == Some(200) && clean.fin,
            "non-vacuity control: a clean GET (Echo backend) must yield 200+FIN \
             (status={:?} fin={})",
            clean.status,
            clean.fin
        );
    }

    // (b) BURST — none may yield a clean 200+FIN. Bounded concurrency on the single-threaded
    // runtime contends concurrent resets on ONE scheduler.
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::ResetMidResponse, seen).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;
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
                set.spawn_local(async move {
                    drive_h3(
                        gw,
                        &ca,
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
                    "F-MD-4 MIRROR (burst iter {finished}/{ITERS}): a mid/after-body \
                     backend RESET yielded a clean 200+FIN to the client \
                     (status={:?} fin={} body_len={})",
                    out.status,
                    out.fin,
                    out.body.len()
                );
                if launched < ITERS {
                    spawn_one(&mut set);
                    launched += 1;
                }
            }
        })
        .await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();
    eprintln!("H3H3_FMD4_MIRROR_BURST iters={ITERS} (all reset, none clean-complete)");
}

/// Case 7 (BINDING — request-side smuggling parity) — a client RESET MID request body aborts
/// without FIN, so the backend NEVER observes the request as cleanly ended.
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

/// Case 7 R13 (b)+(c) — a ≥50-iteration burst on a SINGLE-THREADED runtime (the configuration
/// that exposes timing-dependent smuggling races) plus a clean-POST non-vacuity control.
///
/// SIGNAL: `BackendSeen::requests` increments ONLY on a cleanly-ended (FIN) request, so a
/// smuggled truncated request would move it. The control moves it 0→1; the burst must leave 1.
///
/// VERIFIER mutation: H3→H3 has no pre-fix bug to revert — flip the connector's request-abort
/// arm to a clean FIN and confirm this burst then FAILS.
#[tokio::test(flavor = "current_thread")]
async fn h3h3_e2e_client_reset_midrequest_burst_current_thread() {
    const ITERS: usize = 60; // ≥50 per R13 (b)
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::Echo, seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let payload = binary_body(128 * 1024);
    let chunks: Vec<Vec<u8>> = payload.chunks(16 * 1024).map(<[u8]>::to_vec).collect();

    // (c) NON-VACUITY control — a clean POST MUST be counted as cleanly ended.
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

    // (b) BURST — bounded concurrency on the single-threaded runtime is a STRONGER
    // smuggling-race probe than back-to-back sequential requests.
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

/// CASE 8 — the upstream STOP_SENDINGs the REQUEST stream after ~64 KiB of request DATA, so
/// the next request-DATA send returns `Err(StreamStopped)`: no clean 200+FIN, no clean request.
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

/// CASE 8b — the STOP_SENDING is armed as early as possible, so the fault lands on the HEADERS
/// / first request-DATA write instead. Either way: no clean 200+FIN.
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

/// CASE 8c — the STOP_SENDING lands AFTER the request body is drained, so the fault hits the
/// terminal request-FIN write. The binding assertion holds for EITHER landing point.
#[tokio::test]
async fn h3h3_e2e_upstream_stop_sending_at_request_fin_aborts_no_fin() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
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

/// CF-H3H3-HEAD — the response leg MUST forward the FULL non-pseudo header set. LOAD-BEARING:
/// this FAILS on the old lossy `:status`-plus-content-length projection.
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
    assert_eq!(
        out.content_length,
        Some(b"h3-full-head-body".len()),
        "content-length must still be forwarded"
    );
}

/// CASE 9a — a VALID post-DATA trailing-HEADERS frame IS forwarded (unlike REQUEST trailers),
/// so the client must see a SECOND HEADERS frame.
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

/// CASE 9b — a trailer carrying a `:`-prefixed pseudo-header MUST NEVER be forwarded, and the
/// client MUST NOT get a clean complete 200+FIN.
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

/// CASE 10a — an UNKNOWN frame before the DATA body must be skipped transparently, the body
/// still arriving byte-identical with a clean FIN (so the skip demonstrably resumed correctly).
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

/// CASE 10b — a HEADERS-typed frame header over the 1 MiB cap with NO payload ⇒ the HEADERS
/// over-cap arm. No clean complete 200+FIN.
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

/// CASE 10c — as 10b but on an UNKNOWN frame type, driving the OTHER over-cap call site.
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

/// CASE 11 — nothing is listening at the backend address, so `pool.acquire` fails at the dial
/// deadline. The client MUST receive an inline 502 with a clean FIN, not a dropped or hung stream.
#[tokio::test]
async fn h3h3_e2e_pool_acquire_failure_returns_502() {
    let certs = generate_loopback_certs();
    // Bind to grab a free port, capture it, then drop the socket: the pooled dial never gets a
    // handshake response, so `acquire` fails deterministically at the dial deadline.
    let dead_backend = {
        let s =
            std::net::UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
                .unwrap();
        s.local_addr().unwrap()
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

/// CASE 12 — the client STOP_SENDINGs the RESPONSE stream mid-body, so the actor reaps the
/// receiver and the bridge's next send returns `ClientGone`. No clean complete 200+FIN.
#[tokio::test]
async fn h3h3_e2e_client_stop_sending_response_maps_client_gone() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
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
        Duration::from_secs(10),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    assert!(
        !(out.fin && out.body.len() >= body.len()),
        "a client STOP_SENDING mid-response must NOT yield a clean \
         complete delivery of the whole body (fin={} body_len={} of {})",
        out.fin,
        out.body.len(),
        body.len()
    );
}

/// CASE 13 — a request omitting both `:authority` and `Host` is malformed for `https`
/// (RFC 9114 §4.3.1 makes one MANDATORY): reset with `H3_MESSAGE_ERROR`, forward NOTHING.
///
/// Previously a "succeeds via SNI substitution" case, ruled STRICT. The upstream SNI fallback
/// remains for H1→H3 / H2→H3, which build the upstream request from a different ingress.
#[tokio::test]
async fn h3h3_e2e_absent_authority_rejected_message_error() {
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

    assert_ne!(
        out.status,
        Some(200),
        "absent :authority (https) is malformed — must NOT yield 200"
    );
    assert!(
        out.status.is_none(),
        "no :status expected (the gateway resets the stream, not responds); got {:?}",
        out.status
    );
    assert!(
        out.reset || !out.fin,
        "the request stream must be reset (H3_MESSAGE_ERROR), not cleanly completed (reset={}, fin={})",
        out.reset,
        out.fin
    );
    assert!(
        got.is_empty(),
        "a malformed request must NOT be forwarded upstream — backend saw {} body bytes",
        got.len()
    );
}

/// Drive a RAW hand-crafted request stream and return the gateway-initiated close as
/// `(error_code, is_app)`. With `open_control` the client also opens a uni control stream, so
/// the uni-stream drain gate is exercised — the request decoder must NOT trip on it.
async fn drive_raw_request_close(
    gateway: SocketAddr,
    ca: &std::path::Path,
    request_on_stream0: &[u8],
    open_control: bool,
    overall: Duration,
) -> Option<(u64, bool)> {
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
    let mut sent = false;
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
        if let Some(e) = conn.peer_error() {
            return Some((e.error_code, e.is_app));
        }
        if conn.is_closed() {
            break;
        }
        if conn.is_established() && !sent {
            if open_control {
                // client-initiated uni control stream (id 2): stream-type 0x00, then SETTINGS.
                let mut ctrl = vec![0x00u8];
                ctrl.extend_from_slice(
                    &encode_frame(&H3Frame::Settings { params: vec![] }).unwrap(),
                );
                let _ = conn.stream_send(2, &ctrl, false);
            }
            let _ = conn.stream_send(0, request_on_stream0, true);
            sent = true;
        }
        if let Ok(Ok((n, from))) =
            tokio::time::timeout(Duration::from_millis(100), sock.recv_from(&mut in_buf)).await
        {
            let info = quiche::RecvInfo { from, to: local };
            let _ = conn.recv(&mut in_buf[..n], info);
        }
    }
    conn.peer_error().map(|e| (e.error_code, e.is_app))
}

/// h3spec #11 — DATA before HEADERS on a request stream is a CONNECTION error
/// `H3_FRAME_UNEXPECTED`, an APPLICATION close.
#[tokio::test]
async fn h3h3_e2e_data_before_headers_closes_h3_frame_unexpected() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::Echo, seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let data_first = encode_frame(&H3Frame::Data {
        payload: Bytes::from_static(b"early"),
    })
    .unwrap();
    let close =
        drive_raw_request_close(gw, &certs.ca, &data_first, true, Duration::from_secs(10)).await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();
    assert_eq!(
        close,
        Some((0x0105, true)),
        "DATA-before-HEADERS must close with H3_FRAME_UNEXPECTED (0x0105) as an APPLICATION close"
    );
}

/// h3spec #22 — a field section decoding to an invalid static-table index is a CONNECTION
/// error `QPACK_DECOMPRESSION_FAILED`, an APPLICATION close.
#[tokio::test]
async fn h3h3_e2e_invalid_qpack_static_index_closes_decompression_failed() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::Echo, seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    // QPACK block indexing static entry 200 (invalid): prefix 00 00, then 0xFF (0xC0|0x3F)
    // plus varint continuation 0x89 0x01.
    let bad = encode_frame(&H3Frame::Headers {
        header_block: Bytes::from(vec![0x00, 0x00, 0xFF, 0x89, 0x01]),
    })
    .unwrap();
    let close = drive_raw_request_close(gw, &certs.ca, &bad, false, Duration::from_secs(10)).await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();
    assert_eq!(
        close,
        Some((0x0200, true)),
        "invalid QPACK static index must close with QPACK_DECOMPRESSION_FAILED (0x0200) as an APPLICATION close"
    );
}

/// CASE 14 — a ZERO-LENGTH DATA frame before the real body must be skipped, the body still
/// arriving byte-identical.
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

/// CASE 15 — the documented quiche §7.1 gap, RE-SCOPED at the `quiche::h3` migration (an
/// owner-ruled documented behaviour change, NOT a silent weakening): with NO content-length to
/// cross-check, and no quiche API to observe a mid-frame finish, a truncated DATA frame followed
/// by a clean FIN relays as complete.
///
/// LOW severity (owner-assessed): needs a malformed BACKEND, not an untrusted client; H3 streams
/// are independent so there is no cross-stream desync; RESET-based truncation is still caught by
/// the F-MD-4 mirror; a content-length-bearing truncation IS caught by the guard below.
/// CF-QUICHE-FRAME-COMPLETENESS: RE-TIGHTEN this assertion to `!(200 && fin)` once quiche
/// enforces §7.1.
#[tokio::test]
async fn h3h3_e2e_no_cl_truncated_data_delivered_quiche_028_frame_completeness_gap() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h3_upstream(&certs, UpstreamMode::HeadThenTruncatedData, seen).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/no-cl-truncated",
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

    // The documented gap: with NO content-length the truncated body + clean FIN are relayed.
    // RE-TIGHTEN to `!(200 && fin)` once quiche enforces §7.1.
    assert_eq!(
        out.status,
        Some(200),
        "head relayed before the truncated body"
    );
    assert!(
        out.fin,
        "quiche-0.28 §7.1 gap (no content-length): the truncated frame's clean \
         FIN is relayed as a clean complete response (documented residual)"
    );
    assert!(
        out.body.len() <= 16,
        "only the 16 truncated body bytes (declared 4096) were available; \
         got body_len={}",
        out.body.len()
    );
}

/// The content-length TRUNCATION GUARD, which the owner ruled MUST be verified to actually
/// fire: quiche delivers a clean `Finished`, but the gateway cross-checks `body_relayed <
/// content-length` and RESETs downstream. This is the compensation for the gap above.
#[tokio::test]
async fn h3h3_e2e_content_length_truncation_resets_no_clean_complete() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) =
        spawn_h3_upstream(&certs, UpstreamMode::HeadCLThenTruncatedData, seen).await;
    let (listener, gw, sd) = start_h3_listener_h3(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/cl-truncated",
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

    // Load-bearing: remove the guard and quiche's clean `Finished` delivers 200 + 16 bytes + FIN.
    assert!(
        !(out.status == Some(200) && out.fin),
        "content-length under-run (declared 4096, sent 16, clean FIN) MUST NOT \
         yield a clean complete 200+FIN — the truncation guard did not fire \
         (status={:?} fin={} body_len={})",
        out.status,
        out.fin,
        out.body.len()
    );
}
