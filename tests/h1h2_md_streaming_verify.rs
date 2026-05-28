//! S11 / H1→H2 (I2, D2) — INDEPENDENT verifier suite (author≠builder).
//!
//! Verifies builder's `proxy_h1_to_h2_request` (`crates/lb-l7/src/h1_proxy.rs`
//! :1586) on commit 9cc16b61: the H1 M-D-lite STREAMING REQUEST leg feeding
//! the SHARED I1 `drive_h2_upstream_send` egress driver.
//!
//! Cell wiring: a real H1 client (hyper H1 OR raw-TCP for adversarial framing)
//! → an `H1Proxy` listener wired with an `Http2Pool` upstream
//! (`with_multi_proto(...).with_h2_upstream(...)`) → a real H2 backend.
//!
//! Mirrors the promoted h2h2 suite, adapted to an H1 FRONT (the front
//! truncation semantics are H1 — Content-Length-short or chunked-without-
//! terminator — NOT an H2 RST_STREAM). The H2-side completeness witness is
//! `BackendSeen.complete` (true iff the backend's `body.frame()` returned
//! `None` cleanly; false on `Some(Err)` = the gateway RST the upstream stream).
//!
//! Retained-memory gauge: `lb_l7::h1_proxy::H1_REQ_MAX_RETAINED_BODY_BYTES`
//! (read under `--features test-gauges`). WINDOW = depth(8) × 8 KiB = 64 KiB.
//!
//! These tests DO NOT edit the source (except the R13(c) negative-control,
//! which is restored).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http2 as srv_h2;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use lb_io::Runtime;
use lb_io::http2_pool::{Http2Pool, Http2PoolConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts};
use lb_l7::upstream::{RoundRobinUpstreams, UpstreamBackend};
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const SAN_HOST: &str = "expressgateway.test";
const WINDOW: usize = 64 * 1024; // H1_REQ_CHANNEL_DEPTH(8) × H1_REQ_CHUNK_MAX(8 KiB)

// ── TCP pool plumbing (same as proto_translation_e2e) ───────────────────

fn build_tcp_pool() -> TcpPool {
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

fn relaxed_timeouts() -> HttpTimeouts {
    HttpTimeouts {
        header: Duration::from_secs(30),
        body: Duration::from_secs(120),
        total: Duration::from_secs(180),
        head: Duration::from_secs(180),
    }
}

/// Spawn a plaintext H1 gateway listener fronting an H2 backend at
/// `backend_addr` via an `Http2Pool` (H1Proxy::with_multi_proto +
/// with_h2_upstream — the canonical PROTO-001 H1→H2 wiring from
/// proto_translation_e2e::proxy_h1_listener_h2_backend). Returns the gw addr.
async fn spawn_h1_to_h2_listener_full(
    backend_addr: SocketAddr,
    pool_cfg: Http2PoolConfig,
    timeouts: HttpTimeouts,
) -> SocketAddr {
    let tcp_pool = build_tcp_pool();
    let h2_pool = Arc::new(Http2Pool::new(pool_cfg, tcp_pool.clone()));
    let picker =
        Arc::new(RoundRobinUpstreams::new(vec![UpstreamBackend::h2(backend_addr)]).unwrap());
    let h1_proxy = Arc::new(
        H1Proxy::with_multi_proto(
            tcp_pool, picker, None, timeouts, /* is_https = */ false,
        )
        .with_h2_upstream(Arc::clone(&h2_pool)),
    );
    assert!(
        h1_proxy.has_h2_upstream(),
        "listener must have an H2 upstream wired"
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let p = Arc::clone(&h1_proxy);
            tokio::spawn(async move {
                let _ = p.serve_connection(sock, peer).await;
            });
        }
    });
    local
}

async fn spawn_h1_to_h2_listener_cfg(
    backend_addr: SocketAddr,
    pool_cfg: Http2PoolConfig,
) -> SocketAddr {
    spawn_h1_to_h2_listener_full(backend_addr, pool_cfg, relaxed_timeouts()).await
}

async fn spawn_h1_to_h2_listener(backend_addr: SocketAddr) -> SocketAddr {
    spawn_h1_to_h2_listener_cfg(backend_addr, Http2PoolConfig::default()).await
}

/// Connect a genuine hyper plaintext-H1 client to the gateway. Returns the
/// SendRequest handle (the connection driver is spawned).
async fn connect_h1_client(
    gateway: SocketAddr,
) -> hyper::client::conn::http1::SendRequest<
    http_body_util::combinators::BoxBody<Bytes, hyper::Error>,
> {
    let stream = TcpStream::connect(gateway).await.unwrap();
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    sender
}

/// Deterministic non-UTF-8 byte pattern of length `n` (includes 0x00 and
/// bytes > 0x7f so a UTF-8 round-trip would corrupt it).
fn binary_pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + 17) % 256) as u8).collect()
}

