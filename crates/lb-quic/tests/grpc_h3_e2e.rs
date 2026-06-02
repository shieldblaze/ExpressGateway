//! S29 — gRPC-over-HTTP/3 conformance e2e (real wire).
//!
//! Path under test: a real quiche H3 client speaking the **gRPC wire
//! format** (5-byte length-prefixed messages + `grpc-status` trailers) →
//! production [`QuicListener`] (H3-terminate, `quiche::h3`) → `router` →
//! `conn_actor::poll_h3` (H2 branch) → `h3_to_h2_stream` → real hyper H2
//! gRPC backend.
//!
//! The gateway is an **opaque proxy**: it forwards the length-prefixed
//! gRPC messages + the trailing `grpc-status`/`grpc-message` HEADERS
//! end-to-end without interpreting gRPC (S29 R7 model). These tests are
//! the CONFORMANCE characterization — they assert the gRPC-critical
//! invariants survive the H3→H2 relay:
//!
//!  * **A4/A5** — every response ends with a trailing HEADERS carrying
//!    `grpc-status` (+ `grpc-message`); dropping it breaks gRPC silently.
//!  * **A6** — a "Trailers-Only" response (single HEADERS+FIN carrying
//!    `:status` AND `grpc-status`, no DATA, no separate trailing HEADERS,
//!    the immediate-error shape) is preserved.
//!  * **A2/A3** — `content-type: application/grpc*` preserved; the
//!    length-prefixed messages relayed opaquely across DATA boundaries.
//!  * **4 call types** — unary, server-streaming, client-streaming, and
//!    bidi all relay correctly (they differ only in message count/timing;
//!    for an opaque proxy it is uniform DATA relay + trailer delivery).
//!
//! gRPC framing/codec is reused from `lb_grpc` (no `tonic`/`prost`
//! dev-dep); the H3 client + hyper H2 backend harnesses are adapted from
//! `h3_h2_stream_e2e.rs` (hand-rolled hyper IO + executor, no
//! `hyper-util`). No `#[ignore]` anywhere.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::collections::VecDeque;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{HeaderMap, Request, Response, StatusCode};
use lb_grpc::{DEFAULT_MAX_MESSAGE_SIZE, GrpcFrame, decode_grpc_frame, encode_grpc_frame};
use lb_io::http2_pool::{Http2Pool, Http2PoolConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::{QuicListener, QuicListenerParams};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_util::sync::CancellationToken;

use lb_h3_testcodec::{H3Frame, QpackEncoder, decode_frame, encode_frame};

const TEST_SNI: &str = "expressgateway.test";
const H3_ALPN: &[u8] = b"h3";
const MAX_UDP: usize = 65_535;

// ─────────────────────────────────────────────────────────────────────
// §0  Hand-rolled hyper IO adapter + executor (no hyper-util dep), and
//     a quiche-Huffman-capable QPACK response decoder. Copied verbatim
//     from h3_h2_stream_e2e.rs (test harnesses are separate binaries and
//     cannot share a module).
// ─────────────────────────────────────────────────────────────────────

/// Decode a RESPONSE QPACK field block emitted by the migrated egress
/// (`quiche::h3`, which Huffman-encodes). The hand-rolled
/// `lb_h3_testcodec::QpackDecoder` is raw-only, so we use quiche's.
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

struct HyperIo(TcpStream);

impl hyper::rt::Read for HyperIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        let cap = buf.remaining().min(16 * 1024);
        if cap == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut tmp = vec![0u8; cap];
        let mut rb = tokio::io::ReadBuf::new(&mut tmp);
        match Pin::new(&mut self.0).poll_read(cx, &mut rb) {
            Poll::Ready(Ok(())) => {
                let n = rb.filled().len();
                buf.put_slice(tmp.get(..n).unwrap_or(&[]));
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
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

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
// §1  Cert + listener + client harness (mirrors h3_h2_stream_e2e).
// ─────────────────────────────────────────────────────────────────────

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
        "lb-quic-grpc-h3-{}-{}-{counter}",
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

// ─────────────────────────────────────────────────────────────────────
// §2  gRPC framing helpers (lb_grpc) + the H3 client driver.
// ─────────────────────────────────────────────────────────────────────

/// Concatenate `msgs` as length-prefixed gRPC frames (5-byte header each).
fn frame_messages(msgs: &[Bytes]) -> Vec<u8> {
    let mut out = Vec::new();
    for m in msgs {
        let f = GrpcFrame {
            compressed: false,
            data: m.clone(),
        };
        out.extend_from_slice(&encode_grpc_frame(&f).unwrap());
    }
    out
}

/// Decode every gRPC frame in `buf` into its message payloads.
fn decode_all_grpc(buf: &[u8]) -> Vec<Bytes> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < buf.len() {
        match decode_grpc_frame(buf.get(offset..).unwrap_or(&[]), DEFAULT_MAX_MESSAGE_SIZE) {
            Ok((frame, consumed)) => {
                out.push(frame.data);
                offset += consumed;
            }
            Err(_) => break,
        }
    }
    out
}

/// What the driven gRPC-over-H3 client observed.
#[derive(Default)]
struct GrpcClientOut {
    status: Option<u16>,
    /// Raw response DATA bytes (the length-prefixed gRPC message stream).
    body: Vec<u8>,
    fin: bool,
    reset: bool,
    /// Field pairs from the response-head HEADERS frame (frame #1).
    head_pairs: Vec<(String, String)>,
    /// Field pairs from the trailing HEADERS frame(s) (frame #2+).
    trailer_pairs: Vec<(String, String)>,
    /// Number of decoded HEADERS frames (1 = Trailers-Only; 2 = head +
    /// trailers).
    headers_frames: usize,
}

impl GrpcClientOut {
    /// Look up a field by (lowercase) name across head + trailers — gRPC
    /// `grpc-status` lives in the trailer normally, but in a Trailers-Only
    /// response it rides the single head HEADERS frame.
    fn field(&self, name: &str) -> Option<&str> {
        self.trailer_pairs
            .iter()
            .chain(self.head_pairs.iter())
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    /// Decoded gRPC response message payloads.
    fn messages(&self) -> Vec<Bytes> {
        decode_all_grpc(&self.body)
    }
}

#[allow(clippy::struct_excessive_bools)]
struct GrpcDriveCfg {
    path: &'static str,
    /// Request-body DATA chunks (already gRPC-framed). FIN after the last.
    req_chunks: Vec<Vec<u8>>,
    /// Extra request headers (e.g. ("grpc-timeout","1S")).
    extra_headers: Vec<(String, String)>,
    /// content-type to send (defaults to application/grpc when None).
    content_type: Option<&'static str>,
}

/// Drive ONE gRPC request on stream 0: POST + content-type +
/// length-prefixed body, then read the response HEADERS/DATA/trailers.
async fn drive_grpc_h3(
    gateway: SocketAddr,
    ca: &std::path::Path,
    cfg: GrpcDriveCfg,
    overall: Duration,
) -> GrpcClientOut {
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
    let mut req_done = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut out = GrpcClientOut::default();

    let deadline = tokio::time::Instant::now() + overall;
    while tokio::time::Instant::now() < deadline {
        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    let _ = sock.send_to(out_buf.get(..n).unwrap_or(&[]), info.to).await;
                }
                Err(quiche::Error::Done) | Err(_) => break,
            }
        }

        if conn.is_established() && !head_sent {
            let encoder = QpackEncoder::new();
            let mut headers = vec![
                (":method".to_string(), "POST".to_string()),
                (":scheme".to_string(), "https".to_string()),
                (":authority".to_string(), TEST_SNI.to_string()),
                (":path".to_string(), cfg.path.to_string()),
                (
                    "content-type".to_string(),
                    cfg.content_type.unwrap_or("application/grpc").to_string(),
                ),
                ("te".to_string(), "trailers".to_string()),
            ];
            for (k, v) in &cfg.extra_headers {
                headers.push((k.clone(), v.clone()));
            }
            let block = encoder.encode(&headers).unwrap();
            tx_wire.extend_from_slice(
                &encode_frame(&H3Frame::Headers {
                    header_block: block,
                })
                .unwrap(),
            );
            for chunk in &cfg.req_chunks {
                tx_wire.extend_from_slice(
                    &encode_frame(&H3Frame::Data {
                        payload: Bytes::from(chunk.clone()),
                    })
                    .unwrap(),
                );
            }
            head_sent = true;
        }

        if head_sent && !req_done {
            while !req_done && tx_off < tx_wire.len() {
                let remaining = tx_wire.get(tx_off..).unwrap_or(&[]);
                match conn.stream_send(sid, remaining, true) {
                    Ok(0) => break,
                    Ok(n) => {
                        tx_off += n;
                        if tx_off >= tx_wire.len() {
                            req_done = true;
                        }
                    }
                    Err(quiche::Error::Done) | Err(_) => break,
                }
            }
        }

        if conn.is_established() {
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
                        out.headers_frames += 1;
                        let is_head = out.headers_frames == 1;
                        if let Ok(h) = decode_resp_qpack(&header_block) {
                            for (n, v) in h {
                                if n == ":status" {
                                    out.status = v.parse().ok();
                                }
                                if is_head {
                                    out.head_pairs.push((n, v));
                                } else {
                                    out.trailer_pairs.push((n, v));
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
// §3  Real hyper H2 gRPC backend.
// ─────────────────────────────────────────────────────────────────────

/// How the mock gRPC backend behaves.
#[derive(Clone)]
enum GrpcBackendMode {
    /// Echo each inbound message back as one response message.
    Echo,
    /// Emit `per_request` response messages for each inbound message.
    ServerStream { per_request: usize },
    /// Concatenate all inbound messages into one response message.
    ClientStream,
    /// "Trailers-Only": a single HEADERS frame with `:status:200` +
    /// `content-type` + `grpc-status` (+ message) and END_STREAM, NO DATA
    /// and NO separate trailing HEADERS (the gRPC immediate-error shape).
    TrailersOnly { status: u32, message: &'static str },
}

#[derive(Default)]
struct GrpcBackendState {
    hits: AtomicUsize,
    last_content_type: Mutex<Option<String>>,
    last_te: Mutex<Option<String>>,
}

/// Response body that yields a queue of DATA frames then a trailer
/// section — dependency-free (no futures_util / StreamBody).
struct GrpcRespBody {
    data: VecDeque<Bytes>,
    trailers: Option<HeaderMap>,
}
impl hyper::body::Body for GrpcRespBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        if let Some(d) = self.data.pop_front() {
            return Poll::Ready(Some(Ok(hyper::body::Frame::data(d))));
        }
        if let Some(t) = self.trailers.take() {
            return Poll::Ready(Some(Ok(hyper::body::Frame::trailers(t))));
        }
        Poll::Ready(None)
    }
}

fn grpc_trailers(status: u32, message: &str) -> HeaderMap {
    let mut t = HeaderMap::new();
    t.insert(
        "grpc-status",
        hyper::header::HeaderValue::from_str(&status.to_string()).unwrap(),
    );
    t.insert(
        "grpc-message",
        hyper::header::HeaderValue::from_str(message).unwrap_or(hyper::header::HeaderValue::from_static("")),
    );
    t
}

async fn spawn_h2_grpc_backend(
    mode: GrpcBackendMode,
) -> (SocketAddr, Arc<GrpcBackendState>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let state = Arc::new(GrpcBackendState::default());
    let st = Arc::clone(&state);
    let h = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let mode = mode.clone();
            let st = Arc::clone(&st);
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let mode = mode.clone();
                    let st = Arc::clone(&st);
                    async move { Ok::<_, std::convert::Infallible>(grpc_backend_handle(req, mode, st).await) }
                });
                let _ = hyper::server::conn::http2::Builder::new(TokioExec)
                    .serve_connection(HyperIo(sock), svc)
                    .await;
            });
        }
    });
    (local, state, h)
}

