//! End-to-end tests for Pillar 3b.3b-2 — the hyper 1.x H2 proxy with ALPN.
//!
//! These tests drive the full listener stack: TCP accept → rustls
//! TlsAcceptor (with ALPN advertising both `h2` and `http/1.1`) →
//! `H1Proxy` or `H2Proxy` depending on the negotiated protocol. The
//! client is `reqwest` so we exercise a production-shape H2 client
//! (including HPACK, SETTINGS, multiplexing).

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1 as srv_h1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{AltSvcConfig, H1Proxy, HttpTimeouts, RoundRobinAddrs};
use lb_l7::h2_proxy::H2Proxy;
use lb_security::TicketRotator;
use lb_security::build_server_config;
use parking_lot::Mutex;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

const SAN_HOST: &str = "expressgateway.test";

// ── Shared helpers ─────────────────────────────────────────────────────

fn make_cert_for(san: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
    let cert_der = generated.cert.der().to_vec();
    let key_der = generated.key_pair.serialize_der();
    (
        vec![CertificateDer::from(cert_der)],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
    )
}

/// Build the shared rustls `ServerConfig` with both h2 and http/1.1 ALPN
/// so the runtime can exercise its negotiation path.
fn build_server_cfg_with_alpn(
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Arc<rustls::ServerConfig> {
    let rot = TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
    let rot_arc = Arc::new(Mutex::new(rot));
    let alpn: &[&[u8]] = &[b"h2", b"http/1.1"];
    build_server_config(rot_arc, cert_chain, key, alpn).unwrap()
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

/// Build a reqwest client that trusts `trust_anchor`, resolves
/// `SAN_HOST` to the given gateway address, and negotiates ALPN per
/// the supplied `alpn_override`.
fn build_reqwest_client(
    gateway_addr: SocketAddr,
    trust_anchor: &CertificateDer<'_>,
    http1_only: bool,
) -> reqwest::Client {
    let cert = reqwest::Certificate::from_der(trust_anchor.as_ref()).unwrap();
    let mut builder = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(cert)
        // Pin DNS so reqwest connects to the ephemeral gateway port
        // using the SAN the cert was issued for.
        .resolve(SAN_HOST, gateway_addr)
        // One connection per host so the per-stream-LB test sees a
        // single H2 connection multiplexing N requests.
        .pool_max_idle_per_host(1);
    if http1_only {
        builder = builder.http1_only();
    }
    builder.build().unwrap()
}

// ── Mock backends ───────────────────────────────────────────────────────

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

/// Spawn a counter backend that increments `counter` on every request.
async fn spawn_counter_backend(
    counter: Arc<AtomicUsize>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                let svc = service_fn(move |_req: Request<Incoming>| {
                    let counter = Arc::clone(&counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .body(Full::new(Bytes::from_static(b"hit")))
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
    (local, handle)
}

// ── Gateway harness with ALPN dispatch ──────────────────────────────────

/// Spawn an `h1s` gateway that dispatches by ALPN after TLS handshake:
///   h2 → H2Proxy, everything else → H1Proxy.
async fn spawn_h1s_gateway(
    server_cfg: Arc<rustls::ServerConfig>,
    h1_proxy: Arc<H1Proxy>,
    h2_proxy: Arc<H2Proxy>,
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
            let h1 = Arc::clone(&h1_proxy);
            let h2 = Arc::clone(&h2_proxy);
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(sock).await else {
                    return;
                };
                let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                if alpn.as_deref() == Some(b"h2".as_ref()) {
                    let _ = h2.serve_connection(tls, peer).await;
                } else {
                    let _ = h1.serve_connection(tls, peer).await;
                }
            });
        }
    });
    (local, handle)
}

// ── Test 1: e2e GET over H2 with Alt-Svc injection ─────────────────────

