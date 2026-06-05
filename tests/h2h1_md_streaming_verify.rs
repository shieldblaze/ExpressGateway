//! S8 / M-D (H2→H1) — INDEPENDENT verifier suite (author≠builder).
//!
//! These tests are written by `verifier` against builder-1's M-D pump
//! (`crates/lb-l7/src/h2_proxy.rs::proxy_request`, tip 77ae94b8). They do
//! NOT edit the source. They re-prove the BUILT bar on the real wire:
//!
//!   1. real-wire binary bodies both directions, byte-identical, for the
//!      within-window (Branch A) and over-window (Branch B) regimes;
//!   2. non-vacuous retained-memory gauge + the load-bearing inverted probe;
//!   3. bidirectional backpressure with a proven causal chain;
//!   4. over-window adversarial malformed body/trailers, no downstream leak,
//!      upstream not poisoned;
//!   5. Deviation #1 — legitimate over-window upload to a backend that
//!      replies early without reading the body (suspected 413 regression);
//!   6. smuggling parity — client RST mid-body never seen as a complete
//!      request at the H1 upstream.
//!
//! Harness shape mirrors `tests/h2_validation_before_forward.rs` and
//! `tests/h2_security_live.rs` (real rustls TLS, real H1 backend on a real
//! socket). The frontend client is the genuine `h2` crate over TLS, except
//! the raw-framing cases (a pseudo-header in trailers / content-length≠ΣDATA
//! cannot be expressed by a well-behaved h2 client) which use raw frames.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Bytes, Frame, Incoming};
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
const WINDOW: usize = 64 * 1024; // H2_REQ_CHANNEL_DEPTH * H2_REQ_CHUNK_MAX

// ── TLS / pool plumbing (same as the existing harnesses) ───────────────

fn make_cert_for(san: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
    let cert_der = generated.cert.der().to_vec();
    let key_der = generated.signing_key.serialize_der();
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

/// Spawn the h1s listener fronting an arbitrary backend address, returning
/// the gateway addr + trust anchor. Uses [`HttpTimeouts::default`] — the
/// common case. Do NOT change the default here: several tests assert on the
/// default body/header/total budgets, so a longer timeout is opt-in via
/// [`spawn_listener_for_with_timeouts`] instead.
async fn spawn_listener_for(backend_addr: SocketAddr) -> (SocketAddr, CertificateDer<'static>) {
    spawn_listener_for_with_timeouts(backend_addr, HttpTimeouts::default()).await
}

/// As [`spawn_listener_for`] but with caller-chosen gateway [`HttpTimeouts`].
/// Used only by `fcap1_h2_over_cap_upload_yields_413`, which needs a longer
/// body timeout so a 64 MiB push reliably crosses the cap under 8-core gate
/// saturation (CF-SATURATION-1). `HttpTimeouts` is `Copy`, so the same value
/// feeds both the H1 and H2 proxies.
async fn spawn_listener_for_with_timeouts(
    backend_addr: SocketAddr,
    timeouts: HttpTimeouts,
) -> (SocketAddr, CertificateDer<'static>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let h1_proxy = Arc::new(H1Proxy::new(
        pool.clone(),
        Arc::clone(&picker) as _,
        None,
        timeouts,
        true,
    ));
    let h2_proxy = Arc::new(H2Proxy::with_security(
        pool,
        picker as _,
        None,
        timeouts,
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

// ── Backends ───────────────────────────────────────────────────────────

/// Echo backend: returns the request body verbatim with status 200. Used
/// for byte-identical real-wire round-trips. Counts dials.
async fn spawn_echo_backend() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&dials);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let svc = service_fn(|req: HyperRequest<Incoming>| async move {
                    // Fully read the request body, echo it back verbatim.
                    let collected = req.into_body().collect().await;
                    let body = match collected {
                        Ok(b) => b.to_bytes(),
                        Err(_) => Bytes::new(),
                    };
                    Ok::<_, Infallible>(
                        HyperResponse::builder()
                            .status(StatusCode::OK)
                            .body(http_body_util::Full::new(body))
                            .unwrap(),
                    )
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, dials)
}

/// Backend that returns a fixed binary response body (does NOT read the
/// request body fully — exercises the response leg).
async fn spawn_binary_response_backend(resp_body: &'static [u8]) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&dials);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let svc = service_fn(move |req: HyperRequest<Incoming>| async move {
                    // Drain the body so the H1 keep-alive stays consistent.
                    let _ = req.into_body().collect().await;
                    Ok::<_, Infallible>(
                        HyperResponse::builder()
                            .status(StatusCode::OK)
                            .body(http_body_util::Full::new(Bytes::from_static(resp_body)))
                            .unwrap(),
                    )
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, dials)
}

// ── Raw H2 framing (for malformed cases the h2 crate cannot express) ───

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
    s.write_all(&hdr).await.unwrap();
    s.write_all(payload).await.unwrap();
    s.flush().await.unwrap();
}

fn hpack_literal(name: &str, value: &str) -> Vec<u8> {
    let mut out = vec![0x00];
    assert!(name.len() < 128 && value.len() < 128);
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
    out
}

async fn send_preface(s: &mut TlsStream<TcpStream>) {
    s.write_all(H2_PREFACE).await.unwrap();
    write_frame(s, 0x04, 0x00, 0, &[]).await; // empty SETTINGS
    write_frame(s, 0x04, 0x01, 0, &[]).await; // SETTINGS ACK
}

/// Read frames until a server-initiated RST_STREAM/GOAWAY (mandated
/// rejection) OR a DATA frame on stream 1 (the leaked backend body —
/// the defect). Returns Ok(()) on RST/GOAWAY/clean-EOF/transport-reset
/// without any leaked DATA; Err on a DATA leak or timeout.
async fn expect_protocol_error_not_backend_body(
    s: &mut TlsStream<TcpStream>,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut buf = vec![0u8; 16 * 1024];
    let mut acc: Vec<u8> = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err("timeout: neither RST/GOAWAY nor DATA observed".into());
        }
        let n = match tokio::time::timeout(left, s.read(&mut buf)).await {
            Ok(Ok(0)) => return Ok(()),
            Ok(Ok(n)) => n,
            Ok(Err(_)) => return Ok(()),
            Err(_) => return Err("timeout reading server frames".into()),
        };
        acc.extend_from_slice(&buf[..n]);
        let mut pos = 0;
        while acc.len() - pos >= 9 {
            let flen = ((acc[pos] as usize) << 16)
                | ((acc[pos + 1] as usize) << 8)
                | (acc[pos + 2] as usize);
            let fty = acc[pos + 3];
            if acc.len() - pos < 9 + flen {
                break;
            }
            let sid = u32::from_be_bytes([
                acc[pos + 5] & 0x7f,
                acc[pos + 6],
                acc[pos + 7],
                acc[pos + 8],
            ]);
            match fty {
                0x07 => return Ok(()), // GOAWAY
                0x03 => return Ok(()), // RST_STREAM
                0x00 if sid == 1 => {
                    return Err(format!(
                        "LEAK: backend DATA frame (len:{flen}) relayed for a \
                         malformed >window request"
                    ));
                }
                _ => {}
            }
            pos += 9 + flen;
        }
        acc.drain(..pos);
    }
}

