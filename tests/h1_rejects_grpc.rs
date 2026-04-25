//! D3-2 + D4-1 + D4-2 — H1 listener must reject `application/grpc` with 415.
//!
//! gRPC requires HTTP/2 for framing (HEADERS continuation, trailers,
//! multiplexed streams). An H1 listener cannot serve gRPC, but a
//! misconfigured client will happily POST `application/grpc` over an H1
//! socket. Without an explicit reject the request would either time
//! out, get framed against an unrelated H1 backend, or surface a
//! confusing 502 — none of which actionably tells the operator "this
//! listener is the wrong protocol for gRPC."
//!
//! `H1Proxy::handle` short-circuits any inbound request whose
//! `content-type` is exactly `application/grpc` or starts with
//! `application/grpc+<sub>` (case-insensitive, ignoring `;`-prefixed
//! parameters) and returns 415 with a clear message. The H2 listener's
//! gRPC detection (`is_grpc_request` → `GrpcProxy`) is unaffected.
//!
//! Variants covered:
//! * D3-2 baseline: `application/grpc+proto` → 415.
//! * D4-1: `APPLICATION/GRPC` (uppercase) → 415 — RFC 7231 §3.1.1.1
//!   media-type tokens are case-insensitive.
//! * D4-1 cont.: `application/grpc; charset=utf-8` → 415 — `;`-prefixed
//!   parameters do not bypass the reject.
//! * D4-2: `application/grpc-web` (plain HTTP, hyphen) → NOT 415; the
//!   request is forwarded transparently to the witness backend.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1 as srv_h1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts};
use lb_l7::upstream::{RoundRobinUpstreams, UpstreamBackend};
use tokio::net::TcpListener;

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

/// Spawn a backend that records whether it ever saw an inbound request.
/// On a 415-from-gateway path it must NOT be contacted — proving the
/// reject is short-circuit-y and not a downstream pass-through.
async fn spawn_witness_backend(
    contacted: Arc<AtomicBool>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let c = Arc::clone(&contacted);
            tokio::spawn(async move {
                let svc = service_fn(move |_req: Request<Incoming>| {
                    let c = Arc::clone(&c);
                    async move {
                        c.store(true, Ordering::SeqCst);
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .body(Full::new(Bytes::from_static(b"backend-saw-it")))
                                .unwrap(),
                        )
                    }
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, handle)
}

#[tokio::test]
async fn h1_listener_rejects_grpc_with_415() {
    let contacted = Arc::new(AtomicBool::new(false));
    let (backend_addr, _bh) = spawn_witness_backend(Arc::clone(&contacted)).await;

    // Pure H1 backend — gateway is configured single-protocol, but the
    // gRPC reject must happen BEFORE picker dispatch.
    let picker =
        Arc::new(RoundRobinUpstreams::new(vec![UpstreamBackend::h1(backend_addr)]).unwrap());
    let proxy = Arc::new(H1Proxy::with_multi_proto(
        build_pool(),
        picker,
        None,
        HttpTimeouts::default(),
        /* is_https = */ false,
    ));

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let gw_addr = listener.local_addr().unwrap();
    let p = Arc::clone(&proxy);
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let p = Arc::clone(&p);
            tokio::spawn(async move {
                let _ = p.serve_connection(sock, peer).await;
            });
        }
    });

    // Send a real H1 POST with application/grpc.
    let stream = tokio::net::TcpStream::connect(gw_addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("POST")
        .uri("/grpc.svc/Method")
        .header("host", "example.test")
        .header("content-type", "application/grpc+proto")
        .body(Full::new(Bytes::from_static(b"\0\0\0\0\0")))
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "H1 listener must reject gRPC with 415"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body_str = std::str::from_utf8(&body).unwrap_or("");
    assert!(
        body_str.contains("gRPC requires HTTP/2"),
        "expected the 415 body to name HTTP/2 as the requirement; got: {body_str:?}"
    );
    // Give the spawned listener loop a moment to settle.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !contacted.load(Ordering::SeqCst),
        "backend must NOT have been contacted on the gRPC reject path"
    );
}

