//! S13 H2→H3 (R8) BUILT-bar — real-wire H2 client → real `H2Proxy` listener
//! (h3_upstream wired) → real H3/QUIC backend, exercising the STREAMING H2→H3
//! cell (`proxy_h2_to_h3` on `lb_quic::stream_request_to_h3_upstream`). Closes
//! the H-to-H matrix at 9/9.
//!
//! Mirrors `tests/h1h3_md_streaming_verify.rs` (the H1→H3 BUILT-bar) with two
//! H2-front deltas: the request truncation primitive is a genuine H2
//! `RST_STREAM` mid-body (Hazard (a), the `h2` crate) rather than a TCP drop,
//! and the response-trailer mandate is a POSITIVE assertion (H2 carries trailers
//! natively — hyper's H2 server encoder flushes `Frame::trailers` with no
//! `Trailer:` pre-declaration, so `grpc-status` MUST reach the H2 client; this is
//! the trailer-mandate WIN that H1→H3 could only record empirically).
//!
//! Coverage (plan §5 1-9):
//!  1. binary body byte-identical BOTH directions (~5 MiB, crosses
//!     `H3_BODY_CHUNK_MAX` ~640×).
//!  2. request-trailer FORWARDING to the H3 backend (`forward_req_trailers=true`).
//!  3. F-MD-4 REQUEST R13 a/b/c — Hazard (a): a downstream H2 client RST mid-body
//!     → the H3 backend sees `complete==0` (RESET-without-FIN), NEVER a
//!     truncated-as-complete request. Burst ≥50 on `current_thread`; LOAD-BEARING
//!     negative control (clean upload → `complete==1`).
//!  4. F-MD-4 RESPONSE R13 a/b/c — Hazard (b): a truncating H3 backend (partial
//!     DATA, no FIN) → the H2 client sees an errored/RST body, NOT a clean
//!     END_STREAM. The CHUNKED (no-CL) arm is the LOAD-BEARING one (guard is the
//!     sole discriminator); a CL-declared arm is kept non-load-bearing.
//!  5. F-CAP-1 — mid-body over-cap → RST (no 413, no clean END_STREAM); pre-dial
//!     down → 502.
//!  6. Trailer mandate gRPC-shaped POSITIVE assertion (`grpc-status` reaches the
//!     H2 client).
//!  7. Memory gauge non-vacuous + load-bearing inverted probe.
//!  8. Backpressure both legs.
//!
//! CF-SATURATION-1: every backend wait + client timeout is generous so 8-core
//! gate saturation cannot false-flake.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, StreamBody};
use hyper::Request;
use hyper::body::Frame;
use hyper_util::rt::{TokioExecutor, TokioIo};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::quic_pool::{QuicPoolConfig, QuicUpstreamPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::HttpTimeouts;
use lb_l7::h2_proxy::H2Proxy;
use lb_l7::h2_security::H2SecurityThresholds;
use lb_l7::upstream::{RoundRobinUpstreams, UpstreamBackend};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

use lb_h3::{H3Frame, QpackEncoder, encode_frame};

const TEST_SNI: &str = "expressgateway.test";
const LB_QUIC_ALPN: &[u8] = b"lb-quic";
const MAX_UDP: usize = 65_535;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

// ── Shared cert + pool + factory helpers (mirror h1h3_md_streaming_verify) ──

struct TestCerts {
    _dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    ca: PathBuf,
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
        "h2h3-md-verify-{}-{}-{counter}",
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
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ca_path = dir.join("ca.pem");
    std::fs::write(&cert_path, cert.pem().as_bytes()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem().as_bytes()).unwrap();
    std::fs::write(&ca_path, cert.pem().as_bytes()).unwrap();
    TestCerts {
        _dir: dir,
        cert: cert_path,
        key: key_path,
        ca: ca_path,
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
        cfg.set_initial_max_data(16 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(8 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(8 * 1024 * 1024);
        cfg.set_initial_max_stream_data_uni(1024 * 1024);
        cfg.set_initial_max_streams_bidi(16);
        cfg.set_initial_max_streams_uni(16);
        cfg.set_disable_active_migration(true);
        Ok(cfg)
    })
}

// ── H3 echo/drain backend with request-body + clean-FIN tracking ─────────
// (verbatim from h1h3_md_streaming_verify — the H3 side is identical; only the
// front protocol changes.)

/// Observations the H3 backend records for assertions.
#[derive(Clone)]
struct BackendObs {
    /// Total request DATA bytes the backend read off stream 0, snapshotted at
    /// clean-FIN time (the byte-identity assertion source).
    req_body_bytes: Arc<AtomicUsize>,
    /// LIVE running total of request DATA bytes received on stream 0, updated on
    /// EVERY drain regardless of FIN. A truncated/RST upload still increments
    /// this (unlike `req_body_bytes`), so it is the non-vacuity "did the smuggle
    /// path actually forward body upstream?" signal for the F-MD-4 burst loop.
    req_body_bytes_live: Arc<AtomicUsize>,
    /// Count of requests the backend saw terminate with a CLEAN QUIC stream
    /// FIN on stream 0 (a truncated/RESET upload never increments this — the
    /// F-MD-4 `complete==0` load-bearing signal).
    complete: Arc<AtomicUsize>,
    /// Whether the inbound request carried a post-DATA HEADERS (trailers).
    saw_req_trailers: Arc<AtomicUsize>,
}

impl BackendObs {
    fn new() -> Self {
        Self {
            req_body_bytes: Arc::new(AtomicUsize::new(0)),
            req_body_bytes_live: Arc::new(AtomicUsize::new(0)),
            complete: Arc::new(AtomicUsize::new(0)),
            saw_req_trailers: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// Spawn an H3 backend that reads the whole request body on stream 0, records
/// (bytes, clean-FIN, trailers-seen), and — once the stream FINs cleanly —
/// echoes the body back with status 200. On a truncated upload (RESET / no
/// FIN) it NEVER responds and NEVER increments `complete`.
///
/// Default QUIC flow-control (16 MiB conn / 8 MiB stream) — sufficient for the
/// ≤5 MiB byte-identity / gauge / burst uploads. The F-CAP-1 over-cap test needs
/// MORE than the 64 MiB request cap to flow upstream before the cap trips, so it
/// uses [`spawn_h3_echo_backend_with_flow`] with a >64 MiB window instead.
async fn spawn_h3_echo_backend(
    cert_path: PathBuf,
    key_path: PathBuf,
    obs: BackendObs,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    spawn_h3_echo_backend_with_flow(
        cert_path,
        key_path,
        obs,
        /* max_data = */ 16 * 1024 * 1024,
        /* max_stream_data = */ 8 * 1024 * 1024,
    )
    .await
}

/// Same H3 echo backend as [`spawn_h3_echo_backend`] but with caller-chosen QUIC
/// flow-control windows. The F-CAP-1 over-cap test (R4 fix) passes a window
/// LARGER than `MAX_REQUEST_BODY_BYTES` (64 MiB) so a >64 MiB upload can actually
/// FORWARD past the cap and trip the pump's mid-body over-cap `Reset` arm
/// (h2_proxy.rs over-cap branch). With the default 16 MiB window the gateway's
/// upstream QUIC send window closes at ~16 MiB and the pump parks long before
/// `forwarded_total` reaches 64 MiB, so the cap arm is never exercised.
async fn spawn_h3_echo_backend_with_flow(
    cert_path: PathBuf,
    key_path: PathBuf,
    obs: BackendObs,
    max_data: u64,
    max_stream_data: u64,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = socket.local_addr().unwrap();
    let socket = Arc::new(socket);

    let handle = tokio::spawn(async move {
        let Ok(mut cfg) = quiche::Config::new(quiche::PROTOCOL_VERSION) else {
            return;
        };
        let _ = cfg.set_application_protos(&[LB_QUIC_ALPN]);
        cfg.set_max_idle_timeout(10_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(max_data);
        cfg.set_initial_max_stream_data_bidi_local(max_stream_data);
        cfg.set_initial_max_stream_data_bidi_remote(max_stream_data);
        cfg.set_initial_max_stream_data_uni(1024 * 1024);
        cfg.set_initial_max_streams_bidi(16);
        cfg.set_initial_max_streams_uni(16);
        cfg.set_disable_active_migration(true);
        if cfg
            .load_cert_chain_from_pem_file(cert_path.to_str().unwrap_or(""))
            .is_err()
            || cfg
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
        let _ = conn.recv(
            in_buf.get_mut(..n).unwrap_or(&mut []),
            quiche::RecvInfo {
                from: peer,
                to: addr,
            },
        );

        let mut rx_tail: Vec<u8> = Vec::new();
        let mut body_acc: Vec<u8> = Vec::new();
        let mut decoded_status_seen = false; // first HEADERS consumed
        let mut req_fin = false;
        let mut responded = false;
        let mut req_body_total: usize = 0;
        let mut resp_out: Vec<u8> = Vec::new();
        let mut resp_sent: usize = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);

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

            if conn.is_established() {
                for sid in conn.readable().collect::<Vec<u64>>() {
                    if sid != 0 {
                        continue;
                    }
                    let mut chunk = [0u8; 8192];
                    loop {
                        match conn.stream_recv(sid, &mut chunk) {
                            Ok((rn, fin)) => {
                                rx_tail.extend_from_slice(chunk.get(..rn).unwrap_or(&[]));
                                if fin {
                                    req_fin = true;
                                }
                            }
                            Err(quiche::Error::Done)
                            | Err(quiche::Error::InvalidStreamState(_)) => break,
                            Err(_) => break,
                        }
                    }
                }

                drain_h3_request_frames(
                    &mut rx_tail,
                    &mut decoded_status_seen,
                    &mut body_acc,
                    &mut req_body_total,
                    &obs,
                );
                // LIVE byte count (updated even for truncated/RST uploads that
                // never FIN) — the non-vacuity "did body forward upstream?"
                // signal for the F-MD-4 request burst.
                obs.req_body_bytes_live
                    .store(req_body_total, Ordering::SeqCst);

                // Build the response ONCE after a CLEAN FIN (a truncated upload
                // never gets a response and never counts as complete).
                if req_fin && !responded {
                    obs.req_body_bytes.store(req_body_total, Ordering::SeqCst);
                    obs.complete.fetch_add(1, Ordering::SeqCst);
                    let encoder = QpackEncoder::new();
                    let resp_headers = vec![
                        (":status".to_string(), "200".to_string()),
                        ("content-length".to_string(), body_acc.len().to_string()),
                        ("x-backend-tag".to_string(), "h3-echo".to_string()),
                    ];
                    if let Ok(block) = encoder.encode(&resp_headers) {
                        if let Ok(hframe) = encode_frame(&H3Frame::Headers {
                            header_block: block,
                        }) {
                            if let Ok(dframe) = encode_frame(&H3Frame::Data {
                                payload: Bytes::from(body_acc.clone()),
                            }) {
                                resp_out.extend_from_slice(&hframe);
                                resp_out.extend_from_slice(&dframe);
                            }
                        }
                    }
                    responded = true;
                }

                if responded && resp_sent < resp_out.len() {
                    while resp_sent < resp_out.len() {
                        let sl = resp_out.get(resp_sent..).unwrap_or(&[]);
                        let fin = true;
                        match conn.stream_send(0, sl, fin) {
                            Ok(ns) if ns > 0 => resp_sent += ns,
                            _ => break,
                        }
                    }
                }
            }

            let to = conn
                .timeout()
                .unwrap_or(Duration::from_millis(20))
                .min(Duration::from_millis(20));
            match tokio::time::timeout(to, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((rn, from))) => {
                    let _ = conn.recv(
                        in_buf.get_mut(..rn).unwrap_or(&mut []),
                        quiche::RecvInfo { from, to: addr },
                    );
                }
                _ => conn.on_timeout(),
            }
        }
    });
    (addr, handle)
}

/// Incrementally parse buffered H3 request frames (header-only parse, the R8
/// discipline): consume a HEADERS frame (first = head, later = trailers),
/// accumulate DATA payload into `body_acc` + the running byte total.
fn drain_h3_request_frames(
    rx_tail: &mut Vec<u8>,
    head_seen: &mut bool,
    body_acc: &mut Vec<u8>,
    body_total: &mut usize,
    obs: &BackendObs,
) {
    use lb_h3::decode_varint;
    loop {
        let (ftype, type_len) = match decode_varint(rx_tail) {
            Ok(v) => v,
            Err(_) => return,
        };
        let rest = rx_tail.get(type_len..).unwrap_or(&[]);
        let (flen, len_len) = match decode_varint(rest) {
            Ok(v) => v,
            Err(_) => return,
        };
        let hdr = type_len + len_len;
        let flen = flen as usize;
        if rx_tail.len() < hdr + flen {
            return; // whole frame not yet buffered
        }
        let payload: Vec<u8> = rx_tail.get(hdr..hdr + flen).unwrap_or(&[]).to_vec();
        rx_tail.drain(..hdr + flen);
        match ftype {
            0x00 => {
                // DATA
                *body_total += payload.len();
                body_acc.extend_from_slice(&payload);
            }
            0x01 => {
                // HEADERS: first = head, subsequent = trailers
                if *head_seen {
                    obs.saw_req_trailers.fetch_add(1, Ordering::SeqCst);
                } else {
                    *head_seen = true;
                }
            }
            _ => { /* skip other frame types */ }
        }
    }
}

// ── Gateway listener (H2 front, h3_upstream wired) ───────────────────────

async fn spawn_gateway(
    backend_addr: SocketAddr,
    ca_path: PathBuf,
    timeouts: HttpTimeouts,
) -> SocketAddr {
    let factory = build_quic_client_config_factory(ca_path);
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
            timeouts,
            /* is_https = */ false,
            H2SecurityThresholds::default(),
        )
        .with_h3_upstream(Arc::clone(&h3_pool)),
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let gw = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let p = Arc::clone(&h2_proxy);
            tokio::spawn(async move {
                let _ = p.serve_connection(sock, peer).await;
            });
        }
    });
    gw
}

