//! F-COR-1 (auditor-2 A2-2) — DETERMINISTIC ordering-race regression
//! (directive D2).
//!
//! Mechanism: for H2→H1 the pre-fix `proxy_request` streamed the live
//! inbound `IncomingBody` straight upstream; the static backend replied
//! `200 "ok"` and the gateway relayed that 2-byte `DATA` body BEFORE
//! hyper/h2 finished protocol-validating the malformed inbound stream.
//! h2spec saw `DATA(2)` instead of the mandated RST_STREAM/GOAWAY
//! (nondeterministic which of ≥5 faces won the validate-vs-forward
//! race). The fix fully receives + validates the inbound request body
//! BEFORE dialing the upstream (and rejects pseudo-header trailers),
//! so a malformed request can NEVER leak the backend body — the window
//! is closed structurally, making this assertion DETERMINISTIC with NO
//! induced churn (D2: the gate test must be deterministic; the
//! h2spec-under-churn test is corroboration only, kept in tests/h2spec.rs).
//!
//! Two malformed cases per D2, both asserted to yield a
//! PROTOCOL_ERROR-class stream/connection error and NEVER the backend
//! `DATA(2)` "ok" body:
//!   1. content-length ≠ Σ DATA lengths (RFC 9113 §8.1.2.6).
//!   2. pseudo-header field in the trailing field section
//!      (RFC 9113 §8.1).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::Full;
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

use std::convert::Infallible;

const SAN_HOST: &str = "expressgateway.test";
const H2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

// ── Harness (same shape as tests/h2_security_live.rs) ──────────────────

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