async fn grpc_backend_handle(
    req: Request<Incoming>,
    mode: GrpcBackendMode,
    state: Arc<GrpcBackendState>,
) -> Response<GrpcRespBody> {
    state.hits.fetch_add(1, Ordering::Relaxed);
    if let Some(ct) = req.headers().get("content-type").and_then(|v| v.to_str().ok()) {
        *state.last_content_type.lock().unwrap() = Some(ct.to_owned());
    }
    if let Some(te) = req.headers().get("te").and_then(|v| v.to_str().ok()) {
        *state.last_te.lock().unwrap() = Some(te.to_owned());
    }

    // Trailers-Only: grpc-status rides the response HEADERS, empty body
    // ⇒ hyper emits a single HEADERS frame with END_STREAM.
    if let GrpcBackendMode::TrailersOnly { status, message } = mode {
        let _ = req.into_body().collect().await;
        let mut builder = Response::builder()
            .status(StatusCode::OK)
            .header(hyper::header::CONTENT_TYPE, "application/grpc")
            .header("grpc-status", status.to_string());
        if !message.is_empty() {
            builder = builder.header("grpc-message", message);
        }
        return builder
            .body(GrpcRespBody {
                data: VecDeque::new(),
                trailers: None,
            })
            .unwrap();
    }

    let body_bytes = req.into_body().collect().await.unwrap().to_bytes();
    let inbound = decode_all_grpc(&body_bytes);

    let resp_msgs: Vec<Bytes> = match mode {
        GrpcBackendMode::Echo => inbound
            .into_iter()
            .map(|p| {
                encode_grpc_frame(&GrpcFrame {
                    compressed: false,
                    data: p,
                })
                .unwrap()
            })
            .collect(),
        GrpcBackendMode::ServerStream { per_request } => {
            let mut out = Vec::new();
            for (i, p) in inbound.iter().enumerate() {
                for j in 0..per_request {
                    let tagged = Bytes::from(format!(
                        "{}-{i}-{j}",
                        std::str::from_utf8(p).unwrap_or("req")
                    ));
                    out.push(
                        encode_grpc_frame(&GrpcFrame {
                            compressed: false,
                            data: tagged,
                        })
                        .unwrap(),
                    );
                }
            }
            out
        }
        GrpcBackendMode::ClientStream => {
            let cat: Bytes = inbound
                .iter()
                .flat_map(|b| b.iter().copied())
                .collect::<Vec<u8>>()
                .into();
            vec![encode_grpc_frame(&GrpcFrame {
                compressed: false,
                data: cat,
            })
            .unwrap()]
        }
        GrpcBackendMode::TrailersOnly { .. } => unreachable!(),
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "application/grpc+proto")
        .body(GrpcRespBody {
            data: resp_msgs.into_iter().collect(),
            trailers: Some(grpc_trailers(0, "")),
        })
        .unwrap()
}

