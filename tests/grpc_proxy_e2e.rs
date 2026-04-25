//! End-to-end tests for Item 3 — the gRPC upstream path.
//!
//! These tests drive the `H2Proxy` with `GrpcProxy` wired in, against a
//! real hyper-H2 backend that speaks the gRPC wire format (5-byte frame
//! header + protobuf payload + trailers). No `tonic`/`prost` dev-deps —
//! we use `lb_grpc`'s frame codec + status/deadline helpers to play
//! both sides of the RPC at the byte level.
//!
//! Coverage:
//!
//! * unary echo (single request frame, single response frame, trailers)
//! * server streaming (one request, N response frames)
//! * client streaming (N request frames, one response frame)
//! * bidirectional streaming (interleaved frames)
//! * deadline clamp visible at the backend (client sends 600S, backend
//!   observes 300S or less)
//! * gateway-side deadline exceeded (backend stalls; client sees
//!   `grpc-status: 4`)
//! * synthesised `/grpc.health.v1.Health/Check`
//! * HTTP→gRPC status translation (backend returns bare HTTP 404; client
//!   observes `grpc-status: 12 UNIMPLEMENTED`)

#![cfg(test)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::{HeaderMap, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use lb_grpc::{DEFAULT_MAX_MESSAGE_SIZE, GrpcFrame, decode_grpc_frame, encode_grpc_frame};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::grpc_proxy::{GrpcConfig, GrpcProxy};
use lb_l7::h1_proxy::{HttpTimeouts, RoundRobinAddrs};
use lb_l7::h2_proxy::H2Proxy;
use parking_lot::Mutex;
use tokio::net::TcpListener;

// ── shared helpers ─────────────────────────────────────────────────────

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

type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

/// Stream body carrying an ordered list of data frames followed by a
/// trailer block. Used for both backend responses and client requests.
fn stream_body(data_frames: Vec<Bytes>, trailers: Option<HeaderMap>) -> BoxBody {
    let mut frames: Vec<Result<Frame<Bytes>, hyper::Error>> = data_frames
        .into_iter()
        .map(|b| Ok(Frame::data(b)))
        .collect();
    if let Some(t) = trailers {
        frames.push(Ok(Frame::trailers(t)));
    }
    let stream = futures_util::stream::iter(frames);
    http_body_util::combinators::BoxBody::new(StreamBody::new(stream))
}

/// Collect the body of a response into (data_bytes, trailers).
async fn collect_grpc_body(resp: Response<Incoming>) -> (Bytes, HeaderMap) {
    let collected = resp.into_body().collect().await.unwrap();
    let trailers = collected.trailers().cloned().unwrap_or_default();
    (collected.to_bytes(), trailers)
}

/// Build a framed gRPC request body: concatenate each message after a
/// 5-byte frame header.
fn frame_messages(msgs: &[Bytes]) -> Bytes {
    let mut out = Vec::new();
    for m in msgs {
        let f = GrpcFrame {
            compressed: false,
            data: m.clone(),
        };
        out.extend_from_slice(&encode_grpc_frame(&f).unwrap());
    }
    Bytes::from(out)
}

/// Decode all frames in `buf` into their payloads. Panics on any frame
/// error (tests only).
fn decode_all(buf: &[u8]) -> Vec<Bytes> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < buf.len() {
        let (frame, consumed) =
            decode_grpc_frame(&buf[offset..], DEFAULT_MAX_MESSAGE_SIZE).unwrap();
        out.push(frame.data);
        offset += consumed;
    }
    out
}

// ── backend harness ────────────────────────────────────────────────────

/// Describes how a mock backend should behave on the NEXT inbound RPC.
#[derive(Clone)]
enum BackendMode {
    /// Echo each inbound data frame back as a response frame.
    Echo,
    /// For each inbound frame, emit `per_request` response frames.
    ServerStream { per_request: usize },
    /// Accept N inbound frames, reply with a single concatenated frame.
    ClientStream,
    /// Sleep `delay` before responding (used to trigger gateway deadline).
    Sleep { delay: Duration },
    /// Return a bare HTTP 404 so the gateway can translate it.
    HttpNotFound,
}

