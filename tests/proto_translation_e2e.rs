//! PROTO-001 — 5 real-wire protocol-translation paths.
//!
//! Each test spins up a real backend server, a real gateway listener
//! wired to dispatch upstream via the right pool, and a real client.
//! The tests prove the gateway's bridge is correct end-to-end:
//! response status + body propagate verbatim, and the upstream server
//! observes the synthesised pseudo-headers / status-line that the
//! protocol translation produced.
//!
//! The 5 paths covered:
//!
//! 1. H1 listener → H2 backend
//! 2. H2 listener → H2 backend
//! 3. H1 listener → H3 backend
//! 4. H2 listener → H3 backend
//! 5. H3 listener → H2 backend
//!
//! Codec-level translation is exercised by the 9 `bridging_*.rs` tests
//! already in this directory (PROTO-001 frozen those). This file
//! exercises the *wiring* — the Http2Pool / QuicUpstreamPool dispatch
//! plus the URL/header reconstruction in `lb_l7::h1_proxy` and
//! `lb_l7::h2_proxy` and the `h3_to_h2_roundtrip` adapter in
//! `lb_quic::h3_bridge`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::server::conn::http2 as srv_h2;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use lb_io::Runtime;
use lb_io::http2_pool::{Http2Pool, Http2PoolConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::quic_pool::{QuicPoolConfig, QuicUpstreamPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts};
use lb_l7::h2_proxy::H2Proxy;
use lb_l7::h2_security::H2SecurityThresholds;
use lb_l7::upstream::{RoundRobinUpstreams, UpstreamBackend};
use lb_quic::{QuicListener, QuicListenerParams};
use tokio::net::{TcpListener, UdpSocket};
use tokio_util::sync::CancellationToken;

use lb_h3::{H3Frame, QpackDecoder, QpackEncoder, decode_frame, encode_frame};

const TEST_SNI: &str = "expressgateway.test";
const LB_QUIC_ALPN: &[u8] = b"lb-quic";
const MAX_UDP: usize = 65_535;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

// ── Shared cert + helpers ──────────────────────────────────────────────

struct TestCerts {
    _dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    ca: PathBuf,
    retry: PathBuf,
}

impl Drop for TestCerts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

fn generate_loopback_certs() -> TestCerts {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "proto-translation-e2e-{}-{}-{counter}",
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let mut params = rcgen::CertificateParams::new(vec![TEST_SNI.to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
    let key_pair = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ca_path = dir.join("ca.pem");
    let retry_path = dir.join("retry.key");
    std::fs::write(&cert_path, cert_pem.as_bytes()).unwrap();
    std::fs::write(&key_path, key_pem.as_bytes()).unwrap();
    std::fs::write(&ca_path, cert_pem.as_bytes()).unwrap();
    TestCerts {
        _dir: dir,
        cert: cert_path,
        key: key_path,
        ca: ca_path,
        retry: retry_path,
    }
}

fn build_tcp_pool() -> TcpPool {
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

fn random_scid_bytes() -> [u8; quiche::MAX_CONN_ID_LEN] {
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new().fill(&mut scid).unwrap();
    scid
}

// ── Plaintext H2 backend (Tests 1, 2, 5) ───────────────────────────────

/// Spawn a plaintext H2 (h2c) echo backend that returns the request
/// body verbatim with status 200 and an `x-backend-tag` header so the
/// test can assert which backend handled the request.
async fn spawn_h2_echo_backend(tag: &'static str) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| async move {
                    let method = req.method().clone();
                    let path = req.uri().path().to_string();
                    let body_bytes = req
                        .into_body()
                        .collect()
                        .await
                        .map(|c| c.to_bytes())
                        .unwrap_or_default();
                    let body_out = if body_bytes.is_empty() {
                        Bytes::from(format!("h2-backend({tag}):{method} {path}"))
                    } else {
                        body_bytes
                    };
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("x-backend-tag", tag)
                            .header("x-backend-proto", "h2c")
                            .body(Full::new(body_out))
                            .unwrap(),
                    )
                });
                let _ = srv_h2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, handle)
}

// ── H3 (quiche) backend (Tests 3, 4) ───────────────────────────────────

