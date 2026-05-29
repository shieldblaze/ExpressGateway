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
//!     head/mid/tail) byte-identical at the H3 client.
//!   * R2 — NON-VACUOUS memory bound: 1 MiB response, stalled H3
//!     client; `MAX_RETAINED_RESP_BYTES` ≤ the §1.5 C5 sound
//!     bound and `≪ 1 MiB`; liveness + byte-identity after resume.
//!   * R3 — slow-client backpressure: upstream read provably pauses
//!     (gauge stays bounded), request still completes correctly.
//!   * R4 — empty response body + zero-length DATA; byte-identical,
//!     clean FIN.
//!   * R5 — upstream resets mid-response ⇒ client observes
//!     RESET_STREAM with a non-`H3_NO_ERROR` code, no truncated
//!     body presented as complete.
//!   * R6 — client cancels mid-response ⇒ proxy stops reading
//!     upstream, per-stream state torn down, no leak.
//!   * R7 — chunked upstream response, byte-identical (new decoder).
//!   * R8 — trailers no-regression (PROTO-2-12: the existing
//!     `h3_h1_trailers_resp_e2e.rs` pc1/pc2 stay green).
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
    /// CF-H3-HEAD — a `Content-Length`-framed body whose head ALSO
    /// carries regular response headers (content-type / cache-control /
    /// a custom x-*) PLUS a `Connection: close` hop-by-hop header. The
    /// proxy MUST forward the regular headers to the H3 client and MUST
    /// strip the hop-by-hop one (load-bearing both ways).
    ContentLengthWithHeaders {
        body: Vec<u8>,
        extra: Vec<(&'static str, &'static str)>,
    },
    /// `Transfer-Encoding: chunked` body, emitted in the given chunk
    /// sizes then the zero terminator (R7: new decoder path).
    Chunked {
        body: Vec<u8>,
        chunk_sizes: Vec<usize>,
    },
    /// SESSION 4 / P1-C (R8/C4): `Transfer-Encoding: chunked` body with
    /// an RFC 9112 §7.1.2 trailer section after the zero-size chunk.
    /// `coalesce` controls whether the `0\r\n`, the trailer fields and
    /// the terminating CRLF are written in ONE socket write (PC-2
    /// coalesced-remainder) or split across separate writes (PC-2
    /// split-across-reads). Empty `trailers` ⇒ a bare `0\r\n\r\n`
    /// terminator (no spurious trailing HEADERS expected).
    ChunkedWithTrailers {
        body: Vec<u8>,
        chunk_sizes: Vec<usize>,
        trailers: Vec<(String, String)>,
        coalesce: bool,
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
    /// Send the status + headers, part of the body, then issue a hard
    /// TCP RST (SO_LINGER 0) mid-body — a true connection reset, not a
    /// graceful FIN (R5: `RespAbort::UpstreamReset`).
    RstMidBody {
        declared_len: usize,
        partial: Vec<u8>,
    },
    /// `Content-Length` declared LARGER than the proxy's `cap`
    /// (`MAX_RESPONSE_BODY_BYTES`); send the head + as much body as the
    /// client will take. The proxy MUST `RespAbort::OverCap` ⇒
    /// RESET_STREAM with 0x0102, never present a body as complete
    /// (R5 over-cap sub-case, binding C1).
    OverCap { declared_len: usize },
    /// Endless body (huge `Content-Length`, never satisfied); writes
    /// until the proxy stops reading (R6 client-cancel: prove the
    /// upstream read halts). `read_done` is fired once the backend's
    /// socket read returns 0/err — i.e. the proxy dropped the pooled
    /// upstream connection — and `bytes_written` records how much was
    /// pushed before the proxy stopped consuming.
    Endless {
        read_closed: Arc<Notify>,
        bytes_written: Arc<std::sync::atomic::AtomicUsize>,
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
            RespBody::ContentLengthWithHeaders { body: b, extra } => {
                // CF-H3-HEAD: head carries the extra regular headers +
                // content-length + Connection: close (hop-by-hop).
                let mut head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {}\r\nConnection: close\r\n",
                    b.len()
                );
                for (n, v) in &extra {
                    head.push_str(n);
                    head.push_str(": ");
                    head.push_str(v);
                    head.push_str("\r\n");
                }
                head.push_str("\r\n");
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
            RespBody::ChunkedWithTrailers {
                body,
                chunk_sizes,
                trailers,
                coalesce,
            } => {
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
                // Zero-size chunk + RFC 9112 §7.1.2 trailer section +
                // terminating CRLF. `coalesce` decides whether they are
                // one write (PC-2 coalesced-remainder, parsed from the
                // SAME read as the `0\r\n` size line) or split across
                // writes (PC-2 split-across-reads).
                let mut tail = Vec::from(&b"0\r\n"[..]);
                for (n, v) in &trailers {
                    tail.extend_from_slice(format!("{n}: {v}\r\n").as_bytes());
                }
                tail.extend_from_slice(b"\r\n");
                if coalesce {
                    let _ = sock.write_all(&tail).await;
                } else {
                    // Split so the size line, the trailer fields and
                    // the terminating CRLF arrive in separate reads.
                    for byte in &tail {
                        let _ = sock.write_all(std::slice::from_ref(byte)).await;
                        let _ = sock.flush().await;
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                }
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
            RespBody::RstMidBody {
                declared_len,
                partial,
            } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&partial).await;
                let _ = sock.flush().await;
                // Hard TCP RST: SO_LINGER 0 then drop ⇒ the peer's next
                // read returns ECONNRESET, exercising the
                // `RespAbort::UpstreamReset` (read error) arm rather
                // than the EOF (read==0) arm. (linger 0 + no queued
                // data ⇒ drop does not block.)
                #[allow(deprecated)]
                let _ = sock.set_linger(Some(Duration::ZERO));
                drop(sock);
            }
            RespBody::OverCap { declared_len } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                // Stream a repeating pattern until the proxy aborts the
                // read (it will, once `total > cap`). Never allocate
                // `declared_len`.
                let chunk = vec![0xABu8; 64 * 1024];
                let mut written = 0usize;
                while written < declared_len {
                    if sock.write_all(&chunk).await.is_err() {
                        break; // proxy reset the upstream (OverCap).
                    }
                    written += chunk.len();
                }
                let _ = sock.shutdown().await;
                drop(sock);
            }
            RespBody::Endless {
                read_closed,
                bytes_written,
            } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    1usize << 40 // 1 TiB — never satisfied
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let chunk = vec![0x5Au8; 32 * 1024];
                // SESSION 4 / P1-C note (NOT a P1-C logic change): this
                // lint is PRE-EXISTING at base HEAD 98f4ed12 (proven via
                // git-stash + clippy at clean HEAD), not introduced by
                // P1-C. The clippy `while let Ok(()) = write_all` rewrite
                // would DROP the essential post-match teardown probe
                // below (the 1 ms `sock.read(&probe)` that detects the
                // proxy closing its read half) — semantically wrong. No
                // test logic / assertion / ordering is changed.
                #[allow(clippy::while_let_loop)]
                loop {
                    match sock.write_all(&chunk).await {
                        Ok(()) => {
                            bytes_written.fetch_add(chunk.len(), Ordering::Relaxed);
                        }
                        Err(_) => break, // proxy stopped reading + closed.
                    }
                    // Probe whether the proxy closed its read half: a
                    // zero-byte read returns Ok(0) on FIN / Err on RST.
                    let mut probe = [0u8; 1];
                    match tokio::time::timeout(Duration::from_millis(1), sock.read(&mut probe))
                        .await
                    {
                        Ok(Ok(0)) | Ok(Err(_)) => break, // upstream torn down.
                        _ => {}
                    }
                }
                read_closed.notify_waiters();
                drop(sock);
            }
        }
    });
    (addr, handle)
}