/// Connect a genuine hyper plaintext-H2 client to the gateway. Returns the
/// SendRequest handle (the connection driver is spawned).
async fn connect_h2_client(
    gateway: SocketAddr,
) -> hyper::client::conn::http2::SendRequest<
    http_body_util::combinators::BoxBody<Bytes, hyper::Error>,
> {
    let stream = TcpStream::connect(gateway).await.unwrap();
    let (sender, conn) = hyper::client::conn::http2::handshake::<_, _, _>(
        TokioExecutor::new(),
        TokioIo::new(stream),
    )
    .await
    .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    sender
}

/// Send an H2 POST with the given streamed body chunks to the gateway, return
/// (status, response-body-bytes). A streamed (multi-frame) request body
/// exercises the gateway's pump path rather than a single buffered frame.
async fn h2_request_with_body(gw: SocketAddr, body_chunks: Vec<Bytes>) -> (u16, Bytes) {
    let mut sender = connect_h2_client(gw).await;
    let (btx, brx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, hyper::Error>>(4);
    tokio::spawn(async move {
        for c in body_chunks {
            if btx.send(Ok(Frame::data(c))).await.is_err() {
                return;
            }
        }
    });
    let body = StreamBody::new(recv_stream(brx)).boxed();
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://{TEST_SNI}/echo"))
        .body(body)
        .unwrap();
    let resp = match tokio::time::timeout(Duration::from_secs(60), sender.send_request(req)).await {
        Ok(Ok(r)) => r,
        _ => return (0, Bytes::new()),
    };
    let status = resp.status().as_u16();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes())
        .unwrap_or_default();
    (status, bytes)
}

/// Adapt a tokio mpsc receiver of body frames into a `futures` Stream.
fn recv_stream(
    mut rx: tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, hyper::Error>>,
) -> impl futures_util::Stream<Item = Result<Frame<Bytes>, hyper::Error>> {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

// ── Test 1: binary body byte-identical BOTH directions ───────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_binary_body_byte_identical_both_directions() {
    let certs = generate_loopback_certs();
    let obs = BackendObs::new();
    let (backend, _bh) =
        spawn_h3_echo_backend(certs.cert.clone(), certs.key.clone(), obs.clone()).await;
    let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;

    // A ~5 MiB body that crosses H3_BODY_CHUNK_MAX (8 KiB) ~640× — sent as many
    // H2 DATA frames so the streaming pump (not one frame) runs; full byte-
    // equality both directions is a non-trivial streaming proof.
    let mut payload = Vec::with_capacity(5 * 1024 * 1024);
    for i in 0..(5u32 * 1024 * 1024 / 4) {
        payload.extend_from_slice(&i.to_le_bytes());
    }
    let payload = Bytes::from(payload); // 5 242 880 bytes
    let chunks: Vec<Bytes> = payload.chunks(7_000).map(Bytes::copy_from_slice).collect();

    let (status, echoed) = h2_request_with_body(gw, chunks).await;
    eprintln!(
        "H2H3_BYTE_IDENTICAL status={status} sent={} echoed={} backend_body_bytes={} complete={}",
        payload.len(),
        echoed.len(),
        obs.req_body_bytes.load(Ordering::SeqCst),
        obs.complete.load(Ordering::SeqCst),
    );
    assert_eq!(status, 200, "expected 200 from the H3 echo backend");
    assert_eq!(
        obs.req_body_bytes.load(Ordering::SeqCst),
        payload.len(),
        "request body bytes mismatch at the H3 backend (request-leg byte-identity)"
    );
    assert_eq!(
        echoed, payload,
        "response body not byte-identical to the request"
    );
    assert_eq!(
        obs.complete.load(Ordering::SeqCst),
        1,
        "backend must have seen exactly one COMPLETE (clean-FIN) request"
    );
}