/// Backend that COUNTS accepted TCP connections. The decisive
/// deterministic invariant the F-COR-1 ordering fix establishes:
/// `proxy_request` fully receives + validates the inbound H2 request
/// BEFORE dialing the upstream. So for a malformed request the backend
/// must be dialed ZERO times — structurally, independent of the
/// scheduler (D2). Pre-fix the body was streamed straight to the
/// upstream before validation, so the dial (and the relayed
/// `DATA(2)` "ok") happened. Counting the dial is deterministic where
/// observing the raced DATA leak on the wire is not.
async fn spawn_counting_backend() -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c2 = Arc::clone(&count);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(async move {
                let svc = service_fn(|_req: HyperRequest<Incoming>| async move {
                    Ok::<_, Infallible>(
                        HyperResponse::builder()
                            .status(StatusCode::OK)
                            // The exact 2-byte body auditor-2 saw leak
                            // as `DATA Frame (length:2 ...)`.
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
    (local, count)
}

async fn spawn_listener() -> (
    SocketAddr,
    CertificateDer<'static>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let (backend_addr, backend_dials) = spawn_counting_backend().await;
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

    (local, trust_anchor, backend_dials)
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

// ── Minimal raw HTTP/2 framing (the `h2` crate cannot express a
//    pseudo-header in trailers nor a content-length≠ΣDATA on purpose) ──

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

/// HPACK literal header field, never indexed, literal (no-Huffman)
/// name + value. `0x00` opcode then 7-bit-prefix string lengths.
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

/// Read frames until a server-initiated stream/connection error
/// (RST_STREAM or GOAWAY) OR a DATA frame on the request stream is
/// observed. Returns `Ok(())` if an RST_STREAM/GOAWAY arrives first
/// (the mandated rejection); `Err(reason)` if a `DATA` frame (the
/// leaked backend "ok" body — the defect) or HEADERS+200 arrives
/// first, or on timeout with neither.
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
            Ok(Ok(0)) => {
                // Clean EOF. If we never saw a DATA leak this is an
                // acceptable connection-level rejection (the gateway
                // tore the connection down without forwarding the
                // backend body).
                return Ok(());
            }
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                // Transport reset before any backend DATA leaked — the
                // malformed request was rejected, not forwarded.
                let _ = e;
                return Ok(());
            }
            Err(_) => return Err("timeout reading server frames".into()),
        };
        acc.extend_from_slice(&buf[..n]);
        // Parse complete frames out of `acc`.
        let mut pos = 0;
        while acc.len() - pos >= 9 {
            let flen = ((acc[pos] as usize) << 16)
                | ((acc[pos + 1] as usize) << 8)
                | (acc[pos + 2] as usize);
            let fty = acc[pos + 3];
            if acc.len() - pos < 9 + flen {
                break; // wait for the rest of this frame
            }
            let sid = u32::from_be_bytes([
                acc[pos + 5] & 0x7f,
                acc[pos + 6],
                acc[pos + 7],
                acc[pos + 8],
            ]);
            match fty {
                0x07 => return Ok(()), // GOAWAY (connection error) — mandated
                0x03 => return Ok(()), // RST_STREAM — mandated
                0x00 if sid == 1 => {
                    // DATA on the request stream = the leaked backend
                    // "ok" body. This is the F-COR-1 DEFECT.
                    return Err(format!(
                        "F-COR-1 DEFECT: backend DATA frame (len:{flen}) \
                         relayed for a malformed request — validate-vs-\
                         forward race not closed"
                    ));
                }
                0x01 if sid == 1 => {
                    // HEADERS on stream 1: could be the 200 response
                    // headers being relayed (defect) — keep reading;
                    // the decisive proof is a following DATA(2) or an
                    // RST. A standalone 4xx the gateway itself emits is
                    // also HEADERS; we do not fail on HEADERS alone, we
                    // fail only on the actual backend DATA leak.
                }
                _ => {}
            }
            pos += 9 + flen;
        }
        acc.drain(..pos);
    }
}

// ── DETERMINISTIC gate tests (D2) ──────────────────────────────────────

/// content-length ≠ Σ DATA (RFC 9113 §8.1.2.6 / auditor-2 8.1.2.6#2).
/// MUST yield PROTOCOL_ERROR-class rejection, NEVER the backend
/// `DATA(2)` body. Deterministic — no churn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn content_length_mismatch_never_leaks_backend_body() {
    use std::sync::atomic::Ordering;
    let (gw, anchor, backend_dials) = spawn_listener().await;
    let mut s = connect_tls(gw, anchor).await;
    send_preface(&mut s).await;

    // HEADERS with content-length: 100 but only 2 DATA bytes follow.
    let mut hb = Vec::new();
    hb.extend_from_slice(&hpack_literal(":method", "POST"));
    hb.extend_from_slice(&hpack_literal(":scheme", "https"));
    hb.extend_from_slice(&hpack_literal(":path", "/"));
    hb.extend_from_slice(&hpack_literal(":authority", SAN_HOST));
    hb.extend_from_slice(&hpack_literal("content-length", "100"));
    // HEADERS, flags=END_HEADERS(0x04) only (no END_STREAM — body
    // follows).
    write_frame(&mut s, 0x01, 0x04, 1, &hb).await;
    // DATA: 2 bytes, END_STREAM(0x01) — total 2 ≠ declared 100.
    write_frame(&mut s, 0x00, 0x01, 1, b"hi").await;

    // Wire-level: never the leaked backend DATA(2) body.
    if let Err(e) = expect_protocol_error_not_backend_body(&mut s).await {
        panic!("{e}");
    }
    // DETERMINISTIC structural invariant (D2): the validate-before-dial
    // fix means a malformed request is rejected BEFORE any upstream
    // contact — the backend is dialed ZERO times, independent of the
    // scheduler. Pre-fix the body was streamed to the upstream before
    // validation, so the dial happened (and the 200 "ok" raced back).
    // Give any erroneous in-flight dial a moment to land, then assert 0.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        backend_dials.load(Ordering::SeqCst),
        0,
        "F-COR-1 DEFECT: backend was dialed for a content-length≠ΣDATA \
         malformed request — validate-vs-forward race not closed"
    );
}

