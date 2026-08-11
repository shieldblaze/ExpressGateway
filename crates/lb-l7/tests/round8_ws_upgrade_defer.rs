//! ROUND8-L7-01 + ROUND8-OPS-06 proof.
//!
//! Pingora GHSA-xq2h-p299-vjwv / Envoy GHSA-rj35-4m94-77jh (both CVSS 9.3): a
//! proxy MUST NOT emit `101 Switching Protocols` until the UPSTREAM WebSocket
//! handshake has completed. The pre-fix `handle_ws_upgrade` returned `101`
//! synchronously and dialed in a detached task.
//!
//! Asserted: backend answers non-101 → client gets 502 and the wire NEVER
//! carries 101; backend never answers → 504, no 101; bytes pipelined after the
//! upgrade request never reach the backend when the handshake is refused (the
//! Pingora reproducer shape); and the upstream request carries a `traceparent`
//! whose trace-id matches the client's but whose parent-id does not — the LB
//! span is the new parent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
use lb_l7::ws_proxy::{WsConfig, WsProxy};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const CLIENT_TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// Spawn the H1+WS proxy on an ephemeral loopback port; returns its address.
async fn spawn_proxy(backend_addr: SocketAddr, header_timeout: Duration) -> SocketAddr {
    let pool = TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts::default(),
        lb_io::Runtime::new(),
    );
    let picker = RoundRobinAddrs::new(vec![backend_addr]).unwrap();
    let ws = Arc::new(WsProxy::new(WsConfig::default()));
    let proxy = Arc::new(
        H1Proxy::new(
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
        )
        .with_websocket(ws),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((sock, peer)) = listener.accept().await {
            let _ = proxy.serve_connection(sock, peer).await;
        }
    });
    addr
}

/// A WS handshake request the upgrade detector accepts.
fn ws_request_bytes(host: &str, extra_headers: &str) -> Vec<u8> {
    format!(
        "GET /chat HTTP/1.1\r\n\
         Host: {host}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         {extra_headers}\r\n"
    )
    .into_bytes()
}

async fn read_response_head(client: &mut TcpStream) -> String {
    let mut resp = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), async {
        let mut tmp = [0u8; 2048];
        loop {
            let n = client.read(&mut tmp).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            resp.extend_from_slice(&tmp[..n]);
            if resp.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
    })
    .await;
    assert!(read.is_ok(), "client must get a response, not a hang");
    String::from_utf8_lossy(&resp).into_owned()
}

#[tokio::test]
async fn client_sees_502_when_backend_rejects_ws() {
    // Faux backend: consume the handshake, answer a deliberate 200, close.
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut s, _) = backend.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = s.read(&mut buf).await;
        let _ = s
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await;
    });

    let proxy_addr = spawn_proxy(backend_addr, Duration::from_secs(3)).await;
    let mut client = TcpStream::connect(proxy_addr).await.unwrap();
    client
        .write_all(&ws_request_bytes("example.com", ""))
        .await
        .unwrap();

    let head = read_response_head(&mut client).await;
    assert!(
        head.starts_with("HTTP/1.1 502"),
        "expected 502 on upstream non-101, got: {head:?}"
    );
    assert!(
        !head.contains("101"),
        "the 101 must NEVER reach the client (Pingora GHSA-xq2h-p299-vjwv): {head:?}"
    );
}

#[tokio::test]
async fn client_sees_504_on_dial_timeout() {
    // The backend accepts TCP but never answers the handshake — the bounded
    // header timeout must elapse → 504.
    let stuck_backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = stuck_backend.local_addr().unwrap();
    tokio::spawn(async move {
        let _conn = stuck_backend.accept().await.unwrap();
        std::future::pending::<()>().await;
    });

    let proxy_addr = spawn_proxy(backend_addr, Duration::from_millis(150)).await;
    let mut client = TcpStream::connect(proxy_addr).await.unwrap();
    client
        .write_all(&ws_request_bytes("example.com", ""))
        .await
        .unwrap();

    let head = read_response_head(&mut client).await;
    assert!(
        head.starts_with("HTTP/1.1 504"),
        "expected 504 on upstream dial/handshake timeout, got: {head:?}"
    );
    assert!(!head.contains("101"), "no 101 on timeout: {head:?}");
}

#[tokio::test]
async fn no_smuggled_request_forwarded_on_failure() {
    // The backend records every byte it receives. With the upstream handshake
    // refused, the client's pipelined `/smuggled` line must never appear in it.
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend.local_addr().unwrap();
    let seen: Arc<tokio::sync::Mutex<Vec<u8>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let seen_bg = Arc::clone(&seen);
    tokio::spawn(async move {
        let (mut s, _) = backend.accept().await.unwrap();
        let mut tmp = [0u8; 4096];
        if let Ok(n) = s.read(&mut tmp).await {
            seen_bg.lock().await.extend_from_slice(&tmp[..n]);
        }
        let _ = s
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            .await;
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(300), s.read(&mut tmp)).await
        {
            seen_bg.lock().await.extend_from_slice(&tmp[..n]);
        }
    });

    let proxy_addr = spawn_proxy(backend_addr, Duration::from_secs(2)).await;
    let mut client = TcpStream::connect(proxy_addr).await.unwrap();
    let mut bytes = ws_request_bytes("example.com", "");
    bytes.extend_from_slice(b"GET /smuggled HTTP/1.1\r\nHost: victim\r\n\r\n");
    client.write_all(&bytes).await.unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;
    let recorded = seen.lock().await.clone();
    let recorded_str = String::from_utf8_lossy(&recorded);
    assert!(
        !recorded_str.contains("/smuggled"),
        "the smuggled pipelined request must NOT reach the backend; saw: {recorded_str:?}"
    );
}

#[tokio::test]
async fn upstream_receives_child_traceparent() {
    // Answer 200 (non-101) so the proxy still drives the full upstream
    // handshake — where the header is injected — before returning 502.
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend.local_addr().unwrap();
    let seen: Arc<tokio::sync::Mutex<Vec<u8>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let seen_bg = Arc::clone(&seen);
    tokio::spawn(async move {
        let (mut s, _) = backend.accept().await.unwrap();
        let mut tmp = [0u8; 4096];
        if let Ok(n) = s.read(&mut tmp).await {
            seen_bg.lock().await.extend_from_slice(&tmp[..n]);
        }
        let _ = s
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await;
    });

    let proxy_addr = spawn_proxy(backend_addr, Duration::from_secs(3)).await;
    let mut client = TcpStream::connect(proxy_addr).await.unwrap();
    client
        .write_all(&ws_request_bytes(
            "example.com",
            &format!("traceparent: {CLIENT_TRACEPARENT}\r\n"),
        ))
        .await
        .unwrap();

    let _ = tokio::time::timeout(Duration::from_secs(4), async {
        let mut tmp = [0u8; 1024];
        let _ = client.read(&mut tmp).await;
    })
    .await;

    let recorded = seen.lock().await.clone();
    let recorded_str = String::from_utf8_lossy(&recorded).to_ascii_lowercase();
    assert!(
        recorded_str.contains("traceparent:"),
        "upstream handshake must carry a traceparent: {recorded_str:?}"
    );
    assert!(
        recorded_str.contains("0af7651916cd43dd8448eb211c80319c"),
        "trace-id must be preserved end-to-end: {recorded_str:?}"
    );
    assert!(
        !recorded_str.contains("b7ad6b7169203331"),
        "parent-id must be replaced by the LB span id, not forwarded verbatim: {recorded_str:?}"
    );
}