/// Adapt a tokio mpsc receiver of body frames into a `futures` Stream.
fn recv_stream(
    mut rx: tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, hyper::Error>>,
) -> impl futures_util::Stream<Item = Result<Frame<Bytes>, hyper::Error>> {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ── H2 BACKENDS (reused from the promoted h2h2 suite) ───────────────────

/// Echo backend that records the FULL received request body (byte-identity at
/// the H2 upstream) and a `complete` flag set ONLY when the request body ended
/// CLEANLY (`None`); a `Some(Err)` (gateway RST the upstream stream) =
/// truncated → complete=false. This is the H1→H2 smuggle witness.
#[derive(Clone, Default)]
struct BackendSeen {
    body: Arc<Mutex<Vec<u8>>>,
    complete: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
}

async fn spawn_h2_echo(seen: BackendSeen) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let seen = seen.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let seen = seen.clone();
                    async move {
                        seen.requests.fetch_add(1, Ordering::SeqCst);
                        let mut got = Vec::new();
                        let mut body = req.into_body();
                        let mut clean = true;
                        loop {
                            match body.frame().await {
                                Some(Ok(f)) => {
                                    if let Some(d) = f.data_ref() {
                                        got.extend_from_slice(d);
                                    }
                                }
                                Some(Err(_)) => {
                                    clean = false;
                                    break;
                                }
                                None => break,
                            }
                        }
                        *seen.body.lock() = got.clone();
                        seen.complete.store(clean, Ordering::SeqCst);
                        let out = if got.is_empty() {
                            Bytes::from_static(b"h2-empty")
                        } else {
                            Bytes::from(got)
                        };
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("x-backend", "h2-echo")
                                .body(Full::new(out))
                                .unwrap(),
                        )
                    }
                });
                let _ = srv_h2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    local
}

/// Drain-then-respond backend for the F-CAP-1 over-cap proof: responds 200
/// after draining inline at echo speed. The gateway's in-pump 64 MiB cap fires
/// mid-stream → injects abort → F-CAP-1 verdict-preferred arm → 413.
async fn spawn_h2_drain_no_response() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let drained = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&drained);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let d3 = Arc::clone(&d2);
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let d4 = Arc::clone(&d3);
                    async move {
                        let mut body = req.into_body();
                        while let Some(Ok(f)) = body.frame().await {
                            if let Some(d) = f.data_ref() {
                                d4.fetch_add(d.len(), Ordering::SeqCst);
                            }
                        }
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::new())))
                    }
                });
                let mut b = srv_h2::Builder::new(TokioExecutor::new());
                b.initial_stream_window_size(8 * 1024 * 1024)
                    .initial_connection_window_size(16 * 1024 * 1024)
                    .max_concurrent_streams(64);
                let _ = b.serve_connection(TokioIo::new(sock), svc).await;
            });
        }
    });
    (local, drained)
}

/// Backend that binds then DROPS its listener so dials are refused — a genuine
/// upstream failure that must surface as 502 (NOT 413/400).
async fn dead_backend_addr() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    drop(listener);
    local
}

/// Backend that reads the request HEADERS but STALLS on the body until
/// released (the H2 service future parks without polling `into_body()`), then
/// drains + replies 200. Stalling an H2 backend closes its flow-control window
/// → the gateway's bounded in-flight window fills → backpressure.
#[cfg(feature = "test-gauges")]
async fn spawn_h2_stalled_backend() -> (SocketAddr, Arc<tokio::sync::Notify>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let r2 = Arc::clone(&release);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let r3 = Arc::clone(&r2);
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let r4 = Arc::clone(&r3);
                    async move {
                        r4.notified().await; // STALL: do not poll the body until released.
                        let mut body = req.into_body();
                        while let Some(f) = body.frame().await {
                            if f.is_err() {
                                break;
                            }
                        }
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .body(Full::new(Bytes::new()))
                                .unwrap(),
                        )
                    }
                });
                let _ = srv_h2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, release)
}

/// Gated-drain H2 backend: stalls reading the body until released, then drains
/// everything (counting bytes) and replies 200.
async fn spawn_h2_gated_drain() -> (SocketAddr, Arc<AtomicUsize>, Arc<tokio::sync::Notify>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let bytes_read = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let br2 = Arc::clone(&bytes_read);
    let r2 = Arc::clone(&release);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let br3 = Arc::clone(&br2);
            let r3 = Arc::clone(&r2);
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let br4 = Arc::clone(&br3);
                    let r4 = Arc::clone(&r3);
                    async move {
                        r4.notified().await; // STALL until released.
                        let mut body = req.into_body();
                        let mut total = 0;
                        while let Some(f) = body.frame().await {
                            match f {
                                Ok(fr) => {
                                    if let Some(d) = fr.data_ref() {
                                        total += d.len();
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        br4.fetch_add(total, Ordering::SeqCst);
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .body(Full::new(Bytes::from(total.to_string())))
                                .unwrap(),
                        )
                    }
                });
                let _ = srv_h2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, bytes_read, release)
}

