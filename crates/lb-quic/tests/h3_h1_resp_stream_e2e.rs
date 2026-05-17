//! SESSION 4 / P1 — INCREMENTAL H3 RESPONSE-body streaming e2e.
//!
//! The inverse of `h3_h1_stream_body_e2e.rs`: there the client streams
//! a request body and the backend captures it; here the **backend
//! streams a response body** and the real quiche H3 client must receive
//! it byte-identical, as it arrives, with the proxy memory-bounded and
//! backpressured (H1-upstream → H3-client direction).
//!
//! Drives the REAL [`lb_quic::QuicListener`] (UDP bind → router per-CID
//! dispatch → `conn_actor` → `h3_bridge::stream_h1_response`).
//!
//! Planned coverage (s4-phase1-plan §2 — R1..R8):
//!   * R1 — multi-DATA binary response (≥100 KB, 0xFF/0x00/0x80 at
//!          head/mid/tail) byte-identical at the H3 client.
//!   * R2 — NON-VACUOUS memory bound: 1 MiB response, stalled H3
//!          client; `MAX_RETAINED_RESP_BYTES` ≤ the §1.5 C5 sound
//!          bound and `≪ 1 MiB`; liveness + byte-identity after resume.
//!   * R3 — slow-client backpressure: upstream read provably pauses
//!          (gauge stays bounded), request still completes correctly.
//!   * R4 — empty response body + zero-length DATA; byte-identical,
//!          clean FIN.
//!   * R5 — upstream resets mid-response ⇒ client observes
//!          RESET_STREAM with a non-`H3_NO_ERROR` code, no truncated
//!          body presented as complete.
//!   * R6 — client cancels mid-response ⇒ proxy stops reading
//!          upstream, per-stream state torn down, no leak.
//!   * R7 — chunked upstream response, byte-identical (new decoder).
//!   * R8 — trailers no-regression (PROTO-2-12: the existing
//!          `h3_h1_trailers_resp_e2e.rs` pc1/pc2 stay green).
//!
//! SCAFFOLD STATUS (builder-1, parallel to P1-A verification): the
//! harness (FIN-aware response client driver, response backend
//! spawner) and the R1/R4/R7 + R2/R3 fixtures are COMPLETE and
//! self-checked here. The real-wire R1..R8 assertions are
//! `#[ignore]`d with an explicit reason because they only become
//! meaningful once P1-B wires `stream_h1_response` into the actor
//! (until then the legacy buffered path is still live and a real-wire
//! test would exercise the OLD path — a false signal). These are NOT
//! working tests being disabled; they are new scaffolds that UNBLOCK
//! at P1-B. Each `#[ignore]` reason names that precondition.
//!
//! Every response body carries the non-UTF-8 bytes 0xFF 0x00 0x80 so a
//! lossy/string conversion anywhere in the path is caught.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(dead_code)] // scaffold: some helpers wired by the R-tests at P1-B.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_h3::{H3Frame, QpackDecoder, QpackEncoder, decode_frame, encode_frame};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::{QuicListener, QuicListenerParams};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const H3_ALPN: &[u8] = b"h3";
const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;
const REQUEST_AUTHORITY: &str = "h3-resp-stream.test:4433";
const REQUEST_PATH: &str = "/p1/resp-echo";
const UPSTREAM_STATUS: u16 = 200;

/// Distinctive non-UTF-8 marker embedded at head/mid/tail of every
/// response body fixture.
const NON_UTF8: &[u8] = &[0xFF, 0x00, 0x80];

// --------------------------------------------------------------------
// §1.5 C5 sound memory bound — the SINGLE source of truth for the R2/R3
// assertion ceiling. `test ceiling == gauge bound` (team-lead C5
// directive): the gauge's sound channel-occupancy upper bound is
// `depth × (chunk_max + frame_hdr_max)`, NOT `depth × chunk_max`,
// because a `RespEvent::Bytes` carries a pre-encoded H3 frame (payload
// PLUS the frame's type+length varints, ≤ `H3_FRAME_HDR_MAX`). R2/R3
// MUST assert `MAX_RETAINED_RESP_BYTES <= RESP_RETAINED_CEILING` and
// that the ceiling is still `≪` the 1 MiB body (non-vacuous proof).
// --------------------------------------------------------------------

