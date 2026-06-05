//! INC-5 — F-S27-2 owner disposition: WS-over-H2 (RFC 8441 extended CONNECT)
//! is OFF by default. This is the LOAD-BEARING proof that the gate holds.
//!
//! With the listener's `WsProxy` enabled but `h2_extended_connect` NOT set
//! (the default), the H2Proxy must:
//!   1. NOT advertise `SETTINGS_ENABLE_CONNECT_PROTOCOL`, and
//!   2. NOT tunnel an inbound extended CONNECT — a hostile client that sends
//!      `:method=CONNECT` + `:protocol=websocket` anyway must NOT receive a
//!      200 tunnel, and the WebSocket BACKEND must never be dialed.
//!
//! Nothing about the gateway is mocked: a real `H2Proxy` over TLS (ALPN h2),
//! a real raw-`h2` client, and a backend whose TCP accept count we observe to
//! prove it is never reached.

#![cfg(test)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
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
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const SAN_HOST: &str = "expressgateway.test";
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

// ── TLS / pool harness (mirrors tests/ws_h2_e2e.rs) ────────────────────

fn make_cert_for(san: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let g = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
    (
        vec![CertificateDer::from(g.cert.der().to_vec())],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(g.signing_key.serialize_der())),
    )
}

fn build_client_cfg(ta: CertificateDer<'static>) -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ta).unwrap();
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

/// A backend that counts every TCP connection it accepts. If the gateway
/// ever dialed it (i.e. erroneously tunneled / forwarded the CONNECT), the
/// counter would be > 0. It does nothing else.
async fn spawn_counting_backend(accepts: Arc<AtomicU64>) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            accepts.fetch_add(1, Ordering::SeqCst);
            // Hold the socket so a (hypothetical) dial doesn't immediately
            // error; we only care that an accept happened at all.
            tokio::spawn(async move {
                let _hold = sock;
                tokio::time::sleep(Duration::from_secs(2)).await;
            });
        }
    });
    local
}

/// Spawn the gateway with WebSocket enabled but the H2 extended-CONNECT gate
/// at its DEFAULT (OFF) — i.e. no `.with_h2_extended_connect(true)`.
async fn spawn_gateway_gate_off(backend: SocketAddr) -> (SocketAddr, CertificateDer<'static>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend]).unwrap());
    let h2 = Arc::new(
        // WsProxy relay enabled, but WS-over-H2 gate left at default OFF.
        H2Proxy::new(pool, picker as _, None, HttpTimeouts::default(), true).with_websocket(
            Arc::new(WsProxy::new(WsConfig {
                idle_timeout: Duration::from_secs(30),
                max_message_size: 1024 * 1024,
                enabled: true,
                ..WsConfig::default()
            })),
        ),
    );
    let (chain, key) = make_cert_for(SAN_HOST);
    let ta = chain[0].clone();
    let rot = TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
    let alpn: &[&[u8]] = &[b"h2", b"http/1.1"];
    let cfg = build_server_config(Arc::new(Mutex::new(rot)), chain, key, alpn).unwrap();
    let acceptor = TlsAcceptor::from(cfg);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let h2 = Arc::clone(&h2);
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(sock).await else {
                    return;
                };
                if tls.get_ref().1.alpn_protocol() == Some(b"h2".as_ref()) {
                    let _ = h2.serve_connection(tls, peer).await;
                }
            });
        }
    });
    (local, ta)
}

async fn connect_h2(
    gateway: SocketAddr,
    ta: CertificateDer<'static>,
) -> (
    h2::client::SendRequest<Bytes>,
    tokio::task::JoinHandle<Result<(), h2::Error>>,
) {
    let sock = TcpStream::connect(gateway).await.unwrap();
    let connector = TlsConnector::from(build_client_cfg(ta));
    let sn = ServerName::try_from(SAN_HOST).unwrap();
    let tls = connector.connect(sn, sock).await.unwrap();
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(b"h2".as_ref()),
        "client must negotiate ALPN h2"
    );
    let (h2c, conn) = tokio::time::timeout(STEP_TIMEOUT, h2::client::handshake(tls))
        .await
        .expect("h2 handshake timed out")
        .expect("h2 handshake failed");
    let conn_task = tokio::spawn(conn);
    (h2c, conn_task)
}

// ── Test 1: gated-off extended CONNECT is NOT tunneled ─────────────────

