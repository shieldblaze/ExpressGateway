//! gRPC-over-HTTP/3 conformance e2e on the real wire: a quiche H3 client speaking the gRPC wire
//! format → production [`QuicListener`] → `conn_actor::poll_h3` (H2 branch) → a real hyper H2
//! gRPC backend. The gateway is an OPAQUE proxy: it forwards length-prefixed messages and the
//! trailing `grpc-status` HEADERS without interpreting gRPC, so these assert the gRPC-critical
//! invariants survive the relay. Framing is reused from `lb_grpc` (no tonic/prost dev-dep).

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

/// Serialize EVERY test in this binary. Each spawns an in-process gateway + backend +
/// real-wire client, and 16 concurrently over-saturate the box under `--all-features`, timing
/// the upstream dial out into a 502 (the CF-SATURATION-1 flake class). It also serializes the
/// process-global `MAX_RETAINED_RESP_BYTES` gauge read. Released on unwind, so a panic cannot
/// deadlock the suite.
static SUITE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Acquire the suite-wide serial lock; held for the test's duration.
macro_rules! serial_guard {
    () => {
        let _suite_serial = SUITE_SERIAL.lock().await;
    };
}

/// The migrated egress Huffman-encodes values; `lb_h3_testcodec::QpackDecoder` is raw-only.
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

#[derive(Default)]
struct GrpcClientOut {
    status: Option<u16>,
    /// Raw response DATA bytes (the length-prefixed gRPC message stream).
    body: Vec<u8>,
    fin: bool,
    reset: bool,
    head_pairs: Vec<(String, String)>,
    trailer_pairs: Vec<(String, String)>,
    /// Decoded HEADERS frames: 1 = Trailers-Only, 2 = head + trailers.
    headers_frames: usize,
}

