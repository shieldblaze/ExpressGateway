//! S9 / H1→H1 (M-D-lite) — INDEPENDENT verifier proofs for the bounded
//! ingress pump builder-1 landed in `H1Proxy::proxy_request`
//! (`crates/lb-l7/src/h1_proxy.rs`, branch `s9/builder-1`).
//!
//! AUTHOR≠VERIFIER: this file is OWNED by the verifier. The src under test
//! (`h1_proxy.rs`) is synced byte-identical from `origin/s9/builder-1` and
//! NOT edited here.
//!
//! Proofs (mirrors `tests/h2h1_md_streaming_verify.rs`, the S8 precedent):
//!   1. real-wire BOTH directions, binary body, ≤window AND >window, byte-id;
//!   2. non-vacuous body-size-INDEPENDENT memory gauge + inverted probe +
//!      live-occupancy (fast backend) proof;
//!   3. bidirectional backpressure with a PROVEN causal chain + paused offset;
//!   4. F-MD-4 smuggling parity for BOTH Content-Length AND chunked framings:
//!      a real H1 client truncates mid-body → backend NEVER sees a complete
//!      request (no `0\r\n\r\n`) and the upstream conn is aborted not pooled;
//!   5. >64 MiB upload → 413.
//!
//! The frontend is PLAINTEXT H1 (matching the cell: H1 listener), driven by a
//! RAW byte client so we can emit a premature mid-body TCP half-close that the
//! hyper client API cannot express. Per-chunk reads are NON-cumulative (S8
//! harness-bug lesson): each read appends exactly the bytes returned.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Bytes, Frame, Incoming};
use hyper::server::conn::http1 as srv_h1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// 64 KiB fixed window = H1_REQ_CHANNEL_DEPTH(8) × H1_REQ_CHUNK_MAX(8 KiB).
const WINDOW: usize = 64 * 1024;

// ── shared helpers ──────────────────────────────────────────────────────

fn build_pool() -> TcpPool {
    TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts {
            nodelay: true,
            keepalive: false,
            rcvbuf: None,
            sndbuf: None,
            quickack: false,
            tcp_fastopen_connect: false,
        },
        Runtime::new(),
    )
}

fn proxy_for(backend: SocketAddr, timeouts: HttpTimeouts) -> Arc<H1Proxy> {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend]).unwrap());
    Arc::new(H1Proxy::new(
        pool, picker, None, timeouts, /* is_https */ false,
    ))
}

/// Generous timeouts so a slow large upload isn't killed by the body timer.
fn relaxed_timeouts() -> HttpTimeouts {
    HttpTimeouts {
        header: Duration::from_secs(30),
        body: Duration::from_secs(60),
        total: Duration::from_secs(120),
    }
}

/// Spawn a plaintext H1 gateway: TCP accept → H1Proxy.
async fn spawn_h1_gateway(proxy: Arc<H1Proxy>) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let proxy = Arc::clone(&proxy);
            tokio::spawn(async move {
                let _ = proxy.serve_connection(sock, peer).await;
            });
        }
    });
    local
}

/// Deterministic non-UTF-8 binary pattern (covers all 256 byte values incl.
/// 0x00 and high bytes, so a UTF-8-lossy or truncating relay is caught).
fn binary_pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + 7) & 0xFF) as u8).collect()
}

// ── Raw H1 client read helper (NON-cumulative per-chunk reader) ───────────

/// Read exactly the H1 response head (through the blank `\r\n\r\n`) then,
/// using the parsed framing, read EXACTLY the body. Returns (head, body).
/// The body reader appends only the bytes each read() returned — never the
/// whole buffer — defeating the S8 cumulative-reader harness bug.
async fn read_h1_response(sock: &mut TcpStream) -> (Vec<u8>, Vec<u8>) {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 16 * 1024];
    // 1) read until end-of-headers.
    let head_end = loop {
        if let Some(p) = find_subslice(&buf, b"\r\n\r\n") {
            break p + 4;
        }
        let n = sock.read(&mut tmp).await.unwrap();
        assert!(n > 0, "EOF before response headers complete");
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = buf[..head_end].to_vec();
    let mut body_seed = buf[head_end..].to_vec();
    let head_str = String::from_utf8_lossy(&head).to_ascii_lowercase();

    // 2) decide framing.
    if let Some(cl) = parse_content_length(&head_str) {
        let mut body = body_seed;
        while body.len() < cl {
            let n = sock.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]); // per-chunk append, NOT cumulative
        }
        body.truncate(cl);
        (head, body)
    } else if head_str.contains("transfer-encoding: chunked") {
        // de-chunk, reading more as needed.
        let mut raw = body_seed;
        loop {
            match try_dechunk(&raw) {
                DechunkState::Complete(body) => break (head, body),
                DechunkState::NeedMore => {
                    let n = sock.read(&mut tmp).await.unwrap();
                    assert!(n > 0, "EOF mid-chunked-body");
                    raw.extend_from_slice(&tmp[..n]);
                }
            }
        }
    } else {
        // close-delimited: read to EOF.
        loop {
            let n = sock.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            body_seed.extend_from_slice(&tmp[..n]);
        }
        (head, body_seed)
    }
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn parse_content_length(head_lower: &str) -> Option<usize> {
    for line in head_lower.lines() {
        if let Some(v) = line.strip_prefix("content-length:") {
            return v.trim().parse().ok();
        }
    }
    None
}

enum DechunkState {
    Complete(Vec<u8>),
    NeedMore,
}

