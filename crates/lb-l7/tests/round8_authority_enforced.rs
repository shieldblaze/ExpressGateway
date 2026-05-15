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
