//! F-S27-1 proof — the WS-over-H2 (RFC 8441 extended CONNECT) mirror of
//! `crates/lb-l7/tests/round8_ws_upgrade_defer.rs`.
//!
//! Reference: Pingora GHSA-xq2h-p299-vjwv / Envoy GHSA-rj35-4m94-77jh — a
//! proxy MUST NOT signal upgrade success to the client until the *upstream*
//! WebSocket handshake has completed. The pre-fix
//! `H2Proxy::handle_ws_extended_connect` returned `200 OK` synchronously and
//! dialed the upstream in a detached task whose failure arms were just
//! `tracing::debug!; return;` — so a backend that refused the WS handshake
//! still left the H2 client holding a `200` (false success), and any DATA
//! the client pipelined behind the extended CONNECT could be relayed toward
//! a backend that never agreed to the upgrade.
//!
//! The fix (h2_proxy.rs) hoists the dial + RFC 6455 client handshake ahead
//! of the response, bounded by `HttpTimeouts::header` (the same knob the H1
//! sibling uses), mapping a refusal to **502** and a dial/handshake-budget
//! elapse to **504** — NEVER 200 — and only spawning the splice task once
//! the upstream `101` is in hand.
//!
//! Invariants asserted (the verifier's 4-point contract):
//!   1. `client_sees_502_when_backend_rejects_ws` — upstream answers non-101
//!      → extended-CONNECT response is **502**, NEVER 200.
//!   2. `client_sees_504_on_handshake_timeout` — upstream accepts TCP but
//!      never answers within the bounded budget → **504**, NEVER 200.
//!   3. `no_smuggled_data_forwarded_on_failure` — client DATA pipelined right
//!      after the extended CONNECT must NOT reach the backend when the
//!      upstream handshake is refused. (The security core.)
//!   4. The happy path stays green — proven by the unchanged
//!      `tests/ws_h2_e2e.rs`; not re-tested here.

#![cfg(test)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{HttpTimeouts, RoundRobinAddrs};
use lb_l7::h2_proxy::H2Proxy;
use lb_l7::ws_proxy::{WsConfig, WsProxy};
use lb_security::{TicketRotator, build_server_config};
use parking_lot::Mutex;
use rustls::ClientConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const SAN_HOST: &str = "expressgateway.test";

// Bounded deadlock tripwire on the client-visible response.
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

// ── TLS / pool harness (mirrors tests/h2_security_live.rs + ws_h2_e2e.rs) ─

fn make_cert_for(san: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
    let cert_der = generated.cert.der().to_vec();
    let key_der = generated.key_pair.serialize_der();
    (
        vec![CertificateDer::from(cert_der)],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
    )
}

fn build_client_cfg(trust_anchor: CertificateDer<'static>) -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(trust_anchor).unwrap();
    let mut cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(Arc::new(roots))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(cfg)
}

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

/// Spawn the gateway H2Proxy (WebSocket enabled) over TLS+ALPN-h2, pointed
/// at `backend_addr`, with an explicit `header` budget (the F-S27-1 dial
/// timeout knob). Returns the bound address + the trust anchor.
async fn spawn_h2_ws_gateway(
    backend_addr: SocketAddr,
    header_timeout: Duration,
) -> (SocketAddr, CertificateDer<'static>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let timeouts = HttpTimeouts {
        header: header_timeout,
        body: Duration::from_secs(5),
        total: Duration::from_secs(10),
        head: Duration::from_secs(10),
    };
    let h2_proxy = Arc::new(
        H2Proxy::new(pool, picker as _, None, timeouts, true)
            .with_websocket(Arc::new(WsProxy::new(WsConfig {
                idle_timeout: Duration::from_secs(30),
                max_message_size: 1024 * 1024,
                enabled: true,
                ..WsConfig::default()
            })))
            // CF-S27-2: WS-over-H2 is opt-in; this test exercises the enabled
            // path's failure ordering, so flip the gate ON.
            .with_h2_extended_connect(true),
    );

    let (cert_chain, key) = make_cert_for(SAN_HOST);
    let trust_anchor = cert_chain[0].clone();
    let rot = TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
    let rot_arc = Arc::new(Mutex::new(rot));
    let alpn: &[&[u8]] = &[b"h2", b"http/1.1"];
    let server_cfg = build_server_config(rot_arc, cert_chain, key, alpn).unwrap();
    let acceptor = TlsAcceptor::from(server_cfg);

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let h2 = Arc::clone(&h2_proxy);
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(sock).await else {
                    return;
                };
                let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                if alpn.as_deref() == Some(b"h2".as_ref()) {
                    let _ = h2.serve_connection(tls, peer).await;
                }
            });
        }
    });

    (local, trust_anchor)
}