/// Outcome the FIN-aware response client driver reports.
#[derive(Debug, Default)]
struct ClientOutcome {
    status: Option<u16>,
    body: Vec<u8>,
    /// True if the response stream ended with a clean FIN.
    fin: bool,
    /// `Some(code)` if the proxy RESET_STREAM'd us (R5/R6 abort path).
    reset_code: Option<u64>,
    /// SESSION 4 / P1-C (R8/C4): fields of the post-DATA trailing
    /// HEADERS frame (RFC 9114 §4.1), empty when the response carried
    /// no trailer section. Additive — existing tests ignore it.
    trailers: Vec<(String, String)>,
    /// CF-H3-HEAD — non-`:status` fields of the response HEAD HEADERS
    /// frame, so the full-header round-trip test can assert regular
    /// headers (content-type / cache-control / custom x-*) survive the
    /// H3→H1→H3 relay and hop-by-hop (connection) is stripped. Additive;
    /// existing tests ignore it.
    head_fields: Vec<(String, String)>,
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
    // SESSION 4 / P1-C (R8/C4): a post-DATA HEADERS frame (no `:status`)
    // is the RFC 9114 §4.1 trailing field section.
    let mut trailers: Vec<(String, String)> = Vec::new();
    // CF-H3-HEAD: non-`:status` fields of the response HEAD frame.
    let mut head_fields: Vec<(String, String)> = Vec::new();

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
                    trailers,
                    head_fields,
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
                        if hdrs.iter().any(|(n, _)| n == ":status") {
                            for (n, v) in hdrs {
                                if n == ":status" {
                                    status = Some(v.parse().map_err(|_| "status".to_string())?);
                                } else {
                                    // CF-H3-HEAD: capture every non-status
                                    // head field for the full-header
                                    // round-trip assertion.
                                    head_fields.push((n, v));
                                }
                            }
                        } else {
                            // SESSION 4 / P1-C (R8/C4): a post-DATA
                            // HEADERS frame with no `:status` is the
                            // RFC 9114 §4.1 trailing field section.
                            trailers.extend(hdrs);
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
                trailers,
                head_fields,
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

/// Like `start_listener` but with a SINGLE-slot `TcpPool` whose handle
/// is returned so a test can observe pool parking (C2: a poisoned
/// upstream connection must be dropped / never parked, mirroring
/// `lb_io::pool::tests::non_reusable_is_not_parked`).
async fn start_listener_single_slot_pool(
    certs: &TestCerts,
    backend: SocketAddr,
) -> (QuicListener, SocketAddr, CancellationToken, TcpPool) {
    let pool = TcpPool::new(
        PoolConfig {
            per_peer_max: 1,
            total_max: 1,
            ..PoolConfig::default()
        },
        BackendSockOpts {
            nodelay: true,
            keepalive: true,
            rcvbuf: Some(65_536),
            sndbuf: Some(65_536),
            quickack: false,
            tcp_fastopen_connect: false,
        },
        Runtime::new(),
    );
    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    )
    .with_backends(vec![backend], pool.clone());
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone()).await.unwrap();
    let addr = listener.local_addr();
    (listener, addr, shutdown, pool)
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

/// Stalled/slow FIN-aware H3 response client driver (R2/R3 memory +
/// backpressure proof). It drives the QUIC connection (so handshake +
/// ACKs progress) but does **not** call `stream_recv` on the response
/// stream for `stall` after it first sees the stream become readable —
/// so quiche never grants the proxy more flow-control credit, the
/// proxy's `Progressive` queue stays non-empty, the §1.4.3 gate stops
/// pulling, the bounded channel fills, and `stream_h1_response`'s
/// `tx.send().await` blocks ⇒ the upstream socket read pauses (genuine
/// end-to-end backpressure). After `stall` it drains the stream to
/// completion and reports the (must-be byte-identical) body + FIN.
///
/// `sample`: invoked once, mid-stall (after the proxy has had time to
/// fill its in-flight window against the stalled client) — the R2/R3
/// gauge read happens there so it observes the proxy at its largest
/// retained-bytes instant.
#[allow(clippy::too_many_lines)]
async fn drive_h3_response_client_stalled(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    stall: Duration,
    deadline: tokio::time::Instant,
    mut sample: impl FnMut(),
) -> Result<ClientOutcome, String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let stream_id: u64 = 0;
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    let encoder = QpackEncoder::new();
    let headers = vec![
        (":method".to_string(), "GET".to_string()),
        (":scheme".to_string(), "https".to_string()),
        (":authority".to_string(), REQUEST_AUTHORITY.to_string()),
        (":path".to_string(), REQUEST_PATH.to_string()),
    ];
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