// ── Test 2: request-trailer FORWARDING to the H3 backend ─────────────────
//
// A genuine H2 client sends DATA then a TRAILERS frame (grpc-shaped). The
// streaming pump validates the trailer, emits ReqBodyEvent::End{trailers}, and
// the connector (forward_req_trailers=true) ships a post-DATA HEADERS frame the
// backend records as saw_req_trailers. Uses the `h2` crate directly because
// hyper's high-level client body API does not expose request-trailer send.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_request_trailers_forwarded_to_backend() {
    use h2 as h2crate;
    let certs = generate_loopback_certs();
    let obs = BackendObs::new();
    let (backend, _bh) =
        spawn_h3_echo_backend(certs.cert.clone(), certs.key.clone(), obs.clone()).await;
    let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;

    let stream = TcpStream::connect(gw).await.unwrap();
    let (h2, conn) = h2crate::client::handshake(stream).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2 = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("http://{TEST_SNI}/echo"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = h2.send_request(req, false).unwrap();
    // DATA then a real H2 trailer field section.
    send_body
        .send_data(Bytes::from_static(b"hello"), false)
        .unwrap();
    let mut trailers = http::HeaderMap::new();
    trailers.insert("x-checksum", http::HeaderValue::from_static("abc123"));
    send_body.send_trailers(trailers).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(20), resp_fut).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while obs.complete.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    eprintln!(
        "H2H3_REQ_TRAILERS complete={} saw_req_trailers={} req_body_bytes={}",
        obs.complete.load(Ordering::SeqCst),
        obs.saw_req_trailers.load(Ordering::SeqCst),
        obs.req_body_bytes.load(Ordering::SeqCst),
    );
    assert_eq!(
        obs.complete.load(Ordering::SeqCst),
        1,
        "backend must see one COMPLETE request"
    );
    assert_eq!(
        obs.req_body_bytes.load(Ordering::SeqCst),
        5,
        "backend must receive the 5-byte body"
    );
    // THE assertion: the request trailer reached the H3 backend as a post-DATA
    // HEADERS frame (forward_req_trailers=true; pump End{trailers} path).
    assert_eq!(
        obs.saw_req_trailers.load(Ordering::SeqCst),
        1,
        "request trailer section was NOT forwarded to the H3 backend \
         (forward_req_trailers regression)"
    );
}

// ── Test 3: F-MD-4 REQUEST cancel-race (Hazard (a)) — H2 RST mid-body ─────
//
// A genuine H2 client streams a >window body then RST_STREAMs mid-body (drop
// `send_body` without END_STREAM → CANCEL). This cancels the gateway's H2
// SERVICE future. The DETACHED ingress pump must survive that cancel and emit
// an explicit ReqBodyEvent::Reset (its `None && !is_end_stream()` arm) so the
// connector RESET-without-FINs the upstream QUIC stream → the H3 backend NEVER
// sees a truncated request as a clean-FIN COMPLETE. A silent body_tx drop would
// (per the connector's pre-event-drop == bodyless-COMPLETE contract,
// h3_bridge.rs:3309-3313) smuggle the truncated request. Gate on the backend's
// QUIC FIN: complete must NOT move.

/// One real-wire H2-RST smuggle attempt on a FRESH backend/gateway. Returns
/// `(complete_after, live_body_bytes_seen)`. A non-zero `complete_after` is the
/// F-MD-4 DEFECT (a truncated request relayed as clean-FIN COMPLETE); the live
/// byte count is the non-vacuity signal that body was actually forwarded.
async fn run_h2_rst_smuggle_once(certs: &TestCerts) -> (usize, usize) {
    use h2 as h2crate;
    let obs = BackendObs::new();
    let (backend, _bh) =
        spawn_h3_echo_backend(certs.cert.clone(), certs.key.clone(), obs.clone()).await;
    let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;

    let stream = TcpStream::connect(gw).await.unwrap();
    let mut builder = h2crate::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(stream).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("http://{TEST_SNI}/smuggle"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = h2sender.send_request(req, false).unwrap();

    // Stream 256 KiB (≫ the 64 KiB in-flight window) so the gateway dials the
    // QUIC upstream and forwards body before we abort (genuinely mid-body).
    let payload = vec![0x5Au8; 256 * 1024];
    let mut off = 0;
    while off < payload.len() {
        let end = (off + 16 * 1024).min(payload.len());
        let chunk = Bytes::copy_from_slice(&payload[off..end]);
        send_body.reserve_capacity(chunk.len());
        match tokio::time::timeout(
            Duration::from_secs(5),
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
    // Let the dial + initial forward land so the abort is genuinely mid-body.
    tokio::time::sleep(Duration::from_millis(300)).await;
    // ABORT mid-body WITHOUT END_STREAM → RST_STREAM(CANCEL).
    drop(send_body);
    let _ = tokio::time::timeout(Duration::from_secs(3), resp_fut).await;
    // Settle so any (defective) clean upstream FIN the gateway might emit is
    // observed + recorded by the backend before we read the flag.
    tokio::time::sleep(Duration::from_millis(700)).await;

    (
        obs.complete.load(Ordering::SeqCst),
        // LIVE byte count: a truncated upload never FINs, so `req_body_bytes`
        // (clean-FIN snapshot) would always read 0 — `req_body_bytes_live` is
        // the count that proves the smuggle path actually forwarded body.
        obs.req_body_bytes_live.load(Ordering::SeqCst),
    )
}

// R13 (a) in-gate single shot + load-bearing negative control.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_fmd4_request_rst_never_complete() {
    let certs = generate_loopback_certs();

    // POSITIVE arm (load-bearing negative control): a CLEAN upload DOES reach
    // the backend complete. If this did not hold, the complete==0 assertion
    // below would be vacuous.
    {
        let obs = BackendObs::new();
        let (backend, _bh) =
            spawn_h3_echo_backend(certs.cert.clone(), certs.key.clone(), obs.clone()).await;
        let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;
        let body = Bytes::from(vec![0x42u8; 256 * 1024]);
        let (status, _echoed) = h2_request_with_body(gw, vec![body]).await;
        assert_eq!(status, 200, "clean arm: expected 200 from the echo backend");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while obs.complete.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            obs.complete.load(Ordering::SeqCst) >= 1,
            "LOAD-BEARING control: a CLEAN H2 upload must reach the backend complete"
        );
    }

    // NEGATIVE arm: an H2 RST mid-body must NEVER reach the backend complete.
    let (complete_after, body_seen) = run_h2_rst_smuggle_once(&certs).await;
    eprintln!("H2H3_FMD4_REQ_RST complete_after={complete_after} backend_body_seen={body_seen}");
    assert_eq!(
        complete_after, 0,
        "SMUGGLING: an H2 RST mid-body reached the H3 backend as a COMPLETE \
         request (complete={complete_after}) — Hazard (a) detached-pump / explicit-Reset broken"
    );
}

// R13 (b) isolation-burst ≥50× on current_thread (the low-contention flavor
// that exposes downstream-RST graceful-drop smuggle races; see memory
// parallel-gate-masks-smuggle). The negative control runs once up-front.
#[tokio::test(flavor = "current_thread")]
async fn h2h3_fmd4_request_rst_burst_current_thread() {
    const ITERS: usize = 60; // ≥50 per the H1→H3 (#13) ruling for →H3 cells
    let certs = generate_loopback_certs();

    // Live control: a clean upload increments complete on a dedicated gateway.
    {
        let obs = BackendObs::new();
        let (backend, _bh) =
            spawn_h3_echo_backend(certs.cert.clone(), certs.key.clone(), obs.clone()).await;
        let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;
        let body = Bytes::from(vec![0x42u8; 256 * 1024]);
        let (status, _e) = h2_request_with_body(gw, vec![body]).await;
        assert_eq!(status, 200);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while obs.complete.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            obs.complete.load(Ordering::SeqCst) >= 1,
            "LOAD-BEARING control: clean upload must count complete before the burst"
        );
    }

    // Burst: ITERS H2-RST-mid-body uploads, each on a fresh gateway/backend.
    // NONE may reach its backend complete. Track dialed iters (body forwarded)
    // so the smuggle path is provably exercised (non-vacuous loop).
    let mut dialed = 0usize;
    for iter in 0..ITERS {
        let (complete_after, body_seen) = run_h2_rst_smuggle_once(&certs).await;
        if body_seen > 0 {
            dialed += 1;
        }
        assert_eq!(
            complete_after, 0,
            "SMUGGLING under burst (iter {iter}/{ITERS}): an H2 RST mid-body reached \
             the H3 backend as a COMPLETE request (body_seen={body_seen}) — F-MD-4 race"
        );
    }
    eprintln!("H2H3_FMD4_REQ_RST_BURST iters={ITERS} dialed={dialed} all-incomplete");
    assert!(
        dialed * 2 >= ITERS,
        "smuggle path under-exercised: only {dialed}/{ITERS} iterations forwarded body \
         to the upstream — the loop is not load-bearing"
    );
}