// ══════════════════════════════════════════════════════════════════════
// 1. REAL-WIRE binary bodies both directions, byte-identical.
// ══════════════════════════════════════════════════════════════════════

/// Deterministic non-UTF-8 byte pattern of length `n` (includes 0x00 and
/// bytes > 0x7f so a UTF-8 round-trip would corrupt it).
fn binary_pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 31 + 17) % 256) as u8).collect()
}

/// Round-trip a `body_len`-byte request body to the echo backend (which
/// echoes it verbatim, so the response body is also `body_len` bytes),
/// reading the response with the CORRECT per-chunk `release_capacity`
/// pattern, and assert byte-identical. The round-1 harness used the BUGGY
/// cumulative `release_capacity(got.len())` here, which stalled the response
/// read — see `harness_bug_*` for the proof that that is a reader bug, not a
/// gateway defect.
async fn real_wire_roundtrip(body_len: usize) {
    let (backend, dials) = spawn_echo_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;
    let (h2, conn) = h2::client::handshake(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/echo"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();

    let payload = binary_pattern(body_len);
    // Confirm the body is genuinely non-UTF-8.
    assert!(
        std::str::from_utf8(&payload).is_err() || body_len == 0,
        "test payload should be non-UTF-8"
    );
    let sent = payload.clone();
    let writer = async move {
        // Stream in 16 KiB pieces so >window bodies exercise the streaming
        // path (Branch B) with multiple chunks.
        let mut off = 0;
        while off < sent.len() {
            let end = (off + 16 * 1024).min(sent.len());
            let chunk = Bytes::copy_from_slice(&sent[off..end]);
            // Reserve capacity, then send.
            send_body.reserve_capacity(chunk.len());
            // Wait for capacity (flow control) — drives real WINDOW_UPDATE.
            loop {
                match futures_util::future::poll_fn(|cx| send_body.poll_capacity(cx)).await {
                    Some(Ok(cap)) if cap > 0 => break,
                    Some(Ok(_)) => continue,
                    Some(Err(_)) | None => return,
                }
            }
            if send_body.send_data(chunk, false).is_err() {
                return;
            }
            off = end;
        }
        let _ = send_body.send_data(Bytes::new(), true);
    };

    let reader = async {
        let resp = tokio::time::timeout(Duration::from_secs(30), resp_fut)
            .await
            .expect("response timed out")
            .expect("response errored");
        assert_eq!(resp.status(), 200, "echo backend should reply 200");
        let mut body = resp.into_body();
        let mut got = Vec::new();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.expect("body data error");
            let n = chunk.len();
            got.extend_from_slice(&chunk);
            // CORRECT: release exactly the bytes this chunk consumed, so the
            // WINDOW_UPDATE replenishes the stream window and the server can
            // keep flushing. (The round-1 bug passed the cumulative
            // `got.len()`, over-releasing → Err → no WINDOW_UPDATE → stall.)
            let _ = body.flow_control().release_capacity(n);
        }
        got
    };

    let (_, got) = tokio::join!(writer, reader);
    assert_eq!(
        got.len(),
        payload.len(),
        "round-trip length mismatch ({} dials)",
        dials.load(Ordering::SeqCst)
    );
    assert_eq!(got, payload, "round-trip bytes are NOT byte-identical");
    assert_eq!(dials.load(Ordering::SeqCst), 1, "exactly one backend dial");
}

/// ≤window (Branch A, zero-dial-validate) — 8 KiB binary body round-trips
/// byte-identical on the real wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_wire_small_body_byte_identical() {
    real_wire_roundtrip(8 * 1024).await;
}

/// >window (Branch B, streaming) — 512 KiB binary body round-trips
/// byte-identical on the real wire, exercising the bounded pump. Reads the
/// response with the CORRECT per-chunk `release_capacity` (the round-1
/// harness over-released cumulatively → false UnexpectedEof; see
/// `harness_bug_cumulative_release_stall_*` for the proof).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_wire_large_body_byte_identical() {
    real_wire_roundtrip(512 * 1024).await;
}

// ── HARNESS-BUG ADJUDICATION (R2: prove the mechanism, do not trust it) ──
//
// The round-1 `real_wire_large_body_byte_identical` failed with
// UnexpectedEof at ~65535 bytes on the RESPONSE read. The claim is a HARNESS
// reader bug (cumulative `release_capacity`), NOT a gateway defect. Three
// independent confirmations:
//
//  (A) the SAME 512 KiB body, read with the CORRECT per-chunk pattern,
//      round-trips byte-identical (above: real_wire_large_body_byte_identical);
//  (B) an INDEPENDENT client (`reqwest` with its own H2 stack, which manages
//      flow control internally) relays the full 512 KiB byte-identical →
//      the gateway response leg is genuinely correct;
//  (C) the SAME cumulative pattern STALLS the response read for a LARGE
//      RESPONSE produced by a SMALL (Branch-A) request — proving the stall is
//      reader-pattern-dependent, independent of the request-side branch.

/// Backend that returns a fixed-size large binary response body REGARDLESS of
/// request size (drains the request first). Used to drive a large response
/// from a SMALL Branch-A request.
async fn spawn_large_response_backend(resp_len: usize) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&dials);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let svc = service_fn(move |req: HyperRequest<Incoming>| async move {
                    let _ = req.into_body().collect().await;
                    let body = Bytes::from(binary_pattern(resp_len));
                    Ok::<_, Infallible>(
                        HyperResponse::builder()
                            .status(StatusCode::OK)
                            .body(http_body_util::Full::new(body))
                            .unwrap(),
                    )
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, dials)
}

/// (B) INDEPENDENT-CLIENT cross-check: reqwest (its own H2 + flow control)
/// relays the full 512 KiB response byte-identical. If THIS fails, the
/// gateway response leg is a real defect — but it passes, so the round-1
/// failure is the harness reader, not the gateway.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn harness_bug_reqwest_independent_512k_byte_identical() {
    let body_len = 512 * 1024;
    let (backend, dials) = spawn_echo_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;

    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::tls::Certificate::from_der(&anchor).unwrap())
        .https_only(true)
        .http2_prior_knowledge()
        .resolve(SAN_HOST, gw)
        .build()
        .unwrap();

    let payload = binary_pattern(body_len);
    let resp = client
        .post(format!("https://{SAN_HOST}:{}/echo", gw.port()))
        .body(payload.clone())
        .send()
        .await
        .expect("reqwest send failed");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "expected 200 from echo backend"
    );
    let got = resp.bytes().await.expect("reqwest body read failed");
    assert_eq!(
        got.len(),
        payload.len(),
        "reqwest round-trip length mismatch"
    );
    assert_eq!(
        got.as_ref(),
        payload.as_slice(),
        "reqwest round-trip NOT byte-identical — gateway response leg is a REAL defect"
    );
    assert_eq!(dials.load(Ordering::SeqCst), 1, "exactly one backend dial");
    eprintln!("HARNESS_CROSSCHECK reqwest 512KiB byte-identical OK (independent H2 client)");
}

