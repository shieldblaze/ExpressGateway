//! ROUND8-L7-09 proof — the protocol-neutral authority validator is
//! ON THE REQUEST PATH for both H1 and H2.
//!
//! Reference: HAProxy `BUG/MAJOR: http: forbid comma character in
//! authority value` + `BUG/MEDIUM: h1: Enforce the authority
//! validation during H1 request parsing` (the H1 parser was missing
//! the check that the H2/H3 path had — the lesson is that the
//! validation must actually RUN on EVERY parser, not merely exist as
//! a library function).
//!
//! The verifier push-back (`audit/round-8/verify/l7.md`) rejected the
//! prior commit because `crate::authority::validate` had ZERO
//! callsites — a `pub fn` that never runs does not close a
//! "validation must run on every parser" finding. These tests drive a
//! REAL H1 connection and a REAL H2 connection through
//! `serve_connection` and assert:
//!   * comma-in-authority  → 400 BEFORE upstream selection (backend is
//!     a closed port; a 400 — not a 502 — proves the validator tripped
//!     ahead of the picker), for both H1 and H2;
//!   * a well-formed authority is NOT rejected by the validator (it
//!     proceeds to the upstream, yielding a 502 from the closed port —
//!     i.e. it got past authority validation).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
use lb_l7::h2_proxy::H2Proxy;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// Backend deliberately points at a closed port: if the validator does
// NOT trip, the request reaches the picker and the closed-port dial
// yields 502. A 400 therefore proves the authority validator ran
// BEFORE upstream selection (the exact HAProxy lesson).
const CLOSED_BACKEND: &str = "127.0.0.1:1";

fn timeouts() -> HttpTimeouts {
    HttpTimeouts {
        header: Duration::from_secs(2),
        body: Duration::from_secs(2),
        total: Duration::from_secs(5),
    }
}

fn pool() -> TcpPool {
    TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts::default(),
        lb_io::Runtime::new(),
    )
}

async fn spawn_h1_proxy() -> SocketAddr {
    let backend: SocketAddr = CLOSED_BACKEND.parse().unwrap();
    let picker = RoundRobinAddrs::new(vec![backend]).unwrap();
    let proxy = Arc::new(H1Proxy::new(
        pool(),
        Arc::new(picker),
        None,
        timeouts(),
        false,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        // Serve a few connections so each test request gets its own.
        for _ in 0..8 {
            if let Ok((sock, peer)) = listener.accept().await {
                let p = Arc::clone(&proxy);
                tokio::spawn(async move {
                    let _ = p.serve_connection(sock, peer).await;
                });
            }
        }
    });
    addr
}

/// Send one raw H1 request and return the status line.
async fn h1_status(proxy: SocketAddr, host: &str) -> String {
    let mut client = TcpStream::connect(proxy).await.unwrap();
    let req = format!("GET /p HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    client.write_all(req.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    let mut tmp = [0u8; 1024];
    let _ = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            match client.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    resp.extend_from_slice(&tmp[..n]);
                    if resp.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
    })
    .await;
    String::from_utf8_lossy(&resp)
        .lines()
        .next()
        .unwrap_or("")
        .to_owned()
}

#[tokio::test]
async fn h1_comma_in_authority_rejected_before_upstream() {
    let proxy = spawn_h1_proxy().await;

    // Comma in authority — HAProxy BUG/MAJOR class. Must be 400, and
    // crucially NOT 502: a 502 would mean we reached the closed
    // backend (validator skipped).
    let status = h1_status(proxy, "example.test,attacker.example").await;
    assert!(
        status.contains(" 400"),
        "H1: comma-in-Host MUST be rejected 400 by the authority \
         validator BEFORE upstream selection; got status line: {status:?}"
    );

    // Whitespace in authority — also a desync primitive.
    let status = h1_status(proxy, "example.test attacker").await;
    assert!(
        status.contains(" 400"),
        "H1: whitespace-in-Host MUST be 400; got: {status:?}"
    );
}

#[tokio::test]
async fn h1_valid_authority_passes_validator() {
    let proxy = spawn_h1_proxy().await;
    // Well-formed authority: the validator must let it through. The
    // closed backend then yields 502 — proving the request got PAST
    // authority validation rather than being short-circuited.
    let status = h1_status(proxy, "example.test:8080").await;
    assert!(
        status.contains(" 502"),
        "H1: a valid authority must pass the validator and reach the \
         (closed) upstream → 502; got: {status:?}"
    );
}

async fn spawn_h2_proxy() -> SocketAddr {
    let backend: SocketAddr = CLOSED_BACKEND.parse().unwrap();
    let picker = RoundRobinAddrs::new(vec![backend]).unwrap();
    let proxy = Arc::new(H2Proxy::new(
        pool(),
        Arc::new(picker),
        None,
        timeouts(),
        false,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..8 {
            if let Ok((sock, peer)) = listener.accept().await {
                let p = Arc::clone(&proxy);
                tokio::spawn(async move {
                    let _ = p.serve_connection(sock, peer).await;
                });
            }
        }
    });
    addr
}