impl GrpcClientOut {
    /// Look up across head + trailers: `grpc-status` normally rides the trailer, but in a
    /// Trailers-Only response it rides the head frame.
    fn field(&self, name: &str) -> Option<&str> {
        self.trailer_pairs
            .iter()
            .chain(self.head_pairs.iter())
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn messages(&self) -> Vec<Bytes> {
        decode_all_grpc(&self.body)
    }
}

#[allow(clippy::struct_excessive_bools)]
struct GrpcDriveCfg {
    path: &'static str,
    /// Request-body DATA chunks (already gRPC-framed). FIN after the last.
    req_chunks: Vec<Vec<u8>>,
    extra_headers: Vec<(String, String)>,
    /// content-type to send (defaults to application/grpc when None).
    content_type: Option<&'static str>,
}

/// Drive ONE gRPC request on stream 0: POST + content-type + `te: trailers` +
/// length-prefixed body, then read the response HEADERS/DATA/trailers.
async fn drive_grpc_h3(
    gateway: SocketAddr,
    ca: &std::path::Path,
    cfg: GrpcDriveCfg,
    overall: Duration,
) -> GrpcClientOut {
    let headers = build_grpc_headers(&cfg, true);
    drive_grpc_h3_core(gateway, ca, headers, cfg.req_chunks, overall).await
}

/// B3 — a gRPC request WITHOUT `te: trailers` (not required on H3).
async fn drive_grpc_h3_no_te(
    gateway: SocketAddr,
    ca: &std::path::Path,
    path: &'static str,
    req_chunks: Vec<Vec<u8>>,
    overall: Duration,
) -> GrpcClientOut {
    let cfg = GrpcDriveCfg {
        path,
        req_chunks: req_chunks.clone(),
        extra_headers: vec![],
        content_type: None,
    };
    let headers = build_grpc_headers(&cfg, false);
    drive_grpc_h3_core(gateway, ca, headers, req_chunks, overall).await
}

fn build_grpc_headers(cfg: &GrpcDriveCfg, include_te: bool) -> Vec<(String, String)> {
    let mut headers = vec![
        (":method".to_string(), "POST".to_string()),
        (":scheme".to_string(), "https".to_string()),
        (":authority".to_string(), TEST_SNI.to_string()),
        (":path".to_string(), cfg.path.to_string()),
        (
            "content-type".to_string(),
            cfg.content_type.unwrap_or("application/grpc").to_string(),
        ),
    ];
    if include_te {
        headers.push(("te".to_string(), "trailers".to_string()));
    }
    for (k, v) in &cfg.extra_headers {
        headers.push((k.clone(), v.clone()));
    }
    headers
}

async fn drive_grpc_h3_core(
    gateway: SocketAddr,
    ca: &std::path::Path,
    headers: Vec<(String, String)>,
    req_chunks: Vec<Vec<u8>>,
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
        while let Ok((n, info)) = conn.send(&mut out_buf) {
            let _ = sock.send_to(out_buf.get(..n).unwrap_or(&[]), info.to).await;
        }

        if conn.is_established() && !head_sent {
            let encoder = QpackEncoder::new();
            let block = encoder.encode(&headers).unwrap();
            tx_wire.extend_from_slice(
                &encode_frame(&H3Frame::Headers {
                    header_block: block,
                })
                .unwrap(),
            );
            for chunk in &req_chunks {
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

#[derive(Clone)]
enum GrpcBackendMode {
    Echo,
    ServerStream { per_request: usize },
    /// Concatenate all inbound messages into one response message.
    ClientStream,
    /// Like `Echo` but many small (≤32 KiB) hyper body frames instead of one giant frame — the
    /// localizer for single-huge-frame vs total-size.
    EchoSmallFrames,
    /// "Trailers-Only": one HEADERS frame with `grpc-status` and END_STREAM, no DATA and no
    /// separate trailing HEADERS (the gRPC immediate-error shape).
    TrailersOnly { status: u32, message: &'static str },
}

#[derive(Default)]
struct GrpcBackendState {
    hits: AtomicUsize,
    last_content_type: Mutex<Option<String>>,
    last_te: Mutex<Option<String>>,
    last_grpc_timeout: Mutex<Option<String>>,
    last_path: Mutex<Option<String>>,
    /// Whether the request body ended cleanly (FIN); false when the body stream errors/aborts.
    last_body_complete: std::sync::atomic::AtomicBool,
}

/// A queue of DATA frames then a trailer section — dependency-free (no futures_util/StreamBody).
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
        hyper::header::HeaderValue::from_str(message)
            .unwrap_or(hyper::header::HeaderValue::from_static("")),
    );
    t
}

async fn spawn_h2_grpc_backend(
    mode: GrpcBackendMode,
) -> (
    SocketAddr,
    Arc<GrpcBackendState>,
    tokio::task::JoinHandle<()>,
) {
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
                    async move {
                        Ok::<_, std::convert::Infallible>(grpc_backend_handle(req, mode, st).await)
                    }
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
    *state.last_path.lock().unwrap() = Some(req.uri().path().to_owned());
    if let Some(ct) = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
    {
        *state.last_content_type.lock().unwrap() = Some(ct.to_owned());
    }
    if let Some(te) = req.headers().get("te").and_then(|v| v.to_str().ok()) {
        *state.last_te.lock().unwrap() = Some(te.to_owned());
    }
    if let Some(t) = req
        .headers()
        .get("grpc-timeout")
        .and_then(|v| v.to_str().ok())
    {
        *state.last_grpc_timeout.lock().unwrap() = Some(t.to_owned());
    }

    // Trailers-Only: grpc-status rides the response HEADERS and the body is empty, so hyper
    // emits a single HEADERS frame with END_STREAM.
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

    let collected = match req.into_body().collect().await {
        Ok(c) => {
            state.last_body_complete.store(true, Ordering::Relaxed);
            c
        }
        Err(_) => {
            // Request body aborted (client cancelled mid-stream): the upstream leg was reset
            // before the body completed.
            state.last_body_complete.store(false, Ordering::Relaxed);
            return Response::builder()
                .status(StatusCode::OK)
                .header(hyper::header::CONTENT_TYPE, "application/grpc")
                .body(GrpcRespBody {
                    data: VecDeque::new(),
                    trailers: Some(grpc_trailers(1, "client cancelled")),
                })
                .unwrap();
        }
    };
    let body_bytes = collected.to_bytes();
    let inbound = decode_all_grpc(&body_bytes);
    let small_frames = matches!(mode, GrpcBackendMode::EchoSmallFrames);

    let resp_msgs: Vec<Bytes> = match mode {
        GrpcBackendMode::Echo | GrpcBackendMode::EchoSmallFrames => inbound
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
            vec![
                encode_grpc_frame(&GrpcFrame {
                    compressed: false,
                    data: cat,
                })
                .unwrap(),
            ]
        }
        GrpcBackendMode::TrailersOnly { .. } => unreachable!(),
    };

    let data: VecDeque<Bytes> = if small_frames {
        let mut all = Vec::new();
        for m in &resp_msgs {
            all.extend_from_slice(m);
        }
        all.chunks(32 * 1024).map(Bytes::copy_from_slice).collect()
    } else {
        resp_msgs.into_iter().collect()
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "application/grpc+proto")
        .body(GrpcRespBody {
            data,
            trailers: Some(grpc_trailers(0, "")),
        })
        .unwrap()
}

/// Emits HEADERS + one partial DATA frame then RST_STREAMs with NO `grpc-status` trailer, to
/// prove the gateway does not launder a mid-stream reset into a clean `grpc-status: 0`.
async fn spawn_h2_grpc_reset_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let h = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| async move {
                    let _ = req.into_body().collect().await;
                    struct ResetBody {
                        sent: bool,
                    }
                    impl hyper::body::Body for ResetBody {
                        type Data = Bytes;
                        type Error = std::io::Error;
                        fn poll_frame(
                            mut self: Pin<&mut Self>,
                            _cx: &mut Context<'_>,
                        ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>>
                        {
                            if !self.sent {
                                self.sent = true;
                                let f = encode_grpc_frame(&GrpcFrame {
                                    compressed: false,
                                    data: Bytes::from_static(b"partial"),
                                })
                                .unwrap();
                                return Poll::Ready(Some(Ok(hyper::body::Frame::data(f))));
                            }
                            Poll::Ready(Some(Err(std::io::Error::other(
                                "backend reset mid-response",
                            ))))
                        }
                    }
                    Ok::<_, std::convert::Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(hyper::header::CONTENT_TYPE, "application/grpc+proto")
                            .body(ResetBody { sent: false })
                            .unwrap(),
                    )
                });
                let _ = hyper::server::conn::http2::Builder::new(TokioExec)
                    .serve_connection(HyperIo(sock), svc)
                    .await;
            });
        }
    });
    (local, h)
}

