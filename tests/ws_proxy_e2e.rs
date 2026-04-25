//! End-to-end tests for Item 2 — the WebSocket upstream path.
//!
//! These tests wire a real backend (a `tokio_tungstenite::accept_async`
//! echo/close server) behind the live `H1Proxy` (with `WsProxy` enabled)
//! and exercise the round-trip framing, close-code forwarding, idle-
//! timeout close, binary payloads, and ping/pong keepalive.
//!
//! The transport is plain TCP on loopback — TLS termination lives in
//! `tests/h1_proxy_e2e.rs`. Item 2 is about the frame layer, not the
//! handshake's TLS wrapper; keeping the tests plain-TCP shaves several
//! seconds off the suite without losing coverage.

#![cfg(test)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
use lb_l7::ws_proxy::{WsConfig, WsProxy};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

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

/// Spawn the gateway H1 listener with WebSocket enabled, pointing at
/// `backend_addr`. Returns the gateway's bound address.
async fn spawn_ws_gateway(
    backend_addr: SocketAddr,
    ws_cfg: WsConfig,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let proxy = Arc::new(
        H1Proxy::new(pool, picker, None, HttpTimeouts::default(), false)
            .with_websocket(Arc::new(WsProxy::new(ws_cfg))),
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let proxy = Arc::clone(&proxy);
            tokio::spawn(async move {
                let _ = proxy.serve_connection(sock, peer).await;
            });
        }
    });
    (local, handle)
}

/// Spawn a WebSocket echo server. Every inbound Text/Binary is sent back
/// verbatim; Ping/Pong/Close are handled by tungstenite's auto-state
/// machine (we only observe the data frames).
async fn spawn_echo_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let ws = match tokio_tungstenite::accept_async(sock).await {
                    Ok(w) => w,
                    Err(_) => return,
                };
                let (mut tx, mut rx) = ws.split();
                while let Some(Ok(msg)) = rx.next().await {
                    match msg {
                        Message::Text(_) | Message::Binary(_) => {
                            if tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
                let _ = tx.close().await;
            });
        }
    });
    (local, handle)
}

/// Spawn a WebSocket server that immediately sends a `Close` frame with
/// code `1011 Internal Error` + reason "backend boom" on the first
/// inbound frame. Used to prove close-code forwarding.
async fn spawn_close_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut ws = match tokio_tungstenite::accept_async(sock).await {
                    Ok(w) => w,
                    Err(_) => return,
                };
                // Wait for one inbound frame, then shut with a precise
                // (code, reason).
                let _ = ws.next().await;
                let frame = CloseFrame {
                    code: CloseCode::Error,
                    reason: std::borrow::Cow::Borrowed("backend boom"),
                };
                let _ = ws.send(Message::Close(Some(frame))).await;
                let _ = ws.close(None).await;
            });
        }
    });
    (local, handle)
}

/// Spawn a WebSocket server that acknowledges the handshake but then
/// sits silent forever. Used to drive the idle timeout.
async fn spawn_silent_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut ws = match tokio_tungstenite::accept_async(sock).await {
                    Ok(w) => w,
                    Err(_) => return,
                };
                // Consume inbound frames (but never reply), so
                // tungstenite's auto-pong doesn't wake the idle loop.
                while let Some(Ok(msg)) = ws.next().await {
                    // Drop everything; break on Close so the upstream
                    // task finishes cleanly when the proxy closes.
                    if matches!(msg, Message::Close(_)) {
                        break;
                    }
                }
            });
        }
    });
    (local, handle)
}

/// Connect to the gateway as a WebSocket client with the given path.
/// Returns the post-handshake stream.
async fn connect_ws_client(
    gateway: SocketAddr,
    path: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>> {
    let url = format!("ws://{gateway}{path}");
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws
}

// ── Test 1: Text echo round-trip ───────────────────────────────────────

#[tokio::test]
async fn ws_echo_roundtrip() {
    let (backend, _b) = spawn_echo_backend().await;
    let (gw, _g) = spawn_ws_gateway(
        backend,
        WsConfig {
            idle_timeout: Duration::from_secs(30),
            max_message_size: 1024 * 1024,
            enabled: true,
            ..WsConfig::default()
        },
    )
    .await;

    let mut client = connect_ws_client(gw, "/chat").await;
    client.send(Message::Text("hello".into())).await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(3), client.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match msg {
        Message::Text(s) => assert_eq!(s, "hello"),
        other => panic!("expected Text(hello), got {other:?}"),
    }

    let _: Result<_, _> = client.close(None).await;
}

// ── Test 2: Close-code forwarding ──────────────────────────────────────

#[tokio::test]
async fn ws_close_code_forwarded() {
    let (backend, _b) = spawn_close_backend().await;
    let (gw, _g) = spawn_ws_gateway(
        backend,
        WsConfig {
            idle_timeout: Duration::from_secs(30),
            max_message_size: 1024 * 1024,
            enabled: true,
            ..WsConfig::default()
        },
    )
    .await;

    let mut client = connect_ws_client(gw, "/boom").await;
    // Poke the backend to trigger its close.
    client.send(Message::Text("ping".into())).await.unwrap();

    // Read until we observe the Close frame (the backend may also have
    // sent a Pong via tungstenite's auto-handler on its way out — drain
    // until the Close lands).
    let close_frame = {
        let mut observed: Option<CloseFrame<'_>> = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            let Ok(Some(Ok(msg))) =
                tokio::time::timeout(Duration::from_millis(500), client.next()).await
            else {
                continue;
            };
            if let Message::Close(Some(f)) = msg {
                observed = Some(f.into_owned());
                break;
            }
        }
        observed.expect("never observed Close frame from gateway")
    };
    assert_eq!(close_frame.code, CloseCode::Error);
    assert_eq!(close_frame.reason, "backend boom");
}