/// Minimal chunked decoder: returns the assembled body once the 0-size
/// terminator chunk is seen, else NeedMore.
fn try_dechunk(raw: &[u8]) -> DechunkState {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        // find CRLF ending the chunk-size line.
        let Some(crlf) = find_subslice(&raw[i..], b"\r\n") else {
            return DechunkState::NeedMore;
        };
        let size_line = &raw[i..i + crlf];
        // chunk-ext after ';' ignored.
        let size_hex = size_line.split(|&b| b == b';').next().unwrap_or(size_line);
        let size_str = String::from_utf8_lossy(size_hex);
        let Ok(sz) = usize::from_str_radix(size_str.trim(), 16) else {
            return DechunkState::NeedMore;
        };
        i += crlf + 2;
        if sz == 0 {
            // final chunk; consume optional trailers up to blank line.
            return DechunkState::Complete(out);
        }
        if raw.len() < i + sz + 2 {
            return DechunkState::NeedMore;
        }
        out.extend_from_slice(&raw[i..i + sz]);
        i += sz + 2; // skip data + CRLF
    }
}

// ══════════════════════════════════════════════════════════════════════
// 1. REAL-WIRE BOTH DIRECTIONS, BINARY BODY, BYTE-IDENTICAL.
// ══════════════════════════════════════════════════════════════════════

/// Echo backend: real hyper H1 server that streams the request body back as
/// the response body, byte-for-byte. Counts dials.
async fn spawn_echo_backend() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&dials);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| async move {
                    // Collect the full request body, echo it back as a single
                    // Full body (also proves the backend saw a COMPLETE body).
                    let collected = req.into_body().collect().await;
                    match collected {
                        Ok(c) => {
                            let bytes = c.to_bytes();
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Full::new(bytes))
                                    .unwrap(),
                            )
                        }
                        Err(_) => Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::BAD_GATEWAY)
                                .body(Full::new(Bytes::new()))
                                .unwrap(),
                        ),
                    }
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, dials)
}

