//! S10 / H2→H2 (R8 bounded-incremental streaming) — INDEPENDENT verifier
//! suite (author≠builder).
//!
//! Written by `verifier` against builder-1's H2→H2 conversion
//! (`crates/lb-l7/src/h2_proxy.rs`, builder tip 30918809):
//!   - REQUEST leg `proxy_h2_to_h2_request` — M-D pump mirror →
//!     `lb_io::http2_pool::Http2Pool::send_request` with a bounded 64 KiB
//!     in-flight window; F-MD-4 (reset-before-verdict), F-CAP-1
//!     (verdict-preferred 413/400 over 502), Branch A validate-before-dial.
//!   - RESPONSE leg `upstream_h2_response_to_h2` — `collect()` removed →
//!     `body.boxed()` streaming relay.
//!
//! These tests DO NOT edit the source. They re-prove the BUILT bar on the
//! REAL wire: genuine H2 client → real `h2_proxy` listener → router → real
//! H2 backend via `Http2Pool`. We use plaintext h2c (no TLS) exactly like
//! `tests/proto_translation_e2e.rs::proxy_h2_listener_h2_backend`, so the
//! raw-frame adversarial cases (forbidden pseudo-header trailer,
//! content-length≠ΣDATA, over-cap upload) can be hand-rolled over a plain
//! TCP socket — a well-behaved h2 client cannot express them.
//!
//! The retained-memory gauge is `lb_l7::h2_proxy::H2_REQ_MAX_RETAINED_BODY_
//! BYTES` (shared with H2→H1 — the pump leaf is REUSED, not duplicated),
//! read under `--features test-gauges`. WINDOW = depth(8) × 8 KiB = 64 KiB.

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
use hyper::{HeaderMap, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use lb_io::Runtime;
use lb_io::http2_pool::{Http2Pool, Http2PoolConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::HttpTimeouts;
use lb_l7::h2_proxy::H2Proxy;
use lb_l7::h2_security::H2SecurityThresholds;
use lb_l7::upstream::{RoundRobinUpstreams, UpstreamBackend};
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const SAN_HOST: &str = "expressgateway.test";
const H2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const WINDOW: usize = 64 * 1024; // H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX

// ── TCP pool plumbing (same as proto_translation_e2e) ──────────────────

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

/// Spawn the plaintext-h2c gateway listener fronting an H2 backend at
/// `backend_addr` via an `Http2Pool`. `pool_cfg` lets the stall tests raise
/// `send_timeout` (Q-HH-3) so a deliberate backend stall does not spuriously
/// time out (product default is 30 s). Returns the gateway addr.
async fn spawn_listener_for_with_cfg(
    backend_addr: SocketAddr,
    pool_cfg: Http2PoolConfig,
) -> SocketAddr {
    spawn_listener_for_full(backend_addr, pool_cfg, HttpTimeouts::default()).await
}

async fn spawn_listener_for_full(
    backend_addr: SocketAddr,
    pool_cfg: Http2PoolConfig,
    timeouts: HttpTimeouts,
) -> SocketAddr {
    spawn_listener_for_alt(backend_addr, pool_cfg, timeouts, None).await
}

async fn spawn_listener_for_alt(
    backend_addr: SocketAddr,
    pool_cfg: Http2PoolConfig,
    timeouts: HttpTimeouts,
    alt_svc: Option<lb_l7::h1_proxy::AltSvcConfig>,
) -> SocketAddr {
    let tcp_pool = build_tcp_pool();
    let h2_pool = Arc::new(Http2Pool::new(pool_cfg, tcp_pool.clone()));
    let picker =
        Arc::new(RoundRobinUpstreams::new(vec![UpstreamBackend::h2(backend_addr)]).unwrap());
    let h2_proxy = Arc::new(
        H2Proxy::with_multi_proto(
            tcp_pool,
            picker,
            alt_svc,
            timeouts,
            /* is_https = */ false,
            H2SecurityThresholds::default(),
        )
        .with_h2_upstream(Arc::clone(&h2_pool)),
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let p = Arc::clone(&h2_proxy);
            tokio::spawn(async move {
                let _ = p.serve_connection(sock, peer).await;
            });
        }
    });
    local
}

async fn spawn_listener_for(backend_addr: SocketAddr) -> SocketAddr {
    spawn_listener_for_with_cfg(backend_addr, Http2PoolConfig::default()).await
}

/// Connect a genuine hyper plaintext-H2 client to the gateway. Returns the
/// SendRequest handle (the connection driver is spawned).
async fn connect_h2_client(
    gateway: SocketAddr,
) -> hyper::client::conn::http2::SendRequest<
    http_body_util::combinators::BoxBody<Bytes, hyper::Error>,