// ── Test 3: Idle timeout → Close 1001 ──────────────────────────────────

#[tokio::test]
async fn ws_idle_timeout_emits_1001() {
    let (backend, _b) = spawn_silent_backend().await;
    let (gw, _g) = spawn_ws_gateway(
        backend,
        WsConfig {
            idle_timeout: Duration::from_secs(1),
            max_message_size: 1024 * 1024,
            enabled: true,
            ..WsConfig::default()
        },
    )
    .await;

    let mut client = connect_ws_client(gw, "/silent").await;
    // Neither side writes; within idle_timeout + slack the proxy must
    // send us a Close(1001).
    let deadline = Duration::from_secs(3);
    let close_frame = {
        let mut observed: Option<CloseFrame<'_>> = None;
        let start = tokio::time::Instant::now();
        while start.elapsed() < deadline {
            let Ok(Some(Ok(msg))) =
                tokio::time::timeout(Duration::from_millis(500), client.next()).await
            else {
                continue;
            };
            if let Message::Close(Some(f)) = msg {
                observed = Some(f.into_owned());
                break;
            }
        }
        observed.expect("no Close frame observed before deadline")
    };
    assert_eq!(close_frame.code, CloseCode::Away);
}

// ── Test 4: Binary payload round-trip ──────────────────────────────────

#[tokio::test]
async fn ws_binary_message_roundtrip() {
    let (backend, _b) = spawn_echo_backend().await;
    let (gw, _g) = spawn_ws_gateway(
        backend,
        WsConfig {
            idle_timeout: Duration::from_secs(30),
            max_message_size: 1024 * 1024,
            enabled: true,
            ..WsConfig::default()
        },
    )
    .await;

    // 4 KiB of non-repeating bytes so the echo path cannot be fooled by
    // a short-circuit.
    let payload: Vec<u8> = (0..4096).map(|i| (i & 0xff) as u8).collect();
    let mut client = connect_ws_client(gw, "/bin").await;
    client.send(Message::Binary(payload.clone())).await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(3), client.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match msg {
        Message::Binary(b) => assert_eq!(b, payload),
        other => panic!("expected Binary, got {other:?}"),
    }
    let _: Result<_, _> = client.close(None).await;
}

// ── Test 5: Ping/Pong keepalive ────────────────────────────────────────