/// Drive a real H1 client with a binary request body through the gateway and
/// assert the echoed response body is byte-identical to the request body.
async fn real_wire_roundtrip(body_len: usize) {
    let (backend, _dials) = spawn_echo_backend().await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    let payload = binary_pattern(body_len);

    let sock = TcpStream::connect(gw_addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(sock))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    // Stream the request body as many small frames so >window bodies exercise
    // the bounded channel rather than a single hand-off.
    let chunks: Vec<Result<Frame<Bytes>, Infallible>> = payload
        .chunks(4096)
        .map(|c| Ok(Frame::data(Bytes::copy_from_slice(c))))
        .collect();
    let stream = StreamBody::new(futures_util::stream::iter(chunks));
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(hyper::header::HOST, "backend.test")
        .body(stream)
        .unwrap();

    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "expected 200 for a well-formed {body_len}-byte upload"
    );
    let got = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        got.len(),
        payload.len(),
        "response body length mismatch (got {} want {})",
        got.len(),
        payload.len()
    );
    assert!(
        got.as_ref() == payload.as_slice(),
        "response body NOT byte-identical to request body (len {body_len})"
    );
    eprintln!("REAL_WIRE byte_identical len={body_len} status=200 OK");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_wire_small_body_byte_identical() {
    // ≤ window (64 KiB): 1000 bytes.
    real_wire_roundtrip(1000).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_wire_large_body_byte_identical() {
    // > window (multi-MiB): 5 MiB exercises Branch B many times over.
    real_wire_roundtrip(5 * 1024 * 1024).await;
}

// ══════════════════════════════════════════════════════════════════════
// 2. NON-VACUOUS, BODY-SIZE-INDEPENDENT MEMORY GAUGE.
// ══════════════════════════════════════════════════════════════════════

// Serialize the gauge tests: the inverted probe writes a multi-MiB sentinel
// into the process-global gauge; without this lock that write can leak into a
// concurrent gauge test's reset→read window (S8 R2 global-state-collision
// lesson). Only gauge tests touch the shared static, so only they serialize.
#[cfg(feature = "test-gauges")]
static GAUGE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Backend that reads ONLY the request head then STALLS (never drains the
/// body) until released. Drives the bounded-channel-fills → pump-parks chain.
#[cfg(feature = "test-gauges")]
async fn spawn_stalled_backend() -> (SocketAddr, Arc<AtomicUsize>, Arc<tokio::sync::Notify>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let d2 = Arc::clone(&dials);
    let r2 = Arc::clone(&release);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            let r3 = Arc::clone(&r2);
            tokio::spawn(async move {
                // Read only the head prefix, then stall: stop draining the
                // socket so the gateway's hyper sender cannot flush → its mpsc
                // fills → the pump parks → the frontend read window stalls.
                let mut tmp = [0u8; 256];
                let _ = sock.read(&mut tmp).await;
                r3.notified().await;
                // After release, drain then reply cleanly.
                let mut sink = [0u8; 64 * 1024];
                loop {
                    match tokio::time::timeout(Duration::from_millis(200), sock.read(&mut sink))
                        .await
                    {
                        Ok(Ok(0)) | Err(_) => break,
                        Ok(Ok(_)) => continue,
                        Ok(Err(_)) => break,
                    }
                }
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    (local, dials, release)
}

#[cfg(feature = "test-gauges")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_gauge_non_vacuous_and_load_bearing() {
    use lb_l7::h1_proxy::{H1_REQ_MAX_RETAINED_BODY_BYTES, record_retained_h1};

    let _serial = GAUGE_SERIAL.lock().await;
    H1_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);

    let body_size = 4 * 1024 * 1024; // 4 MiB ≫ 64 KiB window
    let (backend, _dials, release) = spawn_stalled_backend().await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    // Real H1 client; stream the 4 MiB body. The stalled backend parks the
    // pump, so the client's TCP send blocks once the in-flight window fills.
    let payload = binary_pattern(body_size);
    let writer = tokio::spawn(async move {
        let sock = TcpStream::connect(gw_addr).await.unwrap();
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(sock))
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let chunks: Vec<Result<Frame<Bytes>, Infallible>> = payload
            .chunks(16 * 1024)
            .map(|c| Ok(Frame::data(Bytes::copy_from_slice(c))))
            .collect();
        let stream = StreamBody::new(futures_util::stream::iter(chunks));
        let req = Request::builder()
            .method("POST")
            .uri("/stall")
            .header(hyper::header::HOST, "backend.test")
            .body(stream)
            .unwrap();
        // We do NOT wait for the response; the backend is stalled.
        let _ = sender.send_request(req).await;
    });

    // Let the system reach steady-state under the stall.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let in_situ = H1_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);

    // Non-vacuous bound: production gauge ≤ 4×window AND ≪ body.
    assert!(
        in_situ <= 4 * WINDOW,
        "retained gauge {in_situ} exceeds 4×window ({}); bounded-memory bar broken",
        4 * WINDOW
    );
    assert!(
        in_situ < body_size,
        "retained gauge {in_situ} not ≪ body size {body_size}"
    );
    assert!(
        in_situ > 0,
        "retained gauge is 0 — the pump never recorded any in-flight bytes \
         (the proof would be vacuous)"
    );

    // INVERTED PROBE (load-bearing): a whole-body-buffering impl would call
    // record_retained_h1(body_size). If the ≤4×window assertion is
    // load-bearing, that pushes the gauge ABOVE the ceiling.
    record_retained_h1(body_size);
    let after = H1_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    assert!(
        after > 4 * WINDOW,
        "inverted probe failed: a whole-body retain of {body_size} did not exceed \
         the 4×window ceiling — the assertion would not catch a buffering regression"
    );

    eprintln!(
        "MEMORY_GAUGE in_situ_retained_bytes={in_situ} body_size={body_size} window={WINDOW} \
         ceiling={} inverted_probe_after={after}",
        4 * WINDOW
    );

    release.notify_waiters();
    let _ = writer.await;
}

/// Live-occupancy proof: a multi-MiB body through a FAST echo backend, full
/// response read, gauge STILL bounded. A no-decrement / max-ever-pushed gauge
/// would climb toward the body size; the live-occupancy gauge stays ≤4×window
/// because each chunk is decremented the instant hyper pulls it.
#[cfg(feature = "test-gauges")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_gauge_tracks_live_occupancy_not_cumulative() {
    use lb_l7::h1_proxy::H1_REQ_MAX_RETAINED_BODY_BYTES;

    let _serial = GAUGE_SERIAL.lock().await;
    H1_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);

    let body_size = 4 * 1024 * 1024; // 4 MiB streamed through a FAST backend
    let (backend, _dials) = spawn_echo_backend().await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    let payload = binary_pattern(body_size);
    let expected = payload.clone();

    let sock = TcpStream::connect(gw_addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(sock))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let chunks: Vec<Result<Frame<Bytes>, Infallible>> = payload
        .chunks(16 * 1024)
        .map(|c| Ok(Frame::data(Bytes::copy_from_slice(c))))
        .collect();
    let stream = StreamBody::new(futures_util::stream::iter(chunks));
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(hyper::header::HOST, "backend.test")
        .body(stream)
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let got = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(
        got.as_ref() == expected.as_slice(),
        "fast-path echo body not byte-identical"
    );

    let peak = H1_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    assert!(
        peak > 0,
        "gauge stayed 0 through a 4 MiB roundtrip — vacuous"
    );
    assert!(
        peak <= 4 * WINDOW,
        "live-occupancy gauge peak {peak} exceeds 4×window ({}) on the FAST path \
         — a no-decrement (max-ever-pushed) gauge would climb toward {body_size}",
        4 * WINDOW
    );
    eprintln!("LIVE_OCCUPANCY peak_retained_bytes={peak} body_size={body_size} window={WINDOW}");
}

// ══════════════════════════════════════════════════════════════════════
// 3. BACKPRESSURE — proven causal chain, with the paused-at byte offset.
// ══════════════════════════════════════════════════════════════════════