#[tokio::test]
async fn h2s_proxy_returns_backend_response_with_alt_svc() {
    let (backend_addr, _bh) = spawn_static_backend(b"backend-ok").await;

    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let alt_svc = AltSvcConfig {
        h3_port: 443,
        max_age: 3_600,
    };
    let h1_proxy = Arc::new(H1Proxy::new(
        pool.clone(),
        Arc::clone(&picker) as _,
        Some(alt_svc),
        HttpTimeouts::default(),
        true,
    ));
    let h2_proxy = Arc::new(H2Proxy::new(
        pool,
        picker as _,
        Some(alt_svc),
        HttpTimeouts::default(),
        true,
    ));

    let (cert_chain, key) = make_cert_for(SAN_HOST);
    let trust_anchor = cert_chain[0].clone();
    let server_cfg = build_server_cfg_with_alpn(cert_chain, key);
    let (gateway_addr, _gh) = spawn_h1s_gateway(server_cfg, h1_proxy, h2_proxy).await;

    let client = build_reqwest_client(gateway_addr, &trust_anchor, /* http1_only */ false);
    let resp = client
        .get(format!("https://{SAN_HOST}:{}/", gateway_addr.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(
        resp.version(),
        reqwest::Version::HTTP_2,
        "expected H2 after ALPN negotiation"
    );
    let alt = resp
        .headers()
        .get(reqwest::header::ALT_SVC)
        .expect("Alt-Svc missing")
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(alt, "h3=\":443\"; ma=3600");
    let body = resp.bytes().await.unwrap();
    assert_eq!(&body[..], b"backend-ok");
}

// ── Test 2: per-stream load balancing over a single H2 connection ──────

#[tokio::test]
async fn h2s_per_stream_load_balancing() {
    // 3 backends with per-backend request counters.
    let c0 = Arc::new(AtomicUsize::new(0));
    let c1 = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::new(AtomicUsize::new(0));
    let (a0, _h0) = spawn_counter_backend(Arc::clone(&c0)).await;
    let (a1, _h1) = spawn_counter_backend(Arc::clone(&c1)).await;
    let (a2, _h2) = spawn_counter_backend(Arc::clone(&c2)).await;

    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![a0, a1, a2]).unwrap());
    let h1_proxy = Arc::new(H1Proxy::new(
        pool.clone(),
        Arc::clone(&picker) as _,
        None,
        HttpTimeouts::default(),
        true,
    ));
    let h2_proxy = Arc::new(H2Proxy::new(
        pool,
        picker as _,
        None,
        HttpTimeouts::default(),
        true,
    ));

    let (cert_chain, key) = make_cert_for(SAN_HOST);
    let trust_anchor = cert_chain[0].clone();
    let server_cfg = build_server_cfg_with_alpn(cert_chain, key);
    let (gateway_addr, _gh) = spawn_h1s_gateway(server_cfg, h1_proxy, h2_proxy).await;

    // Single reqwest client (one H2 connection) firing 9 GETs.
    let client = build_reqwest_client(gateway_addr, &trust_anchor, /* http1_only */ false);

    let url = format!("https://{SAN_HOST}:{}/lb", gateway_addr.port());
    // Issue requests sequentially for determinism — reqwest over H2
    // multiplexes them on the same connection regardless.
    for _ in 0..9 {
        let resp = client.get(&url).send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        assert_eq!(resp.version(), reqwest::Version::HTTP_2);
        let _ = resp.bytes().await.unwrap();
    }

    let (n0, n1, n2) = (
        c0.load(Ordering::SeqCst),
        c1.load(Ordering::SeqCst),
        c2.load(Ordering::SeqCst),
    );
    assert_eq!(
        n0 + n1 + n2,
        9,
        "total request count mismatch: {n0}+{n1}+{n2}"
    );
    // Round-robin over 3 backends with 9 requests: exactly 3 each.
    assert_eq!(n0, 3, "backend 0 count: {n0}");
    assert_eq!(n1, 3, "backend 1 count: {n1}");
    assert_eq!(n2, 3, "backend 2 count: {n2}");
}

// ── Test 3: ALPN downgrade from h2 to http/1.1 ─────────────────────────

