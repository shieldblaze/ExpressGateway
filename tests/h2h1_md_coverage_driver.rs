//! S8 / M-D (H2→H1) — COVERAGE DRIVER (verifier).
//!
//! This file exists ONLY to drive the M-D session code (the bounded
//! ingress pump in `crates/lb-l7/src/h2_proxy.rs::proxy_request` plus
//! `validate_request_trailers` / `concat_chunks` / `record_retained`) over
//! the real wire for an INDEPENDENT `cargo llvm-cov` measurement, with a
//! CLEAN process exit (no panics) so the lcov report is produced.
//!
//! It deliberately makes NO behavioral assertions about correctness — the
//! authoritative PASS/FAIL judgement (incl. the Branch-B 413 defect, the
//! Deviation #1 regression, the gauge non-vacuousness) lives in
//! `tests/h2h1_md_streaming_verify.rs`, which is intentionally RED to prove
//! the NOT-BUILT verdict. Here we only EXERCISE the code paths so coverage
//! attributes the executed lines; statuses are merely logged.
//!
//! Paths driven:
//!   - Branch A (within-window): small binary body round-trip.
//!   - Branch B (over-window): large binary body (hits the streaming pump,
//!     send_data! macro, the verdict gate, BodyTooLarge mapping).
//!   - over-window malformed trailers + content-length mismatch (raw frames).
//!   - over-window early-response (Deviation #1 path).
//!   - stalled backend (memory gauge record points, backpressure park).
//!   - client RST mid-body (smuggling abort path).

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::BodyExt;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1 as srv_h1;
use hyper::service::service_fn;
use hyper::{Request as HyperRequest, Response as HyperResponse, StatusCode};
use hyper_util::rt::TokioIo;
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
use lb_l7::h2_proxy::H2Proxy;
use lb_l7::h2_security::H2SecurityThresholds;
use lb_security::{TicketRotator, build_server_config};
use parking_lot::Mutex;
use rustls::ClientConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector, client::TlsStream};

const SAN_HOST: &str = "expressgateway.test";
const H2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

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

async fn spawn_listener_for(backend_addr: SocketAddr) -> (SocketAddr, CertificateDer<'static>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let h1_proxy = Arc::new(H1Proxy::new(
        pool.clone(),
        Arc::clone(&picker) as _,
        None,
        HttpTimeouts::default(),
        true,
    ));
    let h2_proxy = Arc::new(H2Proxy::with_security(
        pool,
        picker as _,
        None,
        HttpTimeouts::default(),
        true,
        H2SecurityThresholds::default(),
    ));
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
    (local, trust_anchor)
}

async fn connect_tls(
    gateway: SocketAddr,
    trust_anchor: CertificateDer<'static>,
) -> TlsStream<TcpStream> {
    let sock = TcpStream::connect(gateway).await.unwrap();
    let connector = TlsConnector::from(build_client_cfg(trust_anchor));
    let sn = ServerName::try_from(SAN_HOST).unwrap();
    connector.connect(sn, sock).await.unwrap()
}

async fn spawn_echo_backend() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let svc = service_fn(|req: HyperRequest<Incoming>| async move {
                    let body = req.into_body().collect().await.map(|b| b.to_bytes());
                    Ok::<_, Infallible>(
                        HyperResponse::builder()
                            .status(StatusCode::OK)
                            .body(http_body_util::Full::new(body.unwrap_or_default()))
                            .unwrap(),
                    )
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    local
}

async fn spawn_early_401_backend() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut tmp = [0u8; 1024];
                let _ = sock.read(&mut tmp).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await;
                tokio::time::sleep(Duration::from_millis(300)).await;
            });
        }
    });
    local
}

async fn spawn_stalled_backend() -> (SocketAddr, Arc<tokio::sync::Notify>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let r2 = Arc::clone(&release);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let r3 = Arc::clone(&r2);
            tokio::spawn(async move {
                let mut tmp = [0u8; 256];
                let _ = sock.read(&mut tmp).await;
                r3.notified().await;
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    (local, release)
}

fn binary_pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + 17) % 256) as u8).collect()
}

async fn write_frame(s: &mut TlsStream<TcpStream>, ty: u8, flags: u8, sid: u32, payload: &[u8]) {
    let len = payload.len() as u32;
    let mut hdr = [0u8; 9];
    hdr[0] = ((len >> 16) & 0xff) as u8;
    hdr[1] = ((len >> 8) & 0xff) as u8;
    hdr[2] = (len & 0xff) as u8;
    hdr[3] = ty;
    hdr[4] = flags;
    let id = sid & 0x7fff_ffff;
    hdr[5] = ((id >> 24) & 0xff) as u8;
    hdr[6] = ((id >> 16) & 0xff) as u8;
    hdr[7] = ((id >> 8) & 0xff) as u8;
    hdr[8] = (id & 0xff) as u8;
    let _ = s.write_all(&hdr).await;
    let _ = s.write_all(payload).await;
    let _ = s.flush().await;
}