/// Spawn a minimal H3 responder that listens on a UDP socket, accepts
/// one connection (no RETRY), reads the first HEADERS frame on bidi
/// stream 0, and writes a 200 + the configured body back. Returns the
/// listener's local UDP address.
async fn spawn_h3_static_backend(
    cert_path: PathBuf,
    key_path: PathBuf,
    body: &'static [u8],
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = socket.local_addr().unwrap();
    let socket = Arc::new(socket);
    let body_len = body.len();

    let handle = tokio::spawn(async move {
        let Ok(mut cfg) = quiche::Config::new(quiche::PROTOCOL_VERSION) else {
            return;
        };
        let _ = cfg.set_application_protos(&[LB_QUIC_ALPN]);
        cfg.set_max_idle_timeout(5_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(4);
        cfg.set_initial_max_streams_uni(4);
        cfg.set_disable_active_migration(true);
        if cfg
            .load_cert_chain_from_pem_file(cert_path.to_str().unwrap_or(""))
            .is_err()
        {
            return;
        }
        if cfg
            .load_priv_key_from_pem_file(key_path.to_str().unwrap_or(""))
            .is_err()
        {
            return;
        }

        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];

        let (n, peer) = match socket.recv_from(&mut in_buf).await {
            Ok(v) => v,
            Err(_) => return,
        };
        let scid = random_scid_bytes();
        let scid_ref = quiche::ConnectionId::from_ref(&scid);
        let mut conn = match quiche::accept(&scid_ref, None, addr, peer, &mut cfg) {
            Ok(c) => c,
            Err(_) => return,
        };
        let info = quiche::RecvInfo {
            from: peer,
            to: addr,
        };
        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
        let _ = conn.recv(slice, info);

        let mut rx_tail: Vec<u8> = Vec::new();
        let mut responded = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);

        while tokio::time::Instant::now() < deadline {
            loop {
                match conn.send(&mut out_buf) {
                    Ok((sent, info)) => {
                        let _ = socket
                            .send_to(out_buf.get(..sent).unwrap_or(&[]), info.to)
                            .await;
                    }
                    Err(quiche::Error::Done) => break,
                    Err(_) => break,
                }
            }
            if conn.is_closed() {
                break;
            }

            if conn.is_established() && !responded {
                let readable: Vec<u64> = conn.readable().collect();
                for sid in readable {
                    if sid != 0 {
                        continue;
                    }
                    let mut chunk = [0u8; 8192];
                    loop {
                        match conn.stream_recv(sid, &mut chunk) {
                            Ok((rn, _fin)) => {
                                rx_tail.extend_from_slice(chunk.get(..rn).unwrap_or(&[]));
                            }
                            Err(quiche::Error::Done)
                            | Err(quiche::Error::InvalidStreamState(_)) => break,
                            Err(_) => break,
                        }
                    }
                }
                if let Ok((H3Frame::Headers { .. }, _)) = decode_frame(&rx_tail, 1 << 20) {
                    let encoder = QpackEncoder::new();
                    let resp_headers: Vec<(String, String)> = vec![
                        (":status".to_string(), "200".to_string()),
                        ("content-length".to_string(), body_len.to_string()),
                        ("x-backend-tag".to_string(), "h3-backend".to_string()),
                    ];
                    if let Ok(block) = encoder.encode(&resp_headers) {
                        if let Ok(hframe) = encode_frame(&H3Frame::Headers {
                            header_block: block,
                        }) {
                            if let Ok(dframe) = encode_frame(&H3Frame::Data {
                                payload: Bytes::from_static(body),
                            }) {
                                let mut out = Vec::with_capacity(hframe.len() + dframe.len());
                                out.extend_from_slice(&hframe);
                                out.extend_from_slice(&dframe);
                                let mut p = 0;
                                while p < out.len() {
                                    let sl = out.get(p..).unwrap_or(&[]);
                                    let fin = p + sl.len() >= out.len();
                                    match conn.stream_send(0, sl, fin) {
                                        Ok(ns) => p += ns,
                                        Err(_) => break,
                                    }
                                }
                                responded = true;
                            }
                        }
                    }
                }
            }

            let timeout = conn.timeout().unwrap_or(Duration::from_millis(50));
            match tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((n, from))) => {
                    let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                    let info = quiche::RecvInfo { from, to: addr };
                    let _ = conn.recv(slice, info);
                }
                Ok(Err(_)) | Err(_) => {
                    conn.on_timeout();
                }
            }
        }
    });
    (addr, handle)
}