#[tokio::test]
async fn h2s_alpn_downgrade_to_http11() {
    let (backend_addr, _bh) = spawn_static_backend(b"backend-ok").await;

    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let h1_proxy = Arc::new(H1Proxy::new(
        pool.clone(),
        Arc::clone(&picker) as _,
        None,
        HttpTimeouts::default(),
        true,
    ));
    let h2_proxy = Arc::new(H2Proxy::new(
        pool,
        picker as _,
        None,
        HttpTimeouts::default(),
        true,
    ));

    let (cert_chain, key) = make_cert_for(SAN_HOST);
    let trust_anchor = cert_chain[0].clone();
    let server_cfg = build_server_cfg_with_alpn(cert_chain, key);
    let (gateway_addr, _gh) = spawn_h1s_gateway(server_cfg, h1_proxy, h2_proxy).await;

    // Client locked to HTTP/1.1 — ALPN must downgrade server-side.
    let client = build_reqwest_client(gateway_addr, &trust_anchor, /* http1_only */ true);
    let resp = client
        .get(format!("https://{SAN_HOST}:{}/", gateway_addr.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(
        resp.version(),
        reqwest::Version::HTTP_11,
        "expected H1 after ALPN downgrade"
    );
    let body = resp.bytes().await.unwrap();
    assert_eq!(&body[..], b"backend-ok");
}

// ── F-CAP-1: pump verdict authoritative over a send_request error (H2→H1) ──
//
// Same conformance fix as the H1 cell: when the M-D streaming pump aborts the
// upstream over the 64 MiB cap, the upstream `send_request` fails because the
// backend (still reading the body) never sent a response head. The caller must
// return the pump's classified 413 verdict, NOT 502 — deterministically.

/// Backend that drains the whole request body and replies `200 OK` only on a
/// clean end-of-body. For an over-cap upload the gateway pump aborts the
/// upstream request mid-body, so this backend never sends a response head —
/// the F-CAP-1 condition (client must still see 413 from the pump verdict).
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
                    use http_body_util::BodyExt as _;
                    let mut body = req.into_body();
                    while let Some(next) = body.frame().await {
                        if next.is_err() {
                            // gateway aborted the upstream request body.
                            return Ok::<_, Infallible>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .body(Full::new(Bytes::from_static(b"ok")))
                                    .unwrap(),
                            );
                        }
                    }
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(Full::new(Bytes::from_static(b"ok")))
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

const OVER_CAP_BYTES: usize = 64 * 1024 * 1024 + 64 * 1024; // > MAX_REQUEST_BODY_BYTES

/// F-CAP-1 (H2→H1): a >64 MiB upload over H2 to a backend still reading the
/// body → the client observes 413, not 502. Exercises the M-D streaming pump's
/// mid-stream `BodyTooLarge` verdict surfacing through the F-CAP-1 caller-side
/// fix even though the backend never sent a response head.
#[tokio::test]
async fn h2_over_cap_upload_yields_413_not_502() {
    let (backend_addr, _bh) = spawn_drain_body_backend().await;

    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    // Generous body timeout so the F-CAP-1 bounded verdict await resolves on
    // the pump verdict, not on elapse.
    let timeouts = HttpTimeouts {
        header: Duration::from_secs(20),
        body: Duration::from_secs(20),
        total: Duration::from_secs(60),
        head: Duration::from_secs(60),
    };
    let h1_proxy = Arc::new(H1Proxy::new(
        pool.clone(),
        Arc::clone(&picker) as _,
        None,
        timeouts,
        true,
    ));
    let h2_proxy = Arc::new(H2Proxy::new(pool, picker as _, None, timeouts, true));

    let (cert_chain, key) = make_cert_for(SAN_HOST);
    let trust_anchor = cert_chain[0].clone();
    let server_cfg = build_server_cfg_with_alpn(cert_chain, key);
    let (gateway_addr, _gh) = spawn_h1s_gateway(server_cfg, h1_proxy, h2_proxy).await;

    let client = build_reqwest_client(gateway_addr, &trust_anchor, /* http1_only */ false);
    // A >64 MiB body. reqwest's H2 stack streams it respecting flow control;
    // the gateway pump counts past the cap and aborts the upstream.
    let body = vec![0xABu8; OVER_CAP_BYTES];
    let resp = tokio::time::timeout(
        Duration::from_secs(40),
        client
            .post(format!("https://{SAN_HOST}:{}/upload", gateway_addr.port()))
            .body(body)
            .send(),
    )
    .await
    .expect("client timed out waiting for gateway response")
    .expect("gateway did not return a response");
    assert_eq!(
        resp.version(),
        reqwest::Version::HTTP_2,
        "expected H2 after ALPN negotiation"
    );
    assert_eq!(
        resp.status().as_u16(),
        413,
        "over-cap H2 upload must yield 413 (pump verdict), not 502"
    );
}

/// F-CAP-1 (H2→H1 negative): a GENUINE upstream failure (backend drops the
/// connection without speaking HTTP) is NOT pump-caused → must still map to
/// 502, never a spurious 413/400.
#[tokio::test]
async fn h2_genuine_upstream_failure_still_yields_502() {
    // Backend that accepts then immediately drops the socket.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let backend_addr = listener.local_addr().unwrap();
    let _bh = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            drop(sock);
        }
    });

    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let timeouts = HttpTimeouts {
        header: Duration::from_secs(20),
        body: Duration::from_secs(20),
        total: Duration::from_secs(60),
        head: Duration::from_secs(60),
    };
    let h1_proxy = Arc::new(H1Proxy::new(
        pool.clone(),
        Arc::clone(&picker) as _,
        None,
        timeouts,
        true,
    ));
    let h2_proxy = Arc::new(H2Proxy::new(pool, picker as _, None, timeouts, true));

    let (cert_chain, key) = make_cert_for(SAN_HOST);
    let trust_anchor = cert_chain[0].clone();
    let server_cfg = build_server_cfg_with_alpn(cert_chain, key);
    let (gateway_addr, _gh) = spawn_h1s_gateway(server_cfg, h1_proxy, h2_proxy).await;

    let client = build_reqwest_client(gateway_addr, &trust_anchor, /* http1_only */ false);
    // Small, well-formed body within the cap; the only failure is the upstream.
    let resp = tokio::time::timeout(
        Duration::from_secs(40),
        client
            .post(format!("https://{SAN_HOST}:{}/ok", gateway_addr.port()))
            .body(vec![0u8; 5])
            .send(),
    )
    .await
    .expect("client timed out waiting for gateway response")
    .expect("gateway did not return a response");
    assert_eq!(
        resp.status().as_u16(),
        502,
        "a genuine upstream failure must still map to 502 (not a spurious 413/400)"
    );
}