// ── Test 5a: F-CAP-1 mid-body over-cap → RST (backend never sees complete) ─
//
// A >64 MiB H2 upload trips MAX_REQUEST_BODY_BYTES MID-body (after ≥1 chunk
// forwarded) → the pump emits Reset → connector RESET-without-FIN → the H3
// backend NEVER sees a complete request, and the client never gets a clean 200.
// (The pre-data-413 path requires a single inbound frame > 64 MiB, impractical
// on the wire; it is connector-unit-covered. The mid-body RESET is the wire-
// reachable, security-meaningful F-CAP-1 + F-MD-4 arm.)
//
// CF-SATURATION-1: a 66 MiB push must complete under 8-core gate saturation, so
// the gateway listener gets a generous (120 s) body timeout (the H2 ingress sits
// under the wall-clock timeouts.body class — CF-BODY-WALLCLOCK).

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_fcap1_over_cap_upload_never_complete() {
    use h2 as h2crate;
    let certs = generate_loopback_certs();
    let obs = BackendObs::new();
    // R4 (verifier finding): the over-cap mid-body Reset arm only triggers once
    // `forwarded_total` crosses the 64 MiB cap, which requires >64 MiB to FORWARD
    // upstream first. With the default 16 MiB backend flow-control the gateway's
    // upstream QUIC send window closes at ~16 MiB → the pump parks → the cap arm
    // is NEVER reached. Give this backend an 80 MiB window (> the 64 MiB cap + the
    // 66 MiB push) so the upload genuinely forwards past 64 MiB and trips the cap.
    let (backend, _bh) = spawn_h3_echo_backend_with_flow(
        certs.cert.clone(),
        certs.key.clone(),
        obs.clone(),
        /* max_data = */ 80 * 1024 * 1024,
        /* max_stream_data = */ 80 * 1024 * 1024,
    )
    .await;
    let gw = spawn_gateway(
        backend,
        certs.ca.clone(),
        HttpTimeouts {
            body: Duration::from_secs(120),
            ..HttpTimeouts::default()
        },
    )
    .await;

    let stream = TcpStream::connect(gw).await.unwrap();
    let mut builder = h2crate::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(stream).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("http://{TEST_SNI}/echo"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = h2sender.send_request(req, false).unwrap();

    // Push up to 66 MiB (> 64 MiB cap) in 64 KiB frames until the gateway resets
    // the stream (the over-cap abort propagates as a send error / capacity stall).
    let chunk = vec![0x5Au8; 64 * 1024];
    let over = 66 * 1024 * 1024usize;
    let mut sent = 0usize;
    let break_reason;
    loop {
        if sent >= over {
            break_reason = "sent-full-66MiB";
            break;
        }
        send_body.reserve_capacity(chunk.len());
        let cap = tokio::time::timeout(
            Duration::from_secs(30),
            futures_util::future::poll_fn(|cx| send_body.poll_capacity(cx)),
        )
        .await;
        match cap {
            // Capacity granted → send a chunk below.
            Ok(Some(Ok(c))) if c > 0 => {}
            // Transient zero capacity is NOT terminal (the gateway's H2 receive
            // window momentarily closed while it drains/forwards) — re-reserve
            // and poll again. Only an Err (RST) or None (closed) is terminal.
            // Breaking on zero here would cut the upload short and (non-
            // deterministically) stop it BEFORE forwarded_total reaches the cap.
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) => {
                break_reason = "cap-err(RST)";
                break;
            }
            Ok(None) => {
                break_reason = "cap-none(closed)";
                break;
            }
            Err(_) => {
                break_reason = "cap-timeout";
                break;
            }
        }
        if send_body
            .send_data(Bytes::copy_from_slice(&chunk), false)
            .is_err()
        {
            break_reason = "send_data-err";
            break;
        }
        sent += chunk.len();
    }
    eprintln!("H2H3_FCAP1 send loop ended: reason={break_reason} sent={sent}");
    // Capture the H2 client-observed outcome. The over-cap mid-body Reset (the
    // connector RESET-without-FINs the upstream) must surface to the H2 client as
    // an ABORT, never a clean 200-with-complete-body. The client sees one of:
    //  - send_request errors / times out (no head, the stream was RST mid-upload),
    //  - a response head whose BODY collect ERRORS (RST_STREAM after a partial), or
    //  - a non-200 status.
    // What it must NEVER see: 200 + a cleanly-terminated body (a smuggled-complete).
    let client_clean_200 = match tokio::time::timeout(Duration::from_secs(15), resp_fut).await {
        Ok(Ok(resp)) => {
            let status = resp.status().as_u16();
            // Drain the body; a clean END_STREAM yields Ok, an RST yields Err.
            let mut body = resp.into_body();
            let mut body_ok = true;
            while let Some(part) = body.data().await {
                match part {
                    Ok(b) => {
                        let _ = body.flow_control().release_capacity(b.len());
                    }
                    Err(_) => {
                        body_ok = false;
                        break;
                    }
                }
            }
            let trailers_ok = body.trailers().await.is_ok();
            eprintln!(
                "H2H3_FCAP1 client head status={status} body_ok={body_ok} trailers_ok={trailers_ok}"
            );
            status == 200 && body_ok && trailers_ok
        }
        Ok(Err(e)) => {
            eprintln!("H2H3_FCAP1 client send_request errored (RST mid-upload): {e}");
            false
        }
        Err(_) => {
            eprintln!("H2H3_FCAP1 client got no response head (stream aborted)");
            false
        }
    };
    // Let the backend finish draining the in-flight upload tail so `live`
    // reflects the full ~64 MiB it received before the over-cap Reset (bounded
    // wait — CF-SATURATION-1, generous so the ×3 gate can't false-flake).
    const CAP: usize = 64 * 1024 * 1024;
    let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while obs.req_body_bytes_live.load(Ordering::SeqCst) < CAP - chunk.len()
        && tokio::time::Instant::now() < drain_deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let live = obs.req_body_bytes_live.load(Ordering::SeqCst);
    eprintln!(
        "H2H3_FCAP1 sent={sent} backend_complete={} backend_body_live={live} backend_body_bytes={} client_clean_200={client_clean_200}",
        obs.complete.load(Ordering::SeqCst),
        obs.req_body_bytes.load(Ordering::SeqCst),
    );
    assert_eq!(
        obs.complete.load(Ordering::SeqCst),
        0,
        "F-CAP-1: an over-cap H2 upload reached the H3 backend as COMPLETE \
         (complete={}) — the cap did not abort the upstream request",
        obs.complete.load(Ordering::SeqCst)
    );
    // Point #3 (lead): the gateway must RST_STREAM the H2 client — the client must
    // NOT observe a clean 200-with-complete-body. This distinguishes the cap-abort
    // from any clean completion and is the downstream half of the F-CAP-1 guard.
    assert!(
        !client_clean_200,
        "F-CAP-1: the H2 client saw a CLEAN 200-with-complete-body for an over-cap \
         upload — the cap-RST did not propagate downstream (response-splitting / \
         smuggled-complete)"
    );
    // R4 non-vacuity: the over-cap mid-body Reset arm only fires once the upload
    // genuinely forwards UP TO the 64 MiB cap. The pump forwards chunks until the
    // NEXT would exceed MAX_REQUEST_BODY_BYTES, then Resets — so the backend sees
    // ≈ the cap's worth (~64 MiB) of DATA. The OLD bug stalled forwarding at the
    // backend's ~16 MiB window and never reached the cap. Require the forwarded
    // total to have reached WITHIN one chunk of the 64 MiB cap (≈64 MiB), proving
    // the over-cap arm — not a flow-control stall — is what aborted the upload.
    assert!(
        live >= CAP - chunk.len(),
        "F-CAP-1 vacuity: the over-cap arm was NOT reached — only {live} B forwarded \
         (the cap is {CAP} B), so the upload stalled on flow-control before the cap \
         instead of tripping the mid-body Reset. Raise the test backend's QUIC window."
    );
}

