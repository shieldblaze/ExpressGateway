//! S6 — H3 → H2 R8 streaming verification suite (real wire).
//!
//! Path under test: real quiche H3 client → production `QuicListener`
//! → `router` → `conn_actor::poll_h3` (H2 branch, I3) →
//! `h3_to_h2_stream_resp` → real hyper H2 backend (`Http2Pool`).
//!
//! This is the H3→H1 BUILT bar, applied to H3→H2 (owner binding
//! conditions):
//!
//! * **cond 1** — `h2_e2e_request_body_byte_identical_at_backend`:
//!   a NON-EMPTY BINARY (non-UTF-8) request body of ≥1 MiB arrives
//!   BYTE-IDENTICAL at a real H2 backend (proves the dropped-request-
//!   body defect — the old `Full::new(Bytes::new())` — is fixed).
//! * **cond 2** — non-vacuous live-gauge memory proofs for BOTH
//!   directions under a stalled peer
//!   (`h2_e2e_response_memory_bounded_through_stalled_client`,
//!   `h2_e2e_request_memory_bounded_through_stalled_backend`) plus a
//!   backpressure proof
//!   (`h2_e2e_backpressure_stalled_client_pauses_h2_upstream_read`).
//! * **cond 3** — every gauge case references the feature-gated
//!   `lb_quic::h3_bridge::MAX_RETAINED_{RESP,BODY}_BYTES` statics, so
//!   the suite ONLY compiles those proofs under
//!   `cargo test -p lb-quic --features test-gauges` (a run without the
//!   flag fails to compile them — an invalid gate, by design, exactly
//!   as for `h3_h1_resp_stream_e2e.rs`).
//!
//! BINDING case 7 (lead A2) —
//! `h2_e2e_client_reset_midrequest_rsts_h2_upstream_no_truncated_request`:
//! the H3 client RESETs MID request body; the real H2 backend NEVER
//! observes a silently-truncated request presented as complete.
//!
//! No `#[ignore]` anywhere. The full real-H2 backend (hyper H2 server)
//! is built WITHOUT `hyper-util` (a hand-rolled tokio IO adapter +
//! executor) so no new dev-dependency is added (A1 scope held).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use lb_io::http2_pool::{Http2Pool, Http2PoolConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::{QuicListener, QuicListenerParams};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_util::sync::CancellationToken;

use lb_h3::{H3Frame, QpackDecoder, QpackEncoder, decode_frame, encode_frame};

const TEST_SNI: &str = "expressgateway.test";
const H3_ALPN: &[u8] = b"h3";
const MAX_UDP: usize = 65_535;

// ─────────────────────────────────────────────────────────────────────
// §0  Hand-rolled hyper IO adapter + executor (no hyper-util dep).
// ─────────────────────────────────────────────────────────────────────

/// Wrap a tokio `TcpStream` so a hyper 1.x server can drive it without
/// the `hyper-util` `TokioIo` shim.
struct HyperIo(TcpStream);

impl hyper::rt::Read for HyperIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        // Read into a scratch buffer sized to the cursor's REMAINING
        // capacity (hyper's `put_slice` panics if we hand it more than
        // it can hold), then copy into hyper's cursor.
        let cap = buf.remaining().min(16 * 1024);
        if cap == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut tmp = vec![0u8; cap];
        let mut rb = tokio::io::ReadBuf::new(&mut tmp);
        match Pin::new(&mut self.0).poll_read(cx, &mut rb) {
            Poll::Ready(Ok(())) => {
                let n = rb.filled().len();
                buf.put_slice(&tmp[..n]);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl hyper::rt::Write for HyperIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

/// Minimal `hyper::rt::Executor` backed by `tokio::spawn`.
#[derive(Clone)]
struct TokioExec;
impl<F> hyper::rt::Executor<F> for TokioExec
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn execute(&self, fut: F) {
        tokio::spawn(fut);
    }
}

// ─────────────────────────────────────────────────────────────────────
// §1  Cert + listener + client harness (mirrors h3_h1_resp_stream_e2e).
// ─────────────────────────────────────────────────────────────────────

/// The §1.5 C5 sound ceiling, IDENTICAL formula to
/// `h3_h1_resp_stream_e2e::resp_retained_ceiling` (`4 ×
/// depth×(chunk+hdr)`). `test ceiling == gauge bound`.
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
        "lb-quic-h3h2-stream-{}-{}-{counter}",
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

fn build_tcp_pool() -> TcpPool {
    TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts {
            nodelay: true,
            keepalive: true,
            rcvbuf: Some(65_536),
            sndbuf: Some(65_536),
            ..BackendSockOpts::default()
        },
        lb_io::Runtime::new(),
    )
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
    // Generous connection-level data so the request-side stream is
    // governed by the per-stream window (stalled-backend test).
    cfg.set_initial_max_data(8 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(8 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(8 * 1024 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(8);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg
}

fn random_scid() -> [u8; quiche::MAX_CONN_ID_LEN] {
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new().fill(&mut scid).unwrap();
    scid
}

/// Pseudo-random NON-UTF-8 body of `n` bytes (deterministic).
fn binary_body(n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n);
    let mut s: u32 = 0x9E37_79B9;
    for i in 0..n {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        // Bias toward high bytes so the body is decidedly non-UTF-8.
        v.push((((s >> 24) as u8) | 0x80).wrapping_add(i as u8));
    }
    v
}

async fn start_h3_listener_h2(
    certs: &TestCerts,
    backend: SocketAddr,
) -> (QuicListener, SocketAddr, CancellationToken) {
    let tcp_pool = build_tcp_pool();
    let h2_pool = Http2Pool::new(Http2PoolConfig::default(), tcp_pool);
    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    )
    .with_h2_backend(h2_pool, backend);
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone()).await.unwrap();
    let addr = listener.local_addr();
    (listener, addr, shutdown)
}