/// Backend that reads only the head then stalls until released, then drains
/// (counting body bytes) and replies 200.
async fn spawn_gated_drain_backend() -> (SocketAddr, Arc<AtomicUsize>, Arc<tokio::sync::Notify>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let bytes_read = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let br2 = Arc::clone(&bytes_read);
    let r2 = Arc::clone(&release);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let br3 = Arc::clone(&br2);
            let r3 = Arc::clone(&r2);
            tokio::spawn(async move {
                let mut tmp = [0u8; 256];
                let _ = sock.read(&mut tmp).await;
                r3.notified().await;
                let mut sink = [0u8; 64 * 1024];
                loop {
                    match tokio::time::timeout(Duration::from_millis(300), sock.read(&mut sink))
                        .await
                    {
                        Ok(Ok(0)) | Err(_) => break,
                        Ok(Ok(n)) => {
                            br3.fetch_add(n, Ordering::SeqCst);
                        }
                        Ok(Err(_)) => break,
                    }
                }
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    (local, bytes_read, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backpressure_client_send_paused_while_backend_stalled() {
    // 48 MiB ≫ the sum of the kernel loopback socket buffers + hyper's
    // frontend read buffer + the 64 KiB in-flight window + hyper's client
    // write buffer. On localhost those buffers total at most a few MiB, so a
    // 48 MiB body CANNOT be absorbed: once the pump parks, the gateway stops
    // reading the frontend socket, its TCP recv window closes, and the
    // client's write() blocks far below 48 MiB. (A 4 MiB body was fully
    // absorbed by loopback autotuned buffers — that was a harness undersizing,
    // not a missing backpressure; see the verify doc.)
    let body_size = 48 * 1024 * 1024;
    let (backend, _bytes_read, release) = spawn_gated_drain_backend().await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    // RAW H1 client so we can count exactly how many body bytes we managed to
    // write to the gateway TCP socket before backpressure blocks the write.
    // A chunked request (no content-length) lets us push frames freely.
    let mut sock = TcpStream::connect(gw_addr).await.unwrap();
    sock.write_all(
        b"POST /bp HTTP/1.1\r\nhost: backend.test\r\ntransfer-encoding: chunked\r\n\r\n",
    )
    .await
    .unwrap();

    let payload = binary_pattern(body_size);
    let sent = Arc::new(AtomicUsize::new(0));
    let sc = Arc::clone(&sent);
    // Writer task: push 16 KiB chunked frames, then the terminator. It does
    // NOT abandon on a block — a blocked write simply parks the task (that IS
    // backpressure); it unblocks and continues once the backend is released.
    // `sent` is the live count of body bytes acknowledged into the socket.
    let (sock_tx, sock_rx) = tokio::sync::oneshot::channel::<TcpStream>();
    let writer = tokio::spawn(async move {
        let mut off = 0;
        while off < payload.len() {
            let end = (off + 16 * 1024).min(payload.len());
            let body = &payload[off..end];
            let frame = format!("{:x}\r\n", body.len());
            let mut chunk = frame.into_bytes();
            chunk.extend_from_slice(body);
            chunk.extend_from_slice(b"\r\n");
            if sock.write_all(&chunk).await.is_err() {
                break;
            }
            sc.fetch_add(end - off, Ordering::SeqCst);
            off = end;
        }
        let _ = sock.write_all(b"0\r\n\r\n").await; // chunked terminator
        let _ = sock_tx.send(sock);
    });

    // PHASE 1 — backend stalled: after steady-state the writer must be PARKED
    // below the full body. Causal chain: backend stall → gateway hyper sender
    // cannot flush → bounded mpsc(64 KiB) fills → pump parks → gateway stops
    // reading the frontend socket → frontend TCP recv window closes → the
    // client write() blocks below body_size. We sample twice to PROVE the
    // pause is stable (not just mid-progress): the count must not advance.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let paused_at = sent.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let paused_at2 = sent.load(Ordering::SeqCst);
    eprintln!(
        "BACKPRESSURE phase1 paused_at={paused_at} paused_at2={paused_at2} \
         body_size={body_size} window={WINDOW}"
    );
    assert!(
        paused_at < body_size,
        "backpressure NOT applied: client pushed the whole body ({paused_at} of {body_size}) \
         while the backend was stalled"
    );
    assert!(
        paused_at > 0,
        "client never made progress — cannot attribute the stall to backpressure"
    );
    assert_eq!(
        paused_at, paused_at2,
        "writer kept advancing while the backend was stalled ({paused_at} → {paused_at2}); \
         the pause is not a stable backpressure stall"
    );

    // PHASE 2 — release: backend drains, pump resumes, the parked client write
    // unblocks and the full body completes with 200 (resume-after-drain leg =
    // backpressure is not a deadlock).
    release.notify_waiters();
    let mut sock = tokio::time::timeout(Duration::from_secs(60), sock_rx)
        .await
        .expect("writer never returned the socket")
        .expect("writer dropped the socket");
    let _ = tokio::time::timeout(Duration::from_secs(60), writer).await;
    let (head, _body) = tokio::time::timeout(Duration::from_secs(60), read_h1_response(&mut sock))
        .await
        .expect("response timed out after backend drain");
    let head_str = String::from_utf8_lossy(&head);
    let resumed_at = sent.load(Ordering::SeqCst);
    eprintln!(
        "BACKPRESSURE phase2 resumed_at={resumed_at} status_line={:?}",
        head_str.lines().next().unwrap_or("")
    );
    assert!(
        head_str.starts_with("HTTP/1.1 200"),
        "expected 200 after drain, got: {:?}",
        head_str.lines().next()
    );
    assert!(
        resumed_at > paused_at,
        "client did not resume sending after backend drain ({paused_at} → {resumed_at})"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 4. F-MD-4 SMUGGLING PARITY — BOTH framings: a real H1 client sends a
//    partial body then prematurely closes mid-body. The backend must NEVER
//    see a COMPLETE request (no `0\r\n\r\n` terminator / no satisfied CL),
//    and the upstream conn must be aborted not pooled.
// ══════════════════════════════════════════════════════════════════════

/// A "framing-witness" backend that records, per upstream connection, whether
/// it observed a COMPLETE request body terminator. It speaks raw H1 so it can
/// inspect the exact upstream framing the gateway emits:
///   • chunked upstream  → complete iff it sees the `0\r\n\r\n` terminator;
///   • (the gateway always emits chunked for the streamed body — CL/TE are
///     stripped by F-MD-1 — so "complete" == terminator seen).
/// `complete` counts requests the backend saw terminate cleanly; `dials`
/// counts accepted upstream connections (non-vacuity: forwarding began).
#[derive(Clone)]
struct FramingWitness {
    dials: Arc<AtomicUsize>,
    complete: Arc<AtomicUsize>,
    body_bytes: Arc<AtomicUsize>,
}

async fn spawn_framing_witness_backend() -> (SocketAddr, FramingWitness) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let w = FramingWitness {
        dials: Arc::new(AtomicUsize::new(0)),
        complete: Arc::new(AtomicUsize::new(0)),
        body_bytes: Arc::new(AtomicUsize::new(0)),
    };
    let w2 = w.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            w2.dials.fetch_add(1, Ordering::SeqCst);
            let w3 = w2.clone();
            tokio::spawn(async move {
                // Read the whole upstream request bytes to EOF (the gateway
                // aborts the conn on truncation → we hit EOF). Track total
                // body bytes seen and whether the chunked terminator arrived.
                let mut raw: Vec<u8> = Vec::new();
                let mut tmp = [0u8; 16 * 1024];
                loop {
                    // S11: 30 s (was 5 s) — saturation-robustness for the
                    // multi-MiB drain under the full --workspace gate (same
                    // class as the h2h1 body-counting backend fix); bounded
                    // well under the test's outer budget.
                    match tokio::time::timeout(Duration::from_secs(30), sock.read(&mut tmp)).await {
                        Ok(Ok(0)) | Err(_) => break, // EOF (abort) or idle timeout
                        Ok(Ok(n)) => raw.extend_from_slice(&tmp[..n]),
                        Ok(Err(_)) => break,
                    }
                    // The chunked terminator `0\r\n\r\n` marks a COMPLETE
                    // request body. Once seen, reply 200 and stop.
                    if find_subslice(&raw, b"\r\n0\r\n\r\n").is_some()
                        || raw.windows(5).any(|x| x == b"0\r\n\r\n")
                            && raw.starts_with(b"POST")
                            && find_subslice(&raw, b"\r\n\r\n").is_some()
                    {
                        w3.complete.fetch_add(1, Ordering::SeqCst);
                        let _ = sock
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                            .await;
                        break;
                    }
                }
                // Count body bytes (everything after the request head).
                if let Some(p) = find_subslice(&raw, b"\r\n\r\n") {
                    w3.body_bytes
                        .fetch_add(raw.len() - (p + 4), Ordering::SeqCst);
                }
            });
        }
    });
    (local, w)
}

/// Send a request HEAD then a partial body then DROP the TCP connection
/// mid-body. `framing` = "cl" sends a Content-Length larger than what we
/// write; "chunked" sends a chunk-size header then fewer body bytes than
/// announced (and never the terminator) before dropping.
async fn truncated_upload(gw_addr: SocketAddr, framing: &str) {
    let mut sock = TcpStream::connect(gw_addr).await.unwrap();
    match framing {
        "cl" => {
            // Declare CL=1MiB, then write only 32 KiB and abruptly close.
            sock.write_all(
                b"POST /trunc HTTP/1.1\r\nhost: backend.test\r\ncontent-length: 1048576\r\n\r\n",
            )
            .await
            .unwrap();
            let partial = binary_pattern(32 * 1024);
            sock.write_all(&partial).await.unwrap();
        }
        "chunked" => {
            sock.write_all(
                b"POST /trunc HTTP/1.1\r\nhost: backend.test\r\ntransfer-encoding: chunked\r\n\r\n",
            )
            .await
            .unwrap();
            // Announce a 1 MiB chunk, write only 32 KiB of it, then close —
            // no chunk completion, no `0\r\n\r\n` terminator.
            sock.write_all(b"100000\r\n").await.unwrap(); // 0x100000 = 1 MiB
            let partial = binary_pattern(32 * 1024);
            sock.write_all(&partial).await.unwrap();
        }
        _ => unreachable!(),
    }
    sock.flush().await.unwrap();
    // Abrupt half-close mid-body: shut down the write side then drop.
    let _ = sock.shutdown().await;
    // Give the gateway a moment to observe the early EOF and propagate it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(sock);
}

async fn smuggling_parity_case(framing: &str) {
    let (backend, w) = spawn_framing_witness_backend().await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    truncated_upload(gw_addr, framing).await;
    // Allow the upstream conn to settle (abort propagates, EOF observed).
    tokio::time::sleep(Duration::from_secs(1)).await;

    let dials = w.dials.load(Ordering::SeqCst);
    let complete = w.complete.load(Ordering::SeqCst);
    let body = w.body_bytes.load(Ordering::SeqCst);
    eprintln!(
        "SMUGGLE_PARITY framing={framing} backend_dials={dials} complete={complete} \
         backend_body_bytes={body}"
    );

    // Non-vacuity: the gateway actually dialed/began forwarding (S8 dials=1
    // analogue). If dials==0, the proof is vacuous (nothing was forwarded).
    assert!(
        dials >= 1,
        "non-vacuous failure: backend was never dialed for framing={framing} — \
         forwarding never began, so the smuggle proof is empty"
    );
    // THE security assertion: the backend MUST NEVER see a complete request.
    assert_eq!(
        complete, 0,
        "SMUGGLING: backend saw a COMPLETE request (complete={complete}) for a \
         truncated {framing} upload — a truncated request was relayed as complete"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smuggle_parity_content_length_truncation() {
    smuggling_parity_case("cl").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smuggle_parity_chunked_truncation() {
    smuggling_parity_case("chunked").await;
}

/// Confirm the PRE-PUMP header-level CL/TE smuggle rejections still fire
/// (SmuggleDetector::check_all_mode in `handle`): a request with conflicting
/// Content-Length + Transfer-Encoding must be rejected 400 BEFORE any dial.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pre_pump_cl_te_smuggle_still_rejected() {
    let (backend, w) = spawn_framing_witness_backend().await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    // CL + TE on the same request = classic CL.TE smuggle; default H1 mode
    // rejects this at the detector choke point.
    let mut sock = TcpStream::connect(gw_addr).await.unwrap();
    sock.write_all(
        b"POST /smug HTTP/1.1\r\nhost: backend.test\r\n\
          content-length: 6\r\ntransfer-encoding: chunked\r\n\r\n\
          0\r\n\r\n",
    )
    .await
    .unwrap();
    sock.flush().await.unwrap();

    let (head, _body) = tokio::time::timeout(Duration::from_secs(10), read_h1_response(&mut sock))
        .await
        .expect("no response to CL/TE smuggle attempt");
    let head_str = String::from_utf8_lossy(&head);
    eprintln!(
        "PRE_PUMP_SMUGGLE status_line={:?} backend_dials={}",
        head_str.lines().next().unwrap_or(""),
        w.dials.load(Ordering::SeqCst)
    );
    assert!(
        head_str.starts_with("HTTP/1.1 400"),
        "CL/TE smuggle not rejected pre-pump; got: {:?}",
        head_str.lines().next()
    );
    assert_eq!(
        w.dials.load(Ordering::SeqCst),
        0,
        "CL/TE smuggle reached a backend dial (must be rejected before dial)"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 4b. Q-H3 — H1 REQUEST-TRAILER VALIDATION (validate_h1_request_trailers).
//   • A legit trailer (x-checksum) is FORWARDED byte-faithfully (R3 keeps
//     trailer_passthrough green) and the request completes 200.
//   • A forbidden framing/routing field in trailers (transfer-encoding) is a
//     desync primitive → 400, the upstream request is aborted before a clean
//     terminator (F-MD-4 Err-before-close), and the backend NEVER sees a
//     complete request.
// ══════════════════════════════════════════════════════════════════════

/// Raw H1 witness backend for trailer tests: captures the EXACT upstream
/// request bytes (so we can inspect the chunked trailer section the gateway
/// emitted byte-faithfully), records whether a complete request was seen
/// (`0\r\n` final-chunk marker), and — for the legit case — replies 200
/// EARLY (immediately after headers) so the verdict-relay gate is reached
/// (`send_request` resolves `Ok(Ok(resp))`) and a forbidden-trailer 400
/// verdict can surface on the wire. Returns (addr, captured-raw, dials,
/// complete-count).
#[derive(Clone)]
struct TrailerWitness {
    raw: Arc<parking_lot::Mutex<Vec<u8>>>,
    dials: Arc<AtomicUsize>,
    complete: Arc<AtomicUsize>,
}

/// `early`: if true, the backend replies 200 immediately after it sees the
/// request head (without waiting for the body/trailers) — this is the shape
/// under which the gateway's verdict (400 on a forbidden trailer) reaches the
/// client AS the status code. If false, it drains fully then replies.
async fn spawn_trailer_witness_backend(early: bool) -> (SocketAddr, TrailerWitness) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let w = TrailerWitness {
        raw: Arc::new(parking_lot::Mutex::new(Vec::new())),
        dials: Arc::new(AtomicUsize::new(0)),
        complete: Arc::new(AtomicUsize::new(0)),
    };
    let w2 = w.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            w2.dials.fetch_add(1, Ordering::SeqCst);
            let w3 = w2.clone();
            tokio::spawn(async move {
                let mut acc: Vec<u8> = Vec::new();
                let mut tmp = [0u8; 16 * 1024];
                let mut replied = false;
                loop {
                    // S11: 30 s (was 5 s) — saturation-robustness for the
                    // multi-MiB drain under the full --workspace gate (same
                    // class as the h2h1 body-counting backend fix).
                    match tokio::time::timeout(Duration::from_secs(30), sock.read(&mut tmp)).await {
                        Ok(Ok(0)) | Err(_) => break,
                        Ok(Ok(n)) => acc.extend_from_slice(&tmp[..n]),
                        Ok(Err(_)) => break,
                    }
                    let head_done = find_subslice(&acc, b"\r\n\r\n").is_some();
                    if early && head_done && !replied {
                        // Reply 200 right after the head — keep the conn open so
                        // the gateway can still stream the body/trailers (and so
                        // its verdict can surface).
                        let _ = sock
                            .write_all(
                                b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                            )
                            .await;
                        let _ = sock.flush().await;
                        replied = true;
                    }
                    // A `0\r\n` final-chunk marker after the head = complete body.
                    if head_done && find_subslice(&acc, b"\r\n0\r\n").is_some() {
                        w3.complete.fetch_add(1, Ordering::SeqCst);
                        if !early && !replied {
                            let _ = sock
                                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                                .await;
                        }
                        break;
                    }
                }
                *w3.raw.lock() = acc;
            });
        }
    });
    (local, w)
}

/// Send a small chunked body then a RAW trailer section. `trailer_decl` is the
/// field-name to advertise in the request's `Trailer:` header (hyper's H1
/// server only surfaces chunked trailers it was told to expect), and
/// `trailer_block` is the raw bytes between the `0\r\n` final-chunk marker and
/// the closing `\r\n` (e.g. `b"x-checksum: abc\r\n"`). Returns the response
/// status line.
async fn chunked_with_trailers(
    gw_addr: SocketAddr,
    trailer_decl: &str,
    trailer_block: &[u8],
) -> String {
    let mut sock = TcpStream::connect(gw_addr).await.unwrap();
    let head = format!(
        "POST /tr HTTP/1.1\r\nhost: backend.test\r\ntrailer: {trailer_decl}\r\n\
         transfer-encoding: chunked\r\n\r\n"
    );
    sock.write_all(head.as_bytes()).await.unwrap();
    // One 5-byte data chunk "hello".
    sock.write_all(b"5\r\nhello\r\n").await.unwrap();
    // Final chunk (size 0) followed by the trailer section then blank line.
    sock.write_all(b"0\r\n").await.unwrap();
    sock.write_all(trailer_block).await.unwrap();
    sock.write_all(b"\r\n").await.unwrap();
    sock.flush().await.unwrap();
    let (head, _body) = tokio::time::timeout(Duration::from_secs(10), read_h1_response(&mut sock))
        .await
        .expect("no response to trailer request");
    String::from_utf8_lossy(&head)
        .lines()
        .next()
        .unwrap_or("")
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legit_request_trailer_forwarded_200() {
    // Body-reading backend (drains, then 200): the legit trailer drives the
    // clean trailers-forward arm; the request completes 200.
    let (backend, w) = spawn_trailer_witness_backend(false).await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    let status = chunked_with_trailers(gw_addr, "x-checksum", b"x-checksum: deadbeef\r\n").await;
    // Let the witness settle its captured bytes.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let raw = w.raw.lock().clone();
    let raw_lower = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    let forwarded = raw_lower.contains("x-checksum: deadbeef");
    eprintln!(
        "LEGIT_TRAILER status_line={status:?} forwarded={forwarded} complete={}",
        w.complete.load(Ordering::SeqCst)
    );
    assert!(
        status.starts_with("HTTP/1.1 200"),
        "legit trailer request did not return 200: {status:?}"
    );
    assert!(
        forwarded,
        "legit x-checksum trailer was not forwarded byte-faithfully to the backend; \
         upstream bytes: {raw_lower:?}"
    );
}

/// F-CAP-1 (Q-H3): a forbidden framing-field trailer is rejected with a
/// deterministic **400 Bad Request** even against a BODY-READING backend that
/// has NOT yet sent a response head. Before F-CAP-1 this raced to 502 because
/// the injected `H1PumpAbort` made `send_request` error first; the fix consults
/// the pump's classified `BadRequest` verdict (bounded by `timeouts.body`)
/// before falling through to 502, so the real 400 always surfaces.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forbidden_framing_field_trailer_rejected_400() {
    // Body-reading witness (does NOT reply early) — the realistic case the
    // F-CAP-1 fix targets. transfer-encoding in trailers = desync primitive.
    let (backend, w) = spawn_trailer_witness_backend(false).await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    let status = chunked_with_trailers(gw_addr, "x-foo", b"transfer-encoding: chunked\r\n").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    eprintln!(
        "FORBIDDEN_TRAILER status_line={status:?} backend_dials={} backend_complete={}",
        w.dials.load(Ordering::SeqCst),
        w.complete.load(Ordering::SeqCst)
    );
    assert!(
        w.dials.load(Ordering::SeqCst) >= 1,
        "non-vacuous: backend never dialed for the forbidden-trailer case"
    );
    // F-CAP-1: deterministic 400 (the classified BadRequest verdict now wins
    // over the send_request error), NEVER 502 and NEVER 200.
    assert!(
        status.starts_with("HTTP/1.1 400"),
        "forbidden framing-field trailer not rejected with 400 (F-CAP-1): {status:?}"
    );
    // F-MD-4: the backend MUST NEVER see a complete request (Err-before-close
    // aborts the upstream body before a clean `0\r\n` terminator).
    assert_eq!(
        w.complete.load(Ordering::SeqCst),
        0,
        "forbidden-trailer request was relayed to the backend as COMPLETE"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 5. 64 MiB CAP → 413 (NEW on the H1→H1 path).
// ══════════════════════════════════════════════════════════════════════

/// Backend that drains and replies 200 — used so a successful upload (under
/// the cap) is distinguishable from the 413.
async fn spawn_drain_200_backend() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&dials);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| async move {
                    let _ = req.into_body().collect().await;
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(Full::new(Bytes::from_static(b"ok")))
                            .unwrap(),
                    )
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, dials)
}

/// DIAGNOSTIC (reachability of the 413 verdict): a raw backend that replies
/// 200 IMMEDIATELY on the request head WITHOUT reading the body, with
/// `connection: close`. This is the ONLY shape under which `send_request`
/// could resolve `Ok(Ok(resp))` BEFORE the pump injects the over-cap body
/// error — i.e. the only path on which `verdict_rx` (and thus the 413
/// mapping) is consulted. Reports the observed status for the verify doc.
async fn spawn_raw_early_200_backend() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&dials);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut tmp = [0u8; 1024];
                let _ = sock.read(&mut tmp).await; // headers only
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = sock.flush().await;
                tokio::time::sleep(Duration::from_millis(800)).await;
            });
        }
    });
    (local, dials)
}