// ── Test 5b: pre-dial upstream-down → 502 ────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_upstream_down_yields_502() {
    let certs = generate_loopback_certs();
    let dead = "127.0.0.1:1".parse().unwrap();
    let gw = spawn_gateway(dead, certs.ca.clone(), HttpTimeouts::default()).await;
    let (status, _b) = h2_request_with_body(gw, vec![Bytes::from_static(b"x")]).await;
    eprintln!("H2H3_UPSTREAM_DOWN status={status}");
    assert_eq!(
        status, 502,
        "a dead H3 upstream must yield 502 (got {status})"
    );
}

// ── Test 7: memory gauge non-vacuous + load-bearing inverted probe ───────
//
// The H2→H3 request pump records the SAME instantaneous in-flight gauge
// (`H2_REQ_MAX_RETAINED_BODY_BYTES`) the H2→H1/H2→H2 cells use. Stream a body ≫
// the bounded window; the retained gauge must stay ≤ a small multiple of the
// window (H3_BODY_CHANNEL_DEPTH(8) × H3_BODY_CHUNK_MAX(8 KiB) = 64 KiB) and ≪ the
// body, proving body-size-INDEPENDENCE. The gauge is process-global → serialize.

static GAUGE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const WINDOW: usize = 64 * 1024; // H3_BODY_CHANNEL_DEPTH(8) × H3_BODY_CHUNK_MAX(8 KiB)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_memory_gauge_non_vacuous_and_load_bearing() {
    use lb_l7::h2_proxy::{H2_REQ_MAX_RETAINED_BODY_BYTES, record_retained};

    let _serial = GAUGE_SERIAL.lock().await;
    H2_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);

    let certs = generate_loopback_certs();
    let obs = BackendObs::new();
    let (backend, _bh) =
        spawn_h3_echo_backend(certs.cert.clone(), certs.key.clone(), obs.clone()).await;
    let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;

    // 4 MiB body ≫ 64 KiB window. The bounded in-flight channel + the upstream
    // QUIC send window keep the pump's retained set bounded regardless of size.
    let body_size = 4 * 1024 * 1024;
    let payload: Vec<u8> = (0..body_size).map(|i| (i % 251) as u8).collect();
    let payload = Bytes::from(payload);
    let chunks: Vec<Bytes> = payload
        .chunks(16 * 1024)
        .map(Bytes::copy_from_slice)
        .collect();
    let (status, echoed) = h2_request_with_body(gw, chunks).await;
    assert_eq!(status, 200, "memory-gauge round-trip should succeed");
    assert_eq!(
        echoed.len(),
        body_size,
        "memory-gauge body must round-trip whole"
    );

    let in_situ = H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    eprintln!("H2H3_MEMORY_GAUGE in_situ={in_situ} window={WINDOW} body={body_size}");
    assert!(
        in_situ > 0,
        "gauge is 0 — the pump never recorded in-flight bytes (vacuous proof)"
    );
    assert!(
        in_situ <= 4 * WINDOW,
        "retained gauge {in_situ} exceeds 4×window ({}) — bounded-memory bar broken",
        4 * WINDOW
    );
    assert!(
        in_situ < body_size,
        "retained gauge {in_situ} not ≪ body size {body_size} (not body-size-independent)"
    );

    // INVERTED PROBE (load-bearing): a whole-body-buffering impl would record
    // body_size; confirm that would breach the ceiling (so the bound is real).
    record_retained(body_size);
    assert!(
        H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed) > 4 * WINDOW,
        "inverted probe failed: a whole-body retain did not exceed the ceiling — \
         the bound would not catch a buffering regression"
    );
}

// ── Test 6: gRPC-shaped RESPONSE trailers — POSITIVE assertion (the WIN) ──
//
// The H3 backend returns a gRPC-shaped response: HEADERS, DATA, then a post-DATA
// TRAILERS frame carrying `grpc-status: 0`. The connector decodes it to
// H3RespEvent::Trailers; the H2 front relays it onto the response body's terminal
// Frame::trailers. UNLIKE H1→H3 (CF-RESP-1: hyper-1 H1 drops streamed trailers
// absent a head Trailer: declaration), hyper's H2 SERVER encoder flushes
// Frame::trailers NATIVELY — so `grpc-status` MUST reach the H2 client. This is a
// POSITIVE assertion, the trailer-mandate WIN (H2→H3 is THE gRPC-capable →H3 cell).

async fn spawn_h3_grpc_backend(
    cert_path: PathBuf,
    key_path: PathBuf,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = socket.local_addr().unwrap();
    let socket = Arc::new(socket);
    let handle = tokio::spawn(async move {
        let Ok(mut cfg) = quiche::Config::new(quiche::PROTOCOL_VERSION) else {
            return;
        };
        let _ = cfg.set_application_protos(&[LB_QUIC_ALPN]);
        cfg.set_max_idle_timeout(10_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(4 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(1024 * 1024);
        cfg.set_initial_max_stream_data_uni(256 * 1024);
        cfg.set_initial_max_streams_bidi(8);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        if cfg
            .load_cert_chain_from_pem_file(cert_path.to_str().unwrap_or(""))
            .is_err()
            || cfg
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
        let _ = conn.recv(
            in_buf.get_mut(..n).unwrap_or(&mut []),
            quiche::RecvInfo {
                from: peer,
                to: addr,
            },
        );
        let mut got_head = false;
        let mut responded = false;
        let mut resp_out: Vec<u8> = Vec::new();
        let mut resp_sent = 0usize;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
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
            if conn.is_established() {
                for sid in conn.readable().collect::<Vec<u64>>() {
                    if sid != 0 {
                        continue;
                    }
                    let mut chunk = [0u8; 4096];
                    while let Ok((rn, _fin)) = conn.stream_recv(sid, &mut chunk) {
                        if rn > 0 {
                            got_head = true;
                        }
                    }
                }
                if got_head && !responded {
                    let encoder = QpackEncoder::new();
                    let head = encoder
                        .encode(&[
                            (":status".to_string(), "200".to_string()),
                            ("content-type".to_string(), "application/grpc".to_string()),
                        ])
                        .ok()
                        .and_then(|b| encode_frame(&H3Frame::Headers { header_block: b }).ok());
                    let data = encode_frame(&H3Frame::Data {
                        payload: Bytes::from_static(b"grpc-body"),
                    })
                    .ok();
                    let trailers = encoder
                        .encode(&[("grpc-status".to_string(), "0".to_string())])
                        .ok()
                        .and_then(|b| encode_frame(&H3Frame::Headers { header_block: b }).ok());
                    if let (Some(h), Some(d), Some(t)) = (head, data, trailers) {
                        resp_out.extend_from_slice(&h);
                        resp_out.extend_from_slice(&d);
                        resp_out.extend_from_slice(&t);
                    }
                    responded = true;
                }
                if responded && resp_sent < resp_out.len() {
                    while resp_sent < resp_out.len() {
                        let sl = resp_out.get(resp_sent..).unwrap_or(&[]);
                        match conn.stream_send(0, sl, true) {
                            Ok(ns) if ns > 0 => resp_sent += ns,
                            _ => break,
                        }
                    }
                }
            }
            let to = conn
                .timeout()
                .unwrap_or(Duration::from_millis(20))
                .min(Duration::from_millis(20));
            match tokio::time::timeout(to, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((rn, from))) => {
                    let _ = conn.recv(
                        in_buf.get_mut(..rn).unwrap_or(&mut []),
                        quiche::RecvInfo { from, to: addr },
                    );
                }
                _ => conn.on_timeout(),
            }
        }
    });
    (addr, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_grpc_response_trailers_reach_h2_client() {
    use h2 as h2crate;
    let certs = generate_loopback_certs();
    let (backend, _bh) = spawn_h3_grpc_backend(certs.cert.clone(), certs.key.clone()).await;
    let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;

    // Use the `h2` crate so we can read the response trailer field section
    // directly (hyper's high-level body collect also surfaces trailers, but the
    // h2 client makes the native trailer frame explicit).
    let stream = TcpStream::connect(gw).await.unwrap();
    let (h2, conn) = h2crate::client::handshake(stream).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2 = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("GET")
        .uri(format!("http://{TEST_SNI}/grpc"))
        .body(())
        .unwrap();
    let (resp_fut, _send) = h2.send_request(req, true).unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(15), resp_fut)
        .await
        .expect("response timed out")
        .expect("response head failed");
    let status = resp.status().as_u16();
    let mut body = resp.into_body();
    let mut data = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.expect("body data error");
        let _ = body.flow_control().release_capacity(chunk.len());
        data.extend_from_slice(&chunk);
    }
    let trailers = body.trailers().await.ok().flatten();
    let grpc_status = trailers
        .as_ref()
        .and_then(|t| t.get("grpc-status"))
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    eprintln!(
        "H2H3_GRPC_TRAILER status={status} body={:?} grpc_status={grpc_status:?}",
        String::from_utf8_lossy(&data)
    );
    assert_eq!(
        status, 200,
        "gRPC-shaped response head did not reach the H2 client"
    );
    assert_eq!(
        &data, b"grpc-body",
        "gRPC response DATA payload did not reach the H2 client"
    );
    // THE trailer-mandate WIN (POSITIVE assertion): grpc-status reaches the
    // H2 client as a native trailer frame (no Trailer: pre-declaration needed).
    assert_eq!(
        grpc_status.as_deref(),
        Some("0"),
        "TRAILER MANDATE: grpc-status did NOT reach the H2 client as a response \
         trailer (got {grpc_status:?}) — the H2-native Frame::trailers relay regressed"
    );
}