// ─────────────────────────────────────────────────────────────────────
// §4  Conformance tests — the 4 call types + trailers + Trailers-Only.
// ─────────────────────────────────────────────────────────────────────

const OVERALL: Duration = Duration::from_secs(20);

/// A1-A5 — unary echo: one request message, one response message, and a
/// trailing `grpc-status: 0` HEADERS delivered end-to-end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_unary_echo_delivers_status_trailer() {
    let certs = generate_loopback_certs();
    let (backend, st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::Echo).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    let payload = Bytes::from_static(b"hello grpc over h3");
    let body = frame_messages(std::slice::from_ref(&payload));
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/echo.Echo/Unary",
            req_chunks: vec![body],
            extra_headers: vec![],
            content_type: None,
        },
        OVERALL,
    )
    .await;

    assert_eq!(out.status, Some(200), "h3 :status (head_pairs={:?})", out.head_pairs);
    let msgs = out.messages();
    assert_eq!(msgs.len(), 1, "one echoed message");
    assert_eq!(msgs[0], payload, "echoed payload byte-identical");
    // A4: grpc-status:0 trailer delivered.
    assert_eq!(out.field("grpc-status"), Some("0"), "grpc-status trailer (trailers={:?})", out.trailer_pairs);
    assert!(out.fin, "clean stream FIN");
    // A2: content-type reached the backend intact.
    assert_eq!(
        st.last_content_type.lock().unwrap().as_deref(),
        Some("application/grpc"),
        "content-type forwarded to backend"
    );
}