/// Shared state captured by the backend for assertion purposes.
#[derive(Default)]
struct BackendState {
    last_grpc_timeout: Mutex<Option<String>>,
    hits: AtomicUsize,
}

async fn spawn_grpc_backend(
    mode: BackendMode,
) -> (SocketAddr, Arc<BackendState>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let state = Arc::new(BackendState::default());
    let state_for_task = Arc::clone(&state);

    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let mode = mode.clone();
            let state = Arc::clone(&state_for_task);
            tokio::spawn(async move {
                let builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
                let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                    let mode = mode.clone();
                    let state = Arc::clone(&state);
                    async move { Ok::<_, Infallible>(backend_handle(req, mode, state).await) }
                });
                let _ = builder.serve_connection(TokioIo::new(sock), svc).await;
            });
        }
    });
    (local, state, handle)
}

async fn backend_handle(
    req: Request<Incoming>,
    mode: BackendMode,
    state: Arc<BackendState>,
) -> Response<BoxBody> {
    state.hits.fetch_add(1, Ordering::Relaxed);
    if let Some(t) = req
        .headers()
        .get("grpc-timeout")
        .and_then(|v| v.to_str().ok())
    {
        *state.last_grpc_timeout.lock() = Some(t.to_owned());
    }

    if matches!(mode, BackendMode::HttpNotFound) {
        // Deliberate non-200 so the gateway has to translate.
        let body: BoxBody = http_body_util::combinators::BoxBody::new(
            Empty::<Bytes>::new().map_err(|never| match never {}),
        );
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(body)
            .unwrap();
    }

    if let BackendMode::Sleep { delay } = mode {
        tokio::time::sleep(delay).await;
        // After sleeping, respond with a trivial gRPC OK.
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", hyper::header::HeaderValue::from_static("0"));
        return Response::builder()
            .status(StatusCode::OK)
            .header(hyper::header::CONTENT_TYPE, "application/grpc+proto")
            .body(stream_body(vec![], Some(trailers)))
            .unwrap();
    }

    // Read inbound body. It's a stream of gRPC frames.
    let body_bytes = req.into_body().collect().await.unwrap().to_bytes();
    let inbound_frames = decode_all(&body_bytes);

    let response_data: Vec<Bytes> = match mode {
        BackendMode::Echo => inbound_frames
            .into_iter()
            .map(|payload| {
                let f = GrpcFrame {
                    compressed: false,
                    data: payload,
                };
                encode_grpc_frame(&f).unwrap()
            })
            .collect(),
        BackendMode::ServerStream { per_request } => {
            let mut out = Vec::new();
            for (i, payload) in inbound_frames.iter().enumerate() {
                for j in 0..per_request {
                    let tagged = Bytes::from(format!(
                        "{}-{i}-{j}",
                        std::str::from_utf8(payload).unwrap_or("req")
                    ));
                    let f = GrpcFrame {
                        compressed: false,
                        data: tagged,
                    };
                    out.push(encode_grpc_frame(&f).unwrap());
                }
            }
            out
        }
        BackendMode::ClientStream => {
            // Concatenate every inbound payload into one response frame.
            let concatenated: Bytes = inbound_frames
                .iter()
                .flat_map(|b| b.iter().copied())
                .collect::<Vec<u8>>()
                .into();
            let f = GrpcFrame {
                compressed: false,
                data: concatenated,
            };
            vec![encode_grpc_frame(&f).unwrap()]
        }
        BackendMode::Sleep { .. } | BackendMode::HttpNotFound => unreachable!(),
    };

    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", hyper::header::HeaderValue::from_static("0"));
    trailers.insert("grpc-message", hyper::header::HeaderValue::from_static(""));
    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "application/grpc+proto")
        .body(stream_body(response_data, Some(trailers)))
        .unwrap()
}

// ── gateway harness ────────────────────────────────────────────────────