/// Raw HTTP/1.1 gRPC backend (no hyper http1 dep): 200 + `content-type: application/grpc` + the
/// echoed body with a `Content-Length` and NO trailer — the realistic H1 case, since an H1 origin
/// cannot reliably carry the always-present `grpc-status` trailing HEADERS.
async fn spawn_h1_grpc_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let h = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    match sock.read(&mut tmp).await {
                        Ok(0) => return,
                        Ok(n) => buf.extend_from_slice(tmp.get(..n).unwrap_or(&[])),
                        Err(_) => return,
                    }
                    let Some(hdr_end) = find_subsequence(&buf, b"\r\n\r\n") else {
                        continue;
                    };
                    let content_len = parse_content_length(buf.get(..hdr_end).unwrap_or(&[]));
                    let body_start = hdr_end + 4;
                    while buf.len().saturating_sub(body_start) < content_len {
                        match sock.read(&mut tmp).await {
                            Ok(0) => break,
                            Ok(n) => buf.extend_from_slice(tmp.get(..n).unwrap_or(&[])),
                            Err(_) => break,
                        }
                    }
                    let end = (body_start + content_len).min(buf.len());
                    let req_body = buf.get(body_start..end).unwrap_or(&[]);
                    let inbound = decode_all_grpc(req_body);
                    let mut resp_body = Vec::new();
                    for p in inbound {
                        resp_body.extend_from_slice(
                            &encode_grpc_frame(&GrpcFrame {
                                compressed: false,
                                data: p,
                            })
                            .unwrap(),
                        );
                    }
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/grpc+proto\r\nContent-Length: {}\r\n\r\n",
                        resp_body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&resp_body).await;
                    return;
                }
            });
        }
    });
    (local, h)
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_content_length(head: &[u8]) -> usize {
    let text = String::from_utf8_lossy(head);
    for line in text.split("\r\n") {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                return v.trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

async fn start_h3_listener_h1(
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

const OVERALL: Duration = Duration::from_secs(20);

/// A1-A5 — unary echo: one request message, one response message, trailing `grpc-status: 0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_unary_echo_delivers_status_trailer() {
    serial_guard!();
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

    assert_eq!(
        out.status,
        Some(200),
        "h3 :status (head_pairs={:?})",
        out.head_pairs
    );
    let msgs = out.messages();
    assert_eq!(msgs.len(), 1, "one echoed message");
    assert_eq!(msgs[0], payload, "echoed payload byte-identical");
    assert_eq!(
        out.field("grpc-status"),
        Some("0"),
        "grpc-status trailer (trailers={:?})",
        out.trailer_pairs
    );
    assert!(out.fin, "clean stream FIN");
    assert_eq!(
        st.last_content_type.lock().unwrap().as_deref(),
        Some("application/grpc"),
        "content-type forwarded to backend"
    );
}

/// Server-streaming: one request message, N response messages, then `grpc-status: 0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_server_stream_delivers_all_messages_and_trailer() {
    serial_guard!();
    let certs = generate_loopback_certs();
    let (backend, _st, _bh) =
        spawn_h2_grpc_backend(GrpcBackendMode::ServerStream { per_request: 16 }).await;
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

/// Client-streaming: N request messages, one concatenated response, then `grpc-status: 0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_client_stream_relays_all_request_messages() {
    serial_guard!();
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
    assert_eq!(
        resp[0], expected,
        "all request messages relayed + concatenated"
    );
    assert_eq!(out.field("grpc-status"), Some("0"));
}

/// Bidi-streaming (sequential drive): 8 messages echoed in order — for an opaque proxy this is
/// the same relay path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_bidi_stream_echoes_in_order() {
    serial_guard!();
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

/// A6 — Trailers-Only: the proxy must preserve the single-field-section shape, so the client
/// sees grpc-status on the head and no body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_trailers_only_immediate_error_preserved() {
    serial_guard!();
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
    assert!(
        out.messages().is_empty(),
        "no DATA in a Trailers-Only response"
    );
    assert_eq!(
        out.field("grpc-status"),
        Some("9"),
        "grpc-status on the single HEADERS frame"
    );
    assert_eq!(out.field("grpc-message"), Some("precondition failed"));
    assert!(out.fin, "clean FIN");
}