    // Stall lifecycle.
    let mut stream_seen_readable = false;
    let mut stall_until: Option<tokio::time::Instant> = None;
    let mut sampled = false;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "deadline; established={} status={status:?} body={} fin={fin}",
                conn.is_established(),
                body.len()
            ));
        }
        if conn.is_closed() {
            if fin || reset_code.is_some() {
                return Ok(ClientOutcome {
                    status,
                    body,
                    fin,
                    reset_code,
                    trailers: Vec::new(),
                    head_fields: Vec::new(),
                });
            }
            return Err(format!(
                "conn closed early: peer={:?} local={:?} status={status:?} body={}",
                conn.peer_error(),
                conn.local_error(),
                body.len()
            ));
        }

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

        let in_stall = match stall_until {
            Some(t) => tokio::time::Instant::now() < t,
            None => false,
        };

        if conn.is_established() {
            let readable: Vec<u64> = conn.readable().collect();
            if readable.contains(&stream_id) && !stream_seen_readable {
                stream_seen_readable = true;
                stall_until = Some(tokio::time::Instant::now() + stall);
            }
            // Do NOT consume the response stream while stalling: the
            // whole point is that quiche grants no further credit so
            // the proxy backpressures. We DO keep recv'ing UDP +
            // sending ACKs (below) so the connection stays alive.
            if !in_stall {
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
                        }
                        Ok((_, c)) => {
                            rx_tail.drain(..c);
                        }
                        Err(lb_h3::H3Error::Incomplete) => break,
                        Err(e) => return Err(format!("decode_frame: {e}")),
                    }
                }
            }
        }

        // Sample the gauge once, mid-stall, after the proxy has had
        // wall-time to fill its bounded in-flight window against us.
        if in_stall && !sampled {
            if let Some(t) = stall_until {
                if t.saturating_duration_since(tokio::time::Instant::now()) <= stall / 2 {
                    sample();
                    sampled = true;
                }
            }
        }

        if (fin && status.is_some()) || reset_code.is_some() {
            return Ok(ClientOutcome {
                status,
                body,
                fin,
                reset_code,
                trailers: Vec::new(),
                head_fields: Vec::new(),
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
// R1..R8 / C2 / C3 real-wire tests — THE H3→H1 R8 VERIFICATION BAR.
//
// INTEGRITY FIX (S6 I0, owner binding condition 3): the prior comment
// here claimed these tests were "SCAFFOLD ONLY" and `#[ignore]`d
// pending builder-2's P1-B response wiring. That claim is FALSE at the
// current tip: P1-B shipped, `stream_h1_response` is wired into
// `conn_actor`, there are NO `#[ignore]` attributes anywhere in this
// file, and ALL of these tests RUN and PASS (16/16 at the S5/S6 tip).
// They are not scaffold — they are the non-vacuous reference proof
// every other H-matrix cell's R8 gate is measured against.
//
// FEATURE GATE (load-bearing): R2 (`r2_response_memory_bounded_through_
// stalled_client`) and R3 (`r3_slow_client_backpressures_upstream_
// read`) reference `lb_quic::h3_bridge::MAX_RETAINED_RESP_BYTES`, a
// `#[cfg(any(test, feature = "test-gauges"))]` static. This test
// crate therefore only COMPILES the memory/backpressure proofs under
// `--features test-gauges`; a CI gate that omits that flag silently
// drops the only non-vacuous memory assertions. Any R8 gate for this
// cell (or a sibling reusing this pattern) MUST run
// `cargo test -p lb-quic --features test-gauges`.
// --------------------------------------------------------------------

#[tokio::test]
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

/// CF-H3-HEAD — the H3→H1 streaming response leg MUST forward the FULL
/// non-hop-by-hop response header set to the H3 client (pre-S12 it
/// dropped everything but `:status` + content-length). The backend
/// sends content-type + cache-control + a custom `x-eg-resp` ALONGSIDE
/// `Connection: close` (hop-by-hop). LOAD-BEARING both ways: the three
/// regular headers MUST round-trip, and `connection` MUST be stripped
/// (it would otherwise leak an upstream hop-by-hop header to the H3
/// client). Temp-revert the stream_h1_response head re-encode to the
/// `:status`+CL-only projection → this test FAILS (the regular headers
/// vanish); restore → PASSES. Body byte-identity + clean FIN confirm
/// the head change does not perturb body framing.
#[tokio::test]
async fn cf_h3_head_h3_to_h1_full_response_headers_round_trip() {
    let certs = generate_loopback_certs();
    let expected = b"h3h1-full-head-body".to_vec();
    let (backend, backend_h) = spawn_resp_backend(
        RespBody::ContentLengthWithHeaders {
            body: expected.clone(),
            extra: vec![
                ("content-type", "application/json"),
                ("cache-control", "no-store"),
                ("x-eg-resp", "round-trip"),
            ],
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
    let out = out.expect("CF-H3-HEAD H3→H1 e2e failed");

    assert_eq!(
        out.status,
        Some(UPSTREAM_STATUS),
        "must be the upstream status"
    );
    assert!(out.fin, "clean FIN expected after the full-header response");
    assert_eq!(out.body, expected, "body must be byte-identical");
    let has = |name: &str, val: &str| out.head_fields.iter().any(|(n, v)| n == name && v == val);
    assert!(
        has("content-type", "application/json"),
        "content-type MUST round-trip H3→H1 (CF-H3-HEAD); got head fields {:?}",
        out.head_fields
    );
    assert!(
        has("cache-control", "no-store"),
        "cache-control MUST round-trip H3→H1 (CF-H3-HEAD); got head fields {:?}",
        out.head_fields
    );
    assert!(
        has("x-eg-resp", "round-trip"),
        "a custom response header MUST round-trip H3→H1 (CF-H3-HEAD); got head fields {:?}",
        out.head_fields
    );
    // Hop-by-hop MUST be stripped — `connection` must NOT reach the H3
    // client (load-bearing the other way).
    assert!(
        !out.head_fields.iter().any(|(n, _)| n == "connection"),
        "the hop-by-hop `connection` header MUST NOT be forwarded to the H3 \
         client (got head fields {:?})",
        out.head_fields
    );
}

/// R2 — NON-VACUOUS memory bound (verifier-authoritative numbers).
///
/// A response far larger than the proxy's in-flight window (≈4 MiB)
/// streams through a STALLED H3 client. The §1.5 `MAX_RETAINED_RESP_BYTES`
/// gauge — sampled mid-stall, the proxy's largest-retained instant —
/// MUST stay ≤ the §1.5 C5 sound ceiling, which is **exactly**
/// `4 × (H3_RESP_CHANNEL_DEPTH × (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX))`
/// (the C5 chunk+hdr form, NOT the looser `depth×chunk`) AND that
/// ceiling is `≪` the body at ≥8× margin. Then the client resumes and
/// the body must arrive byte-identical with a clean FIN (liveness +
/// no-corruption). Mirrors S2 T5 (`h3_h1_stream_body_e2e.rs:791`).
///
/// Authoritative numbers (verifier-owned, from the real crate consts):
///   DEPTH=8, CHUNK_MAX=8192, FRAME_HDR_MAX=16
///   C5 channel bound      = 8×(8192+16)            = 65 664 B
///   R2 ceiling (×4 slack) = 4×65 664               = 262 656 B (~256.5 KiB)
///   body                  = 4 MiB                  = 4 194 304 B
///   margin                = 4 194 304 / 262 656    ≈ 15.97× (≥8× ✓)
#[tokio::test]
async fn r2_response_memory_bounded_through_stalled_client() {
    use lb_quic::h3_bridge::{
        H3_FRAME_HDR_MAX, H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, MAX_RETAINED_RESP_BYTES,
    };

    MAX_RETAINED_RESP_BYTES.store(0, Ordering::SeqCst);

    // The EXACT §1.5 C5 sound bound: depth × (chunk + hdr), NOT
    // depth × chunk (C5 — under-counting the frame header would be an
    // unsound proof). `test ceiling == gauge bound` per the lead's C5
    // directive — this is the same expression `drain_resp_channels`
    // feeds `record_resp_retained` (conn_actor.rs:382-385).
    let c5_channel_bound = H3_RESP_CHANNEL_DEPTH * (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX);
    let ceiling = resp_retained_ceiling(H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, H3_FRAME_HDR_MAX);
    assert_eq!(
        ceiling,
        4 * c5_channel_bound,
        "R2 ceiling MUST equal the §1.5 C5 sound bound (4 × depth×(chunk+hdr))"
    );
    assert_eq!(
        c5_channel_bound, 65_664,
        "C5 channel bound authoritative value"
    );
    assert_eq!(ceiling, 262_656, "R2 ceiling authoritative value");

    let total_body = 4 * 1024 * 1024usize; // 4 MiB
    assert!(
        ceiling * 8 <= total_body,
        "non-vacuous: ceiling ({ceiling}) must be ≪ body ({total_body}) \
         at ≥8× (got {:.2}×)",
        total_body as f64 / ceiling as f64
    );

    let certs = generate_loopback_certs();
    let expected = binary_body(total_body);
    let resume = Arc::new(Notify::new());
    let (backend, backend_h) = spawn_resp_backend(
        RespBody::ContentLength(expected.clone()),
        Some(resume.clone()),
    )
    .await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);

    // Release the backend's body after a grace period so the request
    // can complete (liveness). The grace is long enough that, were the
    // proxy buffering the whole body, it would have tripped the gauge
    // far above the ceiling before resume.
    let resume_c = resume.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        resume_c.notify_waiters();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    // Stall ≈1.6s of NOT reading the response stream — spans the body
    // release, so the proxy's window is provably full while we sample.
    let out =
        drive_h3_response_client_stalled(conn, &sock, Duration::from_millis(1600), deadline, || {})
            .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();

    let out = out.expect("R2 e2e failed (liveness: must complete after resume)");
    let max_retained = MAX_RETAINED_RESP_BYTES.load(Ordering::SeqCst);

    eprintln!(
        "R2-EVIDENCE: max_retained={max_retained} B  ceiling={ceiling} B \
         (C5 channel bound={c5_channel_bound} B)  body={total_body} B  \
         margin={:.2}x  retained/ceiling={:.4}",
        total_body as f64 / ceiling as f64,
        max_retained as f64 / ceiling as f64
    );
    assert!(
        max_retained > 0,
        "gauge must have observed in-flight retained response bytes \
         (else the proof is vacuous)"
    );
    assert!(
        max_retained <= ceiling,
        "R2 memory bound BREACHED: max retained = {max_retained} B, \
         C5 ceiling = {ceiling} B (body = {total_body} B). A buffering \
         proxy would retain ≈ body size."
    );
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(
        out.fin,
        "R2 must end with a clean FIN after resume (liveness)"
    );
    assert_eq!(
        out.body, expected,
        "R2 body must be byte-identical after the stall+resume"
    );
}

/// R3 — slow-client backpressure proof (verifier-authoritative).
///
/// A slow H3 client provably pauses the upstream socket read: the gauge
/// stays ≤ the C5 ceiling while a >> window response is in flight and
/// the client is not reading; the request still completes byte-identical
/// after the client resumes. Distinct from R2 in intent: R2 proves the
/// memory CEILING; R3 proves the BACKPRESSURE causal chain (upstream
/// read does not run ahead while the client stalls — evidenced by the
/// gauge never exceeding the bound for a body 16× the ceiling, which is
/// only possible if `stream_h1_response`'s `tx.send().await` blocked the
/// upstream read).
#[tokio::test]
async fn r3_slow_client_backpressures_upstream_read() {
    use lb_quic::h3_bridge::{
        H3_FRAME_HDR_MAX, H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, MAX_RETAINED_RESP_BYTES,
    };

    MAX_RETAINED_RESP_BYTES.store(0, Ordering::SeqCst);
    let ceiling = resp_retained_ceiling(H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, H3_FRAME_HDR_MAX);

    let total_body = 4 * 1024 * 1024usize; // 4 MiB, 16× the ceiling
    let certs = generate_loopback_certs();
    let expected = binary_body(total_body);
    // No backend stall: the backend is willing to send the whole body
    // as fast as TCP allows. The ONLY thing that can keep the proxy's
    // retained bytes ≤ ceiling for a 4 MiB body is the backpressure
    // chain pausing the upstream socket read while the client stalls.
    let (backend, backend_h) =
        spawn_resp_backend(RespBody::ContentLength(expected.clone()), None).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);

    // Sample the gauge mid-stall: the upstream is firehosing, the
    // client is not reading. If the upstream read were NOT paused the
    // proxy would have pulled the whole 4 MiB and the gauge would be
    // ≈ 4 MiB ≫ ceiling.
    let gauge_mid = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let gauge_mid_c = gauge_mid.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let out = drive_h3_response_client_stalled(
        conn,
        &sock,
        Duration::from_millis(2000),
        deadline,
        move || {
            gauge_mid_c.store(
                MAX_RETAINED_RESP_BYTES.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
        },
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();

    let out = out.expect("R3 e2e failed (must complete after the slow client resumes)");
    let mid = gauge_mid.load(Ordering::SeqCst);
    let max_retained = MAX_RETAINED_RESP_BYTES.load(Ordering::SeqCst);

    eprintln!(
        "R3-EVIDENCE: mid_stall_retained={mid} B  peak_retained={max_retained} B  \
         ceiling={ceiling} B  body={total_body} B  \
         mid/ceiling={:.4}  peak/ceiling={:.4}",
        mid as f64 / ceiling as f64,
        max_retained as f64 / ceiling as f64
    );
    assert!(
        mid > 0,
        "gauge must have observed in-flight bytes mid-stall (proof not vacuous)"
    );
    assert!(
        mid <= ceiling,
        "BACKPRESSURE FAILED: mid-stall retained = {mid} B exceeds the \
         C5 ceiling {ceiling} B for a {total_body} B body — the upstream \
         socket read was NOT paused (it ran ahead of the stalled client)"
    );
    assert!(
        max_retained <= ceiling,
        "BACKPRESSURE FAILED: peak retained = {max_retained} B > ceiling \
         {ceiling} B for a {total_body} B body"
    );
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R3 must complete with a clean FIN after resume");
    assert_eq!(
        out.body, expected,
        "R3 body must be byte-identical despite the backpressure stall"
    );
}

#[tokio::test]
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

/// Run one abort scenario end-to-end and return the client outcome.
async fn run_abort_scenario(body: RespBody) -> ClientOutcome {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_resp_backend(body, None).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();
    out.expect("abort-scenario e2e failed")
}

/// Assert the load-bearing C1 invariant for an abort outcome: the H3
/// client observed RESET_STREAM with `error_code == H3_INTERNAL_ERROR
/// == 0x0102` AND `!= H3_NO_ERROR (0x0100)`, never a clean FIN — a
/// truncated body is NEVER presentable as complete (cache-poisoning /
/// response-splitting guard).
fn assert_c1_reset(out: &ClientOutcome, label: &str) {
    assert!(
        !out.fin,
        "{label}: abort path must NOT end with a clean FIN \
         (truncated-as-complete guard) — got fin=true, body={}",
        out.body.len()
    );
    let code = out.reset_code.unwrap_or_else(|| {
        panic!(
            "{label}: client must observe RESET_STREAM on the abort \
             path; reset_code=None, fin={}, body={}",
            out.fin,
            out.body.len()
        )
    });
    assert_eq!(
        code,
        lb_quic::H3_INTERNAL_ERROR,
        "{label}: RESET_STREAM error_code must == H3_INTERNAL_ERROR \
         (0x0102); got {code:#x}"
    );
    assert_eq!(code, 0x0102, "{label}: explicit codepoint check");
    assert_ne!(
        code,
        lb_quic::H3_NO_ERROR,
        "{label}: RESET_STREAM error_code must NOT be the graceful \
         H3_NO_ERROR (0x0100) — that would let a cache treat the \
         truncated body as a complete response (binding C1)"
    );
}

/// R5 — upstream resets / fails mid-response ⇒ client sees RESET_STREAM
/// with the explicit C1 code, no truncated body presented as complete.
/// Covers all three §1.4-Q2-cited abort sub-cases: hard TCP RST,
/// premature-EOF-before-Content-Length, and over-cap.
#[tokio::test]
async fn r5_upstream_reset_midresponse_yields_reset_stream() {
    // (a) hard TCP RST mid-body ⇒ RespAbort::UpstreamReset.
    let out = run_abort_scenario(RespBody::RstMidBody {
        declared_len: 200_000,
        partial: binary_body(16 * 1024),
    })
    .await;
    assert_c1_reset(&out, "R5a upstream-RST");

    // (b) premature EOF before Content-Length ⇒ RespAbort::PrematureEof.
    let out = run_abort_scenario(RespBody::ResetMidBody {
        declared_len: 200_000,
        partial: binary_body(16 * 1024),
    })
    .await;
    assert_c1_reset(&out, "R5b premature-EOF-before-Content-Length");

    // (c) over-cap (declared > MAX_RESPONSE_BODY_BYTES, 64 MiB) ⇒
    // RespAbort::OverCap. The proxy must abort once total > cap.
    let out = run_abort_scenario(RespBody::OverCap {
        declared_len: (64 * 1024 * 1024) + (4 * 1024 * 1024),
    })
    .await;
    assert_c1_reset(&out, "R5c over-cap");
}

/// R6 — client cancels mid-response ⇒ proxy stops reading the upstream,
/// per-stream state is torn down, no leak. Proven by: (1) the endless
/// backend's socket read closes (the proxy dropped the pooled upstream
/// connection) shortly after the client cancels, and (2) the backend
/// stopped being able to write (bytes_written stops growing) — i.e. the
/// upstream read provably halted rather than draining 1 TiB.
#[tokio::test]
async fn r6_client_cancel_midresponse_stops_upstream_read() {
    let certs = generate_loopback_certs();
    let read_closed = Arc::new(Notify::new());
    let bytes_written = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (backend, backend_h) = spawn_resp_backend(
        RespBody::Endless {
            read_closed: read_closed.clone(),
            bytes_written: bytes_written.clone(),
        },
        None,
    )
    .await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);

    // Cancel after receiving ≥ 32 KiB of body (STOP_SENDING +
    // RESET_STREAM on the response stream, via the driver's
    // `cancel_after`).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let drive = tokio::spawn(async move {
        drive_h3_response_client(conn, &sock, vec![], Some(32 * 1024), deadline).await
    });

    // The proxy must drop the pooled upstream connection promptly after
    // the cancel; the backend's read half then observes close. If this
    // times out the upstream read did NOT stop ⇒ leak / no teardown.
    let torn_down = tokio::time::timeout(Duration::from_secs(20), read_closed.notified()).await;

    let written_at_teardown = bytes_written.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let written_after = bytes_written.load(Ordering::Relaxed);

    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), drive).await;
    backend_h.abort();

    assert!(
        torn_down.is_ok(),
        "R6: proxy did NOT tear down the upstream after the client \
         cancelled (endless backend's read never closed within 20s) — \
         per-stream state leak / upstream read not stopped"
    );
    // The upstream read provably stopped: no further bytes accepted
    // after teardown (a still-reading proxy on a 1 TiB body would keep
    // the backend writing indefinitely).
    assert_eq!(
        written_after, written_at_teardown,
        "R6: backend kept writing after teardown ({written_at_teardown} \
         → {written_after}) — the proxy was still reading the upstream"
    );
}

