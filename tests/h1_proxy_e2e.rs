//! End-to-end tests for Pillar 3b.3b-1 — the hyper 1.x H1 proxy.
//!
//! These tests do NOT spawn the binary. They wire `lb_l7::h1_proxy::H1Proxy`
//! directly against an in-process mock backend so the assertions sit
//! exactly on the proxy's seam. Wiring through `crates/lb/src/main.rs`
//! is a binary smoke test better expressed as part of a release suite.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1 as srv_h1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{AltSvcConfig, H1Proxy, HttpTimeouts, RoundRobinAddrs};
use parking_lot::Mutex;
use rustls::client::Resumption;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const SAN_HOST: &str = "expressgateway.test";

// ── shared helpers ──────────────────────────────────────────────────────

fn make_cert_for(san: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
    let cert_der = generated.cert.der().to_vec();
    let key_der = generated.key_pair.serialize_der();
    (
        vec![CertificateDer::from(cert_der)],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
    )
}

fn build_h1_server_cfg(
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Arc<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .unwrap();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(cfg)
}

fn build_h1_client_cfg(trust_anchor: CertificateDer<'static>) -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    roots.add(trust_anchor).unwrap();
    let mut cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(Arc::new(roots))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    cfg.resumption = Resumption::disabled();
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

// ── Mock backends ───────────────────────────────────────────────────────

/// Spawn a mock backend that returns `200 OK` with the given body for
/// any inbound request. Returns the bound address and a JoinHandle.
async fn spawn_static_backend(body: &'static [u8]) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let svc = service_fn(move |_req: Request<Incoming>| async move {
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(Full::new(Bytes::from_static(body)))
                            .unwrap(),
                    )
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, handle)
}

type CapturedHeaders = Arc<Mutex<Option<hyper::HeaderMap>>>;

/// Spawn a mock backend that captures the inbound request headers into
/// the returned `CapturedHeaders` slot, then replies `200 OK` with an
/// empty body.
async fn spawn_header_capture_backend() -> (SocketAddr, CapturedHeaders, tokio::task::JoinHandle<()>)
{
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let captured: CapturedHeaders = Arc::new(Mutex::new(None));
    let captured_for_task = Arc::clone(&captured);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let captured = Arc::clone(&captured_for_task);
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let captured = Arc::clone(&captured);
                    async move {
                        *captured.lock() = Some(req.headers().clone());
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .body(Empty::<Bytes>::new())
                                .unwrap(),
                        )
                    }
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, captured, handle)
}

/// Spawn a mock backend that accepts the TCP connection then sleeps
/// forever, never sending response bytes. Used to drive the body-timeout
/// path of the gateway.
async fn spawn_blackhole_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            // Hold the socket open so the OS does not RST. Spawn a task
            // that just drops it after a long sleep — long enough that
            // the gateway's body_timeout fires first.
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(30)).await;
                drop(sock);
            });
        }
    });
    (local, handle)
}

// ── Gateway harnesses ──────────────────────────────────────────────────

/// Spawn an `h1s` gateway: TCP accept → TLS handshake → H1Proxy.
async fn spawn_h1s_gateway(
    server_cfg: Arc<ServerConfig>,
    proxy: Arc<H1Proxy>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(server_cfg);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let proxy = Arc::clone(&proxy);
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(sock).await else {
                    return;
                };
                let _ = proxy.serve_connection(tls, peer).await;
            });
        }
    });
    (local, handle)
}

/// Spawn a plain `h1` gateway: TCP accept → H1Proxy.
async fn spawn_h1_gateway(proxy: Arc<H1Proxy>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
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

// ── Test 1: end-to-end with Alt-Svc ─────────────────────────────────────

#[tokio::test]
async fn h1s_proxy_returns_backend_response_with_alt_svc() {
    // 1. Backend.
    let (backend_addr, _backend_h) = spawn_static_backend(b"backend-ok").await;

    // 2. Proxy.
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let proxy = Arc::new(H1Proxy::new(
        pool,
        picker,
        Some(AltSvcConfig {
            h3_port: 443,
            max_age: 3_600,
        }),
        HttpTimeouts::default(),
        /* is_https */ true,
    ));

    // 3. TLS gateway.
    let (cert_chain, key) = make_cert_for(SAN_HOST);
    let trust_anchor = cert_chain[0].clone();
    let server_cfg = build_h1_server_cfg(cert_chain, key);
    let (gateway_addr, _gw_h) = spawn_h1s_gateway(server_cfg, proxy).await;

    // 4. Client (hyper 1.x over rustls 0.23).
    let client_cfg = build_h1_client_cfg(trust_anchor);
    let connector = TlsConnector::from(client_cfg);
    let server_name = ServerName::try_from(SAN_HOST).unwrap();

    let sock = TcpStream::connect(gateway_addr).await.unwrap();
    let tls = connector.connect(server_name, sock).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("GET")
        .uri("/")
        .header(hyper::header::HOST, SAN_HOST)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let alt = resp
        .headers()
        .get(hyper::header::ALT_SVC)
        .expect("Alt-Svc header missing")
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(alt, "h3=\":443\"; ma=3600");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"backend-ok");
}