/// A3 under volume — a single 512 KiB message spanning MANY H3 DATA frames must round-trip
/// byte-identical: the proxy makes no DATA-boundary assumptions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_large_message_roundtrips_byte_identical() {
    serial_guard!();
    let certs = generate_loopback_certs();
    let (backend, _st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::Echo).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    let mut msg = Vec::with_capacity(512 * 1024);
    let mut s: u32 = 0x1234_5678;
    for i in 0..512 * 1024 {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        msg.push(((s >> 24) as u8) ^ (i as u8));
    }
    let payload = Bytes::from(msg);
    let body = frame_messages(std::slice::from_ref(&payload));
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/echo.Echo/BigUnary",
            req_chunks: vec![body],
            extra_headers: vec![],
            content_type: None,
        },
        Duration::from_secs(40),
    )
    .await;

    assert_eq!(out.status, Some(200));
    let msgs = out.messages();
    assert_eq!(msgs.len(), 1, "one large message reassembled");
    assert_eq!(msgs[0].len(), payload.len(), "length preserved");
    assert_eq!(
        msgs[0], payload,
        "512 KiB message byte-identical across DATA frames"
    );
    // The trailer must survive a large response (F-S29-1 regression).
    assert_eq!(
        out.field("grpc-status"),
        Some("0"),
        "grpc-status trailer on 512 KiB response"
    );
    assert!(out.fin, "clean FIN after the large response's trailer");
}