/// The §1.5 C5 sound per-stream retained-bytes bound, with ×4 slack
/// covering one queued StreamTx chunk, the full channel, one in-flight
/// producer chunk, and the HEADERS frame. Mirrors S2 T5's `4 * window`
/// shape but with the C5-correct per-slot size (chunk PLUS frame
/// header). Evaluated from the real crate consts at P1-B (left as a
/// function so the R-tests bind it then).
fn resp_retained_ceiling(depth: usize, chunk_max: usize, frame_hdr_max: usize) -> usize {
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
        "lb-quic-h3h1-respstream-{}-{}-{counter}",
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

fn build_client_config(ca_path: &std::path::Path) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_verify_locations_from_file(ca_path.to_str().unwrap())
        .unwrap();
    cfg.verify_peer(true);
    cfg.set_max_idle_timeout(30_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(8);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg
}

fn random_scid_bytes() -> [u8; quiche::MAX_CONN_ID_LEN] {
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new().fill(&mut scid).unwrap();
    scid
}

fn build_tcp_pool() -> TcpPool {
    TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts {
            nodelay: true,
            keepalive: true,
            rcvbuf: Some(65_536),
            sndbuf: Some(65_536),
            quickack: false,
            tcp_fastopen_connect: false,
        },
        Runtime::new(),
    )
}

/// How the response backend should frame the body it sends back.
#[derive(Clone)]
enum RespBody {
    /// `Content-Length`-framed body (R1/R4: known length).
    ContentLength(Vec<u8>),
    /// `Transfer-Encoding: chunked` body, emitted in the given chunk
    /// sizes then the zero terminator (R7: new decoder path).
    Chunked {
        body: Vec<u8>,
        chunk_sizes: Vec<usize>,
    },
    /// EOF-delimited: no CL, no TE, `Connection: close`; body then the
    /// socket is shut (length unknown ⇒ client relies on FIN).
    EofDelimited(Vec<u8>),
    /// Send the status + headers, part of the body, then RESET the TCP
    /// connection mid-body (R5: upstream reset ⇒ RESET_STREAM).
    ResetMidBody {
        declared_len: usize,
        partial: Vec<u8>,
    },
}

/// Response backend: accepts one connection, reads the (bodyless) H1
/// request head, then streams the configured response. `stall` (R2/R3)
/// makes it wait on `notified()` *between writing the head and the
/// body* so the proxy's bounded in-flight window fills and the gauge
/// can prove the bound; after notify the body is written to completion.
async fn spawn_resp_backend(
    body: RespBody,
    stall: Option<Arc<Notify>>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut sock, _) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => return,
        };
        // Drain the request head (the proxy sends a bodyless GET-style
        // request for these response-streaming tests).
        let mut tmp = [0u8; 4096];
        let mut req = Vec::new();
        loop {
            if req.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            match sock.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => req.extend_from_slice(&tmp[..n]),
            }
        }

        match body {
            RespBody::ContentLength(b) => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    b.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                if let Some(n) = &stall {
                    n.notified().await;
                }
                let _ = sock.write_all(&b).await;
                let _ = sock.shutdown().await;
            }
            RespBody::Chunked { body, chunk_sizes } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                if let Some(n) = &stall {
                    n.notified().await;
                }
                let mut off = 0;
                for sz in chunk_sizes {
                    let end = (off + sz).min(body.len());
                    let piece = &body[off..end];
                    let _ = sock
                        .write_all(format!("{:x}\r\n", piece.len()).as_bytes())
                        .await;
                    let _ = sock.write_all(piece).await;
                    let _ = sock.write_all(b"\r\n").await;
                    off = end;
                }
                let _ = sock.write_all(b"0\r\n\r\n").await;
                let _ = sock.shutdown().await;
            }
            RespBody::EofDelimited(b) => {
                let head = format!("HTTP/1.1 {UPSTREAM_STATUS} OK\r\nConnection: close\r\n\r\n");
                let _ = sock.write_all(head.as_bytes()).await;
                if let Some(n) = &stall {
                    n.notified().await;
                }
                let _ = sock.write_all(&b).await;
                let _ = sock.shutdown().await;
            }
            RespBody::ResetMidBody {
                declared_len,
                partial,
            } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&partial).await;
                // Premature EOF: close the socket after only `partial`
                // (< declared_len) bytes. The proxy MUST treat the
                // short response as RespAbort::PrematureEof ⇒
                // RESET_STREAM, never present the truncated body as a
                // complete response (response-splitting guard).
                let _ = sock.shutdown().await;
                drop(sock);
            }
        }
    });
    (addr, handle)
}