/// Spawn the H2Proxy over plain TCP, with GrpcProxy wired in.
async fn spawn_gateway(
    backend_addr: SocketAddr,
    grpc_cfg: GrpcConfig,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let grpc = GrpcProxy::new(grpc_cfg, pool.clone());
    let h2_proxy =
        Arc::new(H2Proxy::new(pool, picker, None, HttpTimeouts::default(), false).with_grpc(grpc));

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let proxy = Arc::clone(&h2_proxy);
            tokio::spawn(async move {
                let _ = proxy.serve_connection(sock, peer).await;
            });
        }
    });
    (local, handle)
}

// ── client helper ──────────────────────────────────────────────────────

/// Dial the gateway, perform an H2 handshake, and send a single gRPC
/// request, returning the `Response<Incoming>` so the caller can read
/// body + trailers.
async fn send_grpc(
    gateway: SocketAddr,
    path: &str,
    request_body: BoxBody,
    extra_headers: &[(&str, &str)],
) -> Response<Incoming> {
    let stream = tokio::net::TcpStream::connect(gateway).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http2::handshake::<_, _, BoxBody>(
        TokioExecutor::new(),
        TokioIo::new(stream),
    )
    .await
    .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(hyper::header::CONTENT_TYPE, "application/grpc+proto")
        .header("te", "trailers");
    for (k, v) in extra_headers {
        builder = builder.header(*k, *v);
    }
    let req = builder.body(request_body).unwrap();
    sender.send_request(req).await.unwrap()
}

// ── tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn grpc_unary_echo() {
    let (backend, _s, _b) = spawn_grpc_backend(BackendMode::Echo).await;
    let (gw, _g) = spawn_gateway(backend, GrpcConfig::default()).await;

    let payload = Bytes::from_static(b"hello grpc");
    let req_body = stream_body(vec![frame_messages(&[payload.clone()])], None);
    let resp = send_grpc(gw, "/svc/Echo", req_body, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (body, trailers) = collect_grpc_body(resp).await;
    let frames = decode_all(&body);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0], payload);
    assert_eq!(trailers.get("grpc-status").unwrap(), "0");
}

#[tokio::test]
async fn grpc_server_stream() {
    let (backend, _s, _b) = spawn_grpc_backend(BackendMode::ServerStream { per_request: 10 }).await;
    let (gw, _g) = spawn_gateway(backend, GrpcConfig::default()).await;

    let payload = Bytes::from_static(b"req");
    let req_body = stream_body(vec![frame_messages(&[payload])], None);
    let resp = send_grpc(gw, "/svc/ServerStream", req_body, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (body, trailers) = collect_grpc_body(resp).await;
    let frames = decode_all(&body);
    assert_eq!(frames.len(), 10);
    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(&frame[..], format!("req-0-{i}").as_bytes());
    }
    assert_eq!(trailers.get("grpc-status").unwrap(), "0");
}