// ══════════════════════════════════════════════════════════════════════
// 1. BYTE-IDENTITY — H1 client → H2 backend, body arrives verbatim upstream.
//    Branch A (≤window) and Branch B (5/8 MiB > window).
// ══════════════════════════════════════════════════════════════════════

async fn request_byte_identity(body_len: usize) {
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_h1_to_h2_listener(backend).await;
    let mut sender = connect_h1_client(gw).await;

    let payload = binary_pattern(body_len);
    // Stream the body as many small frames so >window bodies exercise the
    // bounded channel rather than a single hand-off.
    let chunks: Vec<Result<Frame<Bytes>, Infallible>> = payload
        .chunks(4096)
        .map(|c| Ok(Frame::data(Bytes::copy_from_slice(c))))
        .collect();
    let body = StreamBody::new(futures_util::stream::iter(chunks))
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(hyper::header::HOST, SAN_HOST)
        .body(body)
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(90), sender.send_request(req))
        .await
        .expect("response timed out")
        .expect("send_request failed");
    assert_eq!(resp.status(), StatusCode::OK, "body_len={body_len}");
    let got = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(got.len(), body_len, "response body length mismatch");
    assert_eq!(
        got.as_ref(),
        payload.as_slice(),
        "response body not byte-identical (body_len={body_len})"
    );
    // Byte-identity AT THE H2 BACKEND (the request leg delivered the verbatim
    // body upstream).
    let upstream = seen.body.lock().clone();
    assert_eq!(
        upstream.len(),
        body_len,
        "backend-observed request body length mismatch"
    );
    assert_eq!(
        upstream.as_slice(),
        payload.as_slice(),
        "backend-observed request body not byte-identical"
    );
    assert!(
        seen.complete.load(Ordering::SeqCst),
        "backend must observe a CLEAN (complete) request"
    );
    eprintln!(
        "REQ_BYTE_IDENTITY body_len={body_len} upstream_len={} status=200",
        upstream.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_branch_a_within_window_byte_identical() {
    request_byte_identity(1024).await; // 1 KiB ≤ 64 KiB → Branch A
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_branch_b_5mib_byte_identical() {
    request_byte_identity(5 * 1024 * 1024).await; // 5 MiB > window → Branch B
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_branch_b_8mib_byte_identical() {
    request_byte_identity(8 * 1024 * 1024).await; // 8 MiB > window → Branch B
}

// ══════════════════════════════════════════════════════════════════════
// 2. NON-VACUOUS MEMORY gauge + LOAD-BEARING inverted probe + live-occupancy.
//    Gauge = lb_l7::h1_proxy::H1_REQ_MAX_RETAINED_BODY_BYTES.
// ══════════════════════════════════════════════════════════════════════

#[cfg(feature = "test-gauges")]
static GAUGE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(feature = "test-gauges")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_gauge_non_vacuous_and_load_bearing() {
    use lb_l7::h1_proxy::{H1_REQ_MAX_RETAINED_BODY_BYTES, record_retained_h1};

    let _gauge_serial = GAUGE_SERIAL.lock().await;
    H1_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);

    let body_size = 4 * 1024 * 1024; // 4 MiB
    let (backend, release) = spawn_h2_stalled_backend().await;
    let cfg = Http2PoolConfig {
        send_timeout: Duration::from_secs(120),
        ..Http2PoolConfig::default()
    };
    let gw = spawn_h1_to_h2_listener_cfg(backend, cfg).await;
    let mut sender = connect_h1_client(gw).await;

    // Stream the body via a channel so the client keeps pushing under the H1
    // read window + the gateway's bounded window until the stalled backend
    // parks it. We do NOT await the response.
    let payload = binary_pattern(body_size);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, hyper::Error>>(4);
    let body = StreamBody::new(recv_stream(rx)).boxed();
    let req = Request::builder()
        .method("POST")
        .uri("/stall")
        .header(hyper::header::HOST, SAN_HOST)
        .body(body)
        .unwrap();
    let writer = tokio::spawn(async move {
        let mut off = 0;
        while off < payload.len() {
            let end = (off + 16 * 1024).min(payload.len());
            let chunk = Bytes::copy_from_slice(&payload[off..end]);
            if tokio::time::timeout(Duration::from_secs(2), tx.send(Ok(Frame::data(chunk))))
                .await
                .is_err()
            {
                break;
            }
            off = end;
        }
    });
    let _send = tokio::spawn(async move {
        let _ = sender.send_request(req).await;
    });

    // Reach steady-state under the stall.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let in_situ = H1_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);

    assert!(
        in_situ <= 4 * WINDOW,
        "retained gauge {in_situ} exceeds 4×window ({}); bounded-memory bar broken",
        4 * WINDOW
    );
    assert!(
        in_situ < body_size,
        "retained gauge {in_situ} not ≪ body size {body_size}"
    );

    // INVERTED PROBE (load-bearing): a whole-body buffering impl would call
    // record_retained_h1(body_size) → must push the gauge ABOVE 4×window, so
    // the bound above WOULD catch a buffering regression.
    record_retained_h1(body_size);
    let after_probe = H1_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    assert!(
        after_probe > 4 * WINDOW,
        "inverted probe failed: a whole-body retain of {body_size} did not \
         exceed the 4×window bound — the assertion would not catch a \
         buffering regression"
    );
    eprintln!(
        "MEMORY_GAUGE in_situ_retained_bytes={in_situ} body_size={body_size} window={WINDOW} \
         after_inverted_probe={after_probe}"
    );

    release.notify_waiters();
    drop(writer);
}

/// Live-occupancy (not cumulative): a full 4 MiB body streams through a FAST
/// echo backend. A no-decrement gauge would climb toward 4 MiB; the real
/// live-occupancy gauge stays ≤ 4×window. Body-size-INDEPENDENT.
#[cfg(feature = "test-gauges")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_gauge_tracks_live_occupancy_not_cumulative() {
    use lb_l7::h1_proxy::H1_REQ_MAX_RETAINED_BODY_BYTES;

    let _gauge_serial = GAUGE_SERIAL.lock().await;
    H1_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);

    let body_size = 4 * 1024 * 1024;
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_h1_to_h2_listener(backend).await;
    let mut sender = connect_h1_client(gw).await;

    let payload = binary_pattern(body_size);
    let chunks: Vec<Result<Frame<Bytes>, Infallible>> = payload
        .chunks(16 * 1024)
        .map(|c| Ok(Frame::data(Bytes::copy_from_slice(c))))
        .collect();
    let body = StreamBody::new(futures_util::stream::iter(chunks))
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(hyper::header::HOST, SAN_HOST)
        .body(body)
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(90), sender.send_request(req))
        .await
        .expect("timed out")
        .expect("send failed");
    assert_eq!(resp.status(), 200);
    let got = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(got.len(), body_size, "full 4 MiB must round-trip");
    assert_eq!(got.as_ref(), payload.as_slice());

    let peak = H1_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    assert!(
        peak <= 4 * WINDOW,
        "streaming gauge peak {peak} exceeds 4×window ({}) for a 4 MiB stream — \
         live-occupancy tracking is broken",
        4 * WINDOW
    );
    assert!(
        peak > 0,
        "streaming gauge never moved — the record site is unreachable (vacuous)"
    );
    eprintln!(
        "MEMORY_GAUGE_LIVE peak_retained={peak} (of 4 MiB streamed through, ≤4×window={})",
        4 * WINDOW
    );
}