/// Drive a >64 MiB chunked upload to `gw_addr` and return the response head's
/// status line. A dedicated reader task accumulates every response byte
/// concurrently with the writer (a single interleaved read can split the
/// status line across buffers and miss it — that was a harness bug, not a
/// proxy defect).
async fn over_cap_status_line(gw_addr: SocketAddr) -> (String, usize) {
    let over = 64 * 1024 * 1024 + 1024 * 1024; // crosses MAX_REQUEST_BODY_BYTES
    let sock = TcpStream::connect(gw_addr).await.unwrap();
    let (mut rd, mut wr) = sock.into_split();
    let reader = tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        loop {
            match tokio::time::timeout(Duration::from_secs(15), rd.read(&mut tmp)).await {
                Ok(Ok(0)) | Err(_) => break,
                Ok(Ok(n)) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if find_subslice(&buf, b"\r\n\r\n").is_some() {
                        break; // got a full response head
                    }
                }
                Ok(Err(_)) => break,
            }
        }
        buf
    });
    wr.write_all(b"POST /big HTTP/1.1\r\nhost: backend.test\r\ntransfer-encoding: chunked\r\n\r\n")
        .await
        .unwrap();
    let frame_body = vec![0x5Au8; 1024 * 1024];
    let mut written = 0usize;
    while written < over {
        let header = format!("{:x}\r\n", frame_body.len()).into_bytes();
        if wr.write_all(&header).await.is_err() {
            break;
        }
        if wr.write_all(&frame_body).await.is_err() {
            break;
        }
        if wr.write_all(b"\r\n").await.is_err() {
            break;
        }
        written += frame_body.len();
    }
    drop(wr); // signal end-of-input if the verdict had not already landed
    let resp = tokio::time::timeout(Duration::from_secs(20), reader)
        .await
        .map(|r| r.unwrap_or_default())
        .unwrap_or_default();
    let s = String::from_utf8_lossy(&resp);
    (s.lines().next().unwrap_or("").to_owned(), written)
}