/// Drive a single h2 request with an explicit `:authority` and return
/// the response status code.
async fn h2_status(proxy: SocketAddr, authority: &str) -> u16 {
    let tcp = TcpStream::connect(proxy).await.unwrap();
    let io = hyper_util::rt::TokioIo::new(tcp);
    let (mut send, conn) =
        hyper::client::conn::http2::handshake(hyper_util::rt::TokioExecutor::new(), io)
            .await
            .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = hyper::Request::builder()
        .method("GET")
        .uri(format!("http://{authority}/p"))
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(4), send.send_request(req))
        .await
        .expect("h2 send_request timed out")
        .expect("h2 send_request failed");
    let status = resp.status().as_u16();
    let _ = resp.into_body().collect().await;
    status
}

#[tokio::test]
async fn h2_comma_in_authority_rejected_before_upstream() {
    let proxy = spawn_h2_proxy().await;
    let status = h2_status(proxy, "example.test,attacker.example").await;
    assert_eq!(
        status, 400,
        "H2: comma-in-:authority MUST be rejected 400 by the SAME \
         authority validator the H1 path uses, BEFORE upstream \
         selection (not 502); got {status}"
    );
}

#[tokio::test]
async fn h2_valid_authority_passes_validator() {
    let proxy = spawn_h2_proxy().await;
    let status = h2_status(proxy, "example.test:8080").await;
    assert_eq!(
        status, 502,
        "H2: a valid :authority must pass the validator and reach the \
         (closed) upstream → 502; got {status}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// ROUND8-L7-09 RE-VERIFY BYPASS CLOSURE (verify task#74 push-back).
//
// The first fix wired `crate::authority::validate` onto the *plain*
// H1/H2 request path only. Three forks reached upstream selection
// BEFORE that validator:
//   1. H1 WebSocket upgrade  (handle_ws_upgrade)
//   2. H2 extended-CONNECT   (handle_ws_extended_connect)
//   3. H2 gRPC               (GrpcProxy::handle)
// The fix hoists the validator to a single choke point at the top of
// each `handle_inner`, ABOVE the fork. These three tests drive a real
// connection down each previously-bypassing fork with a comma in the
// authority and assert (a) the response is 400 and (b) a REAL probe
// backend listener received ZERO connections — proving the request
// never reached upstream selection on any fork.
// ─────────────────────────────────────────────────────────────────────

use std::sync::atomic::{AtomicU32, Ordering};

/// A real listening backend that counts inbound TCP connections. If
/// any fork bypasses the choke point and reaches upstream selection,
/// the picker dials this address and `count` goes non-zero — the
/// "backend was never reached" assertion then fails loudly (stronger
/// than the closed-port status-code inference the older cases use).
async fn spawn_probe_backend() -> (SocketAddr, Arc<AtomicU32>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicU32::new(0));
    let c = Arc::clone(&count);
    tokio::spawn(async move {
        loop {
            if let Ok((sock, _)) = listener.accept().await {
                c.fetch_add(1, Ordering::SeqCst);
                drop(sock);
            }
        }
    });
    (addr, count)
}

async fn spawn_h1_ws_proxy(backend: SocketAddr) -> SocketAddr {
    use lb_l7::ws_proxy::{WsConfig, WsProxy};
    let picker = RoundRobinAddrs::new(vec![backend]).unwrap();
    let proxy = Arc::new(
        H1Proxy::new(pool(), Arc::new(picker), None, timeouts(), false)
            .with_websocket(Arc::new(WsProxy::new(WsConfig::default()))),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..8 {
            if let Ok((sock, peer)) = listener.accept().await {
                let p = Arc::clone(&proxy);
                tokio::spawn(async move {
                    let _ = p.serve_connection(sock, peer).await;
                });
            }
        }
    });
    addr
}

/// H1 WebSocket upgrade with a comma in `Host` must be rejected 400 at
/// the choke point — the previously-bypassing `handle_ws_upgrade`
/// fork. The probe backend must record ZERO connections.
#[tokio::test]
async fn test_ws_upgrade_comma_authority_rejected() {
    let (backend, hits) = spawn_probe_backend().await;
    let proxy = spawn_h1_ws_proxy(backend).await;

    let mut client = TcpStream::connect(proxy).await.unwrap();
    // Structurally valid RFC 6455 handshake (so `is_h1_upgrade_request`
    // returns true and the WS-upgrade fork is taken) but with a comma
    // in the authority — HAProxy BUG/MAJOR class.
    let req = "GET /chat HTTP/1.1\r\n\
               Host: example.test,attacker.example\r\n\
               Upgrade: websocket\r\n\
               Connection: upgrade\r\n\
               Sec-WebSocket-Version: 13\r\n\
               Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
               \r\n";
    client.write_all(req.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    let mut tmp = [0u8; 1024];
    let _ = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            match client.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    resp.extend_from_slice(&tmp[..n]);
                    if resp.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
    })
    .await;
    let status_line = String::from_utf8_lossy(&resp)
        .lines()
        .next()
        .unwrap_or("")
        .to_owned();
    assert!(
        status_line.contains(" 400"),
        "H1 WS-upgrade with comma-in-Host MUST be 400 at the choke \
         point (NOT 101/502 — the WS fork must not bypass the \
         validator); got: {status_line:?}"
    );
    // Give any (erroneously) dispatched upstream dial a beat to land.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "H1 WS-upgrade: backend was reached — the WS fork bypassed \
         the authority choke point"
    );
}