fn hpack_literal(name: &str, value: &str) -> Vec<u8> {
    let mut out = vec![0x00];
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
    out
}

async fn send_preface(s: &mut TlsStream<TcpStream>) {
    let _ = s.write_all(H2_PREFACE).await;
    write_frame(s, 0x04, 0x00, 0, &[]).await;
    write_frame(s, 0x04, 0x01, 0, &[]).await;
}

async fn drain_briefly(s: &mut TlsStream<TcpStream>) {
    let mut buf = [0u8; 16 * 1024];
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match s.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
    })
    .await;
}

/// Drive an h2-client upload of `body_len` bytes; log the status. No assert.
async fn drive_upload(body_len: usize, backend: SocketAddr, large_window: bool) {
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;
    let (h2, conn) = if large_window {
        let mut b = h2::client::Builder::new();
        b.initial_window_size(8 * 1024 * 1024)
            .initial_connection_window_size(8 * 1024 * 1024);
        b.handshake::<_, Bytes>(tls).await.unwrap()
    } else {
        h2::client::handshake::<_>(tls).await.unwrap()
    };
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/cov"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();
    let payload = binary_pattern(body_len);
    let writer = tokio::spawn(async move {
        let mut off = 0;
        while off < payload.len() {
            let end = (off + 16 * 1024).min(payload.len());
            let chunk = Bytes::copy_from_slice(&payload[off..end]);
            send_body.reserve_capacity(chunk.len());
            match tokio::time::timeout(
                Duration::from_secs(3),
                futures_util::future::poll_fn(|cx| send_body.poll_capacity(cx)),
            )
            .await
            {
                Ok(Some(Ok(cap))) if cap > 0 => {}
                _ => break,
            }
            if send_body.send_data(chunk, false).is_err() {
                break;
            }
            off = end;
        }
        let _ = send_body.send_data(Bytes::new(), true);
    });
    let status = match tokio::time::timeout(Duration::from_secs(15), resp_fut).await {
        Ok(Ok(resp)) => {
            let st = resp.status();
            // Drain the response body so the response leg executes too.
            // Bounded by an overall timeout so a never-ending stream cannot
            // wedge the test (the response leg is small in all cases).
            let mut body = resp.into_body();
            let _ = tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(d) = body.data().await {
                    if let Ok(b) = d {
                        let _ = body.flow_control().release_capacity(b.len());
                    }
                }
            })
            .await;
            Some(st)
        }
        _ => None,
    };
    // Bound the writer join — after a stream reset the h2 send half can
    // otherwise leave the task lingering.
    let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
    eprintln!("COV drive_upload len={body_len} status={status:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cov_branch_a_small_body() {
    let backend = spawn_echo_backend().await;
    drive_upload(8 * 1024, backend, false).await; // ≤ window → Branch A
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cov_branch_a_empty_and_exact_window() {
    let backend = spawn_echo_backend().await;
    drive_upload(0, backend, false).await; // empty body
    let backend2 = spawn_echo_backend().await;
    drive_upload(64 * 1024, backend2, false).await; // exactly window
}

/// Drive a >window upload far enough to enter Branch B (dial + pump +
/// send_data! + verdict gate) WITHOUT awaiting the response relay (which,
/// on this defective build, does not complete cleanly — the authoritative
/// behavioral judgement is in `h2h1_md_streaming_verify.rs`). We push the
/// body, give the pump time to run, then drop everything. No assert.
async fn drive_over_window_briefly(body_len: usize, backend: SocketAddr) {
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;
    let mut b = h2::client::Builder::new();
    b.initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = b.handshake::<_, Bytes>(tls).await.unwrap();
    let conn_task = tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/cov"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();
    let payload = binary_pattern(body_len);
    let writer = tokio::spawn(async move {
        let mut off = 0;
        while off < payload.len() {
            let end = (off + 16 * 1024).min(payload.len());
            let chunk = Bytes::copy_from_slice(&payload[off..end]);
            send_body.reserve_capacity(chunk.len());
            match tokio::time::timeout(
                Duration::from_secs(2),
                futures_util::future::poll_fn(|cx| send_body.poll_capacity(cx)),
            )
            .await
            {
                Ok(Some(Ok(cap))) if cap > 0 => {}
                _ => break,
            }
            if send_body.send_data(chunk, false).is_err() {
                break;
            }
            off = end;
        }
        let _ = send_body.send_data(Bytes::new(), true);
    });
    // Let the gateway dial + run the streaming pump + reach its verdict.
    let _ = tokio::time::timeout(Duration::from_secs(4), resp_fut).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    conn_task.abort();
    eprintln!("COV drive_over_window_briefly len={body_len} done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cov_branch_b_large_body() {
    let backend = spawn_echo_backend().await;
    drive_over_window_briefly(512 * 1024, backend).await; // > window → pump
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cov_branch_b_early_response() {
    let backend = spawn_early_401_backend().await;
    drive_over_window_briefly(256 * 1024, backend).await; // Deviation #1 path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cov_branch_b_stalled_backend() {
    let (backend, release) = spawn_stalled_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;
    let mut b = h2::client::Builder::new();
    b.initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = b.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/stall"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();
    let payload = binary_pattern(2 * 1024 * 1024);
    let writer = tokio::spawn(async move {
        let mut off = 0;
        while off < payload.len() {
            let end = (off + 16 * 1024).min(payload.len());
            let chunk = Bytes::copy_from_slice(&payload[off..end]);
            send_body.reserve_capacity(chunk.len());
            match tokio::time::timeout(
                Duration::from_secs(2),
                futures_util::future::poll_fn(|cx| send_body.poll_capacity(cx)),
            )
            .await
            {
                Ok(Some(Ok(cap))) if cap > 0 => {}
                _ => break,
            }
            if send_body.send_data(chunk, false).is_err() {
                break;
            }
            off = end;
        }
        let _ = send_body.send_data(Bytes::new(), true);
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    release.notify_waiters();
    let _ = tokio::time::timeout(Duration::from_secs(10), resp_fut).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
    eprintln!("COV stalled_backend done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cov_over_window_pseudo_trailers() {
    let backend = spawn_echo_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let mut s = connect_tls(gw, anchor).await;
    send_preface(&mut s).await;
    let mut hb = Vec::new();
    hb.extend_from_slice(&hpack_literal(":method", "POST"));
    hb.extend_from_slice(&hpack_literal(":scheme", "https"));
    hb.extend_from_slice(&hpack_literal(":path", "/big"));
    hb.extend_from_slice(&hpack_literal(":authority", SAN_HOST));
    write_frame(&mut s, 0x01, 0x04, 1, &hb).await;
    let chunk = vec![0xABu8; 8 * 1024];
    for _ in 0..16 {
        write_frame(&mut s, 0x00, 0x00, 1, &chunk).await;
    }
    let mut tb = Vec::new();
    tb.extend_from_slice(&hpack_literal("x-trailer", "ok"));
    tb.extend_from_slice(&hpack_literal(":status", "200"));
    write_frame(&mut s, 0x01, 0x05, 1, &tb).await;
    drain_briefly(&mut s).await;
    eprintln!("COV over_window_pseudo_trailers done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cov_over_window_cl_mismatch() {
    let backend = spawn_echo_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let mut s = connect_tls(gw, anchor).await;
    send_preface(&mut s).await;
    let mut hb = Vec::new();
    hb.extend_from_slice(&hpack_literal(":method", "POST"));
    hb.extend_from_slice(&hpack_literal(":scheme", "https"));
    hb.extend_from_slice(&hpack_literal(":path", "/big"));
    hb.extend_from_slice(&hpack_literal(":authority", SAN_HOST));
    hb.extend_from_slice(&hpack_literal("content-length", "999999"));
    write_frame(&mut s, 0x01, 0x04, 1, &hb).await;
    let chunk = vec![0xABu8; 8 * 1024];
    for _ in 0..16 {
        write_frame(&mut s, 0x00, 0x00, 1, &chunk).await;
    }
    write_frame(&mut s, 0x00, 0x01, 1, b"short").await;
    drain_briefly(&mut s).await;
    eprintln!("COV over_window_cl_mismatch done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cov_smuggling_rst_mid_body() {
    let backend = spawn_echo_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let mut s = connect_tls(gw, anchor).await;
    send_preface(&mut s).await;
    let mut hb = Vec::new();
    hb.extend_from_slice(&hpack_literal(":method", "POST"));
    hb.extend_from_slice(&hpack_literal(":scheme", "https"));
    hb.extend_from_slice(&hpack_literal(":path", "/big"));
    hb.extend_from_slice(&hpack_literal(":authority", SAN_HOST));
    write_frame(&mut s, 0x01, 0x04, 1, &hb).await;
    let chunk = vec![0xABu8; 8 * 1024];
    for _ in 0..16 {
        write_frame(&mut s, 0x00, 0x00, 1, &chunk).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    write_frame(&mut s, 0x03, 0x00, 1, &[0x00, 0x00, 0x00, 0x08]).await;
    drain_briefly(&mut s).await;
    eprintln!("COV smuggling_rst_mid_body done");
}