#[tokio::test]
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

// --------------------------------------------------------------------
// C2 — pooled-upstream smuggling guard (binding condition C2). For EACH
// RespAbort variant + a ClientGone client-cancel: trigger the abort,
// then assert (a) the poisoned pooled upstream connection is NOT parked
// (single-slot pool ⇒ `idle_count_for(backend) == 0`, mirroring
// `lb_io::pool::tests::non_reusable_is_not_parked`) so the next acquire
// MUST dial a fresh conn, AND (b) the H3 client observed RESET_STREAM
// `== 0x0102 != 0x0100` (the truncated-as-complete cache-poisoning
// guard) — except ClientGone, where the proxy correctly does NOT
// RESET_STREAM (it tears down on the client's own cancel) but the
// upstream MUST still be dropped, not parked.
// --------------------------------------------------------------------

/// Drive one C2 abort scenario through a single-slot pool; returns the
/// client outcome and the post-abort idle-count for the backend.
async fn run_c2_scenario(body: RespBody, cancel_after: Option<usize>) -> (ClientOutcome, usize) {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_resp_backend(body, None).await;
    let (listener, server, _sd, pool) = start_listener_single_slot_pool(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], cancel_after, deadline).await;
    // Give the proxy a moment to run the abort teardown / drop the
    // pooled connection before we inspect the pool.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let idle = pool.idle_count_for(backend);
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();
    (out.expect("C2 scenario e2e failed"), idle)
}