/// (C) The buggy cumulative pattern STALLS the response read for a
/// SMALL-request / LARGE-response (Branch-A request) case → the stall is
/// reader-pattern-dependent, NOT request-side. We expect either a timeout or
/// a short (≤ ~65535) read with the cumulative pattern. The SAME case with
/// the correct per-chunk pattern relays the full response.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn harness_bug_cumulative_release_stall_is_reader_side() {
    let resp_len = 512 * 1024;
    let req_len = 1024; // tiny → Branch A on the gateway (≤ 64 KiB window).

    // ── Correct per-chunk reader: full response relays byte-identical. ──
    {
        let (backend, _dials) = spawn_large_response_backend(resp_len).await;
        let (gw, anchor) = spawn_listener_for(backend).await;
        let tls = connect_tls(gw, anchor).await;
        let (h2, conn) = h2::client::handshake(tls).await.unwrap();
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let mut sender = h2.ready().await.unwrap();
        let req = http::Request::builder()
            .method("POST")
            .uri(format!("https://{SAN_HOST}/big"))
            .body(())
            .unwrap();
        let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();
        let _ = send_body.send_data(Bytes::from(binary_pattern(req_len)), true);
        let resp = tokio::time::timeout(Duration::from_secs(15), resp_fut)
            .await
            .expect("response timed out (per-chunk should NOT stall)")
            .expect("response errored");
        assert_eq!(resp.status(), 200);
        let mut body = resp.into_body();
        let mut got = 0usize;
        while let Some(chunk) = tokio::time::timeout(Duration::from_secs(15), body.data())
            .await
            .expect("per-chunk reader stalled — unexpected")
        {
            let n = chunk.expect("body data error").len();
            got += n;
            let _ = body.flow_control().release_capacity(n);
        }
        assert_eq!(
            got, resp_len,
            "per-chunk reader: full large response relayed"
        );
        eprintln!("HARNESS_CROSSCHECK per-chunk Branch-A large-response full={got} OK");
    }

    // ── Buggy cumulative reader: same response leg STALLS at ~65535. ──
    {
        let (backend, _dials) = spawn_large_response_backend(resp_len).await;
        let (gw, anchor) = spawn_listener_for(backend).await;
        let tls = connect_tls(gw, anchor).await;
        let (h2, conn) = h2::client::handshake(tls).await.unwrap();
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let mut sender = h2.ready().await.unwrap();
        let req = http::Request::builder()
            .method("POST")
            .uri(format!("https://{SAN_HOST}/big"))
            .body(())
            .unwrap();
        let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();
        let _ = send_body.send_data(Bytes::from(binary_pattern(req_len)), true);
        let resp = tokio::time::timeout(Duration::from_secs(15), resp_fut)
            .await
            .expect("response head timed out")
            .expect("response errored");
        assert_eq!(resp.status(), 200);
        let mut body = resp.into_body();
        let mut got = 0usize;
        let mut stalled = false;
        // Read with the CUMULATIVE pattern; bound each read with a timeout so
        // the stall is observable rather than hanging the test.
        loop {
            match tokio::time::timeout(Duration::from_secs(6), body.data()).await {
                Ok(Some(Ok(chunk))) => {
                    got += chunk.len();
                    // BUGGY: cumulative — over-releases from chunk 2 on.
                    let _ = body.flow_control().release_capacity(got);
                }
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => {
                    stalled = true;
                    break;
                }
            }
        }
        // The defining symptom: the cumulative reader CANNOT relay the full
        // response — it STALLS (read timeout) having received only a small
        // fraction (a couple of stream windows' worth before the cumulative
        // `release_capacity` first over-releases, errors and stops sending
        // WINDOW_UPDATEs). The exact stall point is timing-dependent (the
        // number of valid WINDOW_UPDATEs that landed before the over-release),
        // so we bound it loosely: it must stall AND fall well short of the
        // full response (≤ a quarter), not pin to an exact byte count.
        assert!(
            stalled,
            "cumulative reader did NOT stall (got {got} of {resp_len}) — the \
             harness-bug hypothesis (no WINDOW_UPDATE after over-release) is WRONG"
        );
        assert!(
            got < resp_len / 4,
            "cumulative reader received {got} of {resp_len} bytes — too much; \
             the over-release stall did not bound it well below the full \
             response as hypothesized"
        );
        eprintln!(
            "HARNESS_CROSSCHECK cumulative Branch-A large-response stalled={stalled} got={got} \
             (≪ full {resp_len}) — reader-pattern-dependent, NOT request-side"
        );
    }
}

/// DIAGNOSTIC: report the status the H2 client receives for a LEGITIMATE
/// well-formed upload at sizes straddling the 64 KiB window, against a
/// normal echo backend that fully reads the body. Used to localize the
/// Branch-B 413 regression boundary. (Reported in the verify doc.)
async fn probe_status_for(body_len: usize) -> (Option<u16>, usize) {
    let (backend, dials) = spawn_echo_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;
    let mut builder = h2::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/probe"))
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
        let _ = send_body.send_data(Bytes::new(), true);
    });
    let status = match tokio::time::timeout(Duration::from_secs(20), resp_fut).await {
        Ok(Ok(resp)) => Some(resp.status().as_u16()),
        Ok(Err(_)) => None,
        Err(_) => None,
    };
    let _ = writer.await;
    (status, dials.load(Ordering::SeqCst))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn diag_legit_upload_status_across_window_boundary() {
    for size in [
        32 * 1024,
        64 * 1024,
        64 * 1024 + 1,
        64 * 1024 + 16 * 1024,
        128 * 1024,
        512 * 1024,
    ] {
        let (status, dials) = probe_status_for(size).await;
        eprintln!("DIAG legit_upload size={size} status={status:?} dials={dials}");
    }
}