/// A hostile raw-h2 client sends `:method=CONNECT` + `:protocol=websocket`
/// even though the server did not advertise connect-protocol. The gateway
/// must NOT answer with a 200 tunnel, and the WS backend must never be dialed.
#[tokio::test]
async fn extended_connect_not_tunneled_when_gated_off() {
    let accepts = Arc::new(AtomicU64::new(0));
    let backend = spawn_counting_backend(Arc::clone(&accepts)).await;
    let (gw, ta) = spawn_gateway_gate_off(backend).await;

    let (h2c, _conn_task) = connect_h2(gw, ta).await;
    let mut h2c = tokio::time::timeout(STEP_TIMEOUT, h2c.ready())
        .await
        .expect("h2 ready timed out")
        .expect("h2 not ready");

    let uri: http::Uri = format!("https://{SAN_HOST}/chat").parse().unwrap();
    let mut req = http::Request::builder()
        .method(http::Method::CONNECT)
        .uri(uri)
        .body(())
        .unwrap();
    req.extensions_mut()
        .insert(h2::ext::Protocol::from_static("websocket"));

    // end_of_stream=false: a real extended CONNECT keeps the stream open as a
    // tunnel. Sending it should NOT yield a 200 tunnel here.
    let send_result = h2c.send_request(req, false);

    // The outcome is one of: (a) send_request errors (h2/hyper rejects the
    // :protocol pseudo-header because connect-protocol was not advertised),
    // or (b) the response resolves with a NON-2xx status, or (c) the response
    // future errors (stream reset). ALL are acceptable — the only forbidden
    // outcome is a 2xx tunnel. We assert that AND that the backend was never
    // dialed.
    let outcome: String = match send_result {
        Err(e) => format!("send_request error: {e}"),
        Ok((resp_fut, _send)) => match tokio::time::timeout(STEP_TIMEOUT, resp_fut).await {
            Ok(Ok(resp)) => {
                let status = resp.status();
                assert!(
                    !status.is_success(),
                    "GATE BREACH: gated-off WS-over-H2 returned a 2xx tunnel ({status}); \
                         the extended CONNECT must NOT be tunneled when h2_extended_connect=false"
                );
                format!("non-2xx status: {status}")
            }
            Ok(Err(e)) => format!("response stream error/reset: {e}"),
            Err(_) => "response timed out (no tunnel established)".to_owned(),
        },
    };
    eprintln!("gated-off extended CONNECT outcome: {outcome}");

    // Give any (erroneous) backend dial a moment to land.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let dialed = accepts.load(Ordering::SeqCst);
    assert_eq!(
        dialed, 0,
        "GATE BREACH: the WS backend was dialed {dialed} time(s) for a gated-off \
         extended CONNECT — it must never be reached (no tunnel established)"
    );
}

// ── Test 2: server does not advertise SETTINGS_ENABLE_CONNECT_PROTOCOL ──

/// When gated off, a normal H2 client must NOT see connect-protocol enabled.
/// The `h2` crate surfaces the peer's advertised setting via
/// `is_extended_connect_protocol_enabled()` on the SendRequest handle after
/// SETTINGS negotiation. We drive one trivial request to force the SETTINGS
/// exchange, then assert the flag is false.
#[tokio::test]
async fn connect_protocol_not_advertised_when_gated_off() {
    let accepts = Arc::new(AtomicU64::new(0));
    let backend = spawn_counting_backend(Arc::clone(&accepts)).await;
    let (gw, ta) = spawn_gateway_gate_off(backend).await;

    let (h2c, _conn_task) = connect_h2(gw, ta).await;
    let mut h2c = tokio::time::timeout(STEP_TIMEOUT, h2c.ready())
        .await
        .expect("h2 ready timed out")
        .expect("h2 not ready");

    // Issue a plain GET to force SETTINGS to have been exchanged, then read
    // the advertised connect-protocol bit.
    let uri: http::Uri = format!("https://{SAN_HOST}/").parse().unwrap();
    let req = http::Request::builder()
        .method(http::Method::GET)
        .uri(uri)
        .body(())
        .unwrap();
    let (resp_fut, _send) = h2c.send_request(req, true).unwrap();
    // Drive it (don't care about the body/status; this is a backendless
    // listener so it will be a 5xx — the point is to settle SETTINGS).
    let _ = tokio::time::timeout(STEP_TIMEOUT, resp_fut).await;

    assert!(
        !h2c.is_extended_connect_protocol_enabled(),
        "GATE BREACH: the gateway advertised SETTINGS_ENABLE_CONNECT_PROTOCOL \
         while h2_extended_connect=false"
    );
}