// ── Test 2: hop-by-hop stripping observed at backend ────────────────────

#[tokio::test]
async fn h1_proxy_strips_hop_by_hop_in_request() {
    let (backend_addr, captured, _backend_h) = spawn_header_capture_backend().await;

    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let proxy = Arc::new(H1Proxy::new(
        pool,
        picker,
        None,
        HttpTimeouts::default(),
        /* is_https */ false,
    ));
    let (gateway_addr, _gw_h) = spawn_h1_gateway(proxy).await;

    // Plain TCP client.
    let sock = TcpStream::connect(gateway_addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(sock))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("GET")
        .uri("/sink")
        .header(hyper::header::HOST, "downstream.example")
        .header(hyper::header::CONNECTION, "Keep-Alive, Foo")
        .header("foo", "bar")
        .header("keep-alive", "timeout=5")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Drain so the conn task has a chance to settle the captured slot.
    let _ = resp.into_body().collect().await.unwrap();

    // Give the backend a beat to record headers (the response future
    // returns as soon as headers are sent, but the backend's service
    // closure already wrote the headers slot before responding).
    let mut hdrs = None;
    for _ in 0..50 {
        if let Some(h) = captured.lock().clone() {
            hdrs = Some(h);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let hdrs = hdrs.expect("backend never recorded request headers");

    assert!(
        hdrs.get(hyper::header::CONNECTION).is_none(),
        "Connection must be stripped before reaching the backend"
    );
    assert!(
        hdrs.get("keep-alive").is_none(),
        "Keep-Alive must be stripped before reaching the backend"
    );
    assert!(
        hdrs.get("foo").is_none(),
        "Foo (named in Connection) must be stripped before reaching the backend"
    );
    // Forwarding headers must, however, be present.
    assert!(hdrs.get("x-forwarded-for").is_some());
    assert_eq!(
        hdrs.get("x-forwarded-proto").and_then(|v| v.to_str().ok()),
        Some("http")
    );
}

// ── Test 3: 504 on a stuck upstream ────────────────────────────────────

#[tokio::test]
async fn h1s_proxy_times_out_on_slow_body() {
    // Counter so we can tell whether the gateway actually dialed us; the
    // assertion is loose because hyper's h1 client may retry on h2c
    // upgrade preludes etc. We just need at least one accept.
    let backend_dialed = Arc::new(AtomicUsize::new(0));
    let _ = backend_dialed.clone(); // hide unused-warning under non-debug builds

    let (backend_addr, _backend_h) = spawn_blackhole_backend().await;

    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let proxy = Arc::new(H1Proxy::new(
        pool,
        picker,
        None,
        HttpTimeouts {
            header: Duration::from_millis(200),
            body: Duration::from_millis(200),
            total: Duration::from_secs(5),
        },
        /* is_https */ true,
    ));

    let (cert_chain, key) = make_cert_for(SAN_HOST);
    let trust_anchor = cert_chain[0].clone();
    let server_cfg = build_h1_server_cfg(cert_chain, key);
    let (gateway_addr, _gw_h) = spawn_h1s_gateway(server_cfg, proxy).await;

    let client_cfg = build_h1_client_cfg(trust_anchor);
    let connector = TlsConnector::from(client_cfg);
    let server_name = ServerName::try_from(SAN_HOST).unwrap();
    let sock = TcpStream::connect(gateway_addr).await.unwrap();
    let tls = connector.connect(server_name, sock).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    let conn_done = Arc::new(AtomicBool::new(false));
    let conn_done_marker = Arc::clone(&conn_done);
    tokio::spawn(async move {
        let _ = conn.await;
        conn_done_marker.store(true, Ordering::SeqCst);
    });

    let req = Request::builder()
        .method("GET")
        .uri("/slow")
        .header(hyper::header::HOST, SAN_HOST)
        .body(Empty::<Bytes>::new())
        .unwrap();

    // The gateway must respond within total_timeout + a small slack.
    let resp = tokio::time::timeout(Duration::from_secs(3), sender.send_request(req))
        .await
        .expect("client timed out before gateway returned a response")
        .expect("gateway did not return a response");
    assert_eq!(
        resp.status(),
        StatusCode::GATEWAY_TIMEOUT,
        "expected 504 from gateway when upstream stalls"
    );
}