/// Spin up an H1 gateway in front of a single witness backend and
/// return the gateway's listener address plus the contacted-flag so
/// callers can drive a single request through and assert behaviour.
async fn spawn_gateway_with_witness() -> (SocketAddr, Arc<AtomicBool>) {
    let contacted = Arc::new(AtomicBool::new(false));
    let (backend_addr, _bh) = spawn_witness_backend(Arc::clone(&contacted)).await;

    let picker =
        Arc::new(RoundRobinUpstreams::new(vec![UpstreamBackend::h1(backend_addr)]).unwrap());
    let proxy = Arc::new(H1Proxy::with_multi_proto(
        build_pool(),
        picker,
        None,
        HttpTimeouts::default(),
        /* is_https = */ false,
    ));

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let gw_addr = listener.local_addr().unwrap();
    let p = Arc::clone(&proxy);
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let p = Arc::clone(&p);
            tokio::spawn(async move {
                let _ = p.serve_connection(sock, peer).await;
            });
        }
    });
    (gw_addr, contacted)
}

/// Issue a single H1 POST to `gw_addr` carrying the supplied
/// `content-type` and an empty gRPC-shaped body, returning the response
/// status. Body bytes are intentionally a 5-byte zero gRPC frame
/// (length-prefix + zero-length payload) so any backend that DID forward
/// would see a syntactically plausible request.
async fn h1_post_content_type(gw_addr: SocketAddr, content_type: &str) -> StatusCode {
    let stream = tokio::net::TcpStream::connect(gw_addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("POST")
        .uri("/svc/Method")
        .header("host", "example.test")
        .header("content-type", content_type)
        .body(Full::new(Bytes::from_static(b"\0\0\0\0\0")))
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    resp.status()
}

/// D4-1 — uppercase `APPLICATION/GRPC` must still produce 415. RFC 7231
/// §3.1.1.1 makes media-type tokens case-insensitive; pre-fix code used
/// case-sensitive `starts_with` so a malicious or non-conforming client
/// sending uppercase bypassed the reject and reached the H1 backend.
#[tokio::test]
async fn h1_rejects_grpc_uppercase_with_415() {
    let (gw_addr, contacted) = spawn_gateway_with_witness().await;
    let status = h1_post_content_type(gw_addr, "APPLICATION/GRPC").await;
    assert_eq!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "uppercase `APPLICATION/GRPC` must reject with 415"
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !contacted.load(Ordering::SeqCst),
        "backend must NOT see the uppercase-grpc request on the reject path"
    );
}

/// D4-1 cont. — `application/grpc; charset=utf-8` must reject. The
/// `;`-prefixed parameter section is not part of the media-type token
/// per RFC 7231 §3.1.1.1; stripping at the first `;` is required.
#[tokio::test]
async fn h1_rejects_grpc_with_charset_param_with_415() {
    let (gw_addr, contacted) = spawn_gateway_with_witness().await;
    let status = h1_post_content_type(gw_addr, "application/grpc; charset=utf-8").await;
    assert_eq!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "`application/grpc; charset=utf-8` must reject with 415"
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !contacted.load(Ordering::SeqCst),
        "backend must NOT see the parameterised-grpc request on the reject path"
    );
}

/// D4-2 — `application/grpc-web` is plain HTTP and MUST forward
/// transparently. Pre-fix code used `starts_with("application/grpc")`
/// which greedily caught the `grpc-` prefix of `grpc-web` and emitted
/// a spurious 415. The fix uses `application/grpc+` (with `+`) so the
/// hyphen variant flows through unchanged. The witness backend
/// returns 200 on contact; this test asserts the gateway's response
/// is NOT 415 — meaning the reject did not fire.
#[tokio::test]
async fn h1_passes_grpc_web_through() {
    let (gw_addr, contacted) = spawn_gateway_with_witness().await;
    let status = h1_post_content_type(gw_addr, "application/grpc-web").await;
    assert_ne!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "`application/grpc-web` must NOT be rejected with 415; it is plain HTTP"
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        contacted.load(Ordering::SeqCst),
        "grpc-web must reach the backend (transparent H1 forwarding)"
    );
}

/// D4-2 cont. — `application/grpc-web+proto` is also plain HTTP and
/// MUST forward transparently. Same rationale as the bare-grpc-web
/// case.
#[tokio::test]
async fn h1_passes_grpc_web_proto_through() {
    let (gw_addr, contacted) = spawn_gateway_with_witness().await;
    let status = h1_post_content_type(gw_addr, "application/grpc-web+proto").await;
    assert_ne!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "`application/grpc-web+proto` must NOT be rejected with 415"
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        contacted.load(Ordering::SeqCst),
        "grpc-web+proto must reach the backend (transparent H1 forwarding)"
    );
}