#[tokio::test]
async fn c2_every_abort_variant_drops_pooled_upstream_and_resets() {
    // UpstreamReset (hard RST mid-body).
    let (out, idle) = run_c2_scenario(
        RespBody::RstMidBody {
            declared_len: 200_000,
            partial: binary_body(16 * 1024),
        },
        None,
    )
    .await;
    assert_c1_reset(&out, "C2/UpstreamReset");
    assert_eq!(
        idle, 0,
        "C2/UpstreamReset: poisoned upstream must NOT be parked"
    );

    // PrematureEof (EOF before Content-Length).
    let (out, idle) = run_c2_scenario(
        RespBody::ResetMidBody {
            declared_len: 200_000,
            partial: binary_body(16 * 1024),
        },
        None,
    )
    .await;
    assert_c1_reset(&out, "C2/PrematureEof");
    assert_eq!(
        idle, 0,
        "C2/PrematureEof: poisoned upstream must NOT be parked"
    );

    // ChunkedDecode (malformed chunk framing mid-body): a first chunk
    // decodes, then a non-hex size token ⇒ RespAbort::ChunkedDecode.
    // Uses a raw backend through the single-slot pool.
    let (out, idle) = run_c2_raw_chunked(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\nZZ\r\nx\r\n",
    )
    .await;
    assert_c1_reset(&out, "C2/ChunkedDecode");
    assert_eq!(
        idle, 0,
        "C2/ChunkedDecode: poisoned upstream must NOT be parked"
    );

    // OverCap (declared > MAX_RESPONSE_BODY_BYTES).
    let (out, idle) = run_c2_scenario(
        RespBody::OverCap {
            declared_len: (64 * 1024 * 1024) + (4 * 1024 * 1024),
        },
        None,
    )
    .await;
    assert_c1_reset(&out, "C2/OverCap");
    assert_eq!(idle, 0, "C2/OverCap: poisoned upstream must NOT be parked");

    // BadHead (garbage status line / no CRLFCRLF) — dedicated backend
    // (an empty EOF-delimited body is a *valid* 200, not a BadHead).
    let (out, idle) = run_c2_bad_head().await;
    assert_c1_reset(&out, "C2/BadHead");
    assert_eq!(idle, 0, "C2/BadHead: poisoned upstream must NOT be parked");

    // NOTE: the sixth C2 variant — ClientGone (client cancels
    // mid-response) — is a separate test
    // (`c2_clientgone_drops_pooled_upstream`) because it is currently
    // BLOCKED BY A PRODUCT DEFECT (client cancel of the H3 response
    // stream is not propagated to stop the upstream read; see
    // audit/h3-program/s5-evidence/task6/DEFECT-clientgone-resp-stream-not-propagated.md).
    // It stays as a failing regression lock until the product is
    // fixed — NOT folded in here so the five working C2 arms give a
    // clean signal, NOT weakened/ignored.
}