/// TLS+H2 connect to the gateway and send an RFC 8441 extended CONNECT for
/// `path` with `end_of_stream = false`. Optionally pipeline `smuggle` bytes
/// as a follow-on DATA frame on the same stream immediately after. Returns
/// the response status the client observed (bounded by `STEP_TIMEOUT`).
async fn extended_connect_status(
    gateway: SocketAddr,
    trust_anchor: CertificateDer<'static>,
    path: &str,
    smuggle: Option<&[u8]>,
) -> http::StatusCode {
    let sock = TcpStream::connect(gateway).await.unwrap();
    let connector = TlsConnector::from(build_client_cfg(trust_anchor));
    let sn = ServerName::try_from(SAN_HOST).unwrap();
    let tls = connector.connect(sn, sock).await.unwrap();
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(b"h2".as_ref()),
        "client must negotiate ALPN h2"
    );

    let (h2, conn) = tokio::time::timeout(STEP_TIMEOUT, h2::client::handshake(tls))
        .await
        .expect("h2 handshake timed out")
        .expect("h2 handshake failed");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2 = tokio::time::timeout(STEP_TIMEOUT, h2.ready())
        .await
        .expect("h2 ready timed out")
        .expect("h2 connection not ready");

    let uri: http::Uri = format!("https://{SAN_HOST}{path}").parse().unwrap();
    let mut req = http::Request::builder()
        .method(http::Method::CONNECT)
        .uri(uri)
        .body(())
        .unwrap();
    req.extensions_mut()
        .insert(h2::ext::Protocol::from_static("websocket"));

    // end_of_stream = false → the stream stays open as the tunnel.
    let (resp_fut, mut send_stream) = h2
        .send_request(req, false)
        .expect("send extended CONNECT failed");

    // No-smuggle reproducer: pipeline the client payload as a DATA frame on
    // the open stream IMMEDIATELY, before the gateway has decided the
    // upstream's fate. A correct gateway dials+handshakes the upstream
    // first; with the handshake refused it answers 502 and never establishes
    // the relay, so these bytes must die at the gateway and never touch the
    // backend.
    if let Some(payload) = smuggle {
        send_stream.reserve_capacity(payload.len());
        // Best-effort: send the DATA frame as soon as any capacity exists.
        // The H2 default initial window is well above our tiny payload.
        let _ = send_stream.send_data(bytes::Bytes::copy_from_slice(payload), false);
    }

    let resp = tokio::time::timeout(STEP_TIMEOUT, resp_fut)
        .await
        .expect("extended CONNECT response timed out")
        .expect("extended CONNECT response errored");
    resp.status()
}

// ── Faux backends ──────────────────────────────────────────────────────

/// Backend that consumes the inbound WS handshake and answers `200 OK`
/// (a deliberate non-101), then closes.
async fn spawn_non101_backend() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    local
}

/// Backend that accepts the TCP connection but never answers the WS
/// handshake — the bounded `header` budget must elapse → 504.
async fn spawn_stuck_backend() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                // Hold the socket open and never write a handshake response.
                let _hold = s;
                std::future::pending::<()>().await;
            });
        }
    });
    local
}

// ── Test 1: upstream non-101 → 502 (never 200) ─────────────────────────

#[tokio::test]
async fn client_sees_502_when_backend_rejects_ws() {
    let backend = spawn_non101_backend().await;
    let (gw, anchor) = spawn_h2_ws_gateway(backend, Duration::from_secs(3)).await;

    let status = extended_connect_status(gw, anchor, "/chat", None).await;
    assert_eq!(
        status,
        http::StatusCode::BAD_GATEWAY,
        "upstream non-101 must yield 502, got {status} \
         (a 200 here is F-S27-1: false-success / premature 200)"
    );
    assert_ne!(
        status,
        http::StatusCode::OK,
        "the 200 must NEVER reach the client when the upstream refuses the WS handshake"
    );
}

// ── Test 2: upstream never answers → 504 (never 200) ───────────────────

#[tokio::test]
async fn client_sees_504_on_handshake_timeout() {
    let backend = spawn_stuck_backend().await;
    // Tight header budget so the watchdog fires fast.
    let (gw, anchor) = spawn_h2_ws_gateway(backend, Duration::from_millis(200)).await;

    let status = extended_connect_status(gw, anchor, "/chat", None).await;
    assert_eq!(
        status,
        http::StatusCode::GATEWAY_TIMEOUT,
        "upstream that never answers within the bounded budget must yield 504, got {status}"
    );
    assert_ne!(
        status,
        http::StatusCode::OK,
        "the 200 must NEVER reach the client on a dial/handshake timeout"
    );
}

// ── Test 3: no-smuggle — pipelined DATA must not reach the backend ─────

#[tokio::test]
async fn no_smuggled_data_forwarded_on_failure() {
    // The backend records every byte it ever receives across the whole
    // connection, refuses the WS handshake (non-101), then keeps reading for
    // a short window so any smuggled relay would land in the record.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let backend_addr = listener.local_addr().unwrap();
    let seen: Arc<tokio::sync::Mutex<Vec<u8>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let seen_bg = Arc::clone(&seen);
    tokio::spawn(async move {
        let Ok((mut s, _)) = listener.accept().await else {
            return;
        };
        let mut tmp = [0u8; 4096];
        if let Ok(n) = s.read(&mut tmp).await {
            seen_bg.lock().await.extend_from_slice(&tmp[..n]);
        }
        // Refuse the WS handshake.
        let _ = s
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            .await;
        // Keep reading: if the gateway erroneously established a relay, the
        // client's smuggled DATA would arrive here.
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(500), s.read(&mut tmp)).await
        {
            seen_bg.lock().await.extend_from_slice(&tmp[..n]);
        }
    });

    let (gw, anchor) = spawn_h2_ws_gateway(backend_addr, Duration::from_secs(2)).await;

    // A recognizable marker the backend must never observe.
    const SMUGGLE: &[u8] = b"SMUGGLED-WS-OVER-H2-PAYLOAD-MARKER";
    let status = extended_connect_status(gw, anchor, "/chat", Some(SMUGGLE)).await;
    assert_eq!(
        status,
        http::StatusCode::BAD_GATEWAY,
        "upstream refusal must yield 502, got {status}"
    );

    // Give any (erroneous) relay a moment to forward the smuggled bytes.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let recorded = seen.lock().await.clone();
    let recorded_str = String::from_utf8_lossy(&recorded);
    assert!(
        !recorded_str.contains("SMUGGLED-WS-OVER-H2-PAYLOAD-MARKER"),
        "the smuggled pipelined DATA must NOT reach the backend when the upstream \
         WS handshake is refused; backend saw: {recorded_str:?}"
    );
}