fn build_quic_client_config_factory(
    ca_path: PathBuf,
) -> Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> {
    Arc::new(move || {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
        cfg.set_application_protos(&[LB_QUIC_ALPN])?;
        cfg.load_verify_locations_from_file(ca_path.to_str().unwrap_or(""))?;
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(5_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(4);
        cfg.set_initial_max_streams_uni(4);
        cfg.set_disable_active_migration(true);
        Ok(cfg)
    })
}

// ── Test 1: H1 listener → H2 backend ───────────────────────────────────

#[tokio::test]
async fn proxy_h1_listener_h2_backend() {
    let (backend_addr, _bh) = spawn_h2_echo_backend("t1").await;

    let tcp_pool = build_tcp_pool();
    let h2_pool = Arc::new(Http2Pool::new(Http2PoolConfig::default(), tcp_pool.clone()));
    let picker =
        Arc::new(RoundRobinUpstreams::new(vec![UpstreamBackend::h2(backend_addr)]).unwrap());
    let h1_proxy = Arc::new(
        H1Proxy::with_multi_proto(
            tcp_pool,
            picker,
            None,
            HttpTimeouts::default(),
            /* is_https = */ false,
        )
        .with_h2_upstream(Arc::clone(&h2_pool)),
    );

    // Plain-TCP H1 listener.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let gateway_addr = listener.local_addr().unwrap();
    let h1c = Arc::clone(&h1_proxy);
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let p = Arc::clone(&h1c);
            tokio::spawn(async move {
                let _ = p.serve_connection(sock, peer).await;
            });
        }
    });

    // hyper H1 client.
    let stream = tokio::net::TcpStream::connect(gateway_addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("GET")
        .uri("/test/h1-h2")
        .header("host", TEST_SNI)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let tag = resp
        .headers()
        .get("x-backend-tag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(tag, "t1", "missing/wrong x-backend-tag header");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let s = std::str::from_utf8(&body).unwrap_or_default();
    // The H2 echo backend received an upstream-side request whose
    // path was preserved through the H1→H2 codec bridge.
    assert!(
        s.contains("/test/h1-h2"),
        "backend response did not echo path: {s:?}"
    );
}

// ── Test 2: H2 listener → H2 backend ───────────────────────────────────
//
// Both client→gateway and gateway→backend speak plaintext H2 (h2c) so
// the test does not need TLS+ALPN bring-up — the codec translation
// path is identical to the TLS variant.

#[tokio::test]
async fn proxy_h2_listener_h2_backend() {
    let (backend_addr, _bh) = spawn_h2_echo_backend("t2").await;

    let tcp_pool = build_tcp_pool();
    let h2_pool = Arc::new(Http2Pool::new(Http2PoolConfig::default(), tcp_pool.clone()));
    let picker =
        Arc::new(RoundRobinUpstreams::new(vec![UpstreamBackend::h2(backend_addr)]).unwrap());
    let h2_proxy = Arc::new(
        H2Proxy::with_multi_proto(
            tcp_pool,
            picker,
            None,
            HttpTimeouts::default(),
            /* is_https = */ false,
            H2SecurityThresholds::default(),
        )
        .with_h2_upstream(Arc::clone(&h2_pool)),
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let gateway_addr = listener.local_addr().unwrap();
    let p = Arc::clone(&h2_proxy);
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let pp = Arc::clone(&p);
            tokio::spawn(async move {
                let _ = pp.serve_connection(sock, peer).await;
            });
        }
    });

    // hyper plaintext H2 client.
    let stream = tokio::net::TcpStream::connect(gateway_addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http2::handshake::<
        _,
        _,
        http_body_util::combinators::BoxBody<Bytes, hyper::Error>,
    >(TokioExecutor::new(), TokioIo::new(stream))
    .await
    .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let body = Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("GET")
        .uri(format!("http://{TEST_SNI}/test/h2-h2"))
        .body(body)
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let tag = resp
        .headers()
        .get("x-backend-tag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(tag, "t2");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let s = std::str::from_utf8(&body).unwrap_or_default();
    assert!(s.contains("/test/h2-h2"), "echo body: {s:?}");
}

// ── Test 3: H1 listener → H3 backend ───────────────────────────────────

#[tokio::test]
async fn proxy_h1_listener_h3_backend() {
    let certs = generate_loopback_certs();
    let (backend_addr, _bh) =
        spawn_h3_static_backend(certs.cert.clone(), certs.key.clone(), b"hello-h3").await;

    let factory = build_quic_client_config_factory(certs.ca.clone());
    let h3_pool = Arc::new(QuicUpstreamPool::new(QuicPoolConfig::default(), factory));
    let tcp_pool = build_tcp_pool();
    let picker = Arc::new(
        RoundRobinUpstreams::new(vec![UpstreamBackend::h3(backend_addr, TEST_SNI)]).unwrap(),
    );
    let h1_proxy = Arc::new(
        H1Proxy::with_multi_proto(
            tcp_pool,
            picker,
            None,
            HttpTimeouts::default(),
            /* is_https = */ false,
        )
        .with_h3_upstream(Arc::clone(&h3_pool)),
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let gateway_addr = listener.local_addr().unwrap();
    let h1c = Arc::clone(&h1_proxy);
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let p = Arc::clone(&h1c);
            tokio::spawn(async move {
                let _ = p.serve_connection(sock, peer).await;
            });
        }
    });

    let stream = tokio::net::TcpStream::connect(gateway_addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method("GET")
        .uri("/")
        .header("host", TEST_SNI)
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"hello-h3", "expected H3 backend body");
}