/// Server-streaming: one request message, N response messages, then
/// `grpc-status: 0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_server_stream_delivers_all_messages_and_trailer() {
    let certs = generate_loopback_certs();
    let (backend, _st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::ServerStream { per_request: 16 }).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    let body = frame_messages(&[Bytes::from_static(b"req")]);
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/echo.Echo/ServerStream",
            req_chunks: vec![body],
            extra_headers: vec![],
            content_type: None,
        },
        OVERALL,
    )
    .await;

    assert_eq!(out.status, Some(200));
    let msgs = out.messages();
    assert_eq!(msgs.len(), 16, "all server-stream messages relayed");
    for (i, m) in msgs.iter().enumerate() {
        assert_eq!(&m[..], format!("req-0-{i}").as_bytes());
    }
    assert_eq!(out.field("grpc-status"), Some("0"));
    assert!(out.fin);
}

/// Client-streaming: N request messages, one concatenated response
/// message, then `grpc-status: 0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_client_stream_relays_all_request_messages() {
    let certs = generate_loopback_certs();
    let (backend, _st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::ClientStream).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    let msgs: Vec<Bytes> = (0..12).map(|i| Bytes::from(format!("m{i}"))).collect();
    let body = frame_messages(&msgs);
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/echo.Echo/ClientStream",
            req_chunks: vec![body],
            extra_headers: vec![],
            content_type: None,
        },
        OVERALL,
    )
    .await;

    assert_eq!(out.status, Some(200));
    let resp = out.messages();
    assert_eq!(resp.len(), 1);
    let expected: Bytes = msgs
        .iter()
        .flat_map(|b| b.iter().copied())
        .collect::<Vec<u8>>()
        .into();
    assert_eq!(resp[0], expected, "all request messages relayed + concatenated");
    assert_eq!(out.field("grpc-status"), Some("0"));
}