/// F-S29-1 regression — the trailing `grpc-status` HEADERS must survive a response of ANY size.
/// The drop was a stale-receiver respawn firing a spurious RESET that discarded the still-buffered
/// trailer+FIN above ~448 KiB. Sweeps to 1 MiB.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_trailer_survives_all_response_sizes() {
    serial_guard!();
    let certs = generate_loopback_certs();
    let (backend, _st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::Echo).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    for sz in [1usize, 256 * 1024, 512 * 1024, 1024 * 1024] {
        let payload = Bytes::from(vec![0xABu8; sz]);
        let body = frame_messages(std::slice::from_ref(&payload));
        let out = drive_grpc_h3(
            gw,
            &certs.ca,
            GrpcDriveCfg {
                path: "/echo.Echo/SizeSweep",
                req_chunks: vec![body],
                extra_headers: vec![],
                content_type: None,
            },
            Duration::from_secs(45),
        )
        .await;
        assert_eq!(out.status, Some(200), "sz={sz}");
        assert_eq!(
            out.messages().first().map(|m| m.len()),
            Some(sz),
            "sz={sz} message round-trips byte-length"
        );
        assert_eq!(
            out.field("grpc-status"),
            Some("0"),
            "sz={sz}: grpc-status trailer dropped (F-S29-1 regression)"
        );
        assert!(out.fin, "sz={sz}: clean FIN");
    }
}

/// F-S29-1 regression — the trailer must survive regardless of the backend's body FRAME
/// GRANULARITY: one giant hyper frame and many small ones, at both 512 KiB and 1 MiB.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_trailer_survives_any_frame_granularity() {
    serial_guard!();
    let certs = generate_loopback_certs();
    for (label, mode) in [
        ("giant", GrpcBackendMode::Echo),
        ("small", GrpcBackendMode::EchoSmallFrames),
    ] {
        let (backend, _st, _bh) = spawn_h2_grpc_backend(mode).await;
        let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;
        for sz in [512 * 1024usize, 1024 * 1024] {
            let payload = Bytes::from(vec![0xCDu8; sz]);
            let body = frame_messages(std::slice::from_ref(&payload));
            let out = drive_grpc_h3(
                gw,
                &certs.ca,
                GrpcDriveCfg {
                    path: "/echo.Echo/Granularity",
                    req_chunks: vec![body],
                    extra_headers: vec![],
                    content_type: None,
                },
                Duration::from_secs(45),
            )
            .await;
            assert_eq!(
                out.field("grpc-status"),
                Some("0"),
                "{label} frames, sz={sz}: grpc-status trailer dropped (F-S29-1)"
            );
            assert!(out.fin, "{label} frames, sz={sz}: clean FIN");
        }
    }
}

/// C1 / D1 — `grpc-timeout` is forwarded UNCHANGED: the opaque H3 path does NOT clamp it,
/// unlike the gRPC-aware H2 proxy. A deliberate H2-vs-H3 divergence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_grpc_timeout_forwarded_unclamped() {
    serial_guard!();
    let certs = generate_loopback_certs();
    let (backend, st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::Echo).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    let body = frame_messages(&[Bytes::from_static(b"x")]);
    // 600S far exceeds the H2 path's 300 s clamp; the opaque H3 path must forward it verbatim.
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/echo.Echo/Unary",
            req_chunks: vec![body],
            extra_headers: vec![("grpc-timeout".into(), "600S".into())],
            content_type: None,
        },
        OVERALL,
    )
    .await;

    assert_eq!(out.status, Some(200));
    assert_eq!(out.field("grpc-status"), Some("0"));
    assert_eq!(
        st.last_grpc_timeout.lock().unwrap().as_deref(),
        Some("600S"),
        "opaque H3 path forwards grpc-timeout UNCLAMPED (H2 path would clamp to 300S)"
    );
}