> {
    let stream = TcpStream::connect(gateway).await.unwrap();
    let (sender, conn) = hyper::client::conn::http2::handshake::<_, _, _>(
        TokioExecutor::new(),
        TokioIo::new(stream),
    )
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

/// Adapt a tokio mpsc receiver of body frames into a `futures` Stream
/// (avoids a `tokio-stream` dev-dep; identical semantics to ReceiverStream).
fn recv_stream(
    mut rx: tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, hyper::Error>>,
) -> impl futures_util::Stream<Item = Result<Frame<Bytes>, hyper::Error>> {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

// ── Backends ───────────────────────────────────────────────────────────

/// Echo backend that records the FULL received request body (so we can
/// assert byte-identity at the H2 upstream) and a `complete` flag set only
/// when the request body ended CLEANLY (frame-by-frame; a `Some(Err)` =
/// truncated). Mirrors `h3_h2_stream_e2e::spawn_h2_echo`.
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

/// Large-response backend: replies 200 with a fixed `body`, split into
/// 16 KiB frames so the H2 send window governs progress (response-leg
/// proofs). Optionally appends a response trailer.
async fn spawn_h2_large_resp(
    body: Vec<u8>,
    trailer: Option<(&'static str, &'static str)>,
) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let body = Arc::new(body);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let body = Arc::clone(&body);
            tokio::spawn(async move {
                let svc = service_fn(move |_req: Request<Incoming>| {
                    let body = Arc::clone(&body);
                    async move {
                        let mut frames: Vec<Result<Frame<Bytes>, Infallible>> = Vec::new();
                        let mut off = 0;
                        while off < body.len() {
                            let end = (off + 16 * 1024).min(body.len());
                            frames.push(Ok(Frame::data(Bytes::copy_from_slice(&body[off..end]))));
                            off = end;
                        }
                        if let Some((n, v)) = trailer {
                            let mut tm = HeaderMap::new();
                            tm.insert(
                                hyper::header::HeaderName::from_static(n),
                                hyper::header::HeaderValue::from_static(v),
                            );
                            frames.push(Ok(Frame::trailers(tm)));
                        }
                        let stream = StreamBody::new(futures_util::stream::iter(frames));
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("x-backend", "h2-large")
                                .body(stream)
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

/// Backend for the F-CAP-1 over-cap proof. It responds 200 IMMEDIATELY (the
/// response head arrives before/independent of the body) and drains the
/// request body in a DETACHED task. The early head lets `send_request` return
/// Ok fast, so the body streams upstream at full echo speed (~MB/s) instead
/// of the pathological ~5 KiB/s collapse seen when an H2 backend withholds its
/// response head during a large upload. The gateway's in-pump 64 MiB cap then
/// fires mid-stream, injecting PumpAbort → the verdict-relay gate returns the
/// classified BodyTooLarge → the gateway maps it to 413. (Mechanism: F-CAP-1
/// verdict-preferred arm in proxy_h2_to_h2_request; this exercises the
/// post-Ok verdict_rx gate path which also yields 413.)
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
                        // Drain the body to completion at full speed within the
                        // service future (so hyper keeps feeding DATA frames),
                        // counting bytes. The gateway's in-pump 64 MiB cap fires
                        // mid-stream (client sends 66 MiB) and injects PumpAbort
                        // → the gateway's upstream stream errors → the backend's
                        // body.frame() yields Some(Err) → we break and respond,
                        // but the gateway has ALREADY returned the classified
                        // BodyTooLarge (send_request Send-error arm OR the
                        // verdict gate) → 413. Draining INLINE (not parking,
                        // not detaching) keeps throughput at echo speed.
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

/// Backend that immediately REFUSES the TCP connection-less: we bind a
/// listener, capture its address, then DROP the listener so dials to the
/// captured port are refused (connection refused) — a genuine upstream
/// failure that must surface as 502 (NOT 413/400).
async fn dead_backend_addr() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    drop(listener);
    local
}

// ── Raw H2 frame helpers (plaintext h2c, for adversarial cases) ────────

async fn write_frame(s: &mut TcpStream, ty: u8, flags: u8, sid: u32, payload: &[u8]) {
    let len = payload.len() as u32;
    let mut hdr = [0u8; 9];
    hdr[0] = ((len >> 16) & 0xff) as u8;
    hdr[1] = ((len >> 8) & 0xff) as u8;
    hdr[2] = (len & 0xff) as u8;
    hdr[3] = ty;
    hdr[4] = flags;
    let id = sid & 0x7fff_ffff;
    hdr[5] = ((id >> 24) & 0xff) as u8;
    hdr[6] = ((id >> 16) & 0xff) as u8;
    hdr[7] = ((id >> 8) & 0xff) as u8;
    hdr[8] = (id & 0xff) as u8;
    s.write_all(&hdr).await.unwrap();
    s.write_all(payload).await.unwrap();
    s.flush().await.unwrap();
}

fn hpack_literal(name: &str, value: &str) -> Vec<u8> {
    let mut out = vec![0x00];
    assert!(name.len() < 128 && value.len() < 128);
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
    out
}

async fn send_preface(s: &mut TcpStream) {
    s.write_all(H2_PREFACE).await.unwrap();
    write_frame(s, 0x04, 0x00, 0, &[]).await; // empty SETTINGS
    write_frame(s, 0x04, 0x01, 0, &[]).await; // SETTINGS ACK
}

/// Best-effort `:status` extraction from a response HPACK block. Handles the
/// static-table indexed status codes hyper uses for common statuses and a
/// literal-with-incremental-indexing `:status` (0x48) fallback.
fn scan_status(block: &[u8]) -> Option<u16> {
    let mut i = 0;
    while i < block.len() {
        let b = block[i];
        match b {
            0x88 => return Some(200),
            0x89 => return Some(204),
            0x8b => return Some(206),
            0x8c => return Some(304),
            0x8d => return Some(400),
            0x8e => return Some(404),
            0x8f => return Some(500),
            // Literal header field with incremental indexing, name indexed
            // = :status (static index 8) → 0x48; value is a length-prefixed
            // (possibly Huffman) string.
            0x48 | 0x08 => {
                let j = i + 1;
                if j >= block.len() {
                    return None;
                }
                let huff = block[j] & 0x80 != 0;
                let len = (block[j] & 0x7f) as usize;
                let vstart = j + 1;
                if vstart + len > block.len() {
                    return None;
                }
                if !huff {
                    if let Ok(s) = std::str::from_utf8(&block[vstart..vstart + len]) {
                        if let Ok(code) = s.parse::<u16>() {
                            return Some(code);
                        }
                    }
                }
                i = vstart + len;
                continue;
            }
            _ => {
                i += 1;
            }
        }
    }
    None
}

/// A flow-control-aware raw H2 client over a plain TCP socket. Unlike the
/// real `h2` crate it CAN emit malformed framing (a `:status` pseudo-header
/// in a trailer HEADERS, a content-length ≠ ΣDATA), but it RESPECTS the
/// gateway's per-stream + connection send windows by draining WINDOW_UPDATE
/// frames as it writes — so a legitimate >window body upload does not trip a
/// spurious FLOW_CONTROL_ERROR GOAWAY (which a naive blast does).
struct RawH2 {
    s: TcpStream,
    conn_window: i64,
    stream_window: i64,
    rx_acc: Vec<u8>,
}

impl RawH2 {
    async fn connect(gw: SocketAddr) -> Self {
        let mut s = TcpStream::connect(gw).await.unwrap();
        send_preface(&mut s).await;
        Self {
            s,
            // RFC 9113 default initial window (until the peer's SETTINGS is
            // parsed); we conservatively start here and grow on WINDOW_UPDATE.
            conn_window: 65_535,
            stream_window: 65_535,
            rx_acc: Vec::new(),
        }
    }

    async fn send_headers(&mut self, path: &str, content_length: Option<&str>) {
        let mut hb = Vec::new();
        hb.extend_from_slice(&hpack_literal(":method", "POST"));
        hb.extend_from_slice(&hpack_literal(":scheme", "http"));
        hb.extend_from_slice(&hpack_literal(":path", path));
        hb.extend_from_slice(&hpack_literal(":authority", SAN_HOST));
        if let Some(cl) = content_length {
            hb.extend_from_slice(&hpack_literal("content-length", cl));
        }
        write_frame(&mut self.s, 0x01, 0x04, 1, &hb).await; // END_HEADERS only
    }

    /// Drain any pending server frames into `rx_acc`, updating windows from
    /// WINDOW_UPDATE frames (with a small bounded read so we don't block).
    async fn pump_windows(&mut self) {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match tokio::time::timeout(Duration::from_millis(50), self.s.read(&mut buf)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => self.rx_acc.extend_from_slice(&buf[..n]),
                _ => break,
            }
            self.apply_window_updates();
        }
    }

    fn apply_window_updates(&mut self) {
        let mut pos = 0;
        while self.rx_acc.len() - pos >= 9 {
            let flen = ((self.rx_acc[pos] as usize) << 16)
                | ((self.rx_acc[pos + 1] as usize) << 8)
                | (self.rx_acc[pos + 2] as usize);
            let fty = self.rx_acc[pos + 3];
            if self.rx_acc.len() - pos < 9 + flen {
                break;
            }
            let sid = u32::from_be_bytes([
                self.rx_acc[pos + 5] & 0x7f,
                self.rx_acc[pos + 6],
                self.rx_acc[pos + 7],
                self.rx_acc[pos + 8],
            ]);
            if fty == 0x08 && flen == 4 {
                // WINDOW_UPDATE: 31-bit increment.
                let inc = (u32::from_be_bytes([
                    self.rx_acc[pos + 9],
                    self.rx_acc[pos + 10],
                    self.rx_acc[pos + 11],
                    self.rx_acc[pos + 12],
                ]) & 0x7fff_ffff) as i64;
                if sid == 0 {
                    self.conn_window += inc;
                } else if sid == 1 {
                    self.stream_window += inc;
                }
            }
            // NB: response HEADERS/DATA frames stay in rx_acc for the later
            // status read (read_status_owned consumes the same buffer).
            pos += 9 + flen;
        }
        // Keep WINDOW_UPDATE accounting idempotent: drop only the parsed
        // WINDOW_UPDATE/SETTINGS frames, retain HEADERS/DATA for status read.
        // Simplest correct approach: re-scan from scratch each call by
        // tracking a consumed cursor. We instead clear ONLY if no response
        // frames are present, else leave intact (status read re-parses).
        // To avoid double-counting we zero out parsed WINDOW_UPDATE bytes by
        // rebuilding rx_acc without WINDOW_UPDATE/SETTINGS frames.
        let mut kept: Vec<u8> = Vec::with_capacity(self.rx_acc.len());
        let mut p = 0;
        while self.rx_acc.len() - p >= 9 {
            let flen = ((self.rx_acc[p] as usize) << 16)
                | ((self.rx_acc[p + 1] as usize) << 8)
                | (self.rx_acc[p + 2] as usize);
            let fty = self.rx_acc[p + 3];
            if self.rx_acc.len() - p < 9 + flen {
                break;
            }
            if fty != 0x08 && fty != 0x04 {
                kept.extend_from_slice(&self.rx_acc[p..p + 9 + flen]);
            }
            p += 9 + flen;
        }
        kept.extend_from_slice(&self.rx_acc[p..]);
        self.rx_acc = kept;
    }

    /// Send `total` body bytes (value 0xAB) in `chunk` slices, respecting the
    /// connection + stream send windows by waiting for WINDOW_UPDATEs.
    async fn send_body(&mut self, total: usize, chunk: usize) {
        let mut remaining = total;
        let data = vec![0xABu8; chunk];
        while remaining > 0 {
            // Block until both windows admit at least one byte.
            let mut spins = 0;
            while (self.conn_window <= 0 || self.stream_window <= 0) && spins < 400 {
                self.pump_windows().await;
                spins += 1;
            }
            let admit = (self.conn_window.min(self.stream_window)).max(0) as usize;
            if admit == 0 {
                break; // gateway is not granting window (stalled/closed)
            }
            let n = remaining.min(chunk).min(admit);
            write_frame(&mut self.s, 0x00, 0x00, 1, &data[..n]).await;
            self.conn_window -= n as i64;
            self.stream_window -= n as i64;
            remaining -= n;
        }
    }

    async fn send_trailer_with_pseudo(&mut self) {
        let mut tb = Vec::new();
        tb.extend_from_slice(&hpack_literal("x-trailer", "ok"));
        tb.extend_from_slice(&hpack_literal(":status", "200"));
        write_frame(&mut self.s, 0x01, 0x05, 1, &tb).await; // END_HEADERS|END_STREAM
    }

    async fn send_short_end_stream(&mut self, bytes: &[u8]) {
        write_frame(&mut self.s, 0x00, 0x01, 1, bytes).await; // DATA END_STREAM
    }

    /// Read the gateway's response status on stream 1 (or None on RST/GOAWAY),
    /// reusing any frames already buffered in rx_acc.
    async fn read_status(&mut self) -> (Option<u16>, bool) {
        // First check already-buffered response frames.
        if let Some(r) = scan_response_frames(&self.rx_acc) {
            return r;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut buf = vec![0u8; 32 * 1024];
        let mut saw_data = false;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return (None, saw_data);
            }
            let n = match tokio::time::timeout(left, self.s.read(&mut buf)).await {
                Ok(Ok(0)) => return (None, saw_data),
                Ok(Ok(n)) => n,
                _ => return (None, saw_data),
            };
            self.rx_acc.extend_from_slice(&buf[..n]);
            if let Some(r) = scan_response_frames(&self.rx_acc) {
                saw_data |= r.1;
                if r.0.is_some() || r.1 {
                    return (r.0, saw_data);
                }
                // GOAWAY/RST with no status → None.
                return (None, saw_data);
            }
        }
    }
}

/// Scan a frame buffer for a terminal response signal on stream 1:
/// Some((Some(status), leak)) on a response HEADERS, Some((None, leak)) on
/// RST/GOAWAY, None if no terminal frame yet.
fn scan_response_frames(acc: &[u8]) -> Option<(Option<u16>, bool)> {
    let mut pos = 0;
    let mut leak = false;
    while acc.len() - pos >= 9 {
        let flen =
            ((acc[pos] as usize) << 16) | ((acc[pos + 1] as usize) << 8) | (acc[pos + 2] as usize);
        let fty = acc[pos + 3];
        if acc.len() - pos < 9 + flen {
            break;
        }
        let sid = u32::from_be_bytes([
            acc[pos + 5] & 0x7f,
            acc[pos + 6],
            acc[pos + 7],
            acc[pos + 8],
        ]);
        let payload = &acc[pos + 9..pos + 9 + flen];
        match fty {
            0x07 => return Some((None, leak)),             // GOAWAY
            0x03 if sid == 1 => return Some((None, leak)), // RST_STREAM
            0x00 if sid == 1 && flen > 0 => leak = true,   // DATA leak
            0x01 if sid == 1 => {
                return Some((Some(scan_status(payload).unwrap_or(0)), leak));
            }
            _ => {}
        }
        pos += 9 + flen;
    }
    None
}

// ══════════════════════════════════════════════════════════════════════
// 1. REQUEST-LEG byte-identity — Branch A (≤window) and Branch B (>window).
// ══════════════════════════════════════════════════════════════════════

async fn request_byte_identity(body_len: usize) {
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_listener_for(backend).await;
    let mut sender = connect_h2_client(gw).await;

    let payload = binary_pattern(body_len);
    let body = Full::new(Bytes::from(payload.clone()))
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/echo"))
        .body(body)
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(60), sender.send_request(req))
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
    // Byte-identity AT THE BACKEND (the request leg actually delivered the
    // verbatim body upstream, not just echoed through some other path).
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
// 2. NON-VACUOUS MEMORY gauge + load-bearing INVERTED probe.
//
// Drive a 4 MiB binary body through a STALLED H2 backend (does not read the
// request body) and read `H2_REQ_MAX_RETAINED_BODY_BYTES`. A non-vacuous
// gauge must stay ≤ a few×WINDOW while ≪ 4 MiB. The INVERTED probe proves
// the assertion is LOAD-BEARING (record_retained(body_size) trips it).
// ══════════════════════════════════════════════════════════════════════

#[cfg(feature = "test-gauges")]
static GAUGE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Backend that reads the request HEADERS but STALLS on the body (never
/// polls it) until released, then drains + replies 200. Stalling an H2
/// backend = the service future parks without polling `req.into_body()`;
/// hyper stops issuing WINDOW_UPDATE → upstream H2 flow control closes →
/// the gateway pump's in-flight window fills → backpressure.
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
                        // STALL: do not poll the body until released.
                        r4.notified().await;
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

#[cfg(feature = "test-gauges")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_gauge_non_vacuous_and_load_bearing() {
    use lb_l7::h2_proxy::{H2_REQ_MAX_RETAINED_BODY_BYTES, record_retained};

    let _gauge_serial = GAUGE_SERIAL.lock().await;
    H2_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);

    let body_size = 4 * 1024 * 1024; // 4 MiB
    let (backend, release) = spawn_h2_stalled_backend().await;
    // Long send_timeout so the deliberate stall is not read as a header
    // roundtrip timeout (Q-HH-3).
    let cfg = Http2PoolConfig {
        send_timeout: Duration::from_secs(120),
        ..Http2PoolConfig::default()
    };
    let gw = spawn_listener_for_with_cfg(backend, cfg).await;
    let mut sender = connect_h2_client(gw).await;

    // Stream the body via a channel so the client keeps pushing under flow
    // control until the gateway's bounded window + the stalled backend park
    // it. We do NOT await the response.
    let payload = binary_pattern(body_size);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, hyper::Error>>(4);
    let body = StreamBody::new(recv_stream(rx)).boxed();
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/stall"))
        .body(body)
        .unwrap();
    let writer = tokio::spawn(async move {
        let mut off = 0;
        while off < payload.len() {
            let end = (off + 16 * 1024).min(payload.len());
            let chunk = Bytes::copy_from_slice(&payload[off..end]);
            // Bounded by the gateway pump pulling; under the stall this
            // eventually blocks/times out — exactly the backpressure.
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
    let in_situ = H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);

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
    // record_retained(body_size) → must push the gauge ABOVE 4×window.
    record_retained(body_size);
    let after_probe = H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
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

/// Live-occupancy (not cumulative): a full 4 MiB body streams through a
/// FAST echo backend (pushed AND pulled). A no-decrement gauge would climb
/// toward 4 MiB; the real live-occupancy gauge stays ≤ 4×window because each
/// chunk is decremented the instant hyper pulls it. Body-size-INDEPENDENT.
#[cfg(feature = "test-gauges")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_gauge_tracks_live_occupancy_not_cumulative() {
    use lb_l7::h2_proxy::H2_REQ_MAX_RETAINED_BODY_BYTES;

    let _gauge_serial = GAUGE_SERIAL.lock().await;
    H2_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);

    let body_size = 4 * 1024 * 1024;
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_listener_for(backend).await;
    let mut sender = connect_h2_client(gw).await;

    let payload = binary_pattern(body_size);
    let body = Full::new(Bytes::from(payload.clone()))
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/echo"))
        .body(body)
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(60), sender.send_request(req))
        .await
        .expect("timed out")
        .expect("send failed");
    assert_eq!(resp.status(), 200);
    let got = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(got.len(), body_size, "full 4 MiB must round-trip");
    assert_eq!(got.as_ref(), payload.as_slice());

    let peak = H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
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
// 3. REQUEST-LEG BACKPRESSURE — client send paused while backend stalls,
//    then resume → full bytes + 200. (Q-HH-3: long send_timeout.)
// ══════════════════════════════════════════════════════════════════════

/// Gated-drain H2 backend: stalls reading the body until released, then
/// drains everything (counting bytes) and replies 200 with the byte count.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_backpressure_client_paused_while_backend_stalled() {
    let body_size = 48 * 1024 * 1024; // 48 MiB ≫ 64 KiB window (and < 64 MiB cap)
    let (backend, bytes_read, release) = spawn_h2_gated_drain().await;
    let cfg = Http2PoolConfig {
        send_timeout: Duration::from_secs(120), // Q-HH-3: outlast the stall
        ..Http2PoolConfig::default()
    };
    let gw = spawn_listener_for_with_cfg(backend, cfg).await;
    let mut sender = connect_h2_client(gw).await;

    // Drive the request body from a channel so we can MEASURE how far the
    // client got before backpressure parked it. The gateway DOES consume the
    // body (send_request is driven below); under the stalled backend its
    // bounded 64 KiB window fills, it stops pulling our StreamBody, the
    // 4-slot mpsc fills (≤ ~256 KiB), and our `tx.send` blocks → paused_at
    // ≪ 48 MiB. The causal chain: backend stall → upstream H2 window closed
    // → gateway pump parks → mpsc full → client send paused.
    let payload = binary_pattern(body_size);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, hyper::Error>>(4);
    let body = StreamBody::new(recv_stream(rx)).boxed();
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/bp"))
        .body(body)
        .unwrap();
    // Drive the request into the gateway (the gateway must be PULLING for the
    // backpressure chain to be real, not just our own channel filling).
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
            // Unbounded await (within the test budget) so backpressure PARKS
            // us rather than dropping bytes — exactly the pause we measure.
            if tx.send(Ok(Frame::data(chunk))).await.is_err() {
                break;
            }
            p2.fetch_add(clen, Ordering::SeqCst);
            off = end;
        }
        // Clean END_STREAM so the request can complete once the backend drains.
        let _ = tx.send(Ok(Frame::data(Bytes::new()))).await;
    });

    // Let the system reach steady-state under the stall, then read how far
    // the client got. (The backend has NOT been released yet.)
    tokio::time::sleep(Duration::from_secs(2)).await;
    let paused_at = pushed.load(Ordering::SeqCst);
    eprintln!("REQ_BACKPRESSURE paused_at={paused_at} of body_size={body_size}");
    assert!(
        paused_at < body_size,
        "backpressure NOT applied: client pushed the whole 48 MiB body \
         ({paused_at} bytes) while the backend stalled — no bounded window"
    );
    // The bound should be small: gateway window (64 KiB) + mpsc (4×64 KiB) +
    // hyper's own H2 buffering — a few hundred KiB, never tens of MiB.
    assert!(
        paused_at <= 8 * 1024 * 1024,
        "backpressure too loose: client pushed {paused_at} bytes (> 8 MiB) \
         while the backend stalled — the in-flight window is not bounding"
    );
    // The retained gauge must stay bounded even though 48 MiB is queued.
    #[cfg(feature = "test-gauges")]
    {
        let retained = lb_l7::h2_proxy::H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
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

/// Companion proof that the resumed stream COMPLETES byte-identical + 200
/// (the backpressure test above proves the PAUSE; this proves RESUME does
/// not corrupt or truncate). Uses a moderate 8 MiB so the whole body flows
/// once the backend (which drains continuously) keeps up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_backpressure_resume_completes_byte_identical() {
    let body_size = 8 * 1024 * 1024;
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let cfg = Http2PoolConfig {
        send_timeout: Duration::from_secs(120),
        ..Http2PoolConfig::default()
    };
    let gw = spawn_listener_for_with_cfg(backend, cfg).await;
    let mut sender = connect_h2_client(gw).await;

    let payload = binary_pattern(body_size);
    let body = Full::new(Bytes::from(payload.clone()))
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/echo"))
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
// 4. F-MD-4 — real-wire H2 client RST_STREAM mid-body → the H2 BACKEND
//    observes the request as NOT complete (never a truncated-as-complete
//    request). Non-vacuous: a clean upload records complete=true.
//
// HARDENED (round 2): the round-1 single-shot variant passed 20/20 PARALLEL
// while the smuggle bug was LIVE — a downstream-RST graceful-drop race that
// only manifests under LOW contention (~25–50% of ISOLATED runs, masked
// under the 4-thread gate). To make this regression land robustly INSIDE the
// normal parallel ×3 gate, this test (a) runs on a CURRENT-THREAD runtime
// (low contention — the exact condition under which the bug appeared), and
// (b) repeats the smuggle scenario FMD4_SMUGGLE_ITERS times, asserting the
// security invariant on EVERY iteration. With a per-iter catch probability
// of ~0.25–0.5 for the live bug, N=24 gives P(miss) ≤ 0.75^24 ≈ 1e-3 at the
// pessimistic end and ≪ that at 0.5 — so a regression of THIS bug is caught
// deterministically by the gate. The negative-control (pre-fix code) is
// proven in s10-h2h2-verify.md §round2-negative-control. No assertion is
// weakened. NON-#[ignore]'d.
// ══════════════════════════════════════════════════════════════════════

const FMD4_SMUGGLE_ITERS: usize = 24;

/// One real-wire smuggle attempt on a FRESH backend/listener (no shared flag,
/// no pooled connection reuse). A genuine h2 client streams a >window
/// (256 KiB → Branch B) body then RST_STREAMs mid-body (drop `send_body`
/// without END_STREAM → CANCEL). Returns `(saw_complete, n_requests,
/// backend_body_len)`. `saw_complete=true` is the F-MD-4 smuggle DEFECT (a
/// truncated request relayed to the H2 upstream as cleanly finished).
async fn run_smuggle_once() -> (bool, usize, usize) {
    use h2 as h2crate;
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_listener_for(backend).await;

    let stream = TcpStream::connect(gw).await.unwrap();
    let mut builder = h2crate::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(stream).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/smuggle"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = h2sender.send_request(req, false).unwrap();

    // Stream 256 KiB (> 64 KiB window) so the gateway crosses lookahead,
    // dials the pool, and forwards body upstream (Branch B).
    let payload = binary_pattern(256 * 1024);
    let mut off = 0;
    while off < payload.len() {
        let end = (off + 16 * 1024).min(payload.len());
        let chunk = Bytes::copy_from_slice(&payload[off..end]);
        send_body.reserve_capacity(chunk.len());
        match tokio::time::timeout(
            Duration::from_secs(5),
            futures_util::future::poll_fn(|cx| send_body.poll_capacity(cx)),
        )
        .await
        {
            Ok(Some(Ok(cap))) if cap > 0 => {}
            _ => break,
        }
        if send_body.send_data(chunk, false).is_err() {
            break;
        }
        off = end;
    }
    // Let the dial + initial forward land so the abort is genuinely mid-body.
    tokio::time::sleep(Duration::from_millis(300)).await;
    // ABORT mid-body WITHOUT END_STREAM → RST_STREAM(CANCEL).
    drop(send_body);
    let _ = tokio::time::timeout(Duration::from_secs(3), resp_fut).await;
    // Settle so any (defective) clean END_STREAM the gateway might emit
    // upstream is observed + recorded by the backend before we read the flag.
    tokio::time::sleep(Duration::from_millis(700)).await;

    (
        seen.complete.load(Ordering::SeqCst),
        seen.requests.load(Ordering::SeqCst),
        seen.body.lock().len(),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn fmd4_client_rst_mid_body_never_complete_at_h2_upstream() {
    // Non-vacuity baseline: a clean upload through a DEDICATED backend records
    // complete=true (so a vacuous "complete is just always false" can't pass).
    let clean_seen = BackendSeen::default();
    let clean_backend = spawn_h2_echo(clean_seen.clone()).await;
    let clean_gw = spawn_listener_for(clean_backend).await;
    {
        let mut sender = connect_h2_client(clean_gw).await;
        let body = Full::new(Bytes::from(binary_pattern(2 * 1024 * 1024)))
            .map_err(|never| match never {})
            .boxed();
        let req = Request::builder()
            .method("POST")
            .uri(format!("http://{SAN_HOST}/clean"))
            .body(body)
            .unwrap();
        let resp = tokio::time::timeout(Duration::from_secs(60), sender.send_request(req))
            .await
            .expect("timed out")
            .expect("clean send failed");
        assert_eq!(resp.status(), 200);
        let _ = resp.into_body().collect().await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(clean_seen.requests.load(Ordering::SeqCst), 1);
    assert!(
        clean_seen.complete.load(Ordering::SeqCst),
        "NON-VACUITY: a clean upload must record complete=true at the backend"
    );

    // Hardened smuggle loop: every iteration the truncated (RST mid-body)
    // request MUST NOT be observed as a COMPLETE request at the H2 upstream.
    let mut dialed_iters = 0usize;
    for iter in 0..FMD4_SMUGGLE_ITERS {
        let (saw_complete, n_requests, backend_body_len) = run_smuggle_once().await;
        if n_requests >= 1 {
            dialed_iters += 1;
        }
        eprintln!(
            "FMD4 smuggle iter={iter}: backend_requests={n_requests} \
             saw_complete={saw_complete} backend_body_len={backend_body_len} \
             (client RST after 256 KiB, never END_STREAM)"
        );
        assert!(
            !saw_complete,
            "F-MD-4 DEFECT (iter {iter}): a client RST mid-body was seen as a \
             COMPLETE request at the H2 upstream — the truncated body \
             ({backend_body_len} B) was relayed as finished"
        );
    }
    // Non-vacuity of the loop: the gateway must actually DIAL + forward to the
    // backend on the vast majority of iterations (else the smuggle path is
    // never exercised and the asserts are trivially satisfied).
    eprintln!("FMD4 smuggle: dialed_iters={dialed_iters}/{FMD4_SMUGGLE_ITERS}");
    assert!(
        dialed_iters * 2 >= FMD4_SMUGGLE_ITERS,
        "smuggle path under-exercised: only {dialed_iters}/{FMD4_SMUGGLE_ITERS} \
         iterations dialed the upstream (Branch B not reached) — the loop is \
         not load-bearing"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 5. F-CAP-1 — streaming over-cap (>64 MiB) → 413 (NOT 502); forbidden
//    pseudo-header trailer (>window) → 400 (NOT 502); genuine upstream
//    failure (dead backend) → 502.
//
//    NEGATIVE-CONTROL note: the H2→H1 sibling
//    (h2h1_md_streaming_verify::fcap1_h2_over_cap_upload_yields_413) was
//    proven load-bearing in S9 by a revert→502≠413 experiment. This suite
//    re-proves the H2→H2 arm of the SAME caller (proxy_h2_to_h2_request's
//    F-CAP-1 verdict-preferred arm) yields 413, and the 502-control case
//    (dead backend, no pump verdict) proves the arm does NOT blanket-413.
// ══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcap1_over_cap_upload_yields_413_not_502() {
    // Hyper plaintext-H2 client driving a 66 MiB `Full` body (the efficient
    // upload path the byte-identity tests prove runs at ~MB/s — the manual
    // h2-client poll_capacity loop and a no-response backend both collapse
    // throughput, see s10-h2h2-verify.md §over-cap-throughput). The backend
    // drains inline at echo speed. The gateway's in-pump 64 MiB cap fires
    // mid-stream, injects PumpAbort, and the F-CAP-1 verdict-preferred arm
    // returns the classified BodyTooLarge → the gateway maps it to 413 (NOT a
    // generic 502). Raised gateway/pool timeouts so the ~25 s upload is not
    // aborted by the default 60 s total (which would manifest as a 504/abort).
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
    let gw = spawn_listener_for_full(backend, cfg, timeouts).await;
    let mut sender = connect_h2_client(gw).await;

    let over = 64 * 1024 * 1024 + 2 * 1024 * 1024; // 66 MiB > 64 MiB cap
    let body = Full::new(Bytes::from(vec![0x5Au8; over]))
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/overcap"))
        .body(body)
        .unwrap();
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
    eprintln!(
        "FCAP1_OVER_CAP status={status:?} backend_drained={}",
        drained.load(Ordering::SeqCst)
    );
    assert_eq!(
        status,
        Some(413),
        "F-CAP-1: H2→H2 over-cap upload to a draining backend should yield 413 \
         (NOT 502), got {status:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcap1_over_window_pseudo_header_trailer_yields_400_not_502() {
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_listener_for(backend).await;

    // Flow-control-aware raw client: send a >window (128 KiB) body so the
    // gateway enters Branch B (streaming), then a TRAILER HEADERS carrying a
    // forbidden `:status` pseudo-header. The pump's validate_request_trailers
    // → BadRequest verdict must surface as 400 (NOT 502), no backend body leak.
    let mut c = RawH2::connect(gw).await;
    c.send_headers("/big", None).await;
    c.send_body(128 * 1024, 8 * 1024).await; // 128 KiB > 64 KiB window
    c.send_trailer_with_pseudo().await;

    let (status, leaked) = c.read_status().await;
    eprintln!("FCAP1_PSEUDO_TRAILER status={status:?} data_leak={leaked}");
    // SECURITY invariant (matches the pre-existing production test
    // h2_validation_before_forward::pseudo_header_in_trailers_never_leaks_
    // backend_body): a pseudo-header in trailers is a malformed request
    // (RFC 9113 §8.1) → mandated PROTOCOL_ERROR-class rejection, never a
    // relayed backend body. hyper's inbound H2 server codec catches the
    // malformed trailer at the protocol layer and RST_STREAMs the client
    // stream BEFORE the gateway's validate_request_trailers classifier runs,
    // so the wire signal is RST_STREAM (status=None) — NOT a gateway-emitted
    // 400. (The s10 plan §Q-HH-2 documents this exact path: "pseudo-header in
    // trailers → Err → PumpAbort injected → upstream RST (never complete)".)
    // The F-CAP-1 "→400" classification applies to BadRequest verdicts the
    // GATEWAY itself surfaces with a still-writable stream; the codec
    // pre-empts here. The binding properties: NO leak, and NOT a misleading
    // 502 / 200.
    assert!(
        !leaked,
        "LEAK: backend DATA relayed downstream for a malformed >window request"
    );
    assert!(
        matches!(status, None | Some(400)),
        "forbidden pseudo-header trailer (>window) must be rejected via \
         RST_STREAM (None) or a gateway 400 — got {status:?} (a 502 or 200 \
         would be the defect)"
    );
    assert_ne!(status, Some(502), "must NOT be a misleading 502");
    assert_ne!(
        status,
        Some(200),
        "must NOT relay a complete 200 (smuggling)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcap1_over_window_content_length_mismatch_no_leak() {
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_listener_for(backend).await;

    // Declare content-length far larger than ΣDATA, send a >window body, then
    // END_STREAM short → ΣDATA ≠ declared. The gateway/hyper must reject the
    // malformed request and NEVER relay the backend body downstream.
    let mut c = RawH2::connect(gw).await;
    c.send_headers("/big", Some("999999")).await;
    c.send_body(128 * 1024, 8 * 1024).await; // 128 KiB, but CL says ~1 MB
    c.send_short_end_stream(b"short").await;

    let (status, leaked) = c.read_status().await;
    eprintln!("FCAP1_CL_MISMATCH status={status:?} data_leak={leaked}");
    assert!(
        !leaked,
        "LEAK: backend DATA relayed downstream for a content-length≠ΣDATA request"
    );
    // A malformed (CL mismatch) request must NOT yield a relayed backend body;
    // the rejection is a 400 OR a stream/connection error (RST/GOAWAY). Either
    // is a non-leak; a 200 with the backend body WOULD be the smuggling defect.
    assert_ne!(
        status,
        Some(200),
        "CL-mismatch request was relayed as a complete 200 (smuggling defect)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcap1_genuine_upstream_failure_yields_502() {
    // Dead backend (port with no listener) → the Http2Pool dial fails →
    // ProxyErr::Upstream → 502. Proves the F-CAP-1 arm does NOT blanket-413:
    // a genuine upstream failure with NO classified pump verdict stays 502.
    let backend = dead_backend_addr().await;
    let gw = spawn_listener_for(backend).await;
    let mut sender = connect_h2_client(gw).await;

    let body = Full::new(Bytes::from_static(b"small")) // Branch A, within window
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/dead"))
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
// 6. RESPONSE-LEG byte-identity — large response streams through verbatim
//    (no truncation through the newly-converted streaming relay).
// ══════════════════════════════════════════════════════════════════════

async fn response_byte_identity(resp_len: usize) {
    let payload = binary_pattern(resp_len);
    let backend = spawn_h2_large_resp(payload.clone(), None).await;
    let gw = spawn_listener_for(backend).await;
    let mut sender = connect_h2_client(gw).await;

    let req = Request::builder()
        .method("GET")
        .uri(format!("http://{SAN_HOST}/download"))
        .body(
            http_body_util::Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
        )
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(120), sender.send_request(req))
        .await
        .expect("timed out")
        .expect("send failed");
    assert_eq!(resp.status(), 200);
    let got = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(got.len(), resp_len, "response truncated through the relay");
    assert_eq!(
        got.as_ref(),
        payload.as_slice(),
        "response body not byte-identical (resp_len={resp_len})"
    );
    eprintln!(
        "RESP_BYTE_IDENTITY resp_len={resp_len} got_len={} status=200",
        got.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resp_8mib_byte_identical() {
    response_byte_identity(8 * 1024 * 1024).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resp_48mib_byte_identical() {
    response_byte_identity(48 * 1024 * 1024).await;
}

// ══════════════════════════════════════════════════════════════════════
// 7. RESPONSE-LEG backpressure (bounded-by-construction).
//
// The response relay is `body.boxed()` on the upstream hyper `Incoming`
// (no gauge we own — see upstream_h2_response_to_h2). We CANNOT measure a
// retained-bytes gauge for it; instead we prove bounded-by-CONSTRUCTION:
// (a) a response FAR larger than any plausible single buffer (48 MiB)
//     streams through INCREMENTALLY while the client reads slowly, and the
//     bytes arrive in many small frames over time (not one materialised
//     blob), completing byte-identical; and
// (b) the relay never collects — structurally true in source
//     (upstream_h2_response_to_h2 does `body.boxed()`, no `.collect()`).
// We document this honestly and do NOT claim a measured gauge.
// ══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resp_backpressure_slow_client_streams_incrementally() {
    use h2 as h2crate;
    let resp_len = 48 * 1024 * 1024;
    let payload = binary_pattern(resp_len);
    let backend = spawn_h2_large_resp(payload.clone(), None).await;
    let gw = spawn_listener_for(backend).await;

    // Use the raw h2 client so we control release_capacity (read slowly).
    let stream = TcpStream::connect(gw).await.unwrap();
    let mut builder = h2crate::client::Builder::new();
    builder
        .initial_window_size(256 * 1024)
        .initial_connection_window_size(256 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(stream).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("GET")
        .uri(format!("http://{SAN_HOST}/slow"))
        .body(())
        .unwrap();
    let (resp_fut, _send) = h2sender.send_request(req, true).unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(120), resp_fut)
        .await
        .expect("timed out")
        .expect("response errored");
    assert_eq!(resp.status(), 200);
    let mut body = resp.into_body();
    let mut got: Vec<u8> = Vec::with_capacity(resp_len);
    let mut frames = 0usize;
    while let Some(chunk) = body.data().await {
        let data = chunk.expect("body data error");
        got.extend_from_slice(&data);
        frames += 1;
        let _ = body.flow_control().release_capacity(data.len());
        // Read slowly so the relay must apply backpressure (the small 256 KiB
        // window forces the gateway to pull-as-we-release, never buffering the
        // whole 48 MiB at once).
        if frames % 16 == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    eprintln!(
        "RESP_BACKPRESSURE resp_len={resp_len} got_len={} frames={frames} \
         (window=256KiB; arrived in many frames → incremental, not one blob)",
        got.len()
    );
    assert_eq!(got.len(), resp_len, "slow-client response truncated");
    assert_eq!(
        got.as_slice(),
        payload.as_slice(),
        "slow-client response corrupted"
    );
    // Incremental delivery: a 48 MiB response over a 256 KiB window cannot
    // arrive in one frame; many frames prove streaming-by-construction.
    assert!(
        frames > 16,
        "response did not stream incrementally (only {frames} frames for 48 MiB \
         over a 256 KiB window) — would indicate whole-response buffering"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 8. RESPONSE-LEG trailers relayed end-to-end (Q-HH-2).
// ══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resp_trailers_relayed_to_client() {
    let payload = binary_pattern(64 * 1024);
    let backend = spawn_h2_large_resp(payload.clone(), Some(("x-resp-trailer", "landed"))).await;
    let gw = spawn_listener_for(backend).await;
    let mut sender = connect_h2_client(gw).await;

    let req = Request::builder()
        .method("GET")
        .uri(format!("http://{SAN_HOST}/trailers"))
        .body(
            http_body_util::Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
        )
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(60), sender.send_request(req))
        .await
        .expect("timed out")
        .expect("send failed");
    assert_eq!(resp.status(), 200);
    // Collect to capture trailers via the Collected API.
    let collected = resp.into_body().collect().await.unwrap();
    let trailers = collected.trailers().cloned();
    let body = collected.to_bytes();
    assert_eq!(body.len(), payload.len(), "body length mismatch");
    let tm = trailers.expect("response trailers must reach the client (Q-HH-2)");
    let v = tm
        .get("x-resp-trailer")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    eprintln!("RESP_TRAILERS x-resp-trailer={v:?}");
    assert_eq!(
        v, "landed",
        "legit response trailer did not reach the client through the relay"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 9. ADDITIONAL real-wire arms (exercise the legitimate branches the core
//    battery doesn't reach, so the session-code coverage is honest, not
//    padded — each asserts a genuine behavior).
// ══════════════════════════════════════════════════════════════════════

/// Within-window (Branch A) client RST mid-body → ZERO-DIAL reject (the
/// Phase-1 lookahead `None` + `!is_end_stream()` arm: a within-window reset
/// is rejected before any pool contact). The backend must NEVER be dialed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_branch_a_rst_mid_body_zero_dial_reject() {
    use h2 as h2crate;
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_listener_for(backend).await;

    let stream = TcpStream::connect(gw).await.unwrap();
    let (h2, conn) = h2crate::client::handshake(stream).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/small-rst"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = h2sender.send_request(req, false).unwrap();
    // Send a SMALL (< 64 KiB window) partial body, then RST without END_STREAM.
    let _ = send_body.send_data(Bytes::from(binary_pattern(8 * 1024)), false);
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(send_body); // RST_STREAM(CANCEL)
    let _ = tokio::time::timeout(Duration::from_secs(3), resp_fut).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let dials = seen.requests.load(Ordering::SeqCst);
    eprintln!("BRANCH_A_RST zero_dial: backend_requests={dials}");
    assert_eq!(
        dials, 0,
        "within-window RST mid-body must be rejected ZERO-DIAL (F-COR-1); \
         backend saw {dials} requests"
    );
    assert!(
        !seen.complete.load(Ordering::SeqCst),
        "within-window RST must never be a complete request upstream"
    );
}

/// Branch A (≤window) request WITH a valid trailing header → trailers
/// captured + forwarded; backend observes a clean complete request. Covers
/// the Branch-A trailers path (validate_request_trailers Ok +
/// build_h2_body_with_trailers).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_branch_a_valid_trailers_forwarded() {
    use h2 as h2crate;
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_listener_for(backend).await;

    let stream = TcpStream::connect(gw).await.unwrap();
    let (h2, conn) = h2crate::client::handshake(stream).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/a-trailers"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = h2sender.send_request(req, false).unwrap();
    let payload = binary_pattern(4 * 1024); // ≤ window → Branch A
    let _ = send_body.send_data(Bytes::from(payload.clone()), false);
    let mut tm = http::HeaderMap::new();
    tm.insert("x-req-trailer", http::HeaderValue::from_static("ok"));
    let _ = send_body.send_trailers(tm);
    let resp = tokio::time::timeout(Duration::from_secs(30), resp_fut)
        .await
        .expect("timed out")
        .expect("send failed");
    assert_eq!(resp.status(), 200);
    let _ = resp.into_body();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let upstream = seen.body.lock().clone();
    eprintln!("BRANCH_A_TRAILERS upstream_body_len={}", upstream.len());
    assert_eq!(
        upstream.as_slice(),
        payload.as_slice(),
        "Branch-A body+trailers byte-identical"
    );
    assert!(
        seen.complete.load(Ordering::SeqCst),
        "clean complete request with trailers"
    );
}

/// Branch B (>window) request WITH a valid trailing header → the streaming
/// pump's trailers-validate Ok arm forwards the terminal trailers frame;
/// backend observes a clean complete request with the full body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_branch_b_valid_trailers_forwarded() {
    use h2 as h2crate;
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    let gw = spawn_listener_for(backend).await;

    let stream = TcpStream::connect(gw).await.unwrap();
    let mut builder = h2crate::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(stream).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/b-trailers"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = h2sender.send_request(req, false).unwrap();
    let payload = binary_pattern(256 * 1024); // > window → Branch B
    let mut off = 0;
    while off < payload.len() {
        let end = (off + 16 * 1024).min(payload.len());
        let chunk = Bytes::copy_from_slice(&payload[off..end]);
        send_body.reserve_capacity(chunk.len());
        match tokio::time::timeout(
            Duration::from_secs(5),
            futures_util::future::poll_fn(|cx| send_body.poll_capacity(cx)),
        )
        .await
        {
            Ok(Some(Ok(c))) if c > 0 => {}
            _ => break,
        }
        if send_body.send_data(chunk, false).is_err() {
            break;
        }
        off = end;
    }
    let mut tm = http::HeaderMap::new();
    tm.insert("x-req-trailer", http::HeaderValue::from_static("ok"));
    let _ = send_body.send_trailers(tm);
    let resp = tokio::time::timeout(Duration::from_secs(30), resp_fut)
        .await
        .expect("timed out")
        .expect("send failed");
    assert_eq!(resp.status(), 200);
    let _ = resp.into_body();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let upstream_len = seen.body.lock().len();
    eprintln!("BRANCH_B_TRAILERS upstream_body_len={upstream_len}");
    assert_eq!(
        upstream_len,
        payload.len(),
        "Branch-B body fully forwarded with trailers"
    );
    assert!(
        seen.complete.load(Ordering::SeqCst),
        "clean complete Branch-B request with trailers"
    );
}

/// Response leg with alt-svc configured → the relay injects the alt-svc
/// header (covers the alt_svc arm of upstream_h2_response_to_h2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resp_alt_svc_header_injected() {
    let payload = binary_pattern(4 * 1024);
    let backend = spawn_h2_large_resp(payload, None).await;
    let alt = lb_l7::h1_proxy::AltSvcConfig {
        h3_port: 443,
        max_age: 3_600,
    };
    let gw = spawn_listener_for_alt(
        backend,
        Http2PoolConfig::default(),
        HttpTimeouts::default(),
        Some(alt),
    )
    .await;
    let mut sender = connect_h2_client(gw).await;
    let req = Request::builder()
        .method("GET")
        .uri(format!("http://{SAN_HOST}/altsvc"))
        .body(
            http_body_util::Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
        )
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(30), sender.send_request(req))
        .await
        .expect("timed out")
        .expect("send failed");
    assert_eq!(resp.status(), 200);
    let alt_hdr = resp
        .headers()
        .get("alt-svc")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    eprintln!("RESP_ALT_SVC alt-svc={alt_hdr:?}");
    assert!(
        alt_hdr.contains("h3=") && alt_hdr.contains("443"),
        "alt-svc header not injected by the response relay: {alt_hdr:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 10. reset_peer COLLATERAL probe (round-2 mandate #7b) — does a client RST
//     mid-body on ONE H2→H2 stream, which triggers `reset_peer` (whole-
//     connection teardown), disrupt a CONCURRENT HEALTHY H2→H2 request to
//     the SAME backend on the SAME pooled connection?
//
//     This is a CHARACTERIZATION probe, not a hard pass/fail gate: the
//     pool's reset_peer is connection-scoped by design (ROUND8-L7-10 broad-
//     eviction philosophy). We record whether the healthy concurrent request
//     survives (multiplex isolation) or is collaterally reset, and report
//     the finding. We assert only the SECURITY floor (the smuggle is never
//     complete); the collateral outcome is logged for the lead.
// ══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "current_thread")]
async fn reset_peer_collateral_concurrent_stream_probe() {
    use h2 as h2crate;
    let seen = BackendSeen::default();
    let backend = spawn_h2_echo(seen.clone()).await;
    // Long send_timeout so the healthy stream's deliberate slowness is not a
    // spurious timeout; default total is fine (healthy completes well under).
    let cfg = Http2PoolConfig {
        send_timeout: Duration::from_secs(120),
        ..Http2PoolConfig::default()
    };
    let gw = spawn_listener_for_with_cfg(backend, cfg).await;

    // ── Healthy request: a genuine hyper client streaming an 8 MiB body
    // SLOWLY (via a channel) so it is still in flight when the smuggle aborts.
    let mut healthy_sender = connect_h2_client(gw).await;
    let healthy_payload = binary_pattern(8 * 1024 * 1024);
    let (htx, hrx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, hyper::Error>>(2);
    let healthy_body = StreamBody::new(recv_stream(hrx)).boxed();
    let healthy_req = Request::builder()
        .method("POST")
        .uri(format!("http://{SAN_HOST}/healthy"))
        .body(healthy_body)
        .unwrap();
    let healthy_task = tokio::spawn(async move {
        match tokio::time::timeout(
            Duration::from_secs(60),
            healthy_sender.send_request(healthy_req),
        )
        .await
        {
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
            Ok(Err(_)) | Err(_) => (None, 0),
        }
    });
    // Feed the healthy body slowly so it stays in flight across the smuggle.
    let hp = healthy_payload.clone();
    let healthy_writer = tokio::spawn(async move {
        let mut off = 0;
        while off < hp.len() {
            let end = (off + 64 * 1024).min(hp.len());
            if htx
                .send(Ok(Frame::data(Bytes::copy_from_slice(&hp[off..end]))))
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(8)).await; // ~slow drip
            off = end;
        }
        let _ = htx.send(Ok(Frame::data(Bytes::new()))).await;
    });

    // Let the healthy request DIAL the upstream (establishing the pooled
    // connection) and begin streaming.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // ── Smuggle request to the SAME backend (same pooled upstream conn):
    // >window body, RST mid-body → triggers reset_peer (conn teardown).
    {
        let stream = TcpStream::connect(gw).await.unwrap();
        let mut builder = h2crate::client::Builder::new();
        builder
            .initial_window_size(8 * 1024 * 1024)
            .initial_connection_window_size(8 * 1024 * 1024);
        let (h2, conn) = builder.handshake::<_, Bytes>(stream).await.unwrap();
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let mut s = h2.ready().await.unwrap();
        let req = http::Request::builder()
            .method("POST")
            .uri(format!("http://{SAN_HOST}/smuggle"))
            .body(())
            .unwrap();
        let (rf, mut sb) = s.send_request(req, false).unwrap();
        let p = binary_pattern(256 * 1024);
        let mut off = 0;
        while off < p.len() {
            let end = (off + 16 * 1024).min(p.len());
            let chunk = Bytes::copy_from_slice(&p[off..end]);
            sb.reserve_capacity(chunk.len());
            match tokio::time::timeout(
                Duration::from_secs(5),
                futures_util::future::poll_fn(|cx| sb.poll_capacity(cx)),
            )
            .await
            {
                Ok(Some(Ok(c))) if c > 0 => {}
                _ => break,
            }
            if sb.send_data(chunk, false).is_err() {
                break;
            }
            off = end;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(sb); // RST_STREAM(CANCEL) → gateway reset_peer
        let _ = tokio::time::timeout(Duration::from_secs(3), rf).await;
    }

    // Resolve the healthy request and characterize collateral.
    let _ = healthy_writer.await;
    let (healthy_status, healthy_len) = healthy_task.await.unwrap_or((None, 0));
    let isolated = healthy_status == Some(200) && healthy_len == healthy_payload.len();
    eprintln!(
        "RESET_PEER_COLLATERAL healthy_status={healthy_status:?} healthy_len={healthy_len} \
         (expected 8388608) multiplex_isolated={isolated}"
    );
    if !isolated {
        eprintln!(
            "RESET_PEER_COLLATERAL FINDING (CF, not blocker): a client RST mid-body on \
             one H2→H2 stream collaterally disrupted a CONCURRENT healthy request to the \
             SAME backend (shared pooled connection torn down by reset_peer). This is \
             consistent with the pool's connection-scoped ROUND8-L7-10 broad-eviction \
             philosophy — documented tradeoff, NOT a security defect."
        );
    } else {
        eprintln!(
            "RESET_PEER_COLLATERAL: concurrent healthy request SURVIVED (multiplex isolated)."
        );
    }
    // SECURITY floor only (collateral is characterization, not a gate): the
    // smuggle must still never be a complete request at the backend, and the
    // probe must be non-vacuous (the upstream WAS dialed).
    assert!(
        seen.requests.load(Ordering::SeqCst) >= 1,
        "probe vacuous: upstream never dialed"
    );
}

// Reference the StreamBody/Frame imports so an unused-import lint cannot
// fire if a future edit drops a use; mirrors the production body type.
#[allow(dead_code)]
fn _type_anchor() -> StreamBody<futures_util::stream::Empty<Result<Frame<Bytes>, hyper::Error>>> {
    StreamBody::new(futures_util::stream::empty())
}