/// Bidi-streaming (sequential drive): 8 request messages echoed back as 8
/// response messages, order-preserved, then `grpc-status: 0`. For an
/// opaque proxy this is the same relay path as the other call types.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_bidi_stream_echoes_in_order() {
    let certs = generate_loopback_certs();
    let (backend, _st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::Echo).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    let msgs: Vec<Bytes> = (0..8).map(|i| Bytes::from(format!("ping-{i}"))).collect();
    let body = frame_messages(&msgs);
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/echo.Echo/Bidi",
            req_chunks: vec![body],
            extra_headers: vec![],
            content_type: Some("application/grpc+proto"),
        },
        OVERALL,
    )
    .await;

    assert_eq!(out.status, Some(200));
    let resp = out.messages();
    assert_eq!(resp.len(), 8);
    for (i, m) in resp.iter().enumerate() {
        assert_eq!(&m[..], format!("ping-{i}").as_bytes());
    }
    assert_eq!(out.field("grpc-status"), Some("0"));
}

/// A6 — Trailers-Only: an immediate-error response carried as a single
/// HEADERS frame (`:status:200` + `content-type` + `grpc-status:9`) with
/// END_STREAM, no DATA. The proxy must preserve the single-field-section
/// shape; the client observes grpc-status on the head, no body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_trailers_only_immediate_error_preserved() {
    let certs = generate_loopback_certs();
    let (backend, _st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::TrailersOnly {
        status: 9,
        message: "precondition failed",
    })
    .await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    let body = frame_messages(&[Bytes::from_static(b"x")]);
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/echo.Echo/Failing",
            req_chunks: vec![body],
            extra_headers: vec![],
            content_type: None,
        },
        OVERALL,
    )
    .await;

    assert_eq!(out.status, Some(200), "Trailers-Only carries :status 200");
    assert!(out.messages().is_empty(), "no DATA in a Trailers-Only response");
    assert_eq!(out.field("grpc-status"), Some("9"), "grpc-status on the single HEADERS frame");
    assert_eq!(out.field("grpc-message"), Some("precondition failed"));
    assert!(out.fin, "clean FIN");
}