/// What the driven H3 client observed.
#[derive(Default)]
struct ClientOut {
    status: Option<u16>,
    body: Vec<u8>,
    content_length: Option<usize>,
    fin: bool,
    reset: bool,
}

/// Drive ONE H3 request on stream 0.
///
/// `req_body`: chunks sent as DATA frames (FIN after last). Empty ⇒
/// a bodyless GET. `stall_after`: if `Some(n)`, the client stops
/// reading the response (but keeps the conn alive with ACKs) once it
/// has buffered ≥ `n` response body bytes, holds for `stall_for`, then
/// resumes — the stalled-client memory/backpressure mechanism.
/// `reset_after_req_bytes`: if `Some(k)`, after sending `k` request
/// body bytes the client RESET_STREAMs (mid-request abort, case 7).
#[allow(clippy::struct_excessive_bools)]
struct DriveCfg {
    method: &'static str,
    path: &'static str,
    req_body: Vec<Vec<u8>>,
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
    let mut conn =
        quiche::connect(Some(TEST_SNI), &scid_ref, local, gateway, &mut ccfg).unwrap();

    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let sid: u64 = 0;
    let mut head_sent = false;
    let mut tx_wire: Vec<u8> = Vec::new();
    let mut tx_off = 0usize; // wire bytes accepted by the stream
    let mut data_start = 0usize; // wire offset of the first DATA byte
    let mut req_done = false;
    let mut did_reset = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut out = ClientOut::default();
    let mut stalling_until: Option<tokio::time::Instant> = None;
    let mut stalled_once = false;

    let deadline = tokio::time::Instant::now() + overall;
    while tokio::time::Instant::now() < deadline {
        // Flush egress.
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
            // Pre-serialize HEADERS + every DATA frame into ONE wire
            // buffer. We then push it to the stream honouring partial
            // accepts (QUIC flow control / the gateway's M-A
            // backpressure) and set FIN exactly once, when fully
            // drained. `reset_byte` is the wire offset at/after which
            // the client RESET_STREAMs (case 7) — measured from the
            // first DATA byte so the threshold maps to request-body
            // progress, independent of the HEADERS frame size.
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
            head_sent = true;
        }