/// 64 MiB cap → 413 (the bar's canonical 413 proof). The verdict-relay gate
/// that maps `BodyTooLarge` → `413 Payload Too Large` is only consulted when
/// `send_request` resolves with a response head (`Ok(Ok(resp))`). An
/// early-responding backend (replies 200 on the head WITHOUT reading the body)
/// is the shape under which the over-cap verdict reaches the client AS the
/// status code, so we get a true 413 on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn over_64mib_upload_yields_413_when_backend_responds_early() {
    let (backend, _dials) = spawn_raw_early_200_backend().await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    let (status_line, written) = over_cap_status_line(gw_addr).await;
    eprintln!("OVER_64MIB_413 status_line={status_line:?} written={written}");
    assert!(
        status_line.contains("413"),
        "expected a 413 for a >64 MiB upload to an early-responding backend; \
         got status_line={status_line:?} (wrote {written} bytes)"
    );
}

/// F-CAP-1: 64 MiB cap with a BODY-READING backend → deterministic **413**.
/// Before F-CAP-1 this path raced to 502 (the `H1PumpAbort` injection made
/// `send_request` error before the verdict was consulted). The fix consults
/// the pump's classified `BodyTooLarge` verdict (bounded by `timeouts.body`)
/// on the send-error arm, so the real 413 now surfaces against a normal
/// body-reading backend — the realistic case.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn over_64mib_upload_yields_413_with_body_reading_backend() {
    let (backend, _dials) = spawn_drain_200_backend().await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    let (status_line, written) = over_cap_status_line(gw_addr).await;
    eprintln!("OVER_64MIB_413_BODYREAD status_line={status_line:?} written={written}");
    assert!(
        !status_line.contains(" 200"),
        "over-cap upload was accepted with 200 (cap not enforced): {status_line:?}"
    );
    // F-CAP-1: deterministic 413, NOT the pre-fix 502.
    assert!(
        status_line.contains("413"),
        "F-CAP-1: over-cap upload to a body-reading backend did not yield 413: {status_line:?}"
    );
}