// ══════════════════════════════════════════════════════════════════════
// 3. REQUEST-LEG BACKPRESSURE — H1 client send paused while H2 backend stalls,
//    then resume → full bytes + 200.
// ══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_backpressure_client_paused_while_backend_stalled() {
    let body_size = 48 * 1024 * 1024; // 48 MiB ≫ 64 KiB window (and < 64 MiB cap)
    let (backend, bytes_read, release) = spawn_h2_gated_drain().await;
    let cfg = Http2PoolConfig {
        send_timeout: Duration::from_secs(120),
        ..Http2PoolConfig::default()
    };
    let gw = spawn_h1_to_h2_listener_cfg(backend, cfg).await;
    let mut sender = connect_h1_client(gw).await;

    // Drive the request body from a channel so we can MEASURE how far the
    // client got before backpressure parked it. Causal chain: H2 backend stall
    // → upstream H2 window closed → gateway pump parks → mpsc full → H1 client
    // send paused.
    let payload = binary_pattern(body_size);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, hyper::Error>>(4);
    let body = StreamBody::new(recv_stream(rx)).boxed();
    let req = Request::builder()
        .method("POST")
        .uri("/bp")
        .header(hyper::header::HOST, SAN_HOST)
        .body(body)
        .unwrap();
    let send_task = tokio::spawn(async move {
        match tokio::time::timeout(Duration::from_secs(120), sender.send_request(req)).await {
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                let n = resp
                    .into_body()
                    .collect()
                    .await
                    .map(|c| c.to_bytes().len())
                    .unwrap_or(0);
                (Some(status), n)
            }
            _ => (None, 0),
        }
    });

    let pushed = Arc::new(AtomicUsize::new(0));
    let p2 = Arc::clone(&pushed);
    let payload_for_writer = payload.clone();
    let writer = tokio::spawn(async move {
        let mut off = 0;
        while off < payload_for_writer.len() {
            let end = (off + 64 * 1024).min(payload_for_writer.len());
            let chunk = Bytes::copy_from_slice(&payload_for_writer[off..end]);
            let clen = chunk.len();
            if tx.send(Ok(Frame::data(chunk))).await.is_err() {
                break;
            }
            p2.fetch_add(clen, Ordering::SeqCst);
            off = end;
        }
        // Clean end so the request can complete once the backend drains.
        let _ = tx.send(Ok(Frame::data(Bytes::new()))).await;
    });

    tokio::time::sleep(Duration::from_secs(2)).await;
    let paused_at = pushed.load(Ordering::SeqCst);
    eprintln!("REQ_BACKPRESSURE paused_at={paused_at} of body_size={body_size}");
    assert!(
        paused_at < body_size,
        "backpressure NOT applied: client pushed the whole 48 MiB body \
         ({paused_at} bytes) while the backend stalled — no bounded window"
    );
    assert!(
        paused_at <= 8 * 1024 * 1024,
        "backpressure too loose: client pushed {paused_at} bytes (> 8 MiB) \
         while the backend stalled — the in-flight window is not bounding"
    );
    #[cfg(feature = "test-gauges")]
    {
        let retained = lb_l7::h1_proxy::H1_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
        eprintln!("REQ_BACKPRESSURE retained_gauge={retained} (window={WINDOW})");
        assert!(
            retained <= 4 * WINDOW,
            "retained gauge {retained} exceeds 4×window under the stall — \
             whole-body buffering during backpressure"
        );
    }

    // RESUME: release the backend; the parked client unblocks and the FULL
    // 48 MiB body completes with the backend's 200.
    release.notify_waiters();
    let _ = writer.await;
    let (status, _resp_len) = send_task.await.unwrap_or((None, 0));
    let read = bytes_read.load(Ordering::SeqCst);
    eprintln!("REQ_BACKPRESSURE after_resume status={status:?} backend_read={read}");
    assert_eq!(
        status,
        Some(200),
        "after resume the request must complete with 200"
    );
    assert_eq!(
        read, body_size,
        "after resume the backend must drain the FULL body ({read} != {body_size})"
    );
}