#[tokio::test]
async fn ws_ping_pong_keepalive() {
    let (backend, _b) = spawn_echo_backend().await;
    // Idle timeout deliberately shorter than the total test window —
    // pings must keep the connection alive in the face of it.
    let (gw, _g) = spawn_ws_gateway(
        backend,
        WsConfig {
            idle_timeout: Duration::from_millis(700),
            max_message_size: 1024 * 1024,
            enabled: true,
            ..WsConfig::default()
        },
    )
    .await;

    let mut client = connect_ws_client(gw, "/ka").await;
    // Send 5 pings at 100ms; observe that the connection still works
    // after. tungstenite's auto-handler on the backend side responds
    // with Pongs. We poll for Pongs and ensure no Close arrives.
    let mut pongs = 0usize;
    let start = tokio::time::Instant::now();
    for i in 0..5 {
        client.send(Message::Ping(vec![i as u8])).await.unwrap();
        if let Ok(Some(Ok(msg))) =
            tokio::time::timeout(Duration::from_millis(300), client.next()).await
        {
            if matches!(msg, Message::Pong(_)) {
                pongs += 1;
            } else if matches!(msg, Message::Close(_)) {
                panic!("unexpected Close during ping/pong keepalive: {msg:?}");
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Send a Text after the ping storm: the connection MUST still be up.
    client
        .send(Message::Text("still-here".into()))
        .await
        .unwrap();
    // Drain late-arriving Pongs; accept a Text as the terminal reply.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut saw_text = false;
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(Ok(msg))) =
            tokio::time::timeout(Duration::from_millis(300), client.next()).await
        else {
            continue;
        };
        match msg {
            Message::Text(s) => {
                assert_eq!(s, "still-here");
                saw_text = true;
                break;
            }
            Message::Pong(_) => {
                pongs += 1;
            }
            Message::Close(frame) => {
                panic!("unexpected Close during keepalive: {frame:?}");
            }
            _ => {}
        }
    }
    assert!(saw_text, "did not receive echoed Text after ping storm");
    assert!(pongs >= 1, "expected at least one Pong observed");
    assert!(
        start.elapsed() >= Duration::from_millis(500),
        "test concluded before the ping storm completed"
    );
    let _: Result<_, _> = client.close(None).await;
}

// ── Test 6: Ping flood → Close 1008 (WS-001) ───────────────────────────

#[tokio::test]
async fn ws_ping_flood_closes_with_1008() {
    // Listener configured with a tiny ping budget (5/10s). The silent
    // backend never replies, so the only Close the client can observe
    // is the one the gateway emits on rate-limit trip.
    let (backend, _b) = spawn_silent_backend().await;
    let (gw, _g) = spawn_ws_gateway(
        backend,
        WsConfig {
            idle_timeout: Duration::from_secs(30),
            max_message_size: 1024 * 1024,
            enabled: true,
            ping_rate_limit_per_window: 5,
            ping_rate_limit_window: Duration::from_secs(10),
            ..WsConfig::default()
        },
    )
    .await;

    let mut client = connect_ws_client(gw, "/flood").await;
    // Fire 10 Pings as fast as the SinkExt buffer accepts. The gateway
    // is allowed up to 5 in the rolling window — the 6th must trip the
    // detector and produce Close(1008) at the client.
    for i in 0..10u8 {
        // Ignore send errors past the rate-limit trip — once the
        // gateway closes the client half, further sends are expected
        // to fail. The acceptance check is the Close frame, not the
        // exact count of accepted Pings.
        let _ = client.send(Message::Ping(vec![i])).await;
    }

    let deadline = Duration::from_secs(2);
    let start = tokio::time::Instant::now();
    let close_frame = {
        let mut observed: Option<CloseFrame<'_>> = None;
        while start.elapsed() < deadline {
            let Ok(Some(Ok(msg))) =
                tokio::time::timeout(Duration::from_millis(200), client.next()).await
            else {
                continue;
            };
            if let Message::Close(Some(f)) = msg {
                observed = Some(f.into_owned());
                break;
            }
        }
        observed.expect("did not observe a Close frame within 2s of the ping flood")
    };
    assert_eq!(
        close_frame.code,
        CloseCode::Policy,
        "expected Close 1008 (Policy Violation), got {:?}",
        close_frame.code,
    );
    let reason = close_frame.reason.to_ascii_lowercase();
    assert!(
        reason.contains("ping flood") || reason.contains("rate limit"),
        "expected reason to mention ping flood or rate limit, got {:?}",
        close_frame.reason,
    );
}

// ── Test 7: Read-frame watchdog → Close 1008 (WS-002) ──────────────────

#[tokio::test]
async fn ws_read_frame_timeout_closes_with_1008() {
    // Listener with a tight per-direction read-frame budget. The silent
    // backend never produces a frame, so once the budget elapses on
    // the backend half the gateway must emit Close 1008 (Policy
    // Violation) with reason matching "read frame timeout".
    let (backend, _b) = spawn_silent_backend().await;
    let (gw, _g) = spawn_ws_gateway(
        backend,
        WsConfig {
            // idle_timeout deliberately well above the read-frame
            // budget so the path that fires is the per-direction
            // watchdog, not the all-silent idle path.
            idle_timeout: Duration::from_secs(30),
            max_message_size: 1024 * 1024,
            enabled: true,
            read_frame_timeout: Duration::from_secs(1),
            ..WsConfig::default()
        },
    )
    .await;

    let mut client = connect_ws_client(gw, "/slow-read").await;
    // One Text frame to prove the connection is alive; then sleep past
    // the watchdog budget without sending anything else.
    client.send(Message::Text("hello".into())).await.unwrap();

    let deadline = Duration::from_secs(2);
    let start = tokio::time::Instant::now();
    let close_frame = {
        let mut observed: Option<CloseFrame<'_>> = None;
        while start.elapsed() < deadline {
            let Ok(Some(Ok(msg))) =
                tokio::time::timeout(Duration::from_millis(200), client.next()).await
            else {
                continue;
            };
            if let Message::Close(Some(f)) = msg {
                observed = Some(f.into_owned());
                break;
            }
        }
        observed.expect("did not observe Close frame within 2s of read-frame timeout")
    };
    assert_eq!(
        close_frame.code,
        CloseCode::Policy,
        "expected Close 1008 (Policy Violation), got {:?}",
        close_frame.code,
    );
    let reason = close_frame.reason.to_ascii_lowercase();
    assert!(
        reason.contains("read frame timeout"),
        "expected reason to mention 'read frame timeout', got {:?}",
        close_frame.reason,
    );
}