/// D2 — a Health/Check over H3 is FORWARDED to the backend rather than synthesized locally as
/// the gRPC-aware H2 proxy does. A deliberate H2-vs-H3 divergence; the backend being hit is the proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_health_check_forwarded_not_synthesized() {
    serial_guard!();
    let certs = generate_loopback_certs();
    let (backend, st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::Echo).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    let body = frame_messages(&[Bytes::new()]);
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/grpc.health.v1.Health/Check",
            req_chunks: vec![body],
            extra_headers: vec![],
            content_type: None,
        },
        OVERALL,
    )
    .await;

    assert_eq!(out.status, Some(200));
    assert_eq!(out.field("grpc-status"), Some("0"));
    assert_eq!(
        st.last_path.lock().unwrap().as_deref(),
        Some("/grpc.health.v1.Health/Check"),
        "opaque H3 path FORWARDS health to backend (no local synthesis)"
    );
    assert!(
        st.hits.load(Ordering::Relaxed) >= 1,
        "backend was dialed (no gateway-side health synthesis on H3)"
    );
}

/// B3 — a gRPC request WITHOUT `te: trailers` still succeeds over H3: the H2-path requirement
/// does not apply, since H3 trailers are native (RFC 9114 §4.1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_without_te_header_still_delivers_trailer() {
    serial_guard!();
    let certs = generate_loopback_certs();
    let (backend, _st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::Echo).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    let payload = Bytes::from_static(b"no-te");
    let body = frame_messages(std::slice::from_ref(&payload));
    let out = drive_grpc_h3_no_te(gw, &certs.ca, "/echo.Echo/NoTe", vec![body], OVERALL).await;

    assert_eq!(out.status, Some(200));
    let msgs = out.messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0], payload);
    assert_eq!(
        out.field("grpc-status"),
        Some("0"),
        "trailer delivered without te:trailers"
    );
}

/// ATTRIBUTION — call `stream_h2_response` DIRECTLY against the gRPC backend with no
/// backpressure. If the producer + hyper H2 client deliver `Trailers` + `End` for the large
/// responses, any drop is downstream in the H3 egress; if not, it is lost in the hyper H2 read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_producer_trailer_attribution() {
    serial_guard!();
    use http_body_util::Full;
    use lb_quic::h3_bridge::{MAX_RESPONSE_BODY_BYTES, RespEvent, stream_h2_response};

    for (label, mode) in [
        ("giant", GrpcBackendMode::Echo),
        ("small", GrpcBackendMode::EchoSmallFrames),
    ] {
        for sz in [512 * 1024usize, 1024 * 1024] {
            let (backend, _st, _bh) = spawn_h2_grpc_backend(mode.clone()).await;
            let stream = TcpStream::connect(backend).await.unwrap();
            let (mut sender, conn) = hyper::client::conn::http2::handshake::<_, _, Full<Bytes>>(
                TokioExec,
                HyperIo(stream),
            )
            .await
            .unwrap();
            tokio::spawn(async move {
                let _ = conn.await;
            });
            let payload = Bytes::from(vec![0xCDu8; sz]);
            let req_body = Bytes::from(frame_messages(std::slice::from_ref(&payload)));
            let req = Request::builder()
                .method("POST")
                .uri("/echo.Echo/Attr")
                .header(hyper::header::CONTENT_TYPE, "application/grpc")
                .header("te", "trailers")
                .body(Full::new(req_body))
                .unwrap();
            let resp = sender.send_request(req).await.unwrap();

            // Large channel ⇒ the producer never blocks, isolating "does hyper deliver the
            // trailer to the gateway".
            let (tx, mut rx) = tokio::sync::mpsc::channel::<RespEvent>(100_000);
            let r = stream_h2_response(resp, &tx, MAX_RESPONSE_BODY_BYTES).await;
            drop(tx);
            let mut got_trailers = false;
            let mut got_end = false;
            let mut got_reset = false;
            let mut body = 0usize;
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    RespEvent::Body(b) => body += b.len(),
                    RespEvent::Trailers(_) => got_trailers = true,
                    RespEvent::End => got_end = true,
                    RespEvent::Reset => got_reset = true,
                    RespEvent::Head { .. } => {}
                }
            }
            assert!(
                r.is_ok(),
                "{label} sz={sz}: stream_h2_response returned {r:?}"
            );
            assert!(
                got_trailers,
                "{label} sz={sz}: producer must deliver the grpc-status trailer to the gateway"
            );
            assert!(got_end, "{label} sz={sz}: producer must signal End");
            assert!(!got_reset, "{label} sz={sz}: clean response must not Reset");
            assert_eq!(body, sz + 5, "{label} sz={sz}: full framed body forwarded");
        }
    }
}