        // Push the wire buffer with partial-accept handling. FIN only
        // when fully drained (and not aborting). Honour the mid-body
        // RESET threshold (case 7).
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
                let remaining = &tx_wire[tx_off..];
                let fin = true; // entire request fits in this push
                match conn.stream_send(sid, remaining, fin) {
                    Ok(0) => break, // window closed — retry next tick
                    Ok(n) => {
                        tx_off += n;
                        if tx_off >= tx_wire.len() {
                            req_done = true; // FIN was set on this send
                        } else {
                            // Partial: the FIN we just requested was
                            // NOT applied (more bytes pending). Honour
                            // a RESET threshold reached mid-buffer.
                            if let Some(k) = cfg.reset_after_req_bytes {
                                if tx_off.saturating_sub(data_start) >= k {
                                    let _ = conn.stream_shutdown(
                                        sid,
                                        quiche::Shutdown::Write,
                                        0x10c,
                                    );
                                    did_reset = true;
                                    req_done = true;
                                }
                            }
                        }
                    }
                    Err(quiche::Error::Done) => break,
                    Err(_) => break,
                }
            }
        }

        // Drain response unless stalling.
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
            // Engage the stall EXACTLY ONCE (latched) when we've
            // buffered enough body — re-engaging every tick would
            // never let the download complete (false liveness fail).
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

        // Wait for the first datagram (bounded by the QUIC timer),
        // then DRAIN the whole receive backlog this tick so a 4–8 MiB
        // transfer is not throttled to one datagram per loop. During
        // a stall we still `conn.recv` (ACKs flow, QUIC flow control
        // naturally backpressures the gateway) — only `stream_recv`
        // is withheld above.
        let to = conn.timeout().unwrap_or(Duration::from_millis(20));
        match tokio::time::timeout(to.min(Duration::from_millis(25)), sock.recv_from(&mut in_buf))
            .await
        {
            Ok(Ok((n, from))) => {
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                let _ = conn.recv(slice, quiche::RecvInfo { from, to: local });
            }
            Ok(Err(_)) | Err(_) => conn.on_timeout(),
        }
        // Drain any further queued datagrams this tick (bounded so a
        // flood cannot starve the send/stall logic).
        for _ in 0..64 {
            match sock.try_recv_from(&mut in_buf) {
                Ok((n, from)) => {
                    let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                    let _ = conn.recv(slice, quiche::RecvInfo { from, to: local });
                }
                Err(_) => break, // WouldBlock — backlog drained
            }
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// §2  Real hyper H2 backends.
// ─────────────────────────────────────────────────────────────────────

/// Echo backend: replies 200 with the request body verbatim, and
/// records the FULL received request body so the test can assert it
/// byte-for-byte. `seen` captures the body of the LAST completed
/// request; `complete` is set only when the request body ended
/// cleanly (so a truncated/aborted request is observable as NOT
/// complete — case 7).
#[derive(Clone, Default)]
struct BackendSeen {
    body: Arc<Mutex<Vec<u8>>>,
    complete: Arc<std::sync::atomic::AtomicBool>,
    requests: Arc<AtomicUsize>,
}

async fn spawn_h2_echo(
    seen: BackendSeen,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let h = tokio::spawn(async move {
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
                        // Stream the request body frame-by-frame so a
                        // mid-stream error is observed as an Err (NOT
                        // a clean end) — the truncation guard.
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
                        *seen.body.lock().unwrap() = got.clone();
                        seen.complete.store(clean, Ordering::SeqCst);
                        let out = if got.is_empty() {
                            Bytes::from_static(b"h2-empty")
                        } else {
                            Bytes::from(got)
                        };
                        Ok::<_, std::convert::Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .body(Full::new(out))
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http2::Builder::new(TokioExec)
                    .serve_connection(HyperIo(sock), svc)
                    .await;
            });
        }
    });
    (local, h)
}

/// Large-response backend: replies 200 with a fixed `body` (used for
/// the response-direction memory/backpressure proofs). Splits the
/// body into 16 KiB frames so the H2 send window governs progress.
async fn spawn_h2_large_resp(
    body: Vec<u8>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let body = Arc::new(body);
    let h = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let body = Arc::clone(&body);
            tokio::spawn(async move {
                let svc = service_fn(move |_req: Request<Incoming>| {
                    let body = Arc::clone(&body);
                    async move {
                        Ok::<_, std::convert::Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .body(Full::new(Bytes::from((*body).clone())))
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http2::Builder::new(TokioExec)
                    .serve_connection(HyperIo(sock), svc)
                    .await;
            });
        }
    });
    (local, h)
}

// ─────────────────────────────────────────────────────────────────────
// §3  Cases.
// ─────────────────────────────────────────────────────────────────────

/// Case 1 — liveness floor: small GET, status + body byte-identical,
/// clean FIN.
#[tokio::test]
async fn h2_e2e_get_response_byte_identical() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h2_echo(seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h2(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/hello",
            req_body: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
        },
        Duration::from_secs(20),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    bh.abort();

    assert_eq!(out.status, Some(200), "H3→H2 GET must return 200");
    assert!(out.fin, "clean FIN expected");
    assert_eq!(out.body, b"h2-empty", "bodyless GET ⇒ backend echo sentinel");
}

