//! S12 H1→H3 (R8) BUILT-bar — real-wire H1 client → real `H1Proxy` listener
//! (h3_upstream wired) → real H3/QUIC backend, exercising the STREAMING H1→H3
//! cell (`proxy_h1_to_h3` on `lb_quic::stream_request_to_h3_upstream`).
//!
//! Coverage (mirrors the S11 BUILT bar shape; the prior H1→H3 coverage was only
//! a bodyless GET in `proto_translation_e2e.rs`):
//!  1. binary body byte-identical BOTH directions (request → H3 backend,
//!     response → H1 client), crossing `H3_BODY_CHUNK_MAX`.
//!  2. request-trailer FORWARDING (locks `forward_req_trailers=true`).
//!  3. F-MD-4 H3 mirror — RESET-without-FIN on a truncated upload, with R13:
//!     (a) in the parallel `--all-features` gate, (b) an in-suite isolation-burst
//!     ≥50 iterations on a `current_thread` runtime, (c) a LOAD-BEARING negative
//!     control (a clean upload yields `complete>=1`; a truncated one yields
//!     `complete==0` — gate on the QUIC stream FIN the backend observes).
//!  4. F-CAP-1 — pre-data over-cap → 413; mid-body over-cap → RESET (no 413, no
//!     clean FIN); pre-dial-down → 502.
//!  5. gRPC-shaped response trailers — EMPIRICAL: does `grpc-status` reach the
//!     H1 client (CF-RESP-1 / CASE-ii)? The test records the observed outcome.
//!
//! CF-SATURATION-1: the over-cap test gives the gateway listener a generous body
//! timeout so 8-core gate saturation cannot false-504 a large push.

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
use hyper_util::rt::TokioIo;
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::quic_pool::{QuicPoolConfig, QuicUpstreamPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts};
use lb_l7::upstream::{RoundRobinUpstreams, UpstreamBackend};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

use lb_h3::{H3Frame, QpackEncoder, encode_frame};

const TEST_SNI: &str = "expressgateway.test";
const LB_QUIC_ALPN: &[u8] = b"lb-quic";
const MAX_UDP: usize = 65_535;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

// ── Shared cert + pool + factory helpers (mirrors proto_translation_e2e) ──

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
        "h1h3-md-verify-{}-{}-{counter}",
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

