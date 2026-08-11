//! F-RES-1 — `header_read_timeout` is INERT unless a `Timer` is wired.
//!
//! NEGATIVE CONTROL: a SMALL header timeout (1 s) with a LARGE total (10 s) and
//! a partial head. FAILS pre-fix (held until `total`), passes post-fix.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn spawn_ok_backend() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let mut acc = Vec::new();
                loop {
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    acc.extend_from_slice(&buf[..n]);
                    if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi")
                    .await;
            });
        }
    });
    addr
}

async fn spawn_proxy(backend_addr: SocketAddr, header_timeout: Duration) -> SocketAddr {
    let pool = TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts::default(),
        lb_io::Runtime::new(),
    );
    let picker = RoundRobinAddrs::new(vec![backend_addr]).unwrap();
    let proxy = Arc::new(H1Proxy::new(
        pool,
        Arc::new(picker),
        None,
        HttpTimeouts {
            header: header_timeout,
            body: Duration::from_secs(5),
            total: Duration::from_secs(10),
            head: Duration::from_secs(10),
        },
        /* is_https */ false,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                break;
            };
            let proxy = Arc::clone(&proxy);
            tokio::spawn(async move {
                let _ = proxy.serve_connection(sock, peer).await;
            });
        }
    });
    addr
}

/// NEGATIVE CONTROL: closed at ~1 s, NOT held until the 10 s `total`.
#[tokio::test]
async fn h1_partial_head_closed_at_header_timeout_not_total() {
    let backend = spawn_ok_backend().await;
    let header_timeout = Duration::from_secs(1);
    let proxy = spawn_proxy(backend, header_timeout).await;

    let mut client = TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n")
        .await
        .unwrap();
    client.flush().await.unwrap();

    let start = Instant::now();
    let mut buf = [0u8; 256];
    let closed = tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            match client.read(&mut buf).await {
                Ok(0) => break true,  // server closed (EOF)
                Ok(_) => {}           // maybe a 408 then close
                Err(_) => break true, // reset == closed
            }
        }
    })
    .await;
    let elapsed = start.elapsed();

    assert!(
        closed.is_ok(),
        "connection was NOT closed within 6 s — header timeout is inert \
         (slowloris hold bounded only by the 10 s total)"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "connection closed at {elapsed:?}, expected ~header_timeout (1 s); \
         a close near the 10 s total means header_read_timeout is not wired"
    );
}

/// POSITIVE CONTROL: a complete request still proxies with the timer wired.
#[tokio::test]
async fn h1_complete_request_still_proxies_with_timer() {
    let backend = spawn_ok_backend().await;
    let proxy = spawn_proxy(backend, Duration::from_secs(5)).await;

    let mut client = TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .await
        .unwrap();
    client.flush().await.unwrap();

    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let mut tmp = [0u8; 1024];
        loop {
            match client.read(&mut tmp).await {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
    .await;
    let head = String::from_utf8_lossy(&buf);
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "expected 200 from backend, got: {head:?}"
    );
}