/// Case 2 (BINDING cond 1) — a NON-EMPTY BINARY request body arrives
/// BYTE-IDENTICAL at the real H2 backend. Proves the dropped-request-
/// body defect is fixed.
#[tokio::test]
async fn h2_e2e_request_body_byte_identical_at_backend() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h2_echo(seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h2(&certs, backend).await;

    let payload = binary_body(1024 * 1024 + 777); // ≥1 MiB, non-UTF-8
    // Split into many DATA frames (≈48 KiB) to exercise the
    // multi-frame incremental pump.
    let chunks: Vec<Vec<u8>> = payload
        .chunks(48 * 1024)
        .map(<[u8]>::to_vec)
        .collect();

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/upload",
            req_body: chunks,
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
        },
        Duration::from_secs(45),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let got = seen.body.lock().unwrap().clone();
    bh.abort();

    assert_eq!(out.status, Some(200), "POST must succeed");
    assert!(out.fin, "clean FIN expected");
    assert!(
        seen.complete.load(Ordering::SeqCst),
        "backend must see a CLEANLY-ENDED request body"
    );
    assert_eq!(
        got.len(),
        payload.len(),
        "backend received {} bytes, sent {}",
        got.len(),
        payload.len()
    );
    assert!(
        got == payload,
        "request body must arrive BYTE-IDENTICAL at the H2 backend \
         (dropped-request-body defect fixed)"
    );
    // The echo backend returns the body verbatim — also assert the
    // round-trip response body is identical (both directions intact).
    assert_eq!(out.body, payload, "echoed response body must match");
}

/// Case 3 (BINDING cond 2, response direction) — a 4 MiB H2 response
/// streams through a STALLED H3 client; the live
/// `MAX_RETAINED_RESP_BYTES` gauge stays ≤ the C5 ceiling (≪ body),
/// then the client resumes and the body arrives byte-identical with a
/// clean FIN (non-vacuous: bound + liveness).
#[cfg(feature = "test-gauges")]
#[tokio::test]
async fn h2_e2e_response_memory_bounded_through_stalled_client() {
    use lb_quic::h3_bridge::{
        H3_FRAME_HDR_MAX, H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, MAX_RETAINED_RESP_BYTES,
    };

    MAX_RETAINED_RESP_BYTES.store(0, Ordering::SeqCst);
    let ceiling = retained_ceiling(H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, H3_FRAME_HDR_MAX);
    assert_eq!(ceiling, 262_656, "C5 RESP ceiling authoritative value");

    let body = binary_body(4 * 1024 * 1024); // 4 MiB
    assert!(
        ceiling * 8 <= body.len(),
        "non-vacuous: ceiling ({ceiling}) must be ≪ body ({}) ≥8×",
        body.len()
    );

    let certs = generate_loopback_certs();
    let (backend, bh) = spawn_h2_large_resp(body.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h2(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/big",
            req_body: vec![],
            stall_after: Some(256 * 1024),
            stall_for: Duration::from_secs(2),
            reset_after_req_bytes: None,
        },
        Duration::from_secs(60),
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
/// request body while the H2 backend STALLS reading it; the live
/// `MAX_RETAINED_BODY_BYTES` gauge stays ≤ the C5 ceiling (≪ body),
/// then the backend unblocks and the body is byte-identical.
#[cfg(feature = "test-gauges")]
#[tokio::test]
async fn h2_e2e_request_memory_bounded_through_stalled_backend() {
    use lb_quic::h3_bridge::{H3_BODY_CHUNK_MAX, MAX_FRAME_HEADER_BYTES, MAX_RETAINED_BODY_BYTES};
    use lb_quic::conn_actor::H3_BODY_CHANNEL_DEPTH;

    MAX_RETAINED_BODY_BYTES.store(0, Ordering::SeqCst);
    let ceiling = retained_ceiling(H3_BODY_CHANNEL_DEPTH, H3_BODY_CHUNK_MAX, MAX_FRAME_HEADER_BYTES);
    assert_eq!(ceiling, 262_656, "C5 REQ ceiling authoritative value");

    let payload = binary_body(4 * 1024 * 1024); // 4 MiB
    assert!(
        ceiling * 8 <= payload.len(),
        "non-vacuous: ceiling ({ceiling}) ≪ body ({}) ≥8×",
        payload.len()
    );

    // Backend that accepts the connection but DELAYS reading the
    // request body, forcing the proxy's request pump to backpressure.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let backend = listener.local_addr().unwrap();
    let bh = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| async move {
                    // Stall ~1.5 s BEFORE draining the request body.
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    let got = req
                        .into_body()
                        .collect()
                        .await
                        .map(|c| c.to_bytes())
                        .unwrap_or_default();
                    Ok::<_, std::convert::Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(Full::new(Bytes::copy_from_slice(&got)))
                            .unwrap(),
                    )
                });
                let _ = hyper::server::conn::http2::Builder::new(TokioExec)
                    .serve_connection(HyperIo(sock), svc)
                    .await;
            });
        }
    });

    let certs = generate_loopback_certs();
    let (l, gw, sd) = start_h3_listener_h2(&certs, backend).await;

    let chunks: Vec<Vec<u8>> = payload.chunks(48 * 1024).map(<[u8]>::to_vec).collect();
    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/slow-upload",
            req_body: chunks,
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
        },
        Duration::from_secs(60),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), l.shutdown()).await;
    sd.cancel();
    let retained = MAX_RETAINED_BODY_BYTES.load(Ordering::SeqCst);
    bh.abort();

    assert_eq!(out.status, Some(200), "request must complete after unblock");
    assert_eq!(out.body, payload, "4 MiB request body byte-identical");
    assert!(
        retained <= ceiling,
        "REQ retained {retained} must stay ≤ C5 ceiling {ceiling} \
         while the body was {} (request pump NOT whole-body buffering)",
        payload.len()
    );
    assert!(retained > 0, "gauge must be live (non-vacuous)");
}