// ── Test 4: H2 listener → H3 backend ───────────────────────────────────

#[tokio::test]
async fn proxy_h2_listener_h3_backend() {
    let certs = generate_loopback_certs();
    let (backend_addr, _bh) =
        spawn_h3_static_backend(certs.cert.clone(), certs.key.clone(), b"hello-h3-from-h2").await;

    let factory = build_quic_client_config_factory(certs.ca.clone());
    let h3_pool = Arc::new(QuicUpstreamPool::new(QuicPoolConfig::default(), factory));
    let tcp_pool = build_tcp_pool();
    let picker = Arc::new(
        RoundRobinUpstreams::new(vec![UpstreamBackend::h3(backend_addr, TEST_SNI)]).unwrap(),
    );
    let h2_proxy = Arc::new(
        H2Proxy::with_multi_proto(
            tcp_pool,
            picker,
            None,
            HttpTimeouts::default(),
            /* is_https = */ false,
            H2SecurityThresholds::default(),
        )
        .with_h3_upstream(Arc::clone(&h3_pool)),
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let gateway_addr = listener.local_addr().unwrap();
    let p = Arc::clone(&h2_proxy);
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let pp = Arc::clone(&p);
            tokio::spawn(async move {
                let _ = pp.serve_connection(sock, peer).await;
            });
        }
    });

    let stream = tokio::net::TcpStream::connect(gateway_addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http2::handshake::<
        _,
        _,
        http_body_util::combinators::BoxBody<Bytes, hyper::Error>,
    >(TokioExecutor::new(), TokioIo::new(stream))
    .await
    .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let body = Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed();
    let req = Request::builder()
        .method("GET")
        .uri(format!("http://{TEST_SNI}/"))
        .body(body)
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"hello-h3-from-h2");
}

// ── Test 5: H3 listener → H2 backend ───────────────────────────────────