// ── Test 4: F-MD-4 RESPONSE-leg truncation (Hazard (b), the guard) ───────
//
// A truncating-response H3 backend sends a PARTIAL DATA frame, then DROPS the
// QUIC stream WITHOUT a clean FIN. The connector surfaces RespAbort →
// H3RespEvent::Reset → the H2 front injects Err(H2RespAbort) into the response
// body StreamBody → hyper's H2 server RST_STREAMs the downstream stream WITHOUT
// a clean END_STREAM. The H2 client's body collect therefore ERRORS — it NEVER
// receives a clean-terminated body. Response-splitting guard: a truncated
// upstream response is never relayed to the client as complete.
//
// TWO ARMS by upstream framing (the backend's `declare_cl` flag), applying the
// #13b CL-masking lesson (memory h1h3-resp-trunc-cl-masks-guard):
//  • CL-declared: the upstream sends content-length=BIG, forwarded to the H2
//    client, so hyper's H2 client ALSO detects the underrun on its own — this
//    arm passes even with the guard deleted (NOT load-bearing). Kept as CL-path
//    regression. Predicate: complete := Ok(len>=declared).
//  • CHUNKED (no-CL): the upstream omits content-length; the only truncation
//    signal is the guard (Err(H2RespAbort) → RST_STREAM, no clean END_STREAM).
//    With the guard deleted the StreamBody ends with a clean drop → hyper emits
//    a clean END_STREAM → the H2 client gets Ok(partial) = a smuggled-complete
//    leak. So the guard is the SOLE discriminator and the predicate is strict:
//    complete := Ok(_). This is the LOAD-BEARING arm (the verifier will run the
//    guard-deletion mutation: deleting Reset→Err(box) must FAIL this arm).

const TRUNC_DECLARED_LEN: usize = 1024 * 1024; // backend declares 1 MiB
const TRUNC_PARTIAL_LEN: usize = 100 * 1024; // sends only 100 KiB then drops

async fn spawn_h3_truncating_response_backend(
    cert_path: PathBuf,
    key_path: PathBuf,
    declare_cl: bool,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = socket.local_addr().unwrap();
    let socket = Arc::new(socket);
    let handle = tokio::spawn(async move {
        let Ok(mut cfg) = quiche::Config::new(quiche::PROTOCOL_VERSION) else {
            return;
        };
        let _ = cfg.set_application_protos(&[LB_QUIC_ALPN]);
        cfg.set_max_idle_timeout(10_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(4 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(1024 * 1024);
        cfg.set_initial_max_stream_data_uni(256 * 1024);
        cfg.set_initial_max_streams_bidi(8);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        if cfg
            .load_cert_chain_from_pem_file(cert_path.to_str().unwrap_or(""))
            .is_err()
            || cfg
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
        let _ = conn.recv(
            in_buf.get_mut(..n).unwrap_or(&mut []),
            quiche::RecvInfo {
                from: peer,
                to: addr,
            },
        );
        let mut got_head = false;
        let mut responded = false;
        let mut resp_out: Vec<u8> = Vec::new();
        let mut resp_sent = 0usize;
        let mut truncated = false;
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
            if conn.is_established() {
                for sid in conn.readable().collect::<Vec<u64>>() {
                    if sid != 0 {
                        continue;
                    }
                    let mut chunk = [0u8; 4096];
                    while let Ok((rn, _fin)) = conn.stream_recv(sid, &mut chunk) {
                        if rn > 0 {
                            got_head = true;
                        }
                    }
                }
                if got_head && !responded {
                    let mut head_fields = vec![(":status".to_string(), "200".to_string())];
                    if declare_cl {
                        head_fields
                            .push(("content-length".to_string(), TRUNC_DECLARED_LEN.to_string()));
                    }
                    let encoder = QpackEncoder::new();
                    let head = encoder
                        .encode(&head_fields)
                        .ok()
                        .and_then(|b| encode_frame(&H3Frame::Headers { header_block: b }).ok());
                    // PARTIAL DATA: TRUNC_PARTIAL_LEN bytes, then shutdown WITHOUT
                    // a clean FIN so the gateway sees a premature end.
                    let data = encode_frame(&H3Frame::Data {
                        payload: Bytes::from(vec![0x5Au8; TRUNC_PARTIAL_LEN]),
                    })
                    .ok();
                    if let (Some(h), Some(d)) = (head, data) {
                        resp_out.extend_from_slice(&h);
                        resp_out.extend_from_slice(&d);
                    }
                    responded = true;
                }
                if responded && resp_sent < resp_out.len() {
                    while resp_sent < resp_out.len() {
                        let sl = resp_out.get(resp_sent..).unwrap_or(&[]);
                        match conn.stream_send(0, sl, false) {
                            Ok(ns) if ns > 0 => resp_sent += ns,
                            _ => break,
                        }
                    }
                }
                if responded && resp_sent >= resp_out.len() && !truncated {
                    let _ = conn.stream_shutdown(0, quiche::Shutdown::Write, 0x010c);
                    truncated = true;
                }
            }
            let to = conn
                .timeout()
                .unwrap_or(Duration::from_millis(20))
                .min(Duration::from_millis(20));
            match tokio::time::timeout(to, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((rn, from))) => {
                    let _ = conn.recv(
                        in_buf.get_mut(..rn).unwrap_or(&mut []),
                        quiche::RecvInfo { from, to: addr },
                    );
                }
                _ => conn.on_timeout(),
            }
        }
    });
    (addr, handle)
}

