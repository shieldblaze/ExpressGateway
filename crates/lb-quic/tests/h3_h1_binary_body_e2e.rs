//! SESSION 2 / Phase 0 — H3→H1 binary request-body fidelity proof.
//!
//! The S1-B seam (`h3_bridge::build_h1_request`'s `Some(body)` arm,
//! reached through `h3_to_h1_roundtrip`'s `body` param) previously
//! appended the request body via `String::from_utf8_lossy`, which
//! silently corrupts any non-UTF-8 payload (protobuf / images / gzip)
//! and rewrites the byte count vs the already-correct
//! `Content-Length` — a body-corruption + request-smuggling bug.
//!
//! This test exercises `h3_to_h1_roundtrip` DIRECTLY (the function the
//! datapath calls; Phase 1 wires inbound H3 DATA frames into the
//! `Some(..)` argument) against a real tokio TCP backend, sending a
//! deliberately non-UTF-8 body (`0xFF 0xFE 0x00 0x80 'A' 0x01 0xFD`,
//! including a NUL) and asserts the backend received the body bytes
//! byte-for-byte AND that the request's `Content-Length` parsed to
//! exactly `body.len()`.
//!
//! It does not touch or relax any existing test. Pool construction
//! mirrors `h3_h1_bridge_e2e.rs`'s `build_tcp_pool`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::h3_bridge::{H3Request, h3_to_h1_roundtrip};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// Same idiom as `h3_h1_bridge_e2e.rs`.
fn build_tcp_pool() -> TcpPool {
    TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts {
            nodelay: true,
            keepalive: true,
            rcvbuf: Some(65_536),
            sndbuf: Some(65_536),
            quickack: false,
            tcp_fastopen_connect: false,
        },
        Runtime::new(),
    )
}

/// Mock HTTP/1.1 backend: reads the full request (head + the
/// `Content-Length` body), captures the raw bytes AFTER the
/// `\r\n\r\n` separator plus the parsed `Content-Length`, then replies
/// a minimal valid bodyless 200.
async fn spawn_capturing_backend() -> (
    SocketAddr,
    oneshot::Receiver<(Vec<u8>, Option<usize>)>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<(Vec<u8>, Option<usize>)>();
    let handle = tokio::spawn(async move {
        let (mut sock, _) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut all: Vec<u8> = Vec::with_capacity(1024);
        let mut buf = [0u8; 4096];
        // Read until we have the full header block and the declared
        // Content-Length body.
        loop {
            let sep = all.windows(4).position(|w| w == b"\r\n\r\n");
            if let Some(sep_pos) = sep {
                let head = &all[..sep_pos];
                let content_len = std::str::from_utf8(head).ok().and_then(|h| {
                    h.split("\r\n")
                        .filter_map(|l| l.split_once(':'))
                        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                });
                let body_start = sep_pos + 4;
                let have_body = all.len().saturating_sub(body_start);
                if let Some(cl) = content_len {
                    if have_body >= cl {
                        let body = all[body_start..body_start + cl].to_vec();
                        let _ = tx.send((body, content_len));
                        break;
                    }
                } else {
                    let _ = tx.send((all[body_start..].to_vec(), None));
                    break;
                }
            }
            match sock.read(&mut buf).await {
                Ok(0) => {
                    let sep_pos = all.windows(4).position(|w| w == b"\r\n\r\n");
                    let body = sep_pos.map(|p| all[p + 4..].to_vec()).unwrap_or_default();
                    let _ = tx.send((body, None));
                    break;
                }
                Ok(n) => all.extend_from_slice(&buf[..n]),
                Err(_) => return,
            }
        }
        let _ = sock
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await;
        let _ = sock.shutdown().await;
    });
    (addr, rx, handle)
}

/// Non-UTF-8 request body forwarded H3→H1 must arrive at the backend
/// byte-for-byte, and the `Content-Length` must equal `body.len()`.
#[tokio::test]
async fn h3_to_h1_forwards_non_utf8_body_byte_for_byte() {
    // Deliberately non-UTF-8 (0x80/0xFD are invalid continuation/lead
    // bytes) and contains a NUL — a lossy string round-trip would
    // corrupt it.
    let body: [u8; 7] = [0xFF, 0xFE, 0x00, 0x80, b'A', 0x01, 0xFD];
    assert_eq!(body.len(), 7);

    let (backend_addr, body_rx, backend_handle) = spawn_capturing_backend().await;
    let pool = build_tcp_pool();

    let req = H3Request {
        method: "POST".to_string(),
        path: "/binary".to_string(),
        authority: "binary.test:443".to_string(),
        extra: Vec::new(),
        trailers: Vec::new(),
    };

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        h3_to_h1_roundtrip(&req, backend_addr, &pool, Some(&body)),
    )
    .await
    .expect("h3_to_h1_roundtrip timed out");

    let captured = tokio::time::timeout(Duration::from_secs(2), body_rx)
        .await
        .ok()
        .and_then(Result::ok);
    backend_handle.abort();

    // Roundtrip itself must succeed (502 fallback would mean the
    // backend dial/IO failed, not a body-fidelity result).
    let resp_bytes = result.expect("h3_to_h1_roundtrip returned Err");
    assert!(
        !resp_bytes.is_empty(),
        "expected encoded H3 response bytes from the bridge"
    );

    let (captured_body, content_len) = captured.expect("backend did not capture a request body");

    // (1) Content-Length header parsed to exactly body.len() (7).
    assert_eq!(
        content_len,
        Some(body.len()),
        "Content-Length must equal body.len()=={}; got {content_len:?}",
        body.len()
    );

    // (2) The body bytes arrived at the backend byte-for-byte — no
    // lossy UTF-8 substitution, no truncation at the NUL.
    assert_eq!(
        captured_body.as_slice(),
        &body,
        "request body must reach the H1 backend byte-for-byte; got {captured_body:?}"
    );
}