/// C2 sixth variant — ClientGone. Currently FAILS: it is the
/// regression lock for the proven product defect that a client cancel
/// of the H3 response stream does not stop the upstream read / drop
/// the pooled upstream (binding C2 + §1.3.4 ClientGone). Asserts the
/// REAL teardown (the endless backend's read half closes ⇒ the proxy
/// dropped the pooled upstream), not merely `idle==0` (which the
/// defect would spuriously satisfy because the never-finishing
/// producer task simply never parks the conn). Keep failing until
/// fixed; do NOT weaken or ignore.
#[tokio::test]
async fn c2_clientgone_drops_pooled_upstream() {
    let certs = generate_loopback_certs();
    let read_closed = Arc::new(Notify::new());
    let bytes_written = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (backend, backend_h) = spawn_resp_backend(
        RespBody::Endless {
            read_closed: read_closed.clone(),
            bytes_written: bytes_written.clone(),
        },
        None,
    )
    .await;
    let (listener, server, _sd, pool) = start_listener_single_slot_pool(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let drive = tokio::spawn(async move {
        drive_h3_response_client(conn, &sock, vec![], Some(32 * 1024), deadline).await
    });
    let torn_down = tokio::time::timeout(Duration::from_secs(20), read_closed.notified()).await;
    let idle = pool.idle_count_for(backend);
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), drive).await;
    backend_h.abort();
    assert!(
        torn_down.is_ok(),
        "C2/ClientGone: proxy did NOT drop the upstream after the \
         client cancelled (endless backend read never closed in 20s) — \
         binding C2 / §1.3.4 ClientGone violated"
    );
    assert_eq!(
        idle, 0,
        "C2/ClientGone: poisoned upstream must NOT be parked"
    );
}

