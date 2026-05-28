//! ROUND8-L7-06 proof — per-keep-alive-connection request cap (H1).
//!
//! Reference: nginx `keepalive_requests 100` default + Pingora 0.8.0
//! `keepalive_requests` cap (Cloudflare added it after hitting
//! per-connection accounting growth / TLS-session-age / FD-pinning
//! pain at the edge). `ref-l7` handoff Top-10 #1.
//!
//! Invariants:
//!   * `h1_nth_response_carries_connection_close` — with cap = N, the
//!     Nth response on one keep-alive connection carries
//!     `Connection: close` and the server closes the socket after it.
//!   * `h1_cap_zero_disables` — with cap = 0, many sequential requests
//!     on one connection all succeed and the cap never injects
//!     `Connection: close`.
//!   * `counter_increments_on_cap_close` — the cap-termination atomic
//!     advances exactly once per cap-triggered close.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Trivial HTTP/1.1 backend that answers every request with a fixed
/// 200 and stays keep-alive (so the *gateway* is the one that decides
/// to close, per the cap).
async fn spawn_echo_backend() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                loop {
                    // Read one request (tests send small, unpipelined
                    // requests so a single read covers the head).
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    if s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi")
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    addr
}

async fn spawn_proxy(backend: SocketAddr, cap: u32) -> (SocketAddr, Arc<H1Proxy>) {
    let pool = TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts::default(),
        lb_io::Runtime::new(),
    );
    let picker = RoundRobinAddrs::new(vec![backend]).unwrap();
    let proxy = Arc::new(
        H1Proxy::new(
            pool,
            Arc::new(picker),
            None,
            HttpTimeouts {
                header: Duration::from_secs(3),
                body: Duration::from_secs(3),
                total: Duration::from_secs(10),
                head: Duration::from_secs(10),
            },
            false,
        )
        .with_max_keepalive_requests(cap),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let p = Arc::clone(&proxy);
    tokio::spawn(async move {
        if let Ok((sock, peer)) = listener.accept().await {
            let _ = p.serve_connection(sock, peer).await;
        }
    });
    (addr, proxy)
}

/// Send one request on `client` and return the full response head.
async fn one_request(client: &mut TcpStream, path: &str) -> String {
    let req = format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n");
    client.write_all(req.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    let mut tmp = [0u8; 1024];
    // Read until we have headers + the 2-byte body.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        match tokio::time::timeout(Duration::from_millis(800), client.read(&mut tmp)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(n)) => {
                resp.extend_from_slice(&tmp[..n]);
                if resp.windows(4).any(|w| w == b"\r\n\r\n") && resp.ends_with(b"hi") {
                    break;
                }
            }
        }
    }
    String::from_utf8_lossy(&resp).into_owned()
}

#[tokio::test]
async fn h1_nth_response_carries_connection_close() {
    const CAP: u32 = 3;
    let backend = spawn_echo_backend().await;
    let (proxy_addr, _proxy) = spawn_proxy(backend, CAP).await;
    let mut client = TcpStream::connect(proxy_addr).await.unwrap();

    // Requests 1..CAP-1: served, no cap-driven close.
    for i in 1..CAP {
        let head = one_request(&mut client, &format!("/r{i}")).await;
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "request {i} should be 200, got: {head:?}"
        );
        assert!(
            !head.to_ascii_lowercase().contains("connection: close"),
            "request {i} (< cap) must NOT carry Connection: close: {head:?}"
        );
    }
    // The CAP-th request: 200 + Connection: close, then the server
    // closes the socket.
    let head = one_request(&mut client, "/rcap").await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "cap-th request should still be served 200: {head:?}"
    );
    assert!(
        head.to_ascii_lowercase().contains("connection: close"),
        "the cap-th (#{CAP}) response MUST carry Connection: close (nginx keepalive_requests parity): {head:?}"
    );
    // Server closes after the cap-th response: a follow-up read
    // returns 0 (EOF) within the budget.
    let mut tmp = [0u8; 64];
    let eof = tokio::time::timeout(Duration::from_secs(3), client.read(&mut tmp)).await;
    assert!(
        matches!(eof, Ok(Ok(0)) | Ok(Err(_)) | Err(_)),
        "server must close the keep-alive socket after the cap-th response"
    );
}

#[tokio::test]
async fn h1_cap_zero_disables() {
    let backend = spawn_echo_backend().await;
    let (proxy_addr, _proxy) = spawn_proxy(backend, 0).await;
    let mut client = TcpStream::connect(proxy_addr).await.unwrap();

    for i in 0..25 {
        let head = one_request(&mut client, &format!("/n{i}")).await;
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "request {i} with cap=0 should succeed: {head:?}"
        );
        assert!(
            !head.to_ascii_lowercase().contains("connection: close"),
            "cap=0 must never inject Connection: close (req {i}): {head:?}"
        );
    }
}

#[tokio::test]
async fn counter_increments_on_cap_close() {
    const CAP: u32 = 2;
    let backend = spawn_echo_backend().await;
    let (proxy_addr, proxy) = spawn_proxy(backend, CAP).await;
    let counter = proxy.keepalive_cap_termination_counter();
    assert_eq!(counter.load(Ordering::Relaxed), 0);

    let mut client = TcpStream::connect(proxy_addr).await.unwrap();
    let _ = one_request(&mut client, "/a").await;
    let _ = one_request(&mut client, "/b").await; // cap-th
    // Give the service future a beat to bump the counter.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        counter.load(Ordering::Relaxed),
        1,
        "cap-termination counter must advance exactly once per cap-triggered close"
    );
}