/// DIAGNOSTIC 2: a backend that reports how many request-body bytes it
/// received before the connection ended. Localizes whether Branch B sends
/// ANY body upstream before the spurious 413.
async fn spawn_body_counting_backend() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let body_bytes = Arc::new(AtomicUsize::new(0));
    let bb2 = Arc::clone(&body_bytes);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let bb3 = Arc::clone(&bb2);
            tokio::spawn(async move {
                // Raw-read: count every byte after the header terminator,
                // never replying, so we see exactly how much body the
                // gateway streamed before it gave up.
                let mut buf = [0u8; 64 * 1024];
                let mut seen_headers = false;
                let mut acc: Vec<u8> = Vec::new();
                loop {
                    // S11: 30 s (was 3 s). S19: 90 s. The over-cap test streams
                    // 66 MiB and the backend must keep draining for the whole
                    // upload; under the full `--workspace --all-features` gate at
                    // 8-core saturation a >3 s scheduling gap in THIS backend task
                    // fired the read timeout, closed the upstream, and the gateway
                    // (correctly) returned 502 for a genuinely-dropped upstream —
                    // a test-harness fragility, not a product defect.
                    //
                    // S19 re-hardening: 30 s was still the SHORTEST of the three
                    // timeouts (backend-read 30 s < gateway-body 120 s < client
                    // wait 130 s), so under load a >30 s forwarding gap fired HERE
                    // first, dropping the upstream and yielding a 502 BEFORE the
                    // 64 MiB cap could trip → the cap-trip 413 lost the race.
                    // Reproduced ~1/13 in isolation under CPU contention:
                    //   FCAP1_H2_OVER_CAP status=Some(502) written=51314688
                    //   backend_body_bytes=51369274
                    // i.e. the backend had drained ~51 MiB (< the 64 MiB cap) when
                    // its 30 s read timeout fired. Bumping to 90 s gives ~10×
                    // starvation margin over the observed normal-completion budget
                    // (the unsaturated push completes in well under 10 s) yet stays
                    // STRICTLY BELOW the gateway body timeout (120 s) and the client
                    // wait (130 s), so on a TRUE wedge the gateway's own bounded
                    // arm — not this backend — terminates the test. The cap-trip →
                    // 413 assertion is UNCHANGED. (Mirror of the S11
                    // reload_zero_drop hardening; closes the backend-side gap the
                    // S11 30 s left as the weak link.)
                    match tokio::time::timeout(Duration::from_secs(90), sock.read(&mut buf)).await {
                        Ok(Ok(0)) | Err(_) => break,
                        Ok(Ok(n)) => {
                            if !seen_headers {
                                acc.extend_from_slice(&buf[..n]);
                                if let Some(p) = acc.windows(4).position(|w| w == b"\r\n\r\n") {
                                    seen_headers = true;
                                    let body_in_acc = acc.len() - (p + 4);
                                    bb3.fetch_add(body_in_acc, Ordering::SeqCst);
                                }
                            } else {
                                bb3.fetch_add(n, Ordering::SeqCst);
                            }
                        }
                        Ok(Err(_)) => break,
                    }
                }
            });
        }
    });
    (local, body_bytes)
}

