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
            // S14 CF-BODY-WALLCLOCK: `body` is now Phase-A IDLE
            // (no-forward-progress) — for an Empty<Bytes> request, the
            // pump completes immediately so `upload_complete` flips at
            // t≈0 and the helper enters Phase B without `body` ever
            // firing. The wedged-backend timeout for an empty request is
            // therefore governed by `head` (Phase B fixed cap), not
            // `body`. Set `head` to the same short value to preserve the
            // test's "504 promptly on blackhole" property.
            body: Duration::from_millis(200),
            total: Duration::from_secs(5),
            head: Duration::from_millis(200),
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

// ── F-CAP-1: pump verdict is authoritative over a send_request error ──────
//
// When the bounded pump deliberately aborts the upstream (over the 64 MiB
// cap → 413, or a forbidden-framing-field trailer → 400), the upstream
// `send_request` fails because the backend never sent a response head. The
// caller must return the pump's CLASSIFIED verdict (413/400), NOT a 502 —
// deterministically, with no 413-vs-502 race. A GENUINE upstream failure
// (not pump-caused) must still map to 502.

/// Backend that READS and discards the entire request body, replying
/// `200 OK` only on a clean end-of-body. For an over-cap upload the gateway
/// pump aborts the request before the body ends, so this backend never sends
/// a response head — exactly the F-CAP-1 condition (the client must still see
/// 413, from the pump verdict, not 502). Draining the body keeps the gateway's
/// bounded channel from stalling so the pump can count past the cap.
async fn spawn_drain_body_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| async move {
                    // Drain the whole inbound body (discarding it) before
                    // responding. If the gateway aborts the upstream request
                    // mid-body (over-cap / forbidden trailer), `collect()`
                    // resolves with an Err and we never send a 200 head — the
                    // backend stays silent, which is the case under test.
                    let mut body = req.into_body();
                    use http_body_util::BodyExt as _;
                    while let Some(next) = body.frame().await {
                        if next.is_err() {
                            // Upstream request aborted by the gateway → do not
                            // respond; let the connection wind down.
                            return Ok::<_, Infallible>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Empty::<Bytes>::new())
                                    .unwrap(),
                            );
                        }
                    }
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(Empty::<Bytes>::new())
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

/// Backend that accepts the TCP connection then immediately closes it without
/// speaking any HTTP — a GENUINE upstream failure. `send_request` fails for a
/// reason the pump did NOT cause; the gateway must map this to 502.
async fn spawn_rst_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            // Drop the socket immediately → the gateway's client handshake /
            // send_request sees a closed connection (genuine upstream failure).
            drop(sock);
        }
    });
    (local, handle)
}

/// Read the HTTP status line (first line) from a raw gateway response and
/// return the 3-digit status code. Reads until the first CRLF.
async fn read_status_code(sock: &mut TcpStream) -> u16 {
    use tokio::io::AsyncReadExt as _;
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = sock.read(&mut byte).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n") {
            break;
        }
        if buf.len() > 256 {
            break;
        }
    }
    let line = String::from_utf8_lossy(&buf);
    // `HTTP/1.1 413 Payload Too Large\r\n`
    line.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from line: {line:?}"))
}

/// Build a plain-H1 gateway over a backend, with a generous body timeout so
/// the bounded verdict await in the F-CAP-1 path resolves on the pump verdict
/// rather than elapsing.
async fn spawn_gateway_over(backend_addr: SocketAddr) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let proxy = Arc::new(H1Proxy::new(
        pool,
        picker,
        None,
        HttpTimeouts {
            header: Duration::from_secs(10),
            body: Duration::from_secs(10),
            total: Duration::from_secs(30),
            head: Duration::from_secs(30),
        },
        /* is_https */ false,
    ));
    spawn_h1_gateway(proxy).await
}

const OVER_CAP_BYTES: usize = 64 * 1024 * 1024 + 64 * 1024; // > MAX_REQUEST_BODY_BYTES

/// F-CAP-1 (Content-Length framing): a >64 MiB Content-Length upload to a
/// backend that is still reading the body → client observes 413, not 502.
#[tokio::test]
async fn over_cap_content_length_upload_yields_413_not_502() {
    use tokio::io::AsyncWriteExt as _;

    let (backend_addr, _backend_h) = spawn_drain_body_backend().await;
    let (gateway_addr, _gw_h) = spawn_gateway_over(backend_addr).await;

    let mut sock = TcpStream::connect(gateway_addr).await.unwrap();
    let head = format!(
        "POST /upload HTTP/1.1\r\nHost: {SAN_HOST}\r\nContent-Length: {OVER_CAP_BYTES}\r\n\r\n"
    );
    sock.write_all(head.as_bytes()).await.unwrap();

    // Stream the body in chunks. The gateway pump counts forwarded_total and
    // trips the cap before we finish; once it aborts the upstream + writes the
    // 413 status line, our writes may fail with a broken pipe — that is fine.
    let chunk = vec![0xABu8; 64 * 1024];
    let mut written = 0usize;
    while written < OVER_CAP_BYTES {
        let take = chunk.len().min(OVER_CAP_BYTES - written);
        if sock.write_all(&chunk[..take]).await.is_err() {
            break; // gateway already responded 413 and tore down the body
        }
        written += take;
    }
    let _ = sock.flush().await;

    let status = tokio::time::timeout(Duration::from_secs(20), read_status_code(&mut sock))
        .await
        .expect("client timed out waiting for gateway status");
    assert_eq!(
        status, 413,
        "over-cap Content-Length upload must yield 413 (pump verdict), not 502"
    );
}