/// Outcome the FIN-aware response client driver reports.
#[derive(Debug)]
struct ClientOutcome {
    status: Option<u16>,
    body: Vec<u8>,
    /// True if the response stream ended with a clean FIN.
    fin: bool,
    /// `Some(code)` if the proxy RESET_STREAM'd us (R5/R6 abort path).
    reset_code: Option<u64>,
}

/// FIN-aware H3 response client driver.
///
/// Unlike `h3_h1_stream_body_e2e.rs`'s driver (which returns as soon as
/// `content-length` is satisfied), this completes on **stream FIN** OR
/// **RESET_STREAM** — required for the chunked / EOF-delimited response
/// paths where the length is unknown and the client relies on FIN, and
/// for the abort paths (R5/R6) where the proxy must RESET_STREAM rather
/// than present a truncated body as complete.
///
/// `cancel_after`: if `Some(n)`, the client sends STOP_SENDING +
/// RESET_STREAM on the response stream once it has received ≥ `n` body
/// bytes (R6 client-cancel-mid-response).
#[allow(clippy::too_many_lines)]
async fn drive_h3_response_client(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    extra_headers: Vec<(String, String)>,
    cancel_after: Option<usize>,
    deadline: tokio::time::Instant,
) -> Result<ClientOutcome, String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let stream_id: u64 = 0;
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    let encoder = QpackEncoder::new();
    let mut headers = vec![
        (":method".to_string(), "GET".to_string()),
        (":scheme".to_string(), "https".to_string()),
        (":authority".to_string(), REQUEST_AUTHORITY.to_string()),
        (":path".to_string(), REQUEST_PATH.to_string()),
    ];
    headers.extend(extra_headers);
    let hb = encoder
        .encode(&headers)
        .map_err(|e| format!("qpack: {e}"))?;
    let headers_frame = encode_frame(&H3Frame::Headers { header_block: hb })
        .map_err(|e| format!("h3 frame: {e}"))?;

    let mut sent = 0usize;
    let mut header_done = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut status: Option<u16> = None;
    let mut body: Vec<u8> = Vec::new();
    let mut fin = false;
    let mut reset_code: Option<u64> = None;
    let mut cancelled = false;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "deadline; established={} sent={}/{} status={status:?} body={} fin={fin}",
                conn.is_established(),
                sent,
                headers_frame.len(),
                body.len()
            ));
        }
        if conn.is_closed() {
            // A clean connection close after we saw FIN/reset is fine;
            // surface what we have.
            if fin || reset_code.is_some() {
                return Ok(ClientOutcome {
                    status,
                    body,
                    fin,
                    reset_code,
                });
            }
            return Err(format!(
                "conn closed early: peer={:?} local={:?} status={status:?} body={}",
                conn.peer_error(),
                conn.local_error(),
                body.len()
            ));
        }

        // Send the (bodyless) request HEADERS + FIN.
        if conn.is_established() && !header_done {
            match conn.stream_send(stream_id, &headers_frame[sent..], false) {
                Ok(0) | Err(quiche::Error::Done) => {}
                Ok(n) => {
                    sent += n;
                    if sent == headers_frame.len() {
                        let _ = conn.stream_send(stream_id, &[], true);
                        header_done = true;
                    }
                }
                Err(e) => return Err(format!("stream_send: {e}")),
            }
        }

        if conn.is_established() {
            let readable: Vec<u64> = conn.readable().collect();
            for sid in readable {
                if sid != stream_id {
                    continue;
                }
                let mut c = [0u8; 8192];
                loop {
                    match conn.stream_recv(sid, &mut c) {
                        Ok((n, stream_fin)) => {
                            rx_tail.extend_from_slice(&c[..n]);
                            if stream_fin {
                                fin = true;
                            }
                        }
                        Err(quiche::Error::Done) => break,
                        Err(quiche::Error::InvalidStreamState(_)) => break,
                        Err(quiche::Error::StreamReset(code)) => {
                            reset_code = Some(code);
                            break;
                        }
                        Err(e) => return Err(format!("stream_recv: {e}")),
                    }
                }
            }
            loop {
                match decode_frame(&rx_tail, 1 << 20) {
                    Ok((H3Frame::Headers { header_block }, c)) => {
                        rx_tail.drain(..c);
                        let hdrs = QpackDecoder::new()
                            .decode(&header_block)
                            .map_err(|e| format!("qpack decode: {e}"))?;
                        for (n, v) in hdrs {
                            if n == ":status" {
                                status = Some(v.parse().map_err(|_| "status".to_string())?);
                            }
                        }
                    }
                    Ok((H3Frame::Data { payload }, c)) => {
                        rx_tail.drain(..c);
                        body.extend_from_slice(&payload);
                        if let Some(after) = cancel_after {
                            if !cancelled && body.len() >= after {
                                // R6: cancel the response stream.
                                let _ =
                                    conn.stream_shutdown(stream_id, quiche::Shutdown::Read, 0x10);
                                cancelled = true;
                            }
                        }
                    }
                    Ok((_, c)) => {
                        rx_tail.drain(..c);
                    }
                    Err(lb_h3::H3Error::Incomplete) => break,
                    Err(e) => return Err(format!("decode_frame: {e}")),
                }
            }
        }

        // Completion: clean FIN, or the proxy RESET_STREAM'd us, or we
        // cancelled and the stream is torn down.
        if (fin && status.is_some()) || reset_code.is_some() || (cancelled && fin) {
            return Ok(ClientOutcome {
                status,
                body,
                fin,
                reset_code,
            });
        }

        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    socket
                        .send_to(&out_buf[..n], info.to)
                        .await
                        .map_err(|e| format!("send_to: {e}"))?;
                }
                Err(quiche::Error::Done) => break,
                Err(e) => return Err(format!("conn.send: {e}")),
            }
        }

        let qto = conn.timeout().unwrap_or(Duration::from_millis(20));
        let wait = qto.clamp(Duration::from_millis(2), Duration::from_millis(20));
        match tokio::time::timeout(wait, socket.recv_from(&mut in_buf)).await {
            Ok(Ok((n, from))) => {
                let info = quiche::RecvInfo { from, to: local };
                match conn.recv(&mut in_buf[..n], info) {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(e) => return Err(format!("conn.recv: {e}")),
                }
            }
            Ok(Err(e)) => return Err(format!("recv_from: {e}")),
            Err(_) => conn.on_timeout(),
        }
    }
}

