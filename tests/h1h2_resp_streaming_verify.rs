//! S11 / H1←H2 (I3, D3) — INDEPENDENT verifier suite (author≠builder).
//!
//! Verifies builder's `upstream_response_to_h1` (`crates/lb-l7/src/h1_proxy.rs`
//! :2579) on commit 1fa40e26: the H1←H2 RESPONSE leg was converted from
//! `body.collect().await` (R8-violating: whole H2-backend response
//! materialised before relay) to a SYNCHRONOUS STREAMING relay — build the H1
//! head (status + `RESPONSE_HOP_BY_HOP` strip + lowercase + Alt-Svc inject)
//! then `body.boxed()` the H2 `Incoming` body.
//!
//! Cell wiring (REUSED from `h1h2_md_streaming_verify.rs`): a real H1 client
//! (hyper H1 OR raw-TCP for wire-byte inspection) → an `H1Proxy` listener wired
//! with an `Http2Pool` upstream (`with_multi_proto(...).with_h2_upstream(...)`)
//! → a real H2 backend.
//!
//! Checks:
//!  * `resp_large_5mib_byte_identical` / `resp_large_8mib_byte_identical` —
//!    ≥5 MiB binary response is relayed byte-for-byte to the H1 client.
//!  * `resp_streaming_head_before_full_body` — H2 backend sends head + an
//!    early body chunk then STALLS mid-body; the H1 client must receive the
//!    head + early bytes PROMPTLY (a `collect()` relay would withhold the head
//!    until the whole body arrived). Gauge-free streaming proof by
//!    construction, with a load-bearing negative control via a forced delay.
//!  * `resp_trailers_wire_behaviour` — H2 backend emits body + a terminal
//!    trailers HEADERS frame (`x-checksum`); a RAW-TCP H1 client reads the raw
//!    response bytes to DETERMINE empirically whether hyper's H1 encoder
//!    flushes the trailers (case i) or drops them (case ii) — closes D3.
//!
//! These tests DO NOT edit the source (except a restored negative control) and
//! DO NOT touch the request-leg verifier file.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http2 as srv_h2;
use hyper::service::service_fn;
use hyper::{HeaderMap, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use lb_io::Runtime;
use lb_io::http2_pool::{Http2Pool, Http2PoolConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts};
use lb_l7::upstream::{RoundRobinUpstreams, UpstreamBackend};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const SAN_HOST: &str = "expressgateway.test";

/// Adapt a tokio mpsc receiver of body frames into a `futures` Stream (mirror
/// of the helper in `h1h2_md_streaming_verify.rs`). Used to drive H2-backend
/// response bodies frame-by-frame from the spawned task.
fn recv_stream(
    mut rx: tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, Infallible>>,
) -> impl futures_util::Stream<Item = Result<Frame<Bytes>, Infallible>> {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

// ── TCP pool plumbing (mirror of h1h2_md_streaming_verify) ───────────────

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
/// `backend_addr` via an `Http2Pool` (canonical PROTO-001 H1→H2 wiring).
async fn spawn_h1_to_h2_listener(backend_addr: SocketAddr) -> SocketAddr {
    let tcp_pool = build_tcp_pool();
    let h2_pool = Arc::new(Http2Pool::new(Http2PoolConfig::default(), tcp_pool.clone()));
    let picker =
        Arc::new(RoundRobinUpstreams::new(vec![UpstreamBackend::h2(backend_addr)]).unwrap());
    let h1_proxy = Arc::new(
        H1Proxy::with_multi_proto(
            tcp_pool,
            picker,
            None,
            relaxed_timeouts(),
            /* is_https = */ false,
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

/// Genuine hyper plaintext-H1 client. Returns the SendRequest handle.
async fn connect_h1_client(
    gateway: SocketAddr,
) -> hyper::client::conn::http1::SendRequest<Full<Bytes>> {
    let stream = TcpStream::connect(gateway).await.unwrap();
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    sender
}

/// Deterministic non-UTF-8 byte pattern of length `n`.
fn binary_pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + 17) % 256) as u8).collect()
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ══════════════════════════════════════════════════════════════════════
// H2 BACKENDS — RESPONSE-leg variants.
// ══════════════════════════════════════════════════════════════════════

/// Backend that returns a fixed binary RESPONSE body (`Full`), drains the
/// request first so the H2 stream completes cleanly.
async fn spawn_h2_large_resp(resp_body: Vec<u8>) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let body = Arc::new(resp_body);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let body = Arc::clone(&body);
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let body = Arc::clone(&body);
                    async move {
                        let mut rb = req.into_body();
                        while let Some(f) = rb.frame().await {
                            if f.is_err() {
                                break;
                            }
                        }
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("x-backend", "h2-large-resp")
                                .body(Full::new(Bytes::from((*body).clone())))
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

/// Backend that streams the response HEAD + an EARLY body chunk, then STALLS
/// (holds the H2 stream open) until `release` is notified, then sends the
/// remainder + END_STREAM. Lets the verifier prove head+early-bytes arrive at
/// the H1 client BEFORE the full body.
async fn spawn_h2_stalled_response(
    early: Vec<u8>,
    rest: Vec<u8>,
) -> (SocketAddr, Arc<tokio::sync::Notify>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let early = Arc::new(early);
    let rest = Arc::new(rest);
    let r2 = Arc::clone(&release);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let early = Arc::clone(&early);
            let rest = Arc::clone(&rest);
            let r3 = Arc::clone(&r2);
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let early = Arc::clone(&early);
                    let rest = Arc::clone(&rest);
                    let r4 = Arc::clone(&r3);
                    async move {
                        // Drain request body to completion first.
                        let mut rb = req.into_body();
                        while let Some(f) = rb.frame().await {
                            if f.is_err() {
                                break;
                            }
                        }
                        // Response body stream: early chunk, STALL on the
                        // notify, then the rest. A spawned feeder yields the
                        // early frame immediately, awaits the release, then
                        // yields the remainder, over a bounded mpsc channel.
                        let early = (*early).clone();
                        let rest = (*rest).clone();
                        let (tx, rx) =
                            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, Infallible>>(1);
                        tokio::spawn(async move {
                            let _ = tx.send(Ok(Frame::data(Bytes::from(early)))).await;
                            r4.notified().await;
                            let _ = tx.send(Ok(Frame::data(Bytes::from(rest)))).await;
                        });
                        let body =
                            StreamBody::new(recv_stream(rx)).map_err(|e: Infallible| match e {});
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("x-backend", "h2-stalled-resp")
                                .body(BodyExt::boxed(body))
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

/// Backend that emits a response body chunk followed by a TERMINAL trailers
/// HEADERS frame (`x-checksum: abc123`) with END_STREAM on the trailers frame.
async fn spawn_h2_trailer_response(body_chunk: Vec<u8>) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let body_chunk = Arc::new(body_chunk);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let body_chunk = Arc::clone(&body_chunk);
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let body_chunk = Arc::clone(&body_chunk);
                    async move {
                        let mut rb = req.into_body();
                        while let Some(f) = rb.frame().await {
                            if f.is_err() {
                                break;
                            }
                        }
                        let data = (*body_chunk).clone();
                        let (tx, rx) =
                            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, Infallible>>(2);
                        tokio::spawn(async move {
                            let _ = tx.send(Ok(Frame::data(Bytes::from(data)))).await;
                            let mut tr = HeaderMap::new();
                            tr.insert("x-checksum", "abc123".parse().unwrap());
                            let _ = tx.send(Ok(Frame::trailers(tr))).await;
                        });
                        let body =
                            StreamBody::new(recv_stream(rx)).map_err(|e: Infallible| match e {});
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("x-backend", "h2-trailer-resp")
                                .body(BodyExt::boxed(body))
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

// ══════════════════════════════════════════════════════════════════════
// 1. LARGE RESPONSE BYTE-IDENTITY — H2 backend → H1 client, verbatim.
// ══════════════════════════════════════════════════════════════════════

async fn response_byte_identity(body_len: usize) {
    let payload = binary_pattern(body_len);
    let backend = spawn_h2_large_resp(payload.clone()).await;
    let gw = spawn_h1_to_h2_listener(backend).await;
    let mut sender = connect_h1_client(gw).await;

    let req = Request::builder()
        .method("GET")
        .uri("/big")
        .header(hyper::header::HOST, SAN_HOST)
        .body(Full::new(Bytes::new()))
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(90), sender.send_request(req))
        .await
        .expect("response timed out")
        .expect("send_request failed");
    assert_eq!(resp.status(), StatusCode::OK, "body_len={body_len}");
    assert_eq!(
        resp.headers()
            .get("x-backend")
            .and_then(|v| v.to_str().ok()),
        Some("h2-large-resp"),
        "response head not relayed from backend"
    );
    let got = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        got.len(),
        body_len,
        "RESPONSE body length mismatch (body_len={body_len})"
    );
    assert_eq!(
        got.as_ref(),
        payload.as_slice(),
        "RESPONSE body not byte-identical (body_len={body_len})"
    );
    eprintln!(
        "RESP_BYTE_IDENTITY body_len={body_len} got_len={} status=200",
        got.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resp_large_5mib_byte_identical() {
    response_byte_identity(5 * 1024 * 1024).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resp_large_8mib_byte_identical() {
    response_byte_identity(8 * 1024 * 1024).await;
}

// ══════════════════════════════════════════════════════════════════════
// 2. STREAMING PROOF — head + early bytes arrive BEFORE the full body.
//    A buffering collect() relay would withhold the head until the whole
//    body arrived; the streaming relay delivers head + early bytes promptly.
// ══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resp_streaming_head_before_full_body() {
    // Early chunk is small; the "rest" is large and is held until we release.
    let early = binary_pattern(4096);
    let rest = binary_pattern(2 * 1024 * 1024);
    let (backend, release) = spawn_h2_stalled_response(early.clone(), rest.clone()).await;
    let gw = spawn_h1_to_h2_listener(backend).await;
    let mut sender = connect_h1_client(gw).await;

    let t0 = Instant::now();
    let req = Request::builder()
        .method("GET")
        .uri("/stall")
        .header(hyper::header::HOST, SAN_HOST)
        .body(Full::new(Bytes::new()))
        .unwrap();

    // The RESPONSE HEAD must arrive promptly — well before we release the
    // stalled remainder. A collect() relay cannot return the head here because
    // the backend is holding the H2 stream open mid-body.
    let resp = tokio::time::timeout(Duration::from_secs(3), sender.send_request(req))
        .await
        .expect("HEAD did not arrive promptly — buffering relay (collect) would do this")
        .expect("send_request failed");
    let head_at = t0.elapsed();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("x-backend")
            .and_then(|v| v.to_str().ok()),
        Some("h2-stalled-resp"),
        "stalled-resp head not relayed"
    );

    // Pull body FRAMES incrementally. The EARLY chunk must arrive promptly
    // while the remainder is still stalled at the backend.
    let mut body = resp.into_body();
    let mut early_seen: Vec<u8> = Vec::new();
    let early_deadline = Instant::now() + Duration::from_secs(3);
    while early_seen.len() < early.len() {
        let frame = tokio::time::timeout(
            early_deadline.saturating_duration_since(Instant::now()),
            body.frame(),
        )
        .await
        .expect("EARLY body chunk did not arrive before remainder released — NOT streaming")
        .expect("body stream ended early")
        .expect("body frame error");
        if let Some(d) = frame.data_ref() {
            early_seen.extend_from_slice(d);
        }
    }
    let early_at = t0.elapsed();
    assert_eq!(
        &early_seen[..early.len()],
        early.as_slice(),
        "early body bytes corrupted"
    );
    // We have head + all early bytes and have NOT yet released the remainder.
    assert!(
        early_at < Duration::from_secs(3),
        "early bytes too slow (streaming should be sub-second): {early_at:?}"
    );

    // NEGATIVE CONTROL / load-bearing: the remainder is genuinely withheld
    // until we release. Confirm the next frame does NOT arrive within a short
    // window while the backend still stalls.
    let pre_release = tokio::time::timeout(Duration::from_millis(400), body.frame()).await;
    assert!(
        pre_release.is_err(),
        "remainder arrived before release — backend stall not load-bearing, proof vacuous"
    );

    // Release the backend; the remainder + END_STREAM now flow.
    release.notify_one();
    let mut total = early_seen;
    loop {
        let f = tokio::time::timeout(Duration::from_secs(30), body.frame())
            .await
            .expect("remainder timed out after release");
        match f {
            Some(fr) => {
                let fr = fr.expect("body frame error after release");
                if let Some(d) = fr.data_ref() {
                    total.extend_from_slice(d);
                }
            }
            None => break,
        }
    }
    let done_at = t0.elapsed();
    let mut expected = early.clone();
    expected.extend_from_slice(&rest);
    assert_eq!(total.len(), expected.len(), "full body length mismatch");
    assert_eq!(total, expected, "full body not byte-identical");
    eprintln!(
        "RESP_STREAMING head_at={head_at:?} early_at={early_at:?} done_at={done_at:?} \
         early_len={} total_len={} (head+early arrived while backend stalled; \
         pre-release frame timed out = remainder genuinely withheld)",
        early.len(),
        total.len()
    );
}

// ══════════════════════════════════════════════════════════════════════
// 3. TRAILER WIRE BEHAVIOUR (D3 / CF-RESP-1) — RAW-TCP H1 client reads the
//    actual response bytes off the wire and we DETERMINE case i / ii / iii.
// ══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resp_trailers_wire_behaviour() {
    let body_chunk = b"hello-trailer-body".to_vec();
    let backend = spawn_h2_trailer_response(body_chunk.clone()).await;
    let gw = spawn_h1_to_h2_listener(backend).await;

    // RAW-TCP H1 client: send a minimal valid HTTP/1.1 GET and read the raw
    // response bytes until the connection closes or we time out.
    let mut stream = TcpStream::connect(gw).await.unwrap();
    let req = format!("GET /trailers HTTP/1.1\r\nHost: {SAN_HOST}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut raw = Vec::new();
    // Read until EOF (Connection: close) or a quiet timeout. We loop reads with
    // a per-read timeout so a hung proxy (case iii) is detectable.
    loop {
        let mut buf = [0u8; 4096];
        match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf)).await {
            Ok(Ok(0)) => break, // clean EOF
            Ok(Ok(n)) => raw.extend_from_slice(&buf[..n]),
            Ok(Err(e)) => panic!("raw read error (case iii candidate): {e}"),
            Err(_) => {
                // Timed out waiting for more bytes. If we already have a
                // complete-looking response this is just the keep-alive idle;
                // otherwise it's a hang (case iii).
                break;
            }
        }
    }

    let text = String::from_utf8_lossy(&raw);
    eprintln!(
        "RESP_TRAILER_RAW_WIRE ({} bytes):\n----8<----\n{}\n---->8----",
        raw.len(),
        text
    );

    // Sanity: we got a well-formed status line + the body bytes (no corruption,
    // no hang, no panic → not case iii).
    assert!(
        text.starts_with("HTTP/1.1 200"),
        "expected 200 status line; got: {text:?}"
    );
    assert!(
        find_subslice(&raw, &body_chunk).is_some(),
        "response body chunk missing/corrupted on the wire (case iii)"
    );

    // Determine case i vs ii by looking for the trailer field on the wire.
    let has_trailer_field = find_subslice(&raw, b"x-checksum").is_some()
        || find_subslice(&raw, b"x-checksum: abc123").is_some()
        || find_subslice(&raw, b"abc123").is_some();
    let is_chunked = text
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let declares_trailer = text.to_ascii_lowercase().contains("trailer:");

    if has_trailer_field {
        eprintln!(
            "RESP_TRAILER_VERDICT = CASE (i): hyper's H1 encoder FLUSHED the \
             trailer onto the wire (chunked={is_chunked}, head-Trailer-decl={declares_trailer}). \
             H1←H2 response trailers relay for free."
        );
    } else {
        eprintln!(
            "RESP_TRAILER_VERDICT = CASE (ii): trailers DROPPED \
             (chunked={is_chunked}); no x-checksum on the wire. Matches nginx \
             default of not forwarding H2 response trailers to H1; bounded \
             documented behaviour, NOT a regression."
        );
    }

    // Whichever case holds, the response must be well-framed (not iii): body
    // present and either a chunked terminator or a content-length body.
    let has_chunk_terminator = find_subslice(&raw, b"0\r\n").is_some();
    let has_content_length = text.to_ascii_lowercase().contains("content-length:");
    assert!(
        has_chunk_terminator || has_content_length,
        "response not well-framed (no chunk terminator and no content-length) — case iii"
    );
}