/// Bodyless GET to the gateway over H2; return (status, body-collect RESULT).
/// `Ok(bytes)` on a clean END_STREAM; `Err` on a truncated/RST/errored body.
async fn h2_get_collect(gw: SocketAddr) -> (u16, Result<Bytes, String>) {
    let mut sender = connect_h2_client(gw).await;
    let req = Request::builder()
        .method("GET")
        .uri(format!("http://{TEST_SNI}/trunc"))
        .body(
            http_body_util::Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
        )
        .unwrap();
    let resp = match tokio::time::timeout(Duration::from_secs(20), sender.send_request(req)).await {
        Ok(Ok(r)) => r,
        _ => return (0, Err("no response head".to_string())),
    };
    let status = resp.status().as_u16();
    let collected = tokio::time::timeout(Duration::from_secs(20), resp.into_body().collect()).await;
    let body_res = match collected {
        Ok(Ok(b)) => Ok(b.to_bytes()),
        Ok(Err(e)) => Err(format!("body error: {e}")),
        Err(_) => Err("body collect timeout".to_string()),
    };
    (status, body_res)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_fmd4_response_truncation_cl_never_false_complete() {
    let certs = generate_loopback_certs();

    // POSITIVE / non-vacuity arm: a CLEAN echo backend → the H2 client collects a
    // COMPLETE body (Ok). Proves the detector CAN observe a complete response.
    {
        let obs = BackendObs::new();
        let (backend, _bh) =
            spawn_h3_echo_backend(certs.cert.clone(), certs.key.clone(), obs.clone()).await;
        let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;
        let body = Bytes::from(vec![0x42u8; 4096]);
        let (status, echoed) = h2_request_with_body(gw, vec![body.clone()]).await;
        assert_eq!(status, 200, "clean arm: expected 200");
        assert_eq!(
            echoed, body,
            "clean arm: body must round-trip complete (non-vacuity)"
        );
    }

    // NEGATIVE arm (CL-declared — NOT load-bearing for the guard; the H2 client's
    // own CL-underrun detection masks it; kept as CL-framing-path regression).
    {
        let (backend, _bh) = spawn_h3_truncating_response_backend(
            certs.cert.clone(),
            certs.key.clone(),
            /* declare_cl = */ true,
        )
        .await;
        let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;
        let (status, body_res) = h2_get_collect(gw).await;
        let complete = matches!(&body_res, Ok(b) if b.len() >= TRUNC_DECLARED_LEN);
        eprintln!(
            "H2H3_RESP_TRUNC_CL status={status} body_res={} declared={TRUNC_DECLARED_LEN} \
             partial={TRUNC_PARTIAL_LEN} false_complete={complete}",
            match &body_res {
                Ok(b) => format!("Ok(len={})", b.len()),
                Err(e) => format!("Err({e})"),
            }
        );
        assert!(
            !complete,
            "RESPONSE-SPLITTING (CL arm): a truncated upstream response was delivered to the \
             H2 client as a COMPLETE body — F-MD-4 response-leg guard broken"
        );
    }
}

// LOAD-BEARING: chunked (no-CL) — the guard is the SOLE discriminator. With the
// Reset→Err(H2RespAbort) injection deleted, the H2 client would get a clean
// Ok(partial) (END_STREAM) = a smuggled-complete leak. Strict predicate
// complete := Ok(_). The non-vacuity arm proves Ok IS observable on this path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_fmd4_response_truncation_chunked_never_complete() {
    let certs = generate_loopback_certs();

    // POSITIVE / non-vacuity arm: a CLEAN echo backend → Ok body (so the strict
    // Ok(_)=leak predicate below is not vacuous).
    {
        let obs = BackendObs::new();
        let (backend, _bh) =
            spawn_h3_echo_backend(certs.cert.clone(), certs.key.clone(), obs.clone()).await;
        let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;
        let body = Bytes::from(vec![0x42u8; 4096]);
        let (status, echoed) = h2_request_with_body(gw, vec![body.clone()]).await;
        assert_eq!(status, 200, "clean arm: expected 200");
        assert_eq!(
            echoed, body,
            "clean arm: body must round-trip complete (non-vacuity)"
        );
    }

    // NEGATIVE arm: chunked (no-CL) truncating backend. complete := Ok(_) — any
    // clean Ok is a smuggled-complete leak (the guard is the sole signal).
    {
        let (backend, _bh) = spawn_h3_truncating_response_backend(
            certs.cert.clone(),
            certs.key.clone(),
            /* declare_cl = */ false,
        )
        .await;
        let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;
        let (status, body_res) = h2_get_collect(gw).await;
        let leaked = body_res.is_ok();
        eprintln!(
            "H2H3_RESP_TRUNC_CHUNKED status={status} body_res={} partial={TRUNC_PARTIAL_LEN} \
             leaked_complete={leaked}",
            match &body_res {
                Ok(b) => format!("Ok(len={})", b.len()),
                Err(e) => format!("Err({e})"),
            }
        );
        assert!(
            !leaked,
            "RESPONSE-SPLITTING (chunked arm): a truncated upstream response was delivered to the \
             H2 client as a CLEAN-terminated body (Ok) — the H2RespAbort guard did not RST_STREAM \
             the H2 response (F-MD-4 response-leg guard broken)"
        );
    }
}

// R13 (b) burst ≥50× on current_thread — load-bearing chunked arm.
#[tokio::test(flavor = "current_thread")]
async fn h2h3_fmd4_response_truncation_chunked_burst_current_thread() {
    const ITERS: usize = 60;
    let certs = generate_loopback_certs();
    for i in 0..ITERS {
        let (backend, bh) = spawn_h3_truncating_response_backend(
            certs.cert.clone(),
            certs.key.clone(),
            /* declare_cl = */ false,
        )
        .await;
        let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;
        let (_status, body_res) = h2_get_collect(gw).await;
        let leaked = body_res.is_ok();
        assert!(
            !leaked,
            "RESPONSE-SPLITTING under chunked burst (iter {i}/{ITERS}): a truncated upstream \
             response was delivered to the H2 client as a CLEAN-terminated body (Ok) — \
             the guard failed to RST_STREAM (F-MD-4 race)"
        );
        bh.abort();
    }
    eprintln!("H2H3_RESP_TRUNC_CHUNKED_BURST iters={ITERS} all-incomplete (no leak)");
}

// ── Test 8: backpressure both legs ───────────────────────────────────────
//
// A slow-reading H3 backend (reads the request body in small bursts, sleeping
// between) forces the gateway's bounded request in-flight window + the upstream
// QUIC send window to backpressure the H2 ingest: the gateway stops pulling the
// inbound H2 body, the H2 flow-control window stalls, and the client parks. The
// proof that backpressure is REAL (not whole-body buffering) is dual:
//  (i)  the round-trip still completes byte-identical for a body ≫ the window
//       (RESUME works after each park), and
//  (ii) the retained-memory gauge stays bounded (≤ 4×window) throughout — a
//       buffering impl would have grown the gauge to the body size.
// Slow H3 backend also backpressures the RESPONSE leg symmetrically (it echoes
// slowly, so the H2 client's slow drain is matched). CF-SATURATION-1: generous
// deadlines so saturation can't false-flake.