/// Observations the H3 backend records for assertions.
#[derive(Clone)]
struct BackendObs {
    /// Total request DATA bytes the backend read off stream 0.
    req_body_bytes: Arc<AtomicUsize>,
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
            complete: Arc::new(AtomicUsize::new(0)),
            saw_req_trailers: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// Spawn an H3 backend that reads the whole request body on stream 0, records
/// (bytes, clean-FIN, trailers-seen), and — once the stream FINs cleanly —
/// echoes the body back with status 200 (so byte-identity is checkable both
/// directions). On a truncated upload (RESET / no FIN) it NEVER responds and
/// NEVER increments `complete`. `resp_body` overrides the echo when `Some`.
async fn spawn_h3_echo_backend(
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
        cfg.set_max_idle_timeout(10_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(16 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(8 * 1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(8 * 1024 * 1024);
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
        // Resumable response send: the full encoded response (HEADERS+DATA) and a
        // running send offset, so a large response is flushed across loop
        // iterations as the QUIC stream send window opens (a single-pass send
        // would stall once the window fills).
        let mut resp_out: Vec<u8> = Vec::new();
        let mut resp_sent: usize = 0;
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

                // Manual frame-header parse of rx_tail: HEADERS / DATA / trailing
                // HEADERS. We only need byte accounting + trailer detection.
                drain_h3_request_frames(
                    &mut rx_tail,
                    &mut decoded_status_seen,
                    &mut body_acc,
                    &mut req_body_total,
                    &obs,
                );

                // Build the response ONCE after a CLEAN FIN (so a truncated
                // upload never gets a response and never counts as complete).
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

                // Resumable response flush: push as much as the stream send
                // window allows this iteration; FIN on the last byte.
                if responded && resp_sent < resp_out.len() {
                    while resp_sent < resp_out.len() {
                        let sl = resp_out.get(resp_sent..).unwrap_or(&[]);
                        let fin = true; // sl is always the remaining tail → FIN with the last byte
                        match conn.stream_send(0, sl, fin) {
                            Ok(ns) if ns > 0 => resp_sent += ns,
                            _ => break, // window full → resume next iteration
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
/// accumulate DATA payload into `body_acc` + the running byte total. Leaves a
/// partial trailing frame in `rx_tail`.
fn drain_h3_request_frames(
    rx_tail: &mut Vec<u8>,
    head_seen: &mut bool,
    body_acc: &mut Vec<u8>,
    body_total: &mut usize,
    obs: &BackendObs,
) {
    use lb_h3::decode_varint;
    loop {
        // Need at least the two varints (type, len).
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

// ── Gateway listener (H1 front, h3_upstream wired) ───────────────────────

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
    let h1_proxy = Arc::new(
        H1Proxy::with_multi_proto(
            tcp_pool, picker, None, timeouts, /* is_https = */ false,
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
            let p = Arc::clone(&h1_proxy);
            tokio::spawn(async move {
                let _ = p.serve_connection(sock, peer).await;
            });
        }
    });
    gw
}

/// Send an H1 request with the given body to the gateway, return
/// (status, response-body-bytes). Uses a streamed request body so the gateway's
/// pump path (not a single buffered frame) is exercised.
async fn h1_request_with_body(gw: SocketAddr, body_chunks: Vec<Bytes>) -> (u16, Bytes) {
    let stream = TcpStream::connect(gw).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let (btx, brx) =
        tokio::sync::mpsc::channel::<Result<Frame<Bytes>, std::convert::Infallible>>(4);
    tokio::spawn(async move {
        for c in body_chunks {
            if btx.send(Ok(Frame::data(c))).await.is_err() {
                return;
            }
        }
    });
    let body = StreamBody::new(tokio_stream_recv(brx));
    let req = Request::builder()
        .method("POST")
        .uri("/echo")
        .header("host", TEST_SNI)
        .body(body)
        .unwrap();
    let resp = match tokio::time::timeout(Duration::from_secs(30), sender.send_request(req)).await {
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

/// Bridge an mpsc Receiver into a Stream of body frames (tokio-stream is not a
/// dep here, so hand-roll the poll_fn the same way the proxy does).
fn tokio_stream_recv(
    mut rx: tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, std::convert::Infallible>>,
) -> impl futures_util::Stream<Item = Result<Frame<Bytes>, std::convert::Infallible>> {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

// ── Test 1: binary body byte-identical BOTH directions ───────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h1h3_binary_body_byte_identical_both_directions() {
    let certs = generate_loopback_certs();
    let obs = BackendObs::new();
    let (backend, _bh) =
        spawn_h3_echo_backend(certs.cert.clone(), certs.key.clone(), obs.clone()).await;
    let gw = spawn_gateway(backend, certs.ca.clone(), HttpTimeouts::default()).await;

    // A body that crosses H3_BODY_CHUNK_MAX (8 KiB) several times, sent as
    // multiple H1 chunks so the streaming pump (not a single frame) runs.
    let mut payload = Vec::new();
    for i in 0..40_000u32 {
        payload.extend_from_slice(&i.to_le_bytes());
    }
    let payload = Bytes::from(payload); // 160 000 bytes — crosses H3_BODY_CHUNK_MAX (8 KiB) ~20×
    let chunks: Vec<Bytes> = payload
        .chunks(7_000)
        .map(|c| Bytes::copy_from_slice(c))
        .collect();

    let (status, echoed) = h1_request_with_body(gw, chunks).await;
    eprintln!(
        "H1H3_BYTE_IDENTICAL status={status} sent={} echoed={} backend_body_bytes={} complete={}",
        payload.len(),
        echoed.len(),
        obs.req_body_bytes.load(Ordering::SeqCst),
        obs.complete.load(Ordering::SeqCst),
    );
    assert_eq!(status, 200, "expected 200 from the H3 echo backend");
    // Request direction: the H3 backend received every request byte.
    assert_eq!(
        obs.req_body_bytes.load(Ordering::SeqCst),
        payload.len(),
        "request body bytes mismatch at the H3 backend (request-leg byte-identity)"
    );
    // Response direction: the H1 client received the echoed body verbatim.
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