/// F-CAP-1 (chunked framing): a >64 MiB chunked upload to a backend that is
/// still reading the body → client observes 413, not 502.
#[tokio::test]
async fn over_cap_chunked_upload_yields_413_not_502() {
    use tokio::io::AsyncWriteExt as _;

    let (backend_addr, _backend_h) = spawn_drain_body_backend().await;
    let (gateway_addr, _gw_h) = spawn_gateway_over(backend_addr).await;

    let mut sock = TcpStream::connect(gateway_addr).await.unwrap();
    let head =
        format!("POST /upload HTTP/1.1\r\nHost: {SAN_HOST}\r\nTransfer-Encoding: chunked\r\n\r\n");
    sock.write_all(head.as_bytes()).await.unwrap();

    // Each chunk: `<hexlen>\r\n<data>\r\n`. 64 KiB data chunks.
    let data = vec![0xCDu8; 64 * 1024];
    let chunk_header = format!("{:x}\r\n", data.len());
    let mut sent = 0usize;
    while sent < OVER_CAP_BYTES {
        if sock.write_all(chunk_header.as_bytes()).await.is_err() {
            break;
        }
        if sock.write_all(&data).await.is_err() {
            break;
        }
        if sock.write_all(b"\r\n").await.is_err() {
            break;
        }
        sent += data.len();
    }
    // (We never send the terminating `0\r\n\r\n` — the cap trips first.)
    let _ = sock.flush().await;

    let status = tokio::time::timeout(Duration::from_secs(20), read_status_code(&mut sock))
        .await
        .expect("client timed out waiting for gateway status");
    assert_eq!(
        status, 413,
        "over-cap chunked upload must yield 413 (pump verdict), not 502"
    );
}

/// F-CAP-1 (forbidden trailer): a chunked request whose trailer carries a
/// forbidden framing field (`Transfer-Encoding`) to a backend still reading
/// the body → client observes 400, not 502.
#[tokio::test]
async fn forbidden_framing_trailer_yields_400_not_502() {
    use tokio::io::AsyncWriteExt as _;

    let (backend_addr, _backend_h) = spawn_drain_body_backend().await;
    let (gateway_addr, _gw_h) = spawn_gateway_over(backend_addr).await;

    let mut sock = TcpStream::connect(gateway_addr).await.unwrap();
    // Small chunked body + a trailer carrying a forbidden framing field.
    // `TE: trailers` signals trailer-awareness; the trailer section then
    // smuggles `Transfer-Encoding: chunked`, which the gateway must reject.
    let req = "POST /upload HTTP/1.1\r\n\
               Host: expressgateway.test\r\n\
               TE: trailers\r\n\
               Trailer: Transfer-Encoding\r\n\
               Transfer-Encoding: chunked\r\n\r\n\
               5\r\nhello\r\n\
               0\r\n\
               Transfer-Encoding: chunked\r\n\r\n";
    sock.write_all(req.as_bytes()).await.unwrap();
    let _ = sock.flush().await;

    let status = tokio::time::timeout(Duration::from_secs(20), read_status_code(&mut sock))
        .await
        .expect("client timed out waiting for gateway status");
    assert_eq!(
        status, 400,
        "forbidden-framing-field trailer must yield 400 (pump verdict), not 502"
    );
}

/// F-CAP-1 (negative / no-regression): a GENUINE upstream failure (backend
/// drops the connection without speaking HTTP) is NOT pump-caused, so it must
/// still map to 502 — the verdict-first logic must not spuriously emit
/// 413/400 here.
#[tokio::test]
async fn genuine_upstream_failure_still_yields_502() {
    use tokio::io::AsyncWriteExt as _;

    let (backend_addr, _backend_h) = spawn_rst_backend().await;
    let (gateway_addr, _gw_h) = spawn_gateway_over(backend_addr).await;

    let mut sock = TcpStream::connect(gateway_addr).await.unwrap();
    // A small, well-formed request (within the cap, no forbidden trailer) so
    // the only failure is the upstream dropping the connection.
    let req = "POST /ok HTTP/1.1\r\n\
               Host: expressgateway.test\r\n\
               Content-Length: 5\r\n\r\n\
               hello";
    sock.write_all(req.as_bytes()).await.unwrap();
    let _ = sock.flush().await;

    let status = tokio::time::timeout(Duration::from_secs(20), read_status_code(&mut sock))
        .await
        .expect("client timed out waiting for gateway status");
    assert_eq!(
        status, 502,
        "a genuine upstream failure must still map to 502 (not a spurious 413/400)"
    );
}