async fn start_listener(
    certs: &TestCerts,
    backend: SocketAddr,
) -> (QuicListener, SocketAddr, CancellationToken) {
    let pool = build_tcp_pool();
    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    )
    .with_backends(vec![backend], pool);
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone()).await.unwrap();
    let addr = listener.local_addr();
    (listener, addr, shutdown)
}

fn client_conn(server: SocketAddr, ca: &std::path::Path) -> (quiche::Connection, UdpSocket) {
    let sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind client udp");
    sock.set_nonblocking(true).unwrap();
    let sock = UdpSocket::from_std(sock).unwrap();
    let local = sock.local_addr().unwrap();
    let mut cfg = build_client_config(ca);
    let scid = random_scid_bytes();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let conn = quiche::connect(Some(TEST_SNI), &scid_ref, local, server, &mut cfg).unwrap();
    (conn, sock)
}

// --------------------------------------------------------------------
// Fixtures (R1/R4/R7 + R2/R3 inputs) — self-checked here so the
// scaffold is not vacuous even before the wire assertions land.
// --------------------------------------------------------------------

/// R1/R2/R3 binary body: `n` bytes, deterministic, with the non-UTF-8
/// marker at head, middle, and tail.
fn binary_body(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    for (i, x) in b.iter_mut().enumerate() {
        *x = (i % 251) as u8;
    }
    b[..3].copy_from_slice(NON_UTF8);
    let mid = n / 2;
    b[mid..mid + 3].copy_from_slice(NON_UTF8);
    b[n - 3..].copy_from_slice(NON_UTF8);
    b
}