async fn spawn_h2_ws_proxy(backend: SocketAddr) -> SocketAddr {
    use lb_l7::ws_proxy::{WsConfig, WsProxy};
    let picker = RoundRobinAddrs::new(vec![backend]).unwrap();
    let proxy = Arc::new(
        H2Proxy::new(pool(), Arc::new(picker), None, timeouts(), false)
            .with_websocket(Arc::new(WsProxy::new(WsConfig::default()))),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..8 {
            if let Ok((sock, peer)) = listener.accept().await {
                let p = Arc::clone(&proxy);
                tokio::spawn(async move {
                    let _ = p.serve_connection(sock, peer).await;
                });
            }
        }
    });
    addr
}

/// H2 RFC 8441 extended CONNECT (`:method CONNECT` + `:protocol
/// websocket`) with a bad `:authority` must be 400 at the choke point
/// — the previously-bypassing `handle_ws_extended_connect` fork.
#[tokio::test]
async fn test_h2_ext_connect_comma_authority_rejected() {
    let (backend, hits) = spawn_probe_backend().await;
    let proxy = spawn_h2_ws_proxy(backend).await;

    let tcp = TcpStream::connect(proxy).await.unwrap();
    let io = hyper_util::rt::TokioIo::new(tcp);
    let (mut send, conn) =
        hyper::client::conn::http2::handshake(hyper_util::rt::TokioExecutor::new(), io)
            .await
            .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    // CONNECT + `:protocol websocket` extension → routed through
    // `handle_ws_extended_connect`. Comma in `:authority`.
    let mut req = hyper::Request::builder()
        .method("CONNECT")
        .uri("https://example.test,attacker.example/chat")
        .body(Empty::<Bytes>::new())
        .unwrap();
    req.extensions_mut()
        .insert(hyper::ext::Protocol::from_static("websocket"));
    let result = tokio::time::timeout(Duration::from_secs(4), send.send_request(req)).await;
    let status = match result {
        Ok(Ok(resp)) => {
            let s = resp.status().as_u16();
            let _ = resp.into_body().collect().await;
            s
        }
        other => panic!("h2 ext-CONNECT send_request unexpected: {other:?}"),
    };
    assert_eq!(
        status, 400,
        "H2 extended-CONNECT with comma-in-:authority MUST be 400 at \
         the choke point (the ext-CONNECT fork must not bypass the \
         validator); got {status}"
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "H2 ext-CONNECT: backend was reached — the ext-CONNECT fork \
         bypassed the authority choke point"
    );
}

async fn spawn_h2_grpc_proxy(backend: SocketAddr) -> SocketAddr {
    use lb_l7::grpc_proxy::{GrpcConfig, GrpcProxy};
    let picker = RoundRobinAddrs::new(vec![backend]).unwrap();
    let proxy = Arc::new(
        H2Proxy::new(pool(), Arc::new(picker), None, timeouts(), false)
            .with_grpc(GrpcProxy::new(GrpcConfig::default(), pool())),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..8 {
            if let Ok((sock, peer)) = listener.accept().await {
                let p = Arc::clone(&proxy);
                tokio::spawn(async move {
                    let _ = p.serve_connection(sock, peer).await;
                });
            }
        }
    });
    addr
}

/// H2 gRPC request (`content-type: application/grpc`) with a bad
/// `:authority` must be 400 at the choke point — the previously-
/// bypassing `GrpcProxy::handle` fork.
#[tokio::test]
async fn test_h2_grpc_comma_authority_rejected() {
    let (backend, hits) = spawn_probe_backend().await;
    let proxy = spawn_h2_grpc_proxy(backend).await;

    let tcp = TcpStream::connect(proxy).await.unwrap();
    let io = hyper_util::rt::TokioIo::new(tcp);
    let (mut send, conn) =
        hyper::client::conn::http2::handshake(hyper_util::rt::TokioExecutor::new(), io)
            .await
            .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = hyper::Request::builder()
        .method("POST")
        .uri("http://example.test,attacker.example/helloworld.Greeter/SayHello")
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(4), send.send_request(req)).await;
    let status = match result {
        Ok(Ok(resp)) => {
            let s = resp.status().as_u16();
            let _ = resp.into_body().collect().await;
            s
        }
        other => panic!("h2 gRPC send_request unexpected: {other:?}"),
    };
    assert_eq!(
        status, 400,
        "H2 gRPC with comma-in-:authority MUST be 400 at the choke \
         point (the gRPC fork must not bypass the validator); got \
         {status}"
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "H2 gRPC: backend was reached — the gRPC fork bypassed the \
         authority choke point"
    );
}