/// B2 (HIGHEST severity) — a backend RST_STREAM mid-response, after partial DATA and before any
/// grpc-status trailer, must NOT be laundered into a clean finish carrying a synthesized
/// `grpc-status: 0`. The gRPC analog of the migration's Finished-on-reset guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_backend_reset_midresponse_not_laundered_to_clean_status() {
    serial_guard!();
    let certs = generate_loopback_certs();
    let (backend, _bh) = spawn_h2_grpc_reset_backend().await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    let body = frame_messages(&[Bytes::from_static(b"start")]);
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/echo.Echo/Resetting",
            req_chunks: vec![body],
            extra_headers: vec![],
            content_type: None,
        },
        OVERALL,
    )
    .await;

    // Conformant outcomes: a stream reset, OR a finish with NO grpc-status:0. A clean
    // grpc-status:0 would mean the gateway laundered the reset into a successful RPC.
    eprintln!(
        "GRPC_H3_RESET reset={} fin={} status={:?} grpc_status={:?} body_len={}",
        out.reset,
        out.fin,
        out.status,
        out.field("grpc-status"),
        out.body.len()
    );
    assert_ne!(
        out.field("grpc-status"),
        Some("0"),
        "a mid-response RESET must NOT be laundered into grpc-status:0 (Finished-on-reset)"
    );
    assert!(
        out.reset || out.field("grpc-status").is_some_and(|s| s != "0"),
        "client must observe abnormal termination (reset) or a non-OK status, not a silent clean finish"
    );
}

/// B5 — gRPC routed to an HTTP/1.1 backend (a trailer-incompatible transport). gRPC mandates
/// HTTP/2+, so this CHARACTERIZES what the H3→H1 path actually does with such a response.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_to_h1_backend_trailer_characterization() {
    serial_guard!();
    let certs = generate_loopback_certs();
    let (backend, _bh) = spawn_h1_grpc_backend().await;
    let (_l, gw, _sd) = start_h3_listener_h1(&certs, backend).await;

    let payload = Bytes::from_static(b"over-h1");
    let body = frame_messages(std::slice::from_ref(&payload));
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/echo.Echo/OverH1",
            req_chunks: vec![body],
            extra_headers: vec![],
            content_type: None,
        },
        OVERALL,
    )
    .await;

    eprintln!(
        "GRPC_H3_H1 status={:?} grpc_status={:?} fin={} reset={} body_msgs={} head={:?} trailers={:?}",
        out.status,
        out.field("grpc-status"),
        out.fin,
        out.reset,
        out.messages().len(),
        out.head_pairs,
        out.trailer_pairs,
    );
    // The conformance finding (F-S29-2): an H1 backend cannot deliver the `grpc-status` trailer
    // — true whether the relay succeeds (200, trailer dropped) or fails (502). The assertion is
    // status-agnostic so it also holds under gate saturation.
    assert_eq!(
        out.field("grpc-status"),
        None,
        "gRPC over an H1 backend must NOT deliver a grpc-status trailer \
         (status={:?}, trailers={:?})",
        out.status,
        out.trailer_pairs
    );
}