#[tokio::test]
async fn grpc_client_stream() {
    let (backend, _s, _b) = spawn_grpc_backend(BackendMode::ClientStream).await;
    let (gw, _g) = spawn_gateway(backend, GrpcConfig::default()).await;

    let msgs: Vec<Bytes> = (0..10).map(|i| Bytes::from(format!("m{i}"))).collect();
    let req_body = stream_body(vec![frame_messages(&msgs)], None);
    let resp = send_grpc(gw, "/svc/ClientStream", req_body, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (body, trailers) = collect_grpc_body(resp).await;
    let frames = decode_all(&body);
    assert_eq!(frames.len(), 1);
    let expected: Bytes = msgs
        .iter()
        .flat_map(|b| b.iter().copied())
        .collect::<Vec<u8>>()
        .into();
    assert_eq!(frames[0], expected);
    assert_eq!(trailers.get("grpc-status").unwrap(), "0");
}

#[tokio::test]
async fn grpc_bidi_stream() {
    // Echo backend — interleaved doesn't really show; we drive 5
    // request frames in one shot and assert 5 echoed response frames in
    // the same order. gRPC bidi has the same on-wire shape as our
    // sequential send/collect pipeline from the H2 harness's point of
    // view, which is exactly what the proxy passes through.
    let (backend, _s, _b) = spawn_grpc_backend(BackendMode::Echo).await;
    let (gw, _g) = spawn_gateway(backend, GrpcConfig::default()).await;

    let msgs: Vec<Bytes> = (0..5).map(|i| Bytes::from(format!("ping-{i}"))).collect();
    let req_body = stream_body(vec![frame_messages(&msgs)], None);
    let resp = send_grpc(gw, "/svc/Bidi", req_body, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (body, trailers) = collect_grpc_body(resp).await;
    let frames = decode_all(&body);
    assert_eq!(frames.len(), 5);
    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(&frame[..], format!("ping-{i}").as_bytes());
    }
    assert_eq!(trailers.get("grpc-status").unwrap(), "0");
}

#[tokio::test]
async fn grpc_deadline_clamps_at_300s() {
    // Client says "600S" (600 s); gateway clamps to 300 s. The backend
    // receives the clamped value.
    let (backend, state, _b) = spawn_grpc_backend(BackendMode::Echo).await;
    let (gw, _g) = spawn_gateway(backend, GrpcConfig::default()).await;

    let payload = Bytes::from_static(b"x");
    let req_body = stream_body(vec![frame_messages(&[payload])], None);
    let resp = send_grpc(gw, "/svc/Echo", req_body, &[("grpc-timeout", "600S")]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (_body, _tr) = collect_grpc_body(resp).await;

    let observed = state.last_grpc_timeout.lock().clone().unwrap();
    // Parse the observed timeout and assert it's <= 300 s.
    let parsed_ms = lb_grpc::GrpcDeadline::parse_timeout(&observed).unwrap();
    assert!(
        parsed_ms <= 300_000,
        "backend observed {observed:?} ({parsed_ms} ms); expected <= 300000 ms (clamp)"
    );
}

#[tokio::test]
async fn grpc_deadline_exceeded_from_gateway() {
    // 100 ms client-side deadline; backend sleeps 500 ms. Gateway must
    // emit grpc-status: 4 DEADLINE_EXCEEDED before the backend replies.
    let (backend, _s, _b) = spawn_grpc_backend(BackendMode::Sleep {
        delay: Duration::from_millis(500),
    })
    .await;
    let (gw, _g) = spawn_gateway(backend, GrpcConfig::default()).await;

    let payload = Bytes::from_static(b"ping");
    let req_body = stream_body(vec![frame_messages(&[payload])], None);
    let resp = send_grpc(gw, "/svc/Slow", req_body, &[("grpc-timeout", "100m")]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (_body, trailers) = collect_grpc_body(resp).await;
    assert_eq!(trailers.get("grpc-status").unwrap(), "4");
}

#[tokio::test]
async fn grpc_health_check_synthesized() {
    // No backend running — but the synthesised responder bypasses
    // backend dial entirely when health_synthesized is true.
    // Bind an unused TCP port for the listener; the gateway should
    // never dial it.
    let spare = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let spare_addr = spare.local_addr().unwrap();
    drop(spare);
    let (gw, _g) = spawn_gateway(spare_addr, GrpcConfig::default()).await;

    let req_body = stream_body(vec![frame_messages(&[Bytes::new()])], None);
    let resp = send_grpc(gw, "/grpc.health.v1.Health/Check", req_body, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (body, trailers) = collect_grpc_body(resp).await;
    // Body is a single frame whose payload is `0x08 0x01` — the proto
    // encoding of `HealthCheckResponse { status: SERVING }`.
    let frames = decode_all(&body);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].as_ref(), &[0x08, 0x01]);
    assert_eq!(trailers.get("grpc-status").unwrap(), "0");
}

#[tokio::test]
async fn grpc_status_translation_404() {
    let (backend, _s, _b) = spawn_grpc_backend(BackendMode::HttpNotFound).await;
    let (gw, _g) = spawn_gateway(backend, GrpcConfig::default()).await;

    let payload = Bytes::from_static(b"x");
    let req_body = stream_body(vec![frame_messages(&[payload])], None);
    let resp = send_grpc(gw, "/svc/Missing", req_body, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (_body, trailers) = collect_grpc_body(resp).await;
    // HTTP 404 maps to gRPC Unimplemented (12).
    assert_eq!(trailers.get("grpc-status").unwrap(), "12");
}

// ── GRPC-001: oversize response header rejected by gateway ────────────

/// Backend that responds with an oversize response header value. Used
/// to prove the gateway caps the upstream H2 client at the
/// listener-derived `max_header_list_size`. h2 only enforces the
/// SETTINGS-advertised cap on the *initial* response HEADERS frame
/// (RFC 7540 §6.5.2 + h2 0.4 `is_over_size` check), so the backend
/// stuffs the oversize bytes into a custom response header rather
/// than a trailer.
async fn spawn_oversize_header_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
                let svc = hyper::service::service_fn(move |req: Request<Incoming>| async move {
                    // Drain the request body to keep hyper happy.
                    let _ = req.into_body().collect().await;
                    // 16 KiB header value — well above the
                    // gateway's 1 KiB cap. Stuffed into a custom
                    // header so the on-wire HEADERS frame trips
                    // h2's `is_over_size` check on the upstream
                    // client side.
                    let oversize = "A".repeat(16 * 1024);
                    let mut trailers = HeaderMap::new();
                    trailers.insert("grpc-status", hyper::header::HeaderValue::from_static("0"));
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(hyper::header::CONTENT_TYPE, "application/grpc+proto")
                            .header(
                                "x-oversize",
                                hyper::header::HeaderValue::from_str(&oversize).unwrap(),
                            )
                            .body(stream_body(vec![], Some(trailers)))
                            .unwrap(),
                    )
                });
                let _ = builder.serve_connection(TokioIo::new(sock), svc).await;
            });
        }
    });
    (local, handle)
}