/// Raw malformed-chunked backend through the single-slot pool (C2):
/// proves the poisoned upstream is dropped (idle==0) AND the client
/// saw RESET_STREAM 0x0102.
async fn run_c2_raw_chunked(raw: &str) -> (ClientOutcome, usize) {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_raw_backend(raw.as_bytes().to_vec()).await;
    let (l, server, _sd, pool) = start_listener_single_slot_pool(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let idle = pool.idle_count_for(backend);
    let _ = tokio::time::timeout(Duration::from_secs(3), l.shutdown()).await;
    backend_h.abort();
    (out.expect("C2/raw-chunked e2e failed"), idle)
}

/// Bad-head backend: sends bytes that are NOT a valid HTTP/1.1 status
/// line and never a CRLFCRLF terminator, then closes. The proxy MUST
/// `RespAbort::BadHead` ⇒ RESET_STREAM 0x0102.
async fn run_c2_bad_head() -> (ClientOutcome, usize) {
    let certs = generate_loopback_certs();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let backend = listener.local_addr().unwrap();
    let backend_h = tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
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
            let _ = sock.write_all(b"NOT-HTTP garbage no terminator").await;
            let _ = sock.shutdown().await;
        }
    });
    let (l, server, _sd, pool) = start_listener_single_slot_pool(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let idle = pool.idle_count_for(backend);
    let _ = tokio::time::timeout(Duration::from_secs(3), l.shutdown()).await;
    backend_h.abort();
    (out.expect("C2/BadHead e2e failed"), idle)
}

// --------------------------------------------------------------------
// C3 — chunked-decoder negative / smuggling tests, end-to-end. The
// unit-level C3 cases live in h3_bridge.rs::chunk_decoder_rejects_
// malformed_framing_c3; here we additionally prove the two cases the
// lead named that the unit test did not cover (declared-size overflow,
// junk after the zero-size terminator) AND that, real-wire, a malformed
// chunked upstream response ⇒ RESET_STREAM 0x0102, never a truncated /
// forwarded body presented as complete.
// --------------------------------------------------------------------

/// Raw backend emitting a caller-supplied byte stream verbatim after the
/// request head (so arbitrarily malformed chunked framing can be sent).
async fn spawn_raw_backend(raw_response: Vec<u8>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let h = tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
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
            let _ = sock.write_all(&raw_response).await;
            let _ = sock.shutdown().await;
        }
    });
    (addr, h)
}

async fn run_raw_chunked_abort(raw: &str) -> ClientOutcome {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_raw_backend(raw.as_bytes().to_vec()).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();
    out.expect("C3 raw-chunked e2e failed")
}

#[tokio::test]
async fn c3_malformed_chunked_responses_reset_never_forward_truncated() {
    // (1) non-hex chunk size.
    let out = run_raw_chunked_abort(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nZZ\r\nabc\r\n0\r\n\r\n",
    )
    .await;
    assert_c1_reset(&out, "C3 non-hex-chunk-size");

    // (2) missing CRLF after chunk data (junk where CRLF must be).
    let out = run_raw_chunked_abort(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabcXX0\r\n\r\n",
    )
    .await;
    assert_c1_reset(&out, "C3 missing-CRLF-after-chunk");

    // (3) declared chunk-size larger than the data, then EOF (declared-
    // size overflow / EOF before terminator).
    let out = run_raw_chunked_abort(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nFFFF\r\nshort",
    )
    .await;
    assert_c1_reset(&out, "C3 declared-size-overflow");

    // (4) junk after the zero-size terminator's framing — a smuggled
    // second "response". The decoder must not accept the trailing
    // garbage as a valid body/terminator. (Well-formed terminator is
    // `0\r\n\r\n`; here the final CRLF is corrupted.)
    let out = run_raw_chunked_abort(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n0\r\nXJUNK",
    )
    .await;
    // Either a clean completion at the valid terminator with the junk
    // ignored as connection-close trailing bytes, OR a decode reset —
    // but NEVER the junk forwarded as body. Assert the body is exactly
    // "abc" and, if it completed, nothing smuggled in.
    if out.reset_code.is_some() {
        assert_c1_reset(&out, "C3 junk-after-terminator (reset)");
    } else {
        assert_eq!(
            out.body, b"abc",
            "C3 junk-after-terminator: only the valid chunk body may be \
             forwarded; trailing junk must NEVER be smuggled into the body"
        );
        assert!(out.fin, "C3 junk-after-terminator: clean completion");
    }
}