/// R8 — a ~1 MiB gRPC server-stream across 512 messages must keep the H3 egress BOUNDED: the
/// `MAX_RETAINED_RESP_BYTES` peak stays within `4 × depth × (chunk + frame-hdr)` INDEPENDENT of
/// total response size. The total far exceeds the ceiling (non-vacuous) and the gauge must be > 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_server_stream_bounded_memory_r8() {
    serial_guard!();
    lb_quic::h3_bridge::MAX_RETAINED_RESP_BYTES.store(0, Ordering::Relaxed);

    let certs = generate_loopback_certs();
    let per_request = 512usize;
    let (backend, _st, _bh) =
        spawn_h2_grpc_backend(GrpcBackendMode::ServerStream { per_request }).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    // ~2 KiB per server-stream message ⇒ ~1 MiB total, far exceeding the egress retained ceiling.
    let payload = Bytes::from("A".repeat(2048));
    let body = frame_messages(std::slice::from_ref(&payload));
    let out = drive_grpc_h3(
        gw,
        &certs.ca,
        GrpcDriveCfg {
            path: "/echo.Echo/ServerStreamBig",
            req_chunks: vec![body],
            extra_headers: vec![],
            content_type: None,
        },
        Duration::from_secs(60),
    )
    .await;

    assert_eq!(out.status, Some(200));
    assert_eq!(
        out.messages().len(),
        per_request,
        "all server-stream messages relayed"
    );
    assert_eq!(
        out.field("grpc-status"),
        Some("0"),
        "trailer delivered after a large server-stream"
    );

    let retained = lb_quic::h3_bridge::MAX_RETAINED_RESP_BYTES.load(Ordering::Relaxed);
    let total: usize = out.body.len();
    let ceiling = 4
        * lb_quic::h3_bridge::H3_RESP_CHANNEL_DEPTH
        * (lb_quic::h3_bridge::H3_RESP_CHUNK_MAX + lb_quic::h3_bridge::H3_FRAME_HDR_MAX);
    eprintln!("GRPC_H3_R8 retained={retained} ceiling={ceiling} total={total}");
    assert!(
        retained > 0,
        "gauge vacuous: the egress never recorded in-flight bytes"
    );
    assert!(
        total > ceiling,
        "test vacuous: response total {total} must exceed the ceiling {ceiling} to exercise the bound"
    );
    assert!(
        retained <= ceiling,
        "retained {retained} exceeds 4×depth×(chunk+hdr) ceiling {ceiling} — \
         the egress is buffering the stream, not bounded (R8)"
    );
}

/// R13 (b) — burst: 50 sequential unary cycles, each on a fresh H3 connection, every one
/// delivering the echoed message + a clean `grpc-status: 0` + FIN. The load-bearing negative
/// control is the mid-response-reset test, which must NOT be laundered into a clean status.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_h3_burst_50_unary_cycles() {
    serial_guard!();
    let certs = generate_loopback_certs();
    let (backend, _st, _bh) = spawn_h2_grpc_backend(GrpcBackendMode::Echo).await;
    let (_l, gw, _sd) = start_h3_listener_h2(&certs, backend).await;

    for i in 0..50 {
        let payload = Bytes::from(format!("burst-{i}"));
        let body = frame_messages(std::slice::from_ref(&payload));
        let out = drive_grpc_h3(
            gw,
            &certs.ca,
            GrpcDriveCfg {
                path: "/echo.Echo/Burst",
                req_chunks: vec![body],
                extra_headers: vec![],
                content_type: None,
            },
            OVERALL,
        )
        .await;
        assert_eq!(out.status, Some(200), "iter {i}: status");
        assert_eq!(
            out.messages().first().cloned(),
            Some(payload.clone()),
            "iter {i}: echoed message"
        );
        assert_eq!(out.field("grpc-status"), Some("0"), "iter {i}: trailer");
        assert!(out.fin, "iter {i}: clean FIN");
    }
}