/// Variant of [`spawn_gateway`] that lets the test fix the listener's
/// H2 `max_header_list_size`. The default ctor uses 64 KiB, which would
/// not block a 16 KiB trailer; we want a tight 1 KiB cap so the GRPC-001
/// path is exercised.
async fn spawn_gateway_with_h2_security(
    backend_addr: SocketAddr,
    grpc_cfg: GrpcConfig,
    max_header_list_size: u32,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    use lb_l7::h2_security::H2SecurityThresholds;
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let grpc = GrpcProxy::new(grpc_cfg, pool.clone());
    let security = H2SecurityThresholds {
        max_header_list_size,
        ..H2SecurityThresholds::default()
    };
    let h2_proxy = Arc::new(
        H2Proxy::with_security(pool, picker, None, HttpTimeouts::default(), false, security)
            .with_grpc(grpc),
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let proxy = Arc::clone(&h2_proxy);
            tokio::spawn(async move {
                let _ = proxy.serve_connection(sock, peer).await;
            });
        }
    });
    (local, handle)
}

#[tokio::test]
async fn grpc_upstream_oversize_trailer_rejected_by_gateway() {
    // Backend responds with a 16 KiB response header value.
    // Listener's max_header_list_size = 1 KiB. The hyper upstream H2
    // client therefore refuses the response header block (h2
    // `is_over_size` → REFUSED_STREAM / 502-class error); the gateway
    // synthesises a gateway-origin gRPC error trailer. The acceptance
    // criterion is "client does NOT see a successful gRPC OK" — we
    // accept any gateway-origin code (Internal=13 / Unavailable=14)
    // since hyper's error shape varies, but `grpc-status: 0` must
    // never appear with the malicious trailer intact.
    let (backend, _b) = spawn_oversize_header_backend().await;
    let (gw, _g) = spawn_gateway_with_h2_security(backend, GrpcConfig::default(), 1024).await;

    let payload = Bytes::from_static(b"x");
    let req_body = stream_body(vec![frame_messages(&[payload])], None);
    let resp = send_grpc(gw, "/svc/Echo", req_body, &[]).await;
    // Gateway translates upstream errors into 200 OK + gRPC trailers.
    assert_eq!(resp.status(), StatusCode::OK);
    let (_body, trailers) = collect_grpc_body(resp).await;
    let status = trailers
        .get("grpc-status")
        .map(|v| v.to_str().unwrap_or(""));
    // Must be a gateway-origin error, not a transparent OK forward.
    assert_ne!(
        status,
        Some("0"),
        "oversize upstream response header must NOT yield grpc-status: 0"
    );
    assert!(
        matches!(status, Some("13" | "14")),
        "expected gateway-origin grpc-status (Internal=13 / Unavailable=14), got {status:?}"
    );
}