#[tokio::test]
async fn proxy_h3_listener_h2_backend() {
    let certs = generate_loopback_certs();
    let (backend_addr, _bh) = spawn_h2_echo_backend("t5").await;

    let tcp_pool = build_tcp_pool();
    let h2_pool = Http2Pool::new(Http2PoolConfig::default(), tcp_pool);

    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    )
    .with_h2_backend(h2_pool, backend_addr);

    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone()).await.unwrap();
    let gateway_addr = listener.local_addr();

    // Drive an H3 GET against the gateway. We reuse the inline driver
    // pattern from quic_listener_e2e.rs.
    let client_sock = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
    let client_local = client_sock.local_addr().unwrap();
    let mut client_cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    client_cfg.set_application_protos(&[LB_QUIC_ALPN]).unwrap();
    client_cfg
        .load_verify_locations_from_file(certs.ca.to_str().unwrap_or(""))
        .unwrap();
    client_cfg.verify_peer(true);
    client_cfg.set_max_idle_timeout(5_000);
    client_cfg.set_max_recv_udp_payload_size(1_350);
    client_cfg.set_max_send_udp_payload_size(1_350);
    client_cfg.set_initial_max_data(1024 * 1024);
    client_cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
    client_cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
    client_cfg.set_initial_max_stream_data_uni(64 * 1024);
    client_cfg.set_initial_max_streams_bidi(4);
    client_cfg.set_initial_max_streams_uni(4);
    client_cfg.set_disable_active_migration(true);

    let scid = random_scid_bytes();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::connect(
        Some(TEST_SNI),
        &scid_ref,
        client_local,
        gateway_addr,
        &mut client_cfg,
    )
    .unwrap();

    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let mut request_sent = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut decoded_status: Option<u16> = None;
    let mut decoded_body: Vec<u8> = Vec::new();
    let mut expected_len: Option<usize> = None;
    let stream_id: u64 = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

    while tokio::time::Instant::now() < deadline {
        if !request_sent && conn.is_established() {
            let encoder = QpackEncoder::new();
            let headers = vec![
                (":method".to_string(), "GET".to_string()),
                (":scheme".to_string(), "https".to_string()),
                (":authority".to_string(), TEST_SNI.to_string()),
                (":path".to_string(), "/test/h3-h2".to_string()),
            ];
            let block = encoder.encode(&headers).unwrap();
            let frame = encode_frame(&H3Frame::Headers {
                header_block: block,
            })
            .unwrap();
            let mut p = 0;
            while p < frame.len() {
                let chunk = frame.get(p..).unwrap_or(&[]);
                let fin = p + chunk.len() >= frame.len();
                match conn.stream_send(stream_id, chunk, fin) {
                    Ok(n) => p += n,
                    Err(_) => break,
                }
            }
            request_sent = true;
        }

        if conn.is_established() {
            let readable: Vec<u64> = conn.readable().collect();
            for sid in readable {
                if sid != stream_id {
                    continue;
                }
                let mut chunk = [0u8; 8192];
                while let Ok((n, _fin)) = conn.stream_recv(sid, &mut chunk) {
                    rx_tail.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                }
            }
            loop {
                match decode_frame(&rx_tail, 1 << 20) {
                    Ok((H3Frame::Headers { header_block }, consumed)) => {
                        rx_tail.drain(..consumed);
                        let dec = QpackDecoder::new();
                        let hdrs = dec.decode(&header_block).unwrap();
                        for (n, v) in hdrs {
                            if n == ":status" {
                                decoded_status = v.parse::<u16>().ok();
                            } else if n == "content-length" {
                                expected_len = v.parse::<usize>().ok();
                            }
                        }
                    }
                    Ok((H3Frame::Data { payload }, consumed)) => {
                        rx_tail.drain(..consumed);
                        decoded_body.extend_from_slice(&payload);
                    }
                    Ok((_other, consumed)) => {
                        rx_tail.drain(..consumed);
                    }
                    Err(lb_h3::H3Error::Incomplete) => break,
                    Err(_) => break,
                }
            }
        }

        if let Some(_status) = decoded_status {
            let cl_done = expected_len.is_some_and(|cl| decoded_body.len() >= cl);
            if cl_done || (expected_len.is_none() && !decoded_body.is_empty()) {
                break;
            }
        }

        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    let _ = client_sock
                        .send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                        .await;
                }
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }

        let timeout = conn.timeout().unwrap_or(Duration::from_millis(50));
        match tokio::time::timeout(timeout, client_sock.recv_from(&mut in_buf)).await {
            Ok(Ok((n, from))) => {
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                let info = quiche::RecvInfo {
                    from,
                    to: client_local,
                };
                let _ = conn.recv(slice, info);
            }
            Ok(Err(_)) | Err(_) => {
                conn.on_timeout();
            }
        }
    }

    // Shut listener down so the test can exit cleanly.
    let join = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), join).await;

    assert_eq!(
        decoded_status,
        Some(200),
        "expected 200 status from H2 backend"
    );
    let body_str = std::str::from_utf8(&decoded_body).unwrap_or_default();
    assert!(
        body_str.contains("/test/h3-h2"),
        "expected backend echo of path; got: {body_str:?}"
    );
}