/// Case 5 (BINDING cond 2, backpressure) — a stalled H3 client must
/// pause the H2 upstream read: retained ≤ ceiling for a body ≫
/// ceiling, AND the body still completes byte-identical after resume
/// (the causal chain held without dropping/corrupting bytes).
#[cfg(feature = "test-gauges")]
#[tokio::test]
async fn h2_e2e_backpressure_stalled_client_pauses_h2_upstream_read() {
    use lb_quic::h3_bridge::{
        H3_FRAME_HDR_MAX, H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, MAX_RETAINED_RESP_BYTES,
    };
    MAX_RETAINED_RESP_BYTES.store(0, Ordering::SeqCst);
    let ceiling = retained_ceiling(H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, H3_FRAME_HDR_MAX);

    let body = binary_body(8 * 1024 * 1024); // 8 MiB ≫ ceiling (≈32×)
    assert!(ceiling * 16 <= body.len(), "non-vacuous ≥16×");

    let certs = generate_loopback_certs();
    let (backend, bh) = spawn_h2_large_resp(body.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h2(&certs, backend).await;

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/huge",
            req_body: vec![],
            stall_after: Some(256 * 1024),
            stall_for: Duration::from_secs(3),
            reset_after_req_bytes: None,
        },
        Duration::from_secs(90),
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
        "stalled client MUST pause the H2 upstream read: retained \
         {retained} ≤ ceiling {ceiling} for an 8 MiB body"
    );
}

/// Case 6 — H2 backend RESETs mid-body ⇒ the H3 client gets
/// RESET_STREAM, NEVER a clean FIN (response-splitting guard).
#[tokio::test]
async fn h2_e2e_upstream_reset_midbody_resets_client_no_fin() {
    let certs = generate_loopback_certs();
    // Backend: send 200 + a Content-Length far larger than the bytes
    // it actually writes, then drop the connection mid-body ⇒ hyper
    // surfaces a body error to the proxy.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let backend = listener.local_addr().unwrap();
    let bh = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let svc = service_fn(move |_req: Request<Incoming>| async move {
                    // Declare a 1 MiB body but the body stream yields
                    // only ~64 KiB then ERRORS — the proxy must NOT
                    // deliver a clean, complete 200 to the client.
                    let s = futures_like_error_body();
                    Ok::<_, std::convert::Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-length", "1048576")
                            .body(s)
                            .unwrap(),
                    )
                });
                let _ = hyper::server::conn::http2::Builder::new(TokioExec)
                    .serve_connection(HyperIo(sock), svc)
                    .await;
            });
        }
    });

    let (l, gw, sd) = start_h3_listener_h2(&certs, backend).await;
    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "GET",
            path: "/broken",
            req_body: vec![],
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: None,
        },
        Duration::from_secs(30),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), l.shutdown()).await;
    sd.cancel();
    bh.abort();

    // The response-splitting guard: a mid-body upstream error must
    // NEVER be delivered to the client as a clean, COMPLETE 200 (FIN
    // with the full declared 1 MiB body). Acceptable outcomes: a
    // RESET_STREAM (no FIN), an inline 5xx (clean FIN but NOT 200),
    // or a short/incomplete body that does not satisfy the declared
    // content-length.
    let delivered_complete_200 = out.status == Some(200)
        && out.fin
        && out.body.len() >= 1_048_576;
    assert!(
        !delivered_complete_200,
        "mid-body upstream error MUST NOT yield a clean complete 200 \
         to the client (status={:?} fin={} body_len={} declared=1048576)",
        out.status,
        out.fin,
        out.body.len()
    );
    // And specifically: if it FIN'd with status 200, the body must be
    // SHORT of the declared length (truncation visible, not masked).
    if out.status == Some(200) && out.fin {
        assert!(
            out.body.len() < 1_048_576,
            "a 200+FIN must not carry the full declared body after a \
             mid-body upstream abort"
        );
    }
}