/// Echo backend (real hyper H1 service) that ALSO records how many body
/// bytes its service observed. Distinguishes "body never flowed" from
/// "body flowed but gateway still 413'd".
async fn spawn_echo_counting_backend() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let body_seen = Arc::new(AtomicUsize::new(0));
    let bs2 = Arc::clone(&body_seen);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let bs3 = Arc::clone(&bs2);
            tokio::spawn(async move {
                let bs4 = Arc::clone(&bs3);
                let svc = service_fn(move |req: HyperRequest<Incoming>| {
                    let bs5 = Arc::clone(&bs4);
                    async move {
                        let mut body = req.into_body();
                        let mut total = 0usize;
                        while let Some(frame) = futures_util::StreamExt::next(
                            &mut http_body_util::BodyStream::new(&mut body),
                        )
                        .await
                        {
                            if let Ok(f) = frame {
                                if let Some(d) = f.data_ref() {
                                    total += d.len();
                                    bs5.fetch_add(d.len(), Ordering::SeqCst);
                                }
                            }
                        }
                        Ok::<_, Infallible>(
                            HyperResponse::builder()
                                .status(StatusCode::OK)
                                .header("x-body-seen", total.to_string())
                                .body(http_body_util::Full::new(Bytes::new()))
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
    (local, body_seen)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn diag_branch_b_echo_body_seen() {
    let body_size = 512 * 1024;
    let (backend, body_seen) = spawn_echo_counting_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;
    let mut builder = h2::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/echocount"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();
    let payload = binary_pattern(body_size);
    let writer = tokio::spawn(async move {
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
        let _ = send_body.send_data(Bytes::new(), true);
    });
    let status = match tokio::time::timeout(Duration::from_secs(10), resp_fut).await {
        Ok(Ok(resp)) => Some(resp.status().as_u16()),
        _ => None,
    };
    let _ = writer.await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    eprintln!(
        "DIAG branch_b echo_body_seen={} of {body_size}, client_status={status:?}",
        body_seen.load(Ordering::SeqCst)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn diag_branch_b_body_bytes_reaching_upstream() {
    let body_size = 512 * 1024;
    let (backend, body_bytes) = spawn_body_counting_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;
    let mut builder = h2::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/count"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();
    let payload = binary_pattern(body_size);
    let writer = tokio::spawn(async move {
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
        let _ = send_body.send_data(Bytes::new(), true);
    });
    let status = match tokio::time::timeout(Duration::from_secs(10), resp_fut).await {
        Ok(Ok(resp)) => Some(resp.status().as_u16()),
        _ => None,
    };
    let _ = writer.await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    eprintln!(
        "DIAG branch_b body_bytes_reaching_upstream={} of {body_size}, client_status={status:?}",
        body_bytes.load(Ordering::SeqCst)
    );
}

// ══════════════════════════════════════════════════════════════════════
// 2. NON-VACUOUS MEMORY gauge + load-bearing inverted probe.
//
// Drive a 4 MiB binary body through a STALLED backend; read
// `H2_REQ_MAX_RETAINED_BODY_BYTES`. A non-vacuous gauge must reflect
// ACTUAL retained bytes and stay ≤ ~256 KiB while ≪ 4 MiB.
//
// INVERTED PROBE: prove the gauge is LOAD-BEARING by feeding
// `record_retained` what a whole-body-buffering impl WOULD retain (the
// full 4 MiB) and asserting the bound trips. If the production gauge in
// the streaming branch only ever stores the CONSTANT 64 KiB regardless of
// real occupancy, the in-situ proof is vacuous — reported as a FINDING.
// ══════════════════════════════════════════════════════════════════════

// S8 gate fix (global-state collision, R2): the gauge tests share the
// process-global `H2_REQ_MAX_RETAINED_BODY_BYTES`, and the inverted-probe test
// deliberately writes a 4 MiB sentinel into it. Under the Phase-3 gate's
// 8-thread parallelism that write leaked into a concurrent gauge test's
// reset→read window (run-3 flake: peak read as exactly 4194304). This lock
// serializes the gauge tests so a measurement window never overlaps another
// gauge test's deliberate write. Background non-gauge proxy tests only record
// per-request retained (≤ ~136 KiB ≪ the 256 KiB bound), so they need no lock.
#[cfg(feature = "test-gauges")]
static GAUGE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(feature = "test-gauges")]
async fn spawn_stalled_backend() -> (SocketAddr, Arc<AtomicUsize>, Arc<tokio::sync::Notify>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let d2 = Arc::clone(&dials);
    let r2 = Arc::clone(&release);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            let r3 = Arc::clone(&r2);
            tokio::spawn(async move {
                // Read only the request headers (a tiny prefix) then STALL:
                // never read the body. This stops draining the backend's
                // socket recv buffer; the gateway's hyper H1 sender cannot
                // flush more body → its mpsc fills → the pump parks → the H2
                // client's flow-control window closes. Hold until released.
                let mut tmp = [0u8; 256];
                let _ = sock.read(&mut tmp).await;
                r3.notified().await;
                // After release, drain and reply so the connection ends
                // cleanly (avoids dangling-task noise).
                let mut sink = [0u8; 64 * 1024];
                loop {
                    match tokio::time::timeout(Duration::from_millis(200), sock.read(&mut sink))
                        .await
                    {
                        Ok(Ok(0)) | Err(_) => break,
                        Ok(Ok(_)) => continue,
                        Ok(Err(_)) => break,
                    }
                }
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    (local, dials, release)
}

#[cfg(feature = "test-gauges")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_gauge_non_vacuous_and_load_bearing() {
    use lb_l7::h2_proxy::{H2_REQ_MAX_RETAINED_BODY_BYTES, record_retained};

    // Hold for the whole test: the inverted probe below writes a 4 MiB sentinel
    // into the shared global gauge; serialize so it cannot leak into a
    // concurrent gauge test's reset→read window.
    let _gauge_serial = GAUGE_SERIAL.lock().await;

    H2_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);

    let body_size = 4 * 1024 * 1024; // 4 MiB
    let (backend, _dials, release) = spawn_stalled_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;

    // Give the H2 client a large connection/stream window so flow-control
    // does not artificially cap the in-flight bytes below the gateway's
    // own bound — we want the gateway's pump to be the limiting factor.
    let mut builder = h2::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/stall"))
        .body(())
        .unwrap();
    let (_resp_fut, mut send_body) = sender.send_request(req, false).unwrap();

    // Push the whole 4 MiB as fast as flow-control allows; the backend is
    // stalled so the gateway pump will park once its window fills. We do
    // NOT wait for the response.
    let payload = binary_pattern(body_size);
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
                // Flow-control closed (gateway paused us) or timed out —
                // EXACTLY the backpressure we want; stop pushing.
                _ => break,
            }
            if send_body.send_data(chunk, false).is_err() {
                break;
            }
            off = end;
        }
        off
    });

    // Let the system reach steady-state under the stall.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let in_situ = H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);

    // ── Non-vacuous bound: the production gauge must stay ≤ 256 KiB and
    //    be ≪ the 4 MiB body. (Whether it is non-vacuous is decided by the
    //    inverted probe below.)
    assert!(
        in_situ <= 4 * WINDOW,
        "retained gauge {in_situ} exceeds 4×window ({}); bounded-memory bar broken",
        4 * WINDOW
    );
    assert!(
        in_situ < body_size,
        "retained gauge {in_situ} not ≪ body size {body_size}"
    );

    // ── INVERTED PROBE (load-bearing check): what WOULD a whole-body
    //    buffering impl record? It would call record_retained(body_size).
    //    If the bound is load-bearing, that pushes the gauge ABOVE 4×window.
    record_retained(body_size);
    let after_probe = H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    assert!(
        after_probe > 4 * WINDOW,
        "inverted probe failed: a whole-body retain of {body_size} did not \
         exceed the 4×window bound — the assertion would not catch a \
         buffering regression"
    );

    // Record the in-situ value for the verify doc via the test name.
    eprintln!(
        "MEMORY_GAUGE in_situ_retained_bytes={in_situ} body_size={body_size} window={WINDOW}"
    );

    release.notify_waiters();
    let _ = writer.await;
}

/// F-MD-3 RE-DETERMINATION — the streaming-phase gauge now tracks the LIVE
/// in-flight channel occupancy (incremented on push at h2_proxy.rs:1574,
/// DECREMENTED when hyper pulls at :1524, recorded as `lookahead_remaining +
/// in_flight_bytes` at :1581). This test proves the gauge is NOT the round-1
/// constant by exercising the HAPPY path: a full 4 MiB body streams through a
/// FAST-draining echo backend (so 4 MiB of chunks are pushed AND pulled). A
/// gauge that recorded "max bytes ever pushed" (no working decrement) would
/// climb toward 4 MiB; the real live-occupancy gauge stays ≤ ~256 KiB because
/// each chunk is decremented the instant hyper pulls it. Combined with the
/// inverted probe in `memory_gauge_non_vacuous_and_load_bearing`, this shows
/// the streaming-phase record site is genuine, not vacuous.
#[cfg(feature = "test-gauges")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_gauge_tracks_live_occupancy_not_cumulative() {
    use lb_l7::h2_proxy::H2_REQ_MAX_RETAINED_BODY_BYTES;

    // Serialize against the inverted-probe test that writes a 4 MiB sentinel
    // into the shared global gauge (else its write leaks into this read window).
    let _gauge_serial = GAUGE_SERIAL.lock().await;

    H2_REQ_MAX_RETAINED_BODY_BYTES.store(0, Ordering::Relaxed);

    let body_size = 4 * 1024 * 1024; // 4 MiB streamed through a FAST backend.
    let (backend, dials) = spawn_echo_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;
    let mut builder = h2::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/echo"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();
    let payload = binary_pattern(body_size);
    let writer = tokio::spawn(async move {
        let mut off = 0;
        while off < payload.len() {
            let end = (off + 16 * 1024).min(payload.len());
            let chunk = Bytes::copy_from_slice(&payload[off..end]);
            send_body.reserve_capacity(chunk.len());
            match tokio::time::timeout(
                Duration::from_secs(10),
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
    // Read the full response with the CORRECT per-chunk release (so the whole
    // 4 MiB really flows through the pump and gets decremented).
    let resp = tokio::time::timeout(Duration::from_secs(30), resp_fut)
        .await
        .expect("response timed out")
        .expect("response errored");
    assert_eq!(resp.status(), 200);
    let mut body = resp.into_body();
    let mut got = 0usize;
    while let Some(chunk) = body.data().await {
        let n = chunk.expect("body data error").len();
        got += n;
        let _ = body.flow_control().release_capacity(n);
    }
    let _ = writer.await;
    assert_eq!(got, body_size, "full 4 MiB must round-trip");
    assert_eq!(dials.load(Ordering::SeqCst), 1);

    let peak = H2_REQ_MAX_RETAINED_BODY_BYTES.load(Ordering::Relaxed);
    // The decisive non-vacuousness check: 4 MiB flowed through the pump, yet
    // the live-occupancy gauge peaked ≤ 4×window. A constant gauge or a
    // no-decrement (cumulative) gauge could not produce this — it proves the
    // streaming record site measures REAL retained bytes that are released.
    assert!(
        peak <= 4 * WINDOW,
        "streaming gauge peak {peak} exceeds 4×window ({}) for a 4 MiB stream — \
         live-occupancy tracking is broken",
        4 * WINDOW
    );
    assert!(
        peak > 0,
        "streaming gauge never moved — the record site is unreachable (vacuous)"
    );
    eprintln!(
        "MEMORY_GAUGE_LIVE peak_retained={peak} (of 4 MiB streamed through, ≤4×window={}) \
         — decrement works, gauge tracks live occupancy",
        4 * WINDOW
    );
}

// ══════════════════════════════════════════════════════════════════════
// 3. BACKPRESSURE — proven causal chain.
//
// Stall the backend read; show the H2 client's send is paused (it cannot
// push the whole body) while the backend stalls, and resumes/completes
// once the backend drains.
// ══════════════════════════════════════════════════════════════════════

async fn spawn_gated_drain_backend() -> (SocketAddr, Arc<AtomicUsize>, Arc<tokio::sync::Notify>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let bytes_read = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let br2 = Arc::clone(&bytes_read);
    let r2 = Arc::clone(&release);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let br3 = Arc::clone(&br2);
            let r3 = Arc::clone(&r2);
            tokio::spawn(async move {
                // Read the request prefix (headers) then STALL — do not
                // read the body until released.
                let mut tmp = [0u8; 256];
                let _ = sock.read(&mut tmp).await;
                r3.notified().await;
                // Now drain everything, counting body bytes.
                let mut sink = [0u8; 64 * 1024];
                loop {
                    match tokio::time::timeout(Duration::from_millis(300), sock.read(&mut sink))
                        .await
                    {
                        Ok(Ok(0)) | Err(_) => break,
                        Ok(Ok(n)) => {
                            br3.fetch_add(n, Ordering::SeqCst);
                        }
                        Ok(Err(_)) => break,
                    }
                }
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    (local, bytes_read, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backpressure_client_send_paused_while_backend_stalled() {
    let body_size = 4 * 1024 * 1024; // 4 MiB — far exceeds the 64 KiB window
    let (backend, _bytes_read, release) = spawn_gated_drain_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;

    let mut builder = h2::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/bp"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();

    let sent_counter = Arc::new(AtomicUsize::new(0));
    let sc2 = Arc::clone(&sent_counter);
    let payload = binary_pattern(body_size);
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
                _ => break, // paused by backpressure or window closed
            }
            if send_body.send_data(chunk, false).is_err() {
                break;
            }
            sc2.fetch_add(end - off, Ordering::SeqCst);
            off = end;
        }
        let _ = send_body.send_data(Bytes::new(), true);
    });

    // PHASE 1 — backend stalled. After steady-state, the client should be
    // PAUSED well below the full body: it can push at most roughly the H2
    // connection/stream flow window the gateway grants, which is gated by
    // the gateway's bounded pump + the backend stall. Crucially it MUST
    // NOT have pushed the whole 4 MiB. This is the causal-chain evidence:
    // backend stall → bounded channel fills → pump parks → H2 client paused.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let paused_at = sent_counter.load(Ordering::SeqCst);
    // Emit the Phase-1 evidence UNCONDITIONALLY before the Phase-2 assertion
    // (which is gated by the separately-reported Branch-B 413 defect).
    eprintln!("BACKPRESSURE phase1 paused_at={paused_at} body_size={body_size}");
    assert!(
        paused_at < body_size,
        "backpressure NOT applied: client pushed the whole body ({paused_at} \
         of {body_size}) while the backend was stalled"
    );

    // PHASE 2 — release the backend; it drains, the pump resumes, the
    // client unblocks and the full body completes with the backend's 200.
    //
    // NOTE: this phase ALSO exercises the Branch-B happy path, which the
    // separate `real_wire_large_body_byte_identical` test proves is broken
    // (every >window body yields a spurious 413). So this assertion is
    // EXPECTED to fail until that defect is fixed; it is kept un-weakened so
    // the NOT-BUILT verdict is unmistakable and the resume-after-drain leg
    // gets re-proven once Branch B works.
    release.notify_waiters();
    let resp = tokio::time::timeout(Duration::from_secs(30), resp_fut)
        .await
        .expect("response timed out after backend drain")
        .expect("response errored");
    let resumed_at = sent_counter.load(Ordering::SeqCst);
    eprintln!(
        "BACKPRESSURE phase2 status={} paused_at={paused_at} resumed_at={resumed_at}",
        resp.status()
    );
    assert_eq!(resp.status(), 200, "backend should reply 200 after drain");
    let _ = writer.await;
    assert!(
        resumed_at > paused_at,
        "client did not resume sending after backend drain ({paused_at} → {resumed_at})"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 4. >WINDOW ADVERSARIAL malformed body/trailers (NEW cases).
//
// (a) >64 KiB body whose TRAILERS carry a pseudo-header → no backend body
//     relayed downstream; client sees RST/GOAWAY.
// (b) >64 KiB body with content-length ≠ ΣDATA → same.
//
// We use raw frames (the h2 crate cannot emit these). The backend echoes
// "ok" so any relayed DATA on stream 1 would be a leak.
// ══════════════════════════════════════════════════════════════════════

/// A backend that replies 200 with a recognizable body, draining the
/// request first. Used to detect a downstream LEAK in the >window cases.
async fn spawn_leak_probe_backend() -> (SocketAddr, Arc<AtomicUsize>) {
    spawn_binary_response_backend(b"LEAKED_BACKEND_BODY").await
}

/// Send a raw HEADERS for a >window POST then `chunks` DATA frames of
/// `chunk_len` bytes each (no END_STREAM), leaving the stream open for a
/// trailing frame the caller appends.
async fn send_large_post_prefix(
    s: &mut TlsStream<TcpStream>,
    content_length: Option<&str>,
    chunk_len: usize,
    chunks: usize,
) {
    let mut hb = Vec::new();
    hb.extend_from_slice(&hpack_literal(":method", "POST"));
    hb.extend_from_slice(&hpack_literal(":scheme", "https"));
    hb.extend_from_slice(&hpack_literal(":path", "/big"));
    hb.extend_from_slice(&hpack_literal(":authority", SAN_HOST));
    if let Some(cl) = content_length {
        hb.extend_from_slice(&hpack_literal("content-length", cl));
    }
    write_frame(s, 0x01, 0x04, 1, &hb).await; // END_HEADERS only
    let chunk = vec![0xABu8; chunk_len];
    for _ in 0..chunks {
        write_frame(s, 0x00, 0x00, 1, &chunk).await; // DATA, no END_STREAM
    }
}

/// (a) >window body + pseudo-header in trailers → no downstream leak.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn over_window_pseudo_header_trailers_no_leak() {
    let (backend, dials) = spawn_leak_probe_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let mut s = connect_tls(gw, anchor).await;
    send_preface(&mut s).await;

    // 128 KiB of body (16 × 8 KiB) → exceeds the 64 KiB window → Branch B.
    send_large_post_prefix(&mut s, None, 8 * 1024, 16).await;
    // Trailing HEADERS with a PSEUDO-header (malformed). END_HEADERS|END_STREAM.
    let mut tb = Vec::new();
    tb.extend_from_slice(&hpack_literal("x-trailer", "ok"));
    tb.extend_from_slice(&hpack_literal(":status", "200"));
    write_frame(&mut s, 0x01, 0x05, 1, &tb).await;

    if let Err(e) = expect_protocol_error_not_backend_body(&mut s).await {
        panic!("over-window pseudo-header-trailers: {e}");
    }
    // The backend MAY be dialed (>window → Branch B dials), but its
    // response body MUST NOT be relayed downstream (asserted above). Record
    // the dial count for the verify doc.
    eprintln!(
        "OVER_WINDOW_PSEUDO_TRAILERS backend_dials={}",
        dials.load(Ordering::SeqCst)
    );
}

/// (b) >window body + content-length ≠ ΣDATA → no downstream leak.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn over_window_content_length_mismatch_no_leak() {
    let (backend, dials) = spawn_leak_probe_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let mut s = connect_tls(gw, anchor).await;
    send_preface(&mut s).await;

    // Declare content-length = 999999 but send 128 KiB then END_STREAM with
    // a final short DATA → ΣDATA ≠ declared. Body > window → Branch B.
    send_large_post_prefix(&mut s, Some("999999"), 8 * 1024, 16).await;
    // Final DATA with END_STREAM, total far below the declared 999999.
    write_frame(&mut s, 0x00, 0x01, 1, b"short").await;

    if let Err(e) = expect_protocol_error_not_backend_body(&mut s).await {
        panic!("over-window content-length mismatch: {e}");
    }
    eprintln!(
        "OVER_WINDOW_CL_MISMATCH backend_dials={}",
        dials.load(Ordering::SeqCst)
    );
}

// ══════════════════════════════════════════════════════════════════════
// 5. DEVIATION #1 (suspected regression).
//
// Send a LEGITIMATE well-formed >window upload (256 KiB binary) to a
// backend that returns an early 401 WITHOUT reading the request body.
// Assert what the H2 client receives. If it is a proxy-manufactured 413
// instead of the backend's 401, that is a CORRECTNESS REGRESSION.
// ══════════════════════════════════════════════════════════════════════

/// Backend that replies 401 immediately on the request HEADERS WITHOUT
/// reading the body (sets `connection: close` so it does not block on the
/// unread body).
async fn spawn_early_401_backend() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&dials);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                // Read just the headers prefix, then reply 401 immediately
                // WITHOUT consuming the request body. `connection: close`
                // tells the client (the gateway's hyper H1 sender) the
                // response is complete and the conn is closing.
                let mut tmp = [0u8; 1024];
                let _ = sock.read(&mut tmp).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 401 Unauthorized\r\n\
                          content-length: 11\r\n\
                          connection: close\r\n\
                          \r\n\
                          UNAUTHORIZE",
                    )
                    .await;
                let _ = sock.flush().await;
                // Hold the socket briefly so the response is delivered
                // before the body write side errors out.
                tokio::time::sleep(Duration::from_millis(500)).await;
            });
        }
    });
    (local, dials)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deviation1_early_response_on_over_window_upload() {
    let body_size = 256 * 1024; // 256 KiB > 64 KiB window → Branch B
    let (backend, _dials) = spawn_early_401_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;

    let mut builder = h2::client::Builder::new();
    builder
        .initial_window_size(4 * 1024 * 1024)
        .initial_connection_window_size(4 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/auth"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();

    let payload = binary_pattern(body_size);
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

    let status = match tokio::time::timeout(Duration::from_secs(20), resp_fut).await {
        Ok(Ok(resp)) => Some(resp.status().as_u16()),
        Ok(Err(e)) => {
            eprintln!("DEVIATION1 response errored (stream reset): {e:?}");
            None
        }
        Err(_) => None,
    };
    let _ = writer.await;

    eprintln!("DEVIATION1 client_received_status={status:?} (backend sent 401)");

    // The behavioral assertion: the buffered baseline relayed the backend's
    // 401 for any <64 MiB body. The M-D streaming impl is suspected to
    // manufacture a 413 instead. Document whichever happens; fail loudly on
    // 413 so the regression is unmistakable. (200 would be an even worse
    // leak.)
    match status {
        Some(401) => { /* baseline behavior preserved — concern refuted */ }
        Some(413) => panic!(
            "DEVIATION #1 REGRESSION CONFIRMED: legitimate >window upload to a \
             backend returning early 401 yielded a proxy-manufactured 413 — \
             the backend's 401 was NOT relayed (buffered baseline did relay it)"
        ),
        Some(other) => panic!(
            "DEVIATION #1: unexpected status {other} (expected backend 401; \
             413 would be the suspected regression)"
        ),
        None => panic!(
            "DEVIATION #1 REGRESSION: client saw a stream RESET instead of the \
             backend's 401 — the early response was not relayed"
        ),
    }
}

// ══════════════════════════════════════════════════════════════════════
// 6. SMUGGLING PARITY — client RST mid-body (after dial) must never be
//    seen as a complete request at the H1 upstream; the upstream conn is
//    aborted / not pooled.
//
// Mechanism: send a >window POST (so the gateway dials + streams), push
// part of the body, then RST_STREAM mid-body. A backend that records
// whether it ever observed a COMPLETE request (clean body EOF) must show
// it never saw one. We also assert the upstream socket is closed (not
// returned to the pool for reuse) by checking the backend connection ends
// without a successful full request.
// ══════════════════════════════════════════════════════════════════════

/// Backend that records, per accepted connection, whether it received a
/// COMPLETE request (the hyper service ran to a clean body EOF). For a
/// smuggled (RST mid-body) request the service must NEVER complete.
async fn spawn_completion_recording_backend() -> (
    SocketAddr,
    Arc<AtomicUsize>, // accepted connections (dials)
    Arc<AtomicUsize>, // COMPLETE requests observed
) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let complete = Arc::new(AtomicUsize::new(0));
    let d2 = Arc::clone(&dials);
    let c2 = Arc::clone(&complete);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            d2.fetch_add(1, Ordering::SeqCst);
            let c3 = Arc::clone(&c2);
            tokio::spawn(async move {
                let c4 = Arc::clone(&c3);
                let svc = service_fn(move |req: HyperRequest<Incoming>| {
                    let c5 = Arc::clone(&c4);
                    async move {
                        // Read the body to EOF. This Ok only if the full
                        // request body arrived (clean END_STREAM relayed).
                        // A smuggled RST mid-body makes the gateway DROP the
                        // upstream body channel → hyper sees an aborted body
                        // → collect() errors → we DO NOT count completion.
                        match req.into_body().collect().await {
                            Ok(_) => {
                                c5.fetch_add(1, Ordering::SeqCst);
                                Ok::<_, Infallible>(
                                    HyperResponse::builder()
                                        .status(StatusCode::OK)
                                        .body(http_body_util::Full::new(Bytes::from_static(
                                            b"complete",
                                        )))
                                        .unwrap(),
                                )
                            }
                            Err(_) => Ok::<_, Infallible>(
                                HyperResponse::builder()
                                    .status(StatusCode::BAD_REQUEST)
                                    .body(http_body_util::Full::new(Bytes::new()))
                                    .unwrap(),
                            ),
                        }
                    }
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    (local, dials, complete)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn smuggling_rst_mid_body_never_complete_at_upstream() {
    let (backend, dials, complete) = spawn_completion_recording_backend().await;
    let (gw, anchor) = spawn_listener_for(backend).await;
    let tls = connect_tls(gw, anchor).await;

    // Now that Branch B works, use a REAL h2 client (which respects the
    // gateway's per-stream flow-control window and receives WINDOW_UPDATEs)
    // so it can actually stream a >window body, the gateway DIALS the
    // upstream and enters streaming (Branch B), and THEN we abort mid-body —
    // exercising the genuine post-DIAL abort path that round-1 could not
    // reach. Dropping `send_body` without an END_STREAM causes the h2 client
    // to RST_STREAM the request (CANCEL); the gateway must drop the upstream
    // body channel so hyper sees the upstream request body terminate
    // abruptly → the backend NEVER observes a complete request.
    let mut builder = h2::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/smuggle"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();

    // Stream 128 KiB (> the 64 KiB window) so the gateway crosses the
    // lookahead window, DIALS, and forwards body bytes upstream (Branch B).
    let payload = binary_pattern(128 * 1024);
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
    tokio::time::sleep(Duration::from_millis(500)).await;
    // ABORT mid-body WITHOUT END_STREAM: drop the body sender → the h2 client
    // sends RST_STREAM(CANCEL). The request was never finished.
    drop(send_body);
    // The response future resolves with a stream error (no complete response).
    let _ = tokio::time::timeout(Duration::from_secs(3), resp_fut).await;

    // Give the gateway time to react and the backend time to (not) complete.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let n_complete = complete.load(Ordering::SeqCst);
    let n_dials = dials.load(Ordering::SeqCst);
    eprintln!("SMUGGLING dials={n_dials} complete_requests={n_complete}");
    // Non-vacuity: with Branch B working the gateway DOES dial (the post-dial
    // abort path is now reachable). The security invariant: the truncated
    // (RST mid-body) request must NEVER be seen as a COMPLETE request at the
    // H1 upstream.
    assert!(
        n_dials >= 1,
        "expected the gateway to dial the upstream for a >window stream \
         (post-dial abort path must be exercised); got {n_dials} dials"
    );
    assert_eq!(
        n_complete, 0,
        "SMUGGLING DEFECT: a client RST mid-body was seen as a COMPLETE \
         request at the H1 upstream ({n_complete} completions over {n_dials} \
         dials) — the truncated body was relayed as a finished request"
    );
}

// ══════════════════════════════════════════════════════════════════════
// S9 / F-CAP-1 (re-verify) — H2→H1 over-cap upload to a BODY-READING backend
// now yields a deterministic 413, not the pre-fix 502. The fix consults the
// pump's classified BodyTooLarge verdict (bounded by timeouts.body) on the
// send_request-error arm. (Appended to this verifier-owned S8 suite.)
// ══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcap1_h2_over_cap_upload_yields_413() {
    // Real H2 client → TLS gateway → real H1 backend that READS (drains) the
    // body continuously and never sends a response head (so send_request fails
    // and the F-CAP-1 verdict path is exercised). The drain backend lets the
    // gateway stream the body upstream at full speed so the forwarded total
    // crosses MAX_REQUEST_BODY_BYTES (64 MiB) and the cap fires.
    let (backend, body_bytes) = spawn_body_counting_backend().await;
    // CF-SATURATION-1 — give THIS gateway listener a 300 s body timeout
    // (vs the 30 s HttpTimeouts::default). S11 hardened the BACKEND read
    // timeout (3 s→30 s, line ~740) but left the gateway's OWN wall-clock
    // body timeout (h2_proxy.rs:1837 — `tokio::time::timeout(self.timeouts
    // .body, send_fut)`, wrapping the WHOLE send_request future, NOT an
    // idle/no-progress timeout) at the 30 s default. The H2 client must push
    // 66 MiB past the 64 MiB cap; under saturation that wall-clock total grows
    // with however starved the runner is. S34 evidence: on the 4-core GitHub
    // hosted runner under the full `--workspace --all-features` gate the
    // client moved ~44 MiB of 66 MiB in 130 s (~0.34 MiB/s, still PROGRESSING
    // — the body-progress watchdog never fired) and the client wait timed out
    // BEFORE the cap was reached → status None, never 413. The upload was not
    // wedged, just slow; it simply needed more wall-clock. 300 s at that
    // observed worst-case rate clears ~100 MiB — comfortably past the 64 MiB
    // cap — yet stays BOUNDED so a genuinely-dead gateway still terminates the
    // test (the client response-wait below is raised to 310 s, just above
    // this, so the gateway's bounded arm — not the client wait — fires on a
    // true wedge). The cap-trip → 413 assertion is UNCHANGED; this only buys
    // the slowest runner enough time to actually reach the cap. (8-core boxes
    // finish the push in well under 30 s, so this margin is inert there.)
    let (gw, anchor) = spawn_listener_for_with_timeouts(
        backend,
        HttpTimeouts {
            body: Duration::from_secs(300),
            ..HttpTimeouts::default()
        },
    )
    .await;
    let tls = connect_tls(gw, anchor).await;

    let mut builder = h2::client::Builder::new();
    builder
        .initial_window_size(8 * 1024 * 1024)
        .initial_connection_window_size(8 * 1024 * 1024);
    let (h2, conn) = builder.handshake::<_, Bytes>(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/big"))
        .body(())
        .unwrap();
    let (resp_fut, mut send_body) = sender.send_request(req, false).unwrap();

    let over = 64 * 1024 * 1024 + 2 * 1024 * 1024; // 66 MiB > 64 MiB cap
    let chunk = vec![0x5Au8; 64 * 1024];
    let writer = tokio::spawn(async move {
        let mut off = 0;
        while off < over {
            send_body.reserve_capacity(chunk.len());
            // Wait (unbounded within the test's outer 130 s budget) for the H2
            // flow-control capacity the gateway grants — backpressure parks us
            // but does not abandon the upload.
            match futures_util::future::poll_fn(|cx| send_body.poll_capacity(cx)).await {
                Some(Ok(cap)) if cap > 0 => {}
                _ => break, // window closed (gateway aborted the stream → cap hit)
            }
            if send_body
                .send_data(Bytes::copy_from_slice(&chunk), false)
                .is_err()
            {
                break;
            }
            off += chunk.len();
        }
        let _ = send_body.send_data(Bytes::new(), true);
        off
    });

    // 310 s: just above the gateway's hardened 300 s body timeout (see the
    // CF-SATURATION-1 comment above) so the gateway's own BOUNDED timeout
    // arm — not this client wait — is what fires on a genuine wedge, and a
    // truly-dead gateway still terminates the test.
    let status = match tokio::time::timeout(Duration::from_secs(310), resp_fut).await {
        Ok(Ok(resp)) => Some(resp.status().as_u16()),
        Ok(Err(e)) => {
            eprintln!("FCAP1_H2 response errored: {e:?}");
            None
        }
        Err(_) => {
            eprintln!("FCAP1_H2 response timed out");
            None
        }
    };
    let written = writer.await.unwrap_or(0);
    eprintln!(
        "FCAP1_H2_OVER_CAP status={status:?} written={written} backend_body_bytes={}",
        body_bytes.load(Ordering::SeqCst)
    );
    assert_eq!(
        status,
        Some(413),
        "F-CAP-1: H2→H1 over-cap upload to a draining backend should yield 413, \
         got {status:?} (wrote {written} bytes)"
    );
}

// Reference the StreamBody/Frame imports so an unused-import lint cannot
// fire if a future edit drops a use; these mirror the production body type.
#[allow(dead_code)]
fn _type_anchor() -> StreamBody<futures_util::stream::Empty<Result<Frame<Bytes>, hyper::Error>>> {
    StreamBody::new(futures_util::stream::empty())
}