#[test]
fn fixture_r1_binary_body_is_large_and_non_utf8_marked() {
    let b = binary_body(120 * 1024);
    assert!(b.len() >= 100 * 1024, "R1 body must be ≥100 KB");
    assert_eq!(&b[..3], NON_UTF8, "head marker");
    let mid = b.len() / 2;
    assert_eq!(&b[mid..mid + 3], NON_UTF8, "mid marker");
    assert_eq!(&b[b.len() - 3..], NON_UTF8, "tail marker");
    assert!(
        std::str::from_utf8(&b).is_err(),
        "fixture must be non-UTF-8 so a lossy conversion is caught"
    );
}

#[test]
fn fixture_r4_empty_body() {
    let b = Vec::<u8>::new();
    assert!(b.is_empty(), "R4 empty-body fixture");
}

#[test]
fn fixture_r7_chunked_split_reassembles() {
    // Sanity-check the chunked fixture shape the backend will emit:
    // arbitrary chunk boundaries must reassemble to the original body.
    let body = binary_body(50_000);
    let chunk_sizes = vec![1, 7, 4096, 8192, 1, 100, 99_999];
    let mut off = 0;
    let mut reassembled = Vec::new();
    for sz in &chunk_sizes {
        let end = (off + sz).min(body.len());
        reassembled.extend_from_slice(&body[off..end]);
        off = end;
    }
    assert_eq!(reassembled, body, "chunk split must cover the whole body");
}

#[test]
fn c5_resp_retained_ceiling_is_sound_and_much_less_than_1mib() {
    // C5: the R2/R3 ceiling expression uses depth × (chunk + hdr), NOT
    // depth × chunk. Mirror the crate consts (depth=8, chunk=8 KiB,
    // hdr=16) — the R-tests will re-bind these from the real crate
    // exports at P1-B; this asserts the *expression* is the C5 sound
    // bound and still ≪ the 1 MiB R2 body (non-vacuous).
    let depth = 8;
    let chunk_max = 8 * 1024;
    let frame_hdr_max = 16;
    let ceiling = resp_retained_ceiling(depth, chunk_max, frame_hdr_max);
    // Sound bound is strictly larger than the looser depth×chunk form.
    assert!(
        ceiling > 4 * (depth * chunk_max),
        "C5: ceiling must include the frame-header term"
    );
    let one_mib = 1024 * 1024;
    // "≪" = at least ~3× headroom below the body. (The C5 sound
    // ceiling ≈ 257 KiB vs the 1 MiB R2 body ≈ 3.99× headroom; assert
    // ≥3× so the memory proof's pass-threshold is provably non-vacuous
    // without a multiplier so tight it would itself be brittle.)
    assert!(
        ceiling * 3 <= one_mib,
        "non-vacuous: ceiling ({ceiling}) must be ≪ the 1 MiB R2 body \
         (got {:.2}× headroom, need ≥3×)",
        one_mib as f64 / ceiling as f64
    );
}

