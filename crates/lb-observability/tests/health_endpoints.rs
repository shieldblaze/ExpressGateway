//! REL-2-04 proof tests: `/livez`, `/readyz`, `/startupz` semantics
//! under every [`ProbeState`] transition.
//!
//! Spawns the admin HTTP listener on `127.0.0.1:0` against a
//! synthetic [`ProbeRegistry`], scrapes each endpoint, and asserts
//! the (status, body) pair matches the table documented in
//! `crates/lb-observability/src/admin_http.rs`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lb_observability::admin_http::serve_with_probes;
use lb_observability::probes::ProbeRegistry;
use lb_observability::{MetricsRegistry, ProbeState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

async fn spawn_admin(probes: Arc<ProbeRegistry>) -> (SocketAddr, CancellationToken) {
    let reg = Arc::new(MetricsRegistry::new());
    let cancel = CancellationToken::new();
    let bind: SocketAddr = "127.0.0.1:0".parse().expect("static addr parse");
    let local = serve_with_probes(reg, probes, bind, cancel.clone())
        .await
        .expect("bind admin listener");
    (local, cancel)
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .await
        .expect("write request");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read response");
    let text = String::from_utf8(buf).expect("utf-8 response");
    // Status line: "HTTP/1.1 <code> <reason>"
    let status_code: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status code");
    // Body after the blank line.
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    (status_code, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_livez_readyz_startupz_states() {
    let probes = ProbeRegistry::shared();
    let (addr, cancel) = spawn_admin(Arc::clone(&probes)).await;

    // ── Phase 1: Starting ────────────────────────────────────────
    assert_eq!(probes.state(), ProbeState::Starting);

    let (livez_code, livez_body) = http_get(addr, "/livez").await;
    assert_eq!(livez_code, 200, "/livez 200 even during boot");
    assert!(
        livez_body.contains("\"status\":\"booting\""),
        "boot livez body: {livez_body}",
    );

    let (readyz_code, readyz_body) = http_get(addr, "/readyz").await;
    assert_eq!(readyz_code, 503, "/readyz 503 during boot");
    assert!(
        readyz_body.contains("\"status\":\"booting\""),
        "boot readyz body: {readyz_body}",
    );

    let (startupz_code, startupz_body) = http_get(addr, "/startupz").await;
    assert_eq!(startupz_code, 503, "/startupz 503 during boot");
    assert!(
        startupz_body.contains("\"status\":\"booting\""),
        "boot startupz body: {startupz_body}",
    );

    // ── Phase 2: Ready ───────────────────────────────────────────
    probes.set_ready();
    let (livez_code, livez_body) = http_get(addr, "/livez").await;
    assert_eq!(livez_code, 200);
    assert!(livez_body.contains("\"status\":\"ok\""));

    let (readyz_code, readyz_body) = http_get(addr, "/readyz").await;
    assert_eq!(readyz_code, 200, "/readyz 200 once ready");
    assert!(readyz_body.contains("\"status\":\"ok\""));

    let (startupz_code, startupz_body) = http_get(addr, "/startupz").await;
    assert_eq!(startupz_code, 200, "/startupz 200 once startup done");
    assert!(startupz_body.contains("\"status\":\"ok\""));

    // ── Phase 3: Draining ────────────────────────────────────────
    probes.set_draining();
    let (livez_code, livez_body) = http_get(addr, "/livez").await;
    assert_eq!(
        livez_code, 200,
        "/livez stays 200 during drain so K8s doesn't yank the pod",
    );
    assert!(livez_body.contains("\"status\":\"draining\""));

    let (readyz_code, readyz_body) = http_get(addr, "/readyz").await;
    assert_eq!(readyz_code, 503, "/readyz flips to 503 on drain");
    assert!(readyz_body.contains("\"status\":\"draining\""));

    let (startupz_code, startupz_body) = http_get(addr, "/startupz").await;
    assert_eq!(
        startupz_code, 200,
        "/startupz stays 200 — startup already completed",
    );
    assert!(startupz_body.contains("\"status\":\"draining\""));

    // ── back-compat: /healthz aliases /livez ─────────────────────
    let (hz_code, hz_body) = http_get(addr, "/healthz").await;
    let (lv_code, lv_body) = http_get(addr, "/livez").await;
    assert_eq!(hz_code, lv_code);
    assert_eq!(hz_body, lv_body);

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn test_readyz_returns_json_content_type() {
    let probes = ProbeRegistry::shared();
    probes.set_ready();
    let (addr, cancel) = spawn_admin(Arc::clone(&probes)).await;

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(b"GET /readyz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read");
    let text = String::from_utf8(buf).expect("utf-8");
    assert!(
        text.to_ascii_lowercase()
            .contains("content-type: application/json"),
        "response was: {text}",
    );

    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