// ── GRPC-002: malformed grpc-timeout → INVALID_ARGUMENT ────────────────

#[tokio::test]
async fn grpc_malformed_timeout_returns_invalid_argument() {
    // Backend bound but the gateway must NOT dial it: malformed
    // grpc-timeout has to short-circuit at the gateway with
    // grpc-status: 3 INVALID_ARGUMENT.
    let (backend, state, _b) = spawn_grpc_backend(BackendMode::Echo).await;
    let (gw, _g) = spawn_gateway(backend, GrpcConfig::default()).await;

    let payload = Bytes::from_static(b"x");
    let req_body = stream_body(vec![frame_messages(&[payload])], None);
    let resp = send_grpc(gw, "/svc/Echo", req_body, &[("grpc-timeout", "foo")]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (_body, trailers) = collect_grpc_body(resp).await;
    assert_eq!(
        trailers.get("grpc-status").unwrap(),
        "3",
        "expected grpc-status: 3 INVALID_ARGUMENT for malformed grpc-timeout"
    );
    let msg = trailers
        .get("grpc-message")
        .map(|v| v.to_str().unwrap_or(""))
        .unwrap_or("");
    assert!(
        msg.to_ascii_lowercase().contains("malformed"),
        "expected grpc-message to mention 'malformed', got {msg:?}"
    );
    // Backend must NOT have observed the request — the gateway short-
    // circuits before dialing.
    assert_eq!(
        state.hits.load(Ordering::Relaxed),
        0,
        "backend should not have been dialed on malformed timeout"
    );
}

// ── GRPC-003: synthesized Health/Check honors `service` field ──────────

#[tokio::test]
async fn grpc_health_check_overall_serving() {
    // Empty service field (the spec's "overall server health" probe)
    // → SERVING. Use a spare port that's never accepted to prove the
    // synth path bypasses the backend.
    let spare = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let spare_addr = spare.local_addr().unwrap();
    drop(spare);
    let (gw, _g) = spawn_gateway(spare_addr, GrpcConfig::default()).await;

    // HealthCheckRequest with no service field == empty payload.
    let req_body = stream_body(vec![frame_messages(&[Bytes::new()])], None);
    let resp = send_grpc(gw, "/grpc.health.v1.Health/Check", req_body, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (body, trailers) = collect_grpc_body(resp).await;
    let frames = decode_all(&body);
    assert_eq!(frames.len(), 1);
    // Body payload is `0x08 0x01` — `HealthCheckResponse { SERVING }`.
    assert_eq!(frames[0].as_ref(), &[0x08, 0x01]);
    assert_eq!(trailers.get("grpc-status").unwrap(), "0");
}

#[tokio::test]
async fn grpc_health_check_unknown_service_not_found() {
    // Non-empty service field → no per-service registry exists in v1
    // → respond NOT_FOUND (5). Still bypasses the backend, so use a
    // spare port that's never accepted.
    let spare = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let spare_addr = spare.local_addr().unwrap();
    drop(spare);
    let (gw, _g) = spawn_gateway(spare_addr, GrpcConfig::default()).await;

    // HealthCheckRequest { service = "foo.Bar" }.
    // protobuf encoding: tag 1 wire 2 → 0x0A, length 7, "foo.Bar".
    let mut pb = Vec::new();
    pb.push(0x0A);
    pb.push(0x07);
    pb.extend_from_slice(b"foo.Bar");
    let req_body = stream_body(vec![frame_messages(&[Bytes::from(pb)])], None);
    let resp = send_grpc(gw, "/grpc.health.v1.Health/Check", req_body, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (_body, trailers) = collect_grpc_body(resp).await;
    assert_eq!(
        trailers.get("grpc-status").unwrap(),
        "5",
        "expected grpc-status: 5 NOT_FOUND for unregistered service"
    );
    let msg = trailers
        .get("grpc-message")
        .map(|v| v.to_str().unwrap_or(""))
        .unwrap_or("");
    assert!(
        msg.contains("foo.Bar") || msg.contains("not registered"),
        "expected grpc-message to mention service name or registration, got {msg:?}"
    );
}