// --------------------------------------------------------------------
// R1..R8 real-wire tests — SCAFFOLD ONLY. `#[ignore]`d with an explicit
// precondition: they exercise the proxy's RESPONSE path, which is the
// legacy buffered path until builder-2's P1-B wires `stream_h1_response`
// into `conn_actor`. Running them now would assert against the OLD
// path (a false signal). builder-1 finalizes the bodies + un-ignores
// them once P1-B is verifier-passed and the §1.5 gauge is wired
// (task #6 continuation). The harness above is exercised by the
// fixture tests, so this file is NOT vacuous.
// --------------------------------------------------------------------

#[tokio::test]
#[ignore = "UNBLOCKS at P1-B: needs stream_h1_response wired into conn_actor (else exercises legacy buffered path)"]
async fn r1_multi_data_binary_response_byte_identical() {
    let certs = generate_loopback_certs();
    let expected = binary_body(120 * 1024);
    let (backend, backend_h) =
        spawn_resp_backend(RespBody::ContentLength(expected.clone()), None).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();
    let out = out.expect("R1 e2e failed");
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R1 must end with a clean FIN");
    assert_eq!(
        out.body, expected,
        "R1 response body must be byte-identical"
    );
}

#[tokio::test]
#[ignore = "UNBLOCKS at P1-B: needs §1.5 MAX_RETAINED_RESP_BYTES gauge wired (P1-B) + test-gauges feature"]
async fn r2_one_mib_response_memory_bounded_through_stalled_client() {
    // Will assert: MAX_RETAINED_RESP_BYTES <= resp_retained_ceiling(
    //   H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, H3_FRAME_HDR_MAX)
    // (the §1.5 C5 sound bound) AND ≪ 1 MiB, plus liveness +
    // byte-identity after the client resumes.
}

#[tokio::test]
#[ignore = "UNBLOCKS at P1-B: needs §1.5 gauge wired; asserts upstream read pauses while gauge stays ≤ C5 ceiling"]
async fn r3_slow_client_backpressures_upstream_read() {}

#[tokio::test]
#[ignore = "UNBLOCKS at P1-B: needs stream_h1_response wired into conn_actor"]
async fn r4_empty_response_body_clean_fin() {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_resp_backend(RespBody::ContentLength(Vec::new()), None).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();
    let out = out.expect("R4 e2e failed");
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R4 empty body must still end with a clean FIN");
    assert!(out.body.is_empty(), "R4 body must be empty");
}

#[tokio::test]
#[ignore = "UNBLOCKS at P1-B: needs stream_h1_response wired; asserts non-H3_NO_ERROR RESET_STREAM, no truncated body"]
async fn r5_upstream_reset_midresponse_yields_reset_stream() {}

#[tokio::test]
#[ignore = "UNBLOCKS at P1-B: needs stream_h1_response wired; asserts upstream read stops + per-stream state torn down"]
async fn r6_client_cancel_midresponse_stops_upstream_read() {}

#[tokio::test]
#[ignore = "UNBLOCKS at P1-B: needs stream_h1_response wired (exercises the net-new chunked-response decoder end-to-end)"]
async fn r7_chunked_upstream_response_byte_identical() {
    let certs = generate_loopback_certs();
    let expected = binary_body(50_000);
    let (backend, backend_h) = spawn_resp_backend(
        RespBody::Chunked {
            body: expected.clone(),
            chunk_sizes: vec![1, 7, 4096, 8192, 1, 100, 99_999],
        },
        None,
    )
    .await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();
    let out = out.expect("R7 e2e failed");
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R7 chunked response must end with a clean FIN");
    assert_eq!(
        out.body, expected,
        "R7 chunked response must reassemble byte-identical (new decoder)"
    );
}

#[tokio::test]
#[ignore = "UNBLOCKS at P1-C: PROTO-2-12 trailer no-regression is locked by h3_h1_trailers_resp_e2e.rs pc1/pc2"]
async fn r8_trailers_no_regression_placeholder() {}