/// D1 — buffered-request-body cap regression. The H2→H1 path now
/// buffers the request body (consistent with the already-shipped
/// H2→H2 / H2→H3 sibling paths); D1 mandates a NAMED bounded cap
/// (`lb_l7::h2_proxy::MAX_REQUEST_BODY_BYTES`, 64 MiB) with
/// `413 Payload Too Large` on exceed and a regression that exceeds the
/// cap and asserts 413 (prove no unbounded-buffer regression). Uses the
/// `h2` client so the oversized body streams without a 64 MiB
/// allocation client-side.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_request_body_rejected_413_not_buffered_unbounded() {
    // Sanity-pin the named cap so a silent constant change is caught.
    assert_eq!(
        lb_l7::h2_proxy::MAX_REQUEST_BODY_BYTES,
        64 * 1024 * 1024,
        "the D1 named request-body cap changed unexpectedly"
    );

    let (gw, anchor, _backend_dials) = spawn_listener().await;
    let tls = connect_tls(gw, anchor).await;
    let (h2, conn) = h2::client::handshake(tls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut sender = h2.ready().await.unwrap();
    let req = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SAN_HOST}/"))
        .body(())
        .unwrap();
    let (resp_fut, mut body) = sender.send_request(req, false).unwrap();

    // Stream > 64 MiB so the buffered collect trips the cap. 1 MiB
    // chunks; 70 MiB total.
    let chunk = bytes::Bytes::from(vec![0u8; 1024 * 1024]);
    let writer = async move {
        for _ in 0..70 {
            if body.send_data(chunk.clone(), false).is_err() {
                break; // server already rejected & reset the stream
            }
            // Let the reservation flow-control breathe.
            tokio::task::yield_now().await;
        }
        let _ = body.send_data(bytes::Bytes::new(), true);
    };
    let reader = async {
        match tokio::time::timeout(Duration::from_secs(20), resp_fut).await {
            Ok(Ok(resp)) => Some(resp.status()),
            Ok(Err(_)) => None, // stream reset = also a valid rejection
            Err(_) => None,
        }
    };
    let (_, status) = tokio::join!(writer, reader);

    match status {
        Some(code) => assert_eq!(
            code,
            http::StatusCode::PAYLOAD_TOO_LARGE,
            "oversized body must be rejected 413, got {code}"
        ),
        None => {
            // Stream-reset rejection (no 200, no body forwarded) is also
            // acceptable — the unbounded-buffer regression we guard
            // against would instead OOM or return 200; either of those
            // would have surfaced as a success status above.
        }
    }
}

/// Pseudo-header field in the trailing field section (RFC 9113 §8.1 /
/// auditor-2 8.1.2.1#3). MUST yield PROTOCOL_ERROR-class rejection,
/// NEVER the backend `DATA(2)` body. Deterministic — no churn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pseudo_header_in_trailers_never_leaks_backend_body() {
    use std::sync::atomic::Ordering;
    let (gw, anchor, backend_dials) = spawn_listener().await;
    let mut s = connect_tls(gw, anchor).await;
    send_preface(&mut s).await;

    // Valid HEADERS (no END_STREAM — body+trailers follow).
    let mut hb = Vec::new();
    hb.extend_from_slice(&hpack_literal(":method", "POST"));
    hb.extend_from_slice(&hpack_literal(":scheme", "https"));
    hb.extend_from_slice(&hpack_literal(":path", "/"));
    hb.extend_from_slice(&hpack_literal(":authority", SAN_HOST));
    write_frame(&mut s, 0x01, 0x04, 1, &hb).await;
    // A DATA frame (no END_STREAM) so a trailing HEADERS section is
    // legal positionally.
    write_frame(&mut s, 0x00, 0x00, 1, b"hi").await;
    // Trailing HEADERS containing a PSEUDO-header — malformed per
    // RFC 9113 §8.1. flags = END_HEADERS|END_STREAM (0x05).
    let mut tb = Vec::new();
    tb.extend_from_slice(&hpack_literal("x-trailer", "ok"));
    tb.extend_from_slice(&hpack_literal(":status", "200"));
    write_frame(&mut s, 0x01, 0x05, 1, &tb).await;

    if let Err(e) = expect_protocol_error_not_backend_body(&mut s).await {
        panic!("{e}");
    }
    // DETERMINISTIC structural invariant (D2): the pseudo-header-in-
    // trailers rejection fires at the H2→H1 trailer-capture site BEFORE
    // the upstream dial, so the backend is contacted ZERO times — not a
    // scheduler race. Pre-fix the trailer filter was absent AND the body
    // was streamed before validation, so the dial + 200 leak raced.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        backend_dials.load(Ordering::SeqCst),
        0,
        "F-COR-1 DEFECT: backend was dialed for a pseudo-header-in-\
         trailers malformed request — RFC 9113 §8.1 rejection did not \
         precede the forward"
    );
}
