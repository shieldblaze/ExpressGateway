//! D3-2 — H1 listener must reject `application/grpc` with 415.
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
//! `content-type` starts with `application/grpc` and returns 415 with a
//! clear message. The H2 listener's gRPC detection (`is_grpc_request`
//! → `GrpcProxy`) is unaffected.

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