/// C3 unit supplement: the two cases the lead named that
/// `chunk_decoder_rejects_malformed_framing_c3` did not cover —
/// declared-size overflow (size > available, then a fed EOF surfaces as
/// the producer's ChunkedDecode) and junk after the zero terminator.
/// These are decoder-level so they live here as a focused unit check
/// over the public producer behaviour via a raw backend is covered by
/// the e2e above; this asserts the DECODER never emits a truncated body.
#[test]
fn c3_unit_supplement_documents_coverage() {
    // The decoder-level malformed cases (non-hex, empty token, wrong
    // post-body byte, oversize size-line) are asserted in
    // h3_bridge.rs::chunk_decoder_rejects_malformed_framing_c3. The
    // remaining lead-named cases — declared-size overflow and junk
    // after the zero terminator — are proven END-TO-END (real wire,
    // real RESET_STREAM 0x0102, never a forwarded truncated body) by
    // `c3_malformed_chunked_responses_reset_never_forward_truncated`
    // cases (3) and (4). This test exists so the coverage mapping is
    // explicit and grep-able.
}

// --------------------------------------------------------------------
// R8 — P1-C (C4): an upstream chunked response that carries an
// RFC 9112 §7.1.2 trailer section is delivered to the H3 client as a
// post-DATA RFC 9114 §4.1 trailing HEADERS frame, AFTER a
// byte-identical binary body and BEFORE a clean FIN. Real wire:
// QuicListener → router → conn_actor → h3_bridge → real H1 backend.
//
// PROTO-2-12 no-regression is locked separately by the unchanged
// `h3_h1_trailers_resp_e2e.rs` pc1/pc2 + the `request_h3_upstream`
// H3-upstream trailer path (this new H1-channel path never touches
// them). The Content-Length / EOF framings emit NO trailer frame —
// R1/R4 already prove those stay byte-identical; this test additionally
// asserts the no-trailer chunked sub-case produces NO spurious empty
// trailing HEADERS.
// --------------------------------------------------------------------

/// Drive one chunked-with-trailers scenario end-to-end and return the
/// FIN-aware client outcome (status + binary body + decoded trailers).
async fn run_r8_scenario(body: RespBody) -> ClientOutcome {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_resp_backend(body, None).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();
    out.expect("R8 e2e failed")
}

#[tokio::test]
async fn r8_chunked_response_trailers_delivered_to_h3_client() {
    let expected = binary_body(60_000);
    let trailers = vec![
        ("x-checksum".to_string(), "abc123-DEADBEEF".to_string()),
        ("x-trailer-two".to_string(), "second-value".to_string()),
    ];

    // (1) PC-2 coalesced: `0\r\n<trailer-fields>\r\n` arrives in ONE
    //     backend write (parsed from the SAME read as the `0\r\n` size
    //     line, not only subsequently-read socket bytes).
    let out = run_r8_scenario(RespBody::ChunkedWithTrailers {
        body: expected.clone(),
        chunk_sizes: vec![1, 7, 4096, 8192, 1, 100, 99_999],
        trailers: trailers.clone(),
        coalesce: true,
    })
    .await;
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(
        out.fin,
        "R8 coalesced: clean FIN after the trailing HEADERS frame"
    );
    assert_eq!(
        out.body, expected,
        "R8 coalesced: body byte-identical (binary 0xFF/0x00/0x80)"
    );
    assert_eq!(
        out.trailers, trailers,
        "R8 coalesced: chunked trailer section delivered as the H3 \
         trailing HEADERS frame, byte-identical"
    );

    // (2) PC-2 split-across-reads: the size line, trailer fields and
    //     terminating CRLF arrive in separate reads — must parse to the
    //     identical decoded outcome.
    let out = run_r8_scenario(RespBody::ChunkedWithTrailers {
        body: expected.clone(),
        chunk_sizes: vec![8192, 8192, 43_616],
        trailers: trailers.clone(),
        coalesce: false,
    })
    .await;
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R8 split: clean FIN after the trailing HEADERS");
    assert_eq!(
        out.body, expected,
        "R8 split: body byte-identical across split-read trailer parse"
    );
    assert_eq!(
        out.trailers, trailers,
        "R8 split: trailer section byte-identical when split across reads"
    );

    // (3) Chunked WITHOUT a trailer section (`0\r\n\r\n`): the trailing
    //     HEADERS frame is CONDITIONAL — no spurious empty trailer
    //     frame, body byte-identical, clean FIN. (PC-2 also covers the
    //     coalesced bare `0\r\n\r\n` terminator here.)
    let out = run_r8_scenario(RespBody::ChunkedWithTrailers {
        body: expected.clone(),
        chunk_sizes: vec![4096, 55_904],
        trailers: vec![],
        coalesce: true,
    })
    .await;
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R8 no-trailers: clean FIN");
    assert_eq!(out.body, expected, "R8 no-trailers: body byte-identical");
    assert!(
        out.trailers.is_empty(),
        "R8 no-trailers: a chunked response with NO trailer section \
         MUST NOT produce a spurious trailing HEADERS frame"
    );

    // (4) No-regression assertion for the Content-Length framing: it
    //     carries NO trailer section and therefore NO trailing HEADERS
    //     frame (R1/R4 already lock CL byte-identity; this makes the
    //     "trailer frame is chunked-only" contract explicit).
    let out = run_r8_scenario(RespBody::ContentLength(expected.clone())).await;
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R8 CL: clean FIN");
    assert_eq!(out.body, expected, "R8 CL: body byte-identical");
    assert!(
        out.trailers.is_empty(),
        "R8 CL: Content-Length framing carries no trailer frame"
    );
}