/// Companion proof: a resumed stream COMPLETES byte-identical + 200 (proves
/// RESUME does not corrupt or truncate). 8 MiB through a fast echo backend.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_backpressure_resume_completes_byte_identical() {
    let body_size = 8 * 1024 * 1024;
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let cfg = Http2PoolConfig {
        send_timeout: Duration::from_secs(120),
        ..Http2PoolConfig::default()
    };
    let gw = spawn_h1_to_h2_listener_cfg(backend, cfg).await;
    let mut sender = connect_h1_client(gw).await;

    let payload = binary_pattern(body_size);
    let chunks: Vec<Result<Frame<Bytes>, Infallible>> = payload
        .chunks(16 * 1024)
        .map(|c| Ok(Frame::data(Bytes::copy_from_slice(c))))
        .collect();
    let body = StreamBody::new(futures_util::stream::iter(chunks))
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header(hyper::header::HOST, SAN_HOST)
        .body(body)
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(120), sender.send_request(req))
        .await
        .expect("timed out")
        .expect("send failed");
    assert_eq!(resp.status(), 200);
    let got = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(got.len(), body_size);
    assert_eq!(got.as_ref(), payload.as_slice(), "resume corrupted body");
    assert!(
        seen.complete.load(Ordering::SeqCst),
        "backend saw clean request"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 4. F-MD-4 (H1 mirror-image — KEY SMUGGLING TEST). A RAW H1 client truncates
//    the request body mid-stream, in BOTH framings:
//      (i)  Content-Length-short premature close,
//      (ii) chunked without terminator / premature TCP close.
//    The real H2 backend MUST observe the request as NEVER complete
//    (complete==0 across all iterations). HARDENED: current_thread runtime +
//    in-gate repetition loop + load-bearing non-vacuity baseline.
//
// Mechanism: a >window body forces Branch B (streaming pump). The mid-body
// truncation surfaces as `Some(Err)` from the inbound H1 body, the pump's
// `inject_abort!` injects H1PumpAbort into the upstream-body channel and holds
// tx open until hyper observes it (bounded), so hyper RSTs the upstream H2
// stream rather than emitting a clean END_STREAM. The detached send task
// (shared I1 driver) keeps the body alive across the H1-front cancel and
// reset_peers on the abort verdict. Either way the H2 backend's body.frame()
// yields Some(Err) → complete=false.
// ══════════════════════════════════════════════════════════════════════

const FMD4_SMUGGLE_ITERS: usize = 24;

/// Send a request HEAD then a >window partial body then DROP the TCP
/// connection mid-body (no terminator / unsatisfied Content-Length).
/// `framing` = "cl" | "chunked". Returns nothing — observation is on the
/// backend witness.
async fn truncated_upload(gw_addr: SocketAddr, framing: &str) {
    let mut sock = TcpStream::connect(gw_addr).await.unwrap();
    // Forward >window so the gateway crosses lookahead, dials the H2 pool, and
    // forwards the partial body upstream (Branch B), THEN truncate.
    let partial = binary_pattern(256 * 1024); // 256 KiB > 64 KiB window
    match framing {
        "cl" => {
            // Declare CL=4 MiB then write only 256 KiB and abruptly close.
            sock.write_all(
                format!(
                    "POST /trunc HTTP/1.1\r\nhost: {SAN_HOST}\r\ncontent-length: 4194304\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
            sock.write_all(&partial).await.unwrap();
        }
        "chunked" => {
            sock.write_all(
                format!(
                    "POST /trunc HTTP/1.1\r\nhost: {SAN_HOST}\r\ntransfer-encoding: chunked\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
            // Announce a 4 MiB chunk, write only 256 KiB, then close — no
            // chunk completion, no `0\r\n\r\n` terminator.
            sock.write_all(b"400000\r\n").await.unwrap(); // 0x400000 = 4 MiB
            sock.write_all(&partial).await.unwrap();
        }
        _ => unreachable!(),
    }
    sock.flush().await.unwrap();
    // Let the dial + initial forward land so the abort is genuinely mid-body.
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Abrupt half-close mid-body then drop.
    let _ = sock.shutdown().await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    drop(sock);
}

/// One real-wire smuggle attempt on a FRESH backend/listener. Returns
/// `(saw_complete, n_requests, backend_body_len)`. `saw_complete=true` is the
/// F-MD-4 smuggle DEFECT (a truncated request relayed to the H2 upstream as
/// cleanly finished).
async fn run_smuggle_once(framing: &str) -> (bool, usize, usize) {
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_h1_to_h2_listener(backend).await;

    truncated_upload(gw, framing).await;
    // Settle so any (defective) clean END_STREAM the gateway might emit
    // upstream is observed + recorded by the backend before we read the flag.
    tokio::time::sleep(Duration::from_millis(700)).await;

    (
        seen.complete.load(Ordering::SeqCst),
        seen.requests.load(Ordering::SeqCst),
        seen.body.lock().len(),
    )
}

async fn fmd4_smuggle_case(framing: &str) {
    // Non-vacuity baseline: a clean >window upload through a DEDICATED backend
    // records complete=true (so a vacuous "complete is always false" can't
    // pass). Uses a hyper H1 client (clean terminator).
    {
        let clean_seen = BackendSeen::default();
        let clean_backend = spawn_h2_echo(clean_seen.clone()).await;
        let clean_gw = spawn_h1_to_h2_listener(clean_backend).await;
        let mut sender = connect_h1_client(clean_gw).await;
        let payload = binary_pattern(256 * 1024);
        let chunks: Vec<Result<Frame<Bytes>, Infallible>> = payload
            .chunks(16 * 1024)
            .map(|c| Ok(Frame::data(Bytes::copy_from_slice(c))))
            .collect();
        let body = StreamBody::new(futures_util::stream::iter(chunks))
            .map_err(|never| match never {})
            .boxed();
        let req = Request::builder()
            .method("POST")
            .uri("/clean")
            .header(hyper::header::HOST, SAN_HOST)
            .body(body)
            .unwrap();
        let resp = tokio::time::timeout(Duration::from_secs(60), sender.send_request(req))
            .await
            .expect("clean baseline timed out")
            .expect("clean baseline send failed");
        assert_eq!(resp.status(), 200);
        let _ = resp.into_body().collect().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            clean_seen.complete.load(Ordering::SeqCst),
            "NON-VACUITY (framing={framing}): a clean >window upload must record \
             complete=true at the backend"
        );
        assert_eq!(
            clean_seen.requests.load(Ordering::SeqCst),
            1,
            "clean baseline must dial the backend exactly once"
        );
    }

    // Hardened smuggle loop: every iteration the truncated request MUST NOT be
    // observed as a COMPLETE request at the H2 upstream.
    let mut dialed_iters = 0usize;
    for iter in 0..FMD4_SMUGGLE_ITERS {
        let (saw_complete, n_requests, backend_body_len) = run_smuggle_once(framing).await;
        if n_requests >= 1 {
            dialed_iters += 1;
        }
        eprintln!(
            "FMD4 smuggle framing={framing} iter={iter}: backend_requests={n_requests} \
             saw_complete={saw_complete} backend_body_len={backend_body_len} \
             (H1 client truncated mid-body, no terminator)"
        );
        assert!(
            !saw_complete,
            "F-MD-4 DEFECT (framing={framing} iter {iter}): a truncated H1 upload \
             was seen as a COMPLETE request at the H2 upstream — the truncated \
             body ({backend_body_len} B) was relayed as finished"
        );
    }
    // Non-vacuity of the loop: the gateway must actually DIAL + forward to the
    // backend on the vast majority of iterations.
    eprintln!("FMD4 smuggle framing={framing}: dialed_iters={dialed_iters}/{FMD4_SMUGGLE_ITERS}");
    assert!(
        dialed_iters * 2 >= FMD4_SMUGGLE_ITERS,
        "smuggle path under-exercised (framing={framing}): only \
         {dialed_iters}/{FMD4_SMUGGLE_ITERS} iterations dialed the upstream \
         (Branch B not reached) — the loop is not testing the streaming pump"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fmd4_h1_cl_short_never_complete_at_h2_upstream() {
    fmd4_smuggle_case("cl").await;
}

#[tokio::test(flavor = "current_thread")]
async fn fmd4_h1_chunked_truncation_never_complete_at_h2_upstream() {
    fmd4_smuggle_case("chunked").await;
}

// ══════════════════════════════════════════════════════════════════════
// 5. F-CAP-1 — over-cap → 413, forbidden trailer → 400-class (no leak),
//    genuine upstream failure → 502. (M-B is shared with sibling cells.)
// ══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcap1_over_cap_upload_yields_413_not_502() {
    // Hyper plaintext-H1 client driving a 66 MiB chunked body; the backend
    // drains inline at echo speed. The gateway's in-pump 64 MiB cap fires
    // mid-stream → injects abort → the F-CAP-1 verdict-preferred arm in the
    // shared driver returns the classified BodyTooLarge → 413 (NOT 502).
    let (backend, drained) = spawn_h2_drain_no_response().await;
    let cfg = Http2PoolConfig {
        send_timeout: Duration::from_secs(300),
        ..Http2PoolConfig::default()
    };
    let timeouts = HttpTimeouts {
        header: Duration::from_secs(30),
        body: Duration::from_secs(300),
        total: Duration::from_secs(300),
        head: Duration::from_secs(300),
    };
    let gw = spawn_h1_to_h2_listener_full(backend, cfg, timeouts).await;
    let mut sender = connect_h1_client(gw).await;

    let over = 64 * 1024 * 1024 + 2 * 1024 * 1024; // 66 MiB > 64 MiB cap
    // Stream the body in 64 KiB frames so the pump's per-chunk cap check fires
    // mid-stream rather than buffering the whole 66 MiB client-side.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, hyper::Error>>(8);
    let body = StreamBody::new(recv_stream(rx)).boxed();
    let req = Request::builder()
        .method("POST")
        .uri("/overcap")
        .header(hyper::header::HOST, SAN_HOST)
        .body(body)
        .unwrap();
    let writer = tokio::spawn(async move {
        let chunk = Bytes::from(vec![0x5Au8; 64 * 1024]);
        let mut sent = 0usize;
        while sent < over {
            if tokio::time::timeout(
                Duration::from_secs(10),
                tx.send(Ok(Frame::data(chunk.clone()))),
            )
            .await
            .is_err()
            {
                break;
            }
            sent += chunk.len();
        }
    });
    let status =
        match tokio::time::timeout(Duration::from_secs(180), sender.send_request(req)).await {
            Ok(Ok(resp)) => Some(resp.status().as_u16()),
            Ok(Err(e)) => {
                eprintln!("FCAP1_OVER_CAP response errored: {e:?}");
                None
            }
            Err(_) => {
                eprintln!("FCAP1_OVER_CAP response timed out");
                None
            }
        };
    drop(writer);
    eprintln!(
        "FCAP1_OVER_CAP status={status:?} backend_drained={}",
        drained.load(Ordering::SeqCst)
    );
    assert_eq!(
        status,
        Some(413),
        "F-CAP-1: H1→H2 over-cap upload to a draining backend should yield 413 \
         (NOT 502), got {status:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcap1_over_window_forbidden_trailer_no_leak_not_502() {
    // Raw H1 chunked request with a >window body then a forbidden trailer
    // (transfer-encoding in trailers, RFC 7230 §4.1.2). The streaming pump's
    // validate_h1_request_trailers → BadRequest verdict + inject_abort must
    // surface as a 400-class rejection (or a stream/connection error), with NO
    // backend body relayed back (no leak), and crucially NOT a misleading 502
    // and NOT a complete 200 (smuggling).
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_h1_to_h2_listener(backend).await;

    let mut sock = TcpStream::connect(gw).await.unwrap();
    sock.write_all(
        format!("POST /big HTTP/1.1\r\nhost: {SAN_HOST}\r\ntransfer-encoding: chunked\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    // >window body across two 64 KiB+ chunks so Branch B (streaming) is entered.
    let part = binary_pattern(64 * 1024);
    for _ in 0..2 {
        sock.write_all(format!("{:x}\r\n", part.len()).as_bytes())
            .await
            .unwrap();
        sock.write_all(&part).await.unwrap();
        sock.write_all(b"\r\n").await.unwrap();
    }
    // Final chunk (0) + a FORBIDDEN trailer, then the trailer-section blank line.
    sock.write_all(b"0\r\ntransfer-encoding: chunked\r\n\r\n")
        .await
        .unwrap();
    sock.flush().await.unwrap();

    // Read whatever the gateway returns (status line) up to a small budget.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8 * 1024];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let status_line = loop {
        if let Some(p) = find_subslice(&buf, b"\r\n") {
            break Some(String::from_utf8_lossy(&buf[..p]).to_string());
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break None;
        }
        match tokio::time::timeout(left, sock.read(&mut tmp)).await {
            Ok(Ok(0)) | Err(_) => break None, // EOF / RST (a non-leak rejection)
            Ok(Ok(n)) => buf.extend_from_slice(&tmp[..n]),
            Ok(Err(_)) => break None,
        }
    };
    let status: Option<u16> = status_line.as_ref().and_then(|l| {
        l.strip_prefix("HTTP/1.1 ")
            .and_then(|r| r.split_whitespace().next())
            .and_then(|c| c.parse().ok())
    });
    // No backend body leaked downstream (the echo backend would echo `part`).
    let leaked = find_subslice(&buf, &part[..1024]).is_some();
    eprintln!(
        "FCAP1_FORBIDDEN_TRAILER status={status:?} status_line={status_line:?} leaked={leaked} \
         backend_complete={}",
        seen.complete.load(Ordering::SeqCst)
    );
    assert!(
        !leaked,
        "LEAK: backend DATA relayed downstream for a forbidden-trailer >window request"
    );
    assert_ne!(status, Some(502), "must NOT be a misleading 502");
    assert_ne!(
        status,
        Some(200),
        "must NOT relay a complete 200 (smuggling)"
    );
    assert!(
        matches!(status, None | Some(400)),
        "forbidden trailer (>window) must be rejected via RST/EOF (None) or a \
         gateway 400 — got {status:?}"
    );
    // The truncated/aborted request must never be a COMPLETE request upstream.
    assert!(
        !seen.complete.load(Ordering::SeqCst),
        "SMUGGLING: forbidden-trailer request relayed as COMPLETE to the H2 upstream"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcap1_genuine_upstream_failure_yields_502() {
    // Dead backend (port with no listener) → the Http2Pool dial fails →
    // H2ProxyErr::Upstream → 502. Proves F-CAP-1 does NOT blanket-413: a
    // genuine upstream failure with NO classified pump verdict stays 502.
    let backend = dead_backend_addr().await;
    let gw = spawn_h1_to_h2_listener(backend).await;
    let mut sender = connect_h1_client(gw).await;

    let body = Full::new(Bytes::from_static(b"small")) // Branch A, within window
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("POST")
        .uri("/dead")
        .header(hyper::header::HOST, SAN_HOST)
        .body(body)
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(30), sender.send_request(req))
        .await
        .expect("timed out")
        .expect("send_request transport error");
    eprintln!("FCAP1_502_CONTROL status={}", resp.status().as_u16());
    assert_eq!(
        resp.status(),
        StatusCode::BAD_GATEWAY,
        "genuine upstream (dial) failure must surface as 502, not 413/400"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 6. Branch-A within-window truncation = ZERO-DIAL reject (validate-before-
//    dial): an H1 client that truncates a ≤window CL-declared body must be
//    rejected with NO backend dial (the within-window Some(Err) returns before
//    any pool contact).
// ══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_branch_a_truncation_zero_dial() {
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_h1_to_h2_listener(backend).await;

    let mut sock = TcpStream::connect(gw).await.unwrap();
    // CL declares 16 KiB (≤ 64 KiB window) but we send only 4 KiB then close.
    sock.write_all(
        format!("POST /short HTTP/1.1\r\nhost: {SAN_HOST}\r\ncontent-length: 16384\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    sock.write_all(&binary_pattern(4096)).await.unwrap();
    sock.flush().await.unwrap();
    let _ = sock.shutdown().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let dials = seen.requests.load(Ordering::SeqCst);
    eprintln!("BRANCH_A_TRUNC backend_dials={dials} (must be 0 — zero-dial reject)");
    assert_eq!(
        dials, 0,
        "within-window truncation must be rejected BEFORE any backend dial \
         (validate-before-dial); backend was dialed {dials} times"
    );
    assert!(
        !seen.complete.load(Ordering::SeqCst),
        "within-window truncation must never reach the backend as complete"
    );
}