async fn spawn_h3_slow_echo_backend(
    cert_path: PathBuf,
    key_path: PathBuf,
    obs: BackendObs,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = socket.local_addr().unwrap();
    let socket = Arc::new(socket);
    let handle = tokio::spawn(async move {
        let Ok(mut cfg) = quiche::Config::new(quiche::PROTOCOL_VERSION) else {
            return;
        };
        let _ = cfg.set_application_protos(&[LB_QUIC_ALPN]);
        cfg.set_max_idle_timeout(30_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        // Deliberately MODEST stream flow-control so the gateway's upstream send
        // window fills quickly → backpressure propagates to the H2 ingest.
        cfg.set_initial_max_data(512 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(128 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(128 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(8);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        if cfg
            .load_cert_chain_from_pem_file(cert_path.to_str().unwrap_or(""))
            .is_err()
            || cfg
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
        let _ = conn.recv(
            in_buf.get_mut(..n).unwrap_or(&mut []),
            quiche::RecvInfo {
                from: peer,
                to: addr,
            },
        );
        let mut rx_tail: Vec<u8> = Vec::new();
        let mut body_acc: Vec<u8> = Vec::new();
        let mut head_seen = false;
        let mut req_fin = false;
        let mut responded = false;
        let mut req_body_total = 0usize;
        let mut resp_out: Vec<u8> = Vec::new();
        let mut resp_sent = 0usize;
        // Throttle the inbound read so the gateway's upstream window stays near-
        // full (backpressure). Read at most READ_BUDGET bytes per loop turn.
        const READ_BUDGET: usize = 16 * 1024;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
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
            if conn.is_established() {
                let mut read_this_turn = 0usize;
                for sid in conn.readable().collect::<Vec<u64>>() {
                    if sid != 0 {
                        continue;
                    }
                    let mut chunk = [0u8; 4096];
                    while read_this_turn < READ_BUDGET {
                        match conn.stream_recv(sid, &mut chunk) {
                            Ok((rn, fin)) => {
                                rx_tail.extend_from_slice(chunk.get(..rn).unwrap_or(&[]));
                                read_this_turn += rn;
                                if fin {
                                    req_fin = true;
                                }
                                if rn == 0 {
                                    break;
                                }
                            }
                            Err(quiche::Error::Done)
                            | Err(quiche::Error::InvalidStreamState(_)) => break,
                            Err(_) => break,
                        }
                    }
                }
                drain_h3_request_frames(
                    &mut rx_tail,
                    &mut head_seen,
                    &mut body_acc,
                    &mut req_body_total,
                    &obs,
                );
                if req_fin && !responded {
                    obs.req_body_bytes.store(req_body_total, Ordering::SeqCst);
                    obs.complete.fetch_add(1, Ordering::SeqCst);
                    let encoder = QpackEncoder::new();
                    let resp_headers = vec![
                        (":status".to_string(), "200".to_string()),
                        ("content-length".to_string(), body_acc.len().to_string()),
                    ];
                    if let Ok(block) = encoder.encode(&resp_headers) {
                        if let Ok(hframe) = encode_frame(&H3Frame::Headers {
                            header_block: block,
                        }) {
                            if let Ok(dframe) = encode_frame(&H3Frame::Data {
                                payload: Bytes::from(body_acc.clone()),
                            }) {
                                resp_out.extend_from_slice(&hframe);
                                resp_out.extend_from_slice(&dframe);
                            }
                        }
                    }
                    responded = true;
                }
                if responded && resp_sent < resp_out.len() {
                    while resp_sent < resp_out.len() {
                        let sl = resp_out.get(resp_sent..).unwrap_or(&[]);
                        match conn.stream_send(0, sl, true) {
                            Ok(ns) if ns > 0 => resp_sent += ns,
                            _ => break,
                        }
                    }
                }
                // Throttle: a small sleep each loop so the inbound read rate is
                // bounded → sustained backpressure on the gateway's ingest.
                if !req_fin {
                    tokio::time::sleep(Duration::from_millis(15)).await;
                }
            }
            let to = conn
                .timeout()
                .unwrap_or(Duration::from_millis(10))
                .min(Duration::from_millis(10));
            match tokio::time::timeout(to, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((rn, from))) => {
                    let _ = conn.recv(
                        in_buf.get_mut(..rn).unwrap_or(&mut []),
                        quiche::RecvInfo { from, to: addr },
                    );
                }
                _ => conn.on_timeout(),
            }
        }
    });
    (addr, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_backpressure_both_legs_round_trips_bounded() {
    use lb_l7::h2_proxy::H2_REQ_MAX_RETAINED_BODY_BYTES;

    let _serial = GAUGE_SERIAL.lock().await;
    H2_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);

    let certs = generate_loopback_certs();
    let obs = BackendObs::new();
    let (backend, _bh) =
        spawn_h3_slow_echo_backend(certs.cert.clone(), certs.key.clone(), obs.clone()).await;
    let gw = spawn_gateway(
        backend,
        certs.ca.clone(),
        HttpTimeouts {
            body: Duration::from_secs(120),
            ..HttpTimeouts::default()
        },
    )
    .await;

    // 1 MiB body ≫ the 64 KiB window and ≫ the backend's 128 KiB stream window,
    // forcing repeated parks. The slow backend reads at ~16 KiB/15 ms.
    let body_size = 1024 * 1024;
    let payload: Vec<u8> = (0..body_size).map(|i| (i % 251) as u8).collect();
    let payload = Bytes::from(payload);
    let chunks: Vec<Bytes> = payload
        .chunks(16 * 1024)
        .map(Bytes::copy_from_slice)
        .collect();

    let (status, echoed) = h2_request_with_body(gw, chunks).await;
    let in_situ = H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    eprintln!(
        "H2H3_BACKPRESSURE status={status} echoed={} body={body_size} retained={in_situ}",
        echoed.len()
    );
    // (i) RESUME works: the round-trip completes byte-identical despite repeated
    // backpressure parks on both legs.
    assert_eq!(status, 200, "backpressured round-trip should still succeed");
    assert_eq!(
        echoed, payload,
        "backpressured body must round-trip byte-identical (resume failed)"
    );
    // (ii) bounded memory throughout: the retained gauge never grew to the body
    // size — backpressure stalled the ingest instead of buffering the whole body.
    assert!(
        in_situ > 0,
        "gauge vacuous: pump never recorded in-flight bytes"
    );
    assert!(
        in_situ <= 4 * WINDOW,
        "retained gauge {in_situ} exceeds 4×window ({}) under backpressure — \
         the ingest buffered instead of stalling (R8 violation)",
        4 * WINDOW
    );
}

// ── §(c)1 connection-header check — does a connection-specific header decoded
// from the H3 backend ever reach the H2 client? ─────────────────────────────
//
// The H2→H3 response head transform (`h2_decoded_resp_head_builder`) uses H2→H2
// semantics: drop `:`-pseudo + lowercase, NO hop-by-hop strip (the H1→H3 cell's
// RESPONSE_HOP_BY_HOP strip is H1-framing-specific). The brief asks: with no
// strip, could a `connection:`/`keep-alive` the H3 backend emits leak to the H2
// client? hyper's H2 server encoder treats connection-specific headers as
// malformed for H2 and OMITS/REJECTS them on the egress (RFC 9113 §8.2.2). This
// test PROVES that: a regular header (`x-keep`) reaches the client (transform
// works) while `connection` does NOT (encoder dropped it). So the missing strip
// is NOT an R12 divergence — no targeted strip needed.

async fn spawn_h3_conn_header_backend(
    cert_path: PathBuf,
    key_path: PathBuf,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = socket.local_addr().unwrap();
    let socket = Arc::new(socket);
    let handle = tokio::spawn(async move {
        let Ok(mut cfg) = quiche::Config::new(quiche::PROTOCOL_VERSION) else {
            return;
        };
        let _ = cfg.set_application_protos(&[LB_QUIC_ALPN]);
        cfg.set_max_idle_timeout(10_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(256 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(8);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        if cfg
            .load_cert_chain_from_pem_file(cert_path.to_str().unwrap_or(""))
            .is_err()
            || cfg
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
        let _ = conn.recv(
            in_buf.get_mut(..n).unwrap_or(&mut []),
            quiche::RecvInfo {
                from: peer,
                to: addr,
            },
        );
        let mut got_head = false;
        let mut responded = false;
        let mut resp_out: Vec<u8> = Vec::new();
        let mut resp_sent = 0usize;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
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
            if conn.is_established() {
                for sid in conn.readable().collect::<Vec<u64>>() {
                    if sid != 0 {
                        continue;
                    }
                    let mut chunk = [0u8; 4096];
                    while let Ok((rn, _fin)) = conn.stream_recv(sid, &mut chunk) {
                        if rn > 0 {
                            got_head = true;
                        }
                    }
                }
                if got_head && !responded {
                    let encoder = QpackEncoder::new();
                    let head = encoder
                        .encode(&[
                            (":status".to_string(), "200".to_string()),
                            ("content-length".to_string(), "2".to_string()),
                            ("x-keep".to_string(), "v".to_string()),
                            // A connection-specific header the H2 egress encoder
                            // must NOT forward to the H2 client.
                            ("connection".to_string(), "keep-alive".to_string()),
                        ])
                        .ok()
                        .and_then(|b| encode_frame(&H3Frame::Headers { header_block: b }).ok());
                    let data = encode_frame(&H3Frame::Data {
                        payload: Bytes::from_static(b"ok"),
                    })
                    .ok();
                    if let (Some(h), Some(d)) = (head, data) {
                        resp_out.extend_from_slice(&h);
                        resp_out.extend_from_slice(&d);
                    }
                    responded = true;
                }
                if responded && resp_sent < resp_out.len() {
                    while resp_sent < resp_out.len() {
                        let sl = resp_out.get(resp_sent..).unwrap_or(&[]);
                        match conn.stream_send(0, sl, true) {
                            Ok(ns) if ns > 0 => resp_sent += ns,
                            _ => break,
                        }
                    }
                }
            }
            let to = conn
                .timeout()
                .unwrap_or(Duration::from_millis(20))
                .min(Duration::from_millis(20));
            match tokio::time::timeout(to, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((rn, from))) => {
                    let _ = conn.recv(
                        in_buf.get_mut(..rn).unwrap_or(&mut []),
                        quiche::RecvInfo { from, to: addr },
                    );
                }
                _ => conn.on_timeout(),
            }
        }
    });
    (addr, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2h3_connection_header_not_forwarded_to_h2_client() {
    use h2 as h2crate;
    let certs = generate_loopback_certs();
    let (backend, _bh) = spawn_h3_conn_header_backend(certs.cert.clone(), certs.key.clone()).await;
    let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;

    let stream = TcpStream::connect(gw).await.unwrap();
    let (h2, conn) = h2crate::client::handshake(stream).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut h2 = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("GET")
        .uri(format!("http://{TEST_SNI}/conn"))
        .body(())
        .unwrap();
    let (resp_fut, _send) = h2.send_request(req, true).unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(15), resp_fut)
        .await
        .expect("response timed out")
        .expect("response head failed");
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let has_conn = headers.contains_key("connection");
    let has_keep = headers
        .get("x-keep")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    eprintln!("H2H3_CONN_HEADER status={status} has_connection={has_conn} x-keep={has_keep:?}");
    assert_eq!(status, 200, "response head did not reach the H2 client");
    // Transform works: a regular header survives.
    assert_eq!(
        has_keep.as_deref(),
        Some("v"),
        "the regular x-keep header did NOT reach the H2 client (transform regression)"
    );
    // §(c)1 ANSWER: the connection-specific header is NOT forwarded. hyper's H2
    // server encoder omits it (no targeted strip needed in the head builder).
    assert!(
        !has_conn,
        "DIVERGENCE: a `connection` header decoded from the H3 backend was \
         forwarded to the H2 client — the H2 egress encoder did NOT drop it, so \
         h2_decoded_resp_head_builder needs a targeted hop-by-hop strip (R12)"
    );
}