/// An error body: emits ~64 KiB across 8 DATA frames (so HEADERS +
/// real body bytes are reliably relayed through the proxy), then
/// ERRORS — the proxy's `stream_h2_response` hits the `body.frame()`
/// Err arm AFTER having forwarded a partial body, exercising the true
/// mid-body response-splitting guard (declared content-length 1 MiB,
/// only ~64 KiB delivered).
fn error_body() -> http_body_util::combinators::BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>
{
    use std::pin::Pin;
    use std::task::{Context, Poll};
    struct ErrBody {
        frames: usize,
    }
    impl hyper::body::Body for ErrBody {
        type Data = Bytes;
        type Error = Box<dyn std::error::Error + Send + Sync>;
        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
            if self.frames >= 8 {
                Poll::Ready(Some(Err("backend mid-body abort".into())))
            } else {
                self.frames += 1;
                Poll::Ready(Some(Ok(hyper::body::Frame::data(Bytes::from(
                    vec![7u8; 8192],
                )))))
            }
        }
    }
    http_body_util::combinators::BoxBody::new(ErrBody { frames: 0 })
}

fn futures_like_error_body()
-> http_body_util::combinators::BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>> {
    error_body()
}

/// Case 7 (BINDING — lead A2, request-side smuggling-parity) — the H3
/// client RESETs MID request body. The streaming request `BoxBody`
/// errors ⇒ hyper RST_STREAMs the H2 upstream; the real H2 backend
/// NEVER observes a silently-truncated request presented as complete
/// (it sees a body stream error, NOT a clean end-of-request with a
/// short body).
#[tokio::test]
async fn h2_e2e_client_reset_midrequest_rsts_h2_upstream_no_truncated_request() {
    let certs = generate_loopback_certs();
    let seen = BackendSeen::default();
    let (backend, bh) = spawn_h2_echo(seen.clone()).await;
    let (listener, gw, sd) = start_h3_listener_h2(&certs, backend).await;

    // 2 MiB intended; the client RESETs after ~256 KiB of body.
    let payload = binary_body(2 * 1024 * 1024);
    let chunks: Vec<Vec<u8>> = payload.chunks(32 * 1024).map(<[u8]>::to_vec).collect();

    let out = drive_h3(
        gw,
        &certs.ca,
        DriveCfg {
            method: "POST",
            path: "/abort",
            req_body: chunks,
            stall_after: None,
            stall_for: Duration::ZERO,
            reset_after_req_bytes: Some(256 * 1024),
        },
        Duration::from_secs(30),
    )
    .await;

    // Give the backend a moment to observe the stream fault.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    sd.cancel();
    let backend_saw_complete = seen.complete.load(Ordering::SeqCst);
    let backend_body_len = seen.body.lock().unwrap().len();
    bh.abort();

    // The load-bearing invariant: the backend must NOT have accepted a
    // truncated request as a COMPLETE one.
    assert!(
        !backend_saw_complete,
        "H2 backend must NEVER see a mid-request-aborted body as a \
         cleanly-ended (complete) request — got complete={backend_saw_complete}, \
         backend_body_len={backend_body_len} (intended {})",
        payload.len()
    );
    // The client must not have received a normal 200+FIN for an
    // aborted request.
    assert!(
        !(out.status == Some(200) && out.fin),
        "an aborted request must not yield a clean 200 FIN to the client"
    );
}