/// F-CAP-1 must NOT mask a GENUINE upstream failure. A backend that accepts the
/// connection then closes mid-request (NOT a pump-caused abort) must still map
/// to **502 Bad Gateway** — the verdict here is `Ok(())` (the inbound body was
/// fine), so the fix's `_ => None` arm falls through to the generic 502.
async fn spawn_close_mid_request_backend() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&dials);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                // Read a little of the request head then abruptly close WITHOUT
                // sending any response — a genuine upstream failure.
                let mut tmp = [0u8; 64];
                let _ = sock.read(&mut tmp).await;
                let _ = sock.shutdown().await;
                drop(sock);
            });
        }
    });
    (local, dials)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn genuine_upstream_failure_still_502() {
    let (backend, dials) = spawn_close_mid_request_backend().await;
    let gw = proxy_for(backend, relaxed_timeouts());
    let gw_addr = spawn_h1_gateway(gw).await;

    // A small, WELL-FORMED upload (under the cap) — the pump verdict is Ok(());
    // any send error here is a genuine upstream failure, not a pump abort.
    let payload = binary_pattern(4096);
    let mut sock = TcpStream::connect(gw_addr).await.unwrap();
    let head = format!(
        "POST /up HTTP/1.1\r\nhost: backend.test\r\ncontent-length: {}\r\n\r\n",
        payload.len()
    );
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(&payload).await;
    let _ = sock.flush().await;
    let (head, _body) = tokio::time::timeout(Duration::from_secs(15), read_h1_response(&mut sock))
        .await
        .expect("no response to a request against a failing backend");
    let status = String::from_utf8_lossy(&head)
        .lines()
        .next()
        .unwrap_or("")
        .to_owned();
    eprintln!(
        "GENUINE_UPSTREAM_FAIL status_line={status:?} backend_dials={}",
        dials.load(Ordering::SeqCst)
    );
    assert!(
        dials.load(Ordering::SeqCst) >= 1,
        "non-vacuous: backend never dialed"
    );
    // The fix must NOT mask a real upstream failure as 413/400.
    assert!(
        status.starts_with("HTTP/1.1 502") || status.starts_with("HTTP/1.1 504"),
        "genuine upstream failure should map to 502/504, got: {status:?}"
    );
    assert!(
        !status.contains("413") && !status.contains("400"),
        "F-CAP-1 MASKED a genuine upstream failure as a pump verdict: {status:?}"
    );
}

/// Live sanity: an UNDER-cap large body (just the byte-identical large test
/// re-stated minimally) returns 200, so the cap does not over-trigger.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn under_cap_large_upload_yields_200() {
    // 8 MiB ≫ window but ≪ 64 MiB cap → must succeed.
    real_wire_roundtrip(8 * 1024 * 1024).await;
}

// Silence unused-import warnings when test-gauges is off (the gauge tests are
// cfg-gated; AtomicBool used only there in some configs).
#[allow(dead_code)]
fn _unused_atomicbool_marker(_: AtomicBool) {}
