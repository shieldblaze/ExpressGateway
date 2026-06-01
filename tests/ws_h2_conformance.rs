//! INC-4 Fix 1 — RFC 8441 §4 conformance for WebSocket-over-HTTP/2 extended
//! CONNECT. A WS extended CONNECT MUST carry `:scheme` and `:path` (plus
//! `:authority`). The gateway previously accepted a request missing either
//! and silently defaulted `:path` to "/" (INC-2V flagged PARTIAL). It must
//! now reject a malformed extended CONNECT with a clean 400 BEFORE any
//! backend dial, while the well-formed path still tunnels.
//!
//! Real-wire, gate ON (`with_h2_extended_connect(true)`): a raw-`h2` client
//! over TLS (ALPN h2) drives the gateway; a backend whose TCP accept count we
//! observe proves the malformed cases never reach it.

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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::client::TlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

const SAN_HOST: &str = "expressgateway.test";
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// HTTP/2 connection preface (RFC 9113 §3.4).
const H2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

// ── TLS / pool harness (mirrors tests/ws_h2_e2e.rs) ────────────────────

fn make_cert_for(san: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let g = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
    (
        vec![CertificateDer::from(g.cert.der().to_vec())],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(g.key_pair.serialize_der())),
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

/// A real WS echo backend that ALSO counts how many TCP connections it
/// accepts, so the malformed-CONNECT tests can assert "never dialed".
async fn spawn_echo_backend_counting(accepts: Arc<AtomicU64>) -> SocketAddr {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            accepts.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let ws = match tokio_tungstenite::accept_async(sock).await {
                    Ok(w) => w,
                    Err(_) => return,
                };
                let (mut tx, mut rx) = ws.split();
                while let Some(Ok(msg)) = rx.next().await {
                    #[allow(clippy::collapsible_match)]
                    match msg {
                        Message::Text(_) | Message::Binary(_) => {
                            if tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
                let _ = tx.close().await;
            });
        }
    });
    local
}

/// Gateway with WS-over-H2 opted IN (gate ON).
async fn spawn_gateway(backend: SocketAddr) -> (SocketAddr, CertificateDer<'static>) {
    let pool = build_pool();
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend]).unwrap());
    let h2 = Arc::new(
        H2Proxy::new(pool, picker as _, None, HttpTimeouts::default(), true)
            .with_websocket(Arc::new(WsProxy::new(WsConfig {
                idle_timeout: Duration::from_secs(30),
                max_message_size: 1024 * 1024,
                enabled: true,
                ..WsConfig::default()
            })))
            .with_h2_extended_connect(true),
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

async fn ready_h2(
    gateway: SocketAddr,
    ta: CertificateDer<'static>,
) -> h2::client::SendRequest<Bytes> {
    let sock = TcpStream::connect(gateway).await.unwrap();
    let connector = TlsConnector::from(build_client_cfg(ta));
    let sn = ServerName::try_from(SAN_HOST).unwrap();
    let tls = connector.connect(sn, sock).await.unwrap();
    let (h2c, conn) = tokio::time::timeout(STEP_TIMEOUT, h2::client::handshake(tls))
        .await
        .expect("h2 handshake timed out")
        .expect("h2 handshake failed");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    tokio::time::timeout(STEP_TIMEOUT, h2c.ready())
        .await
        .expect("h2 ready timed out")
        .expect("h2 not ready")
}

/// Send an extended CONNECT built from `uri` and return the response status
/// (or a marker string for a stream-level error). `end_of_stream=false`.
async fn extended_connect_status(
    h2c: &mut h2::client::SendRequest<Bytes>,
    uri: http::Uri,
) -> Result<http::StatusCode, String> {
    let mut req = http::Request::builder()
        .method(http::Method::CONNECT)
        .uri(uri)
        .body(())
        .unwrap();
    req.extensions_mut()
        .insert(h2::ext::Protocol::from_static("websocket"));
    let (resp_fut, _send) = h2c
        .send_request(req, false)
        .map_err(|e| format!("send_request error: {e}"))?;
    match tokio::time::timeout(STEP_TIMEOUT, resp_fut).await {
        Ok(Ok(resp)) => Ok(resp.status()),
        Ok(Err(e)) => Err(format!("response stream error: {e}")),
        Err(_) => Err("response timed out".to_owned()),
    }
}

// ── Test 1: well-formed extended CONNECT still tunnels (200) ───────────

#[tokio::test]
async fn well_formed_extended_connect_tunnels_200() {
    let accepts = Arc::new(AtomicU64::new(0));
    let backend = spawn_echo_backend_counting(Arc::clone(&accepts)).await;
    let (gw, ta) = spawn_gateway(backend).await;
    let mut h2c = ready_h2(gw, ta).await;

    // Full https://authority/path → :scheme=https, :path=/chat, :authority set.
    let uri: http::Uri = format!("https://{SAN_HOST}/chat").parse().unwrap();
    let status = extended_connect_status(&mut h2c, uri).await;
    assert_eq!(
        status,
        Ok(http::StatusCode::OK),
        "well-formed extended CONNECT (with :scheme + :path) must tunnel (200), got {status:?}"
    );
    // Backend WAS dialed for the well-formed case (the tunnel established).
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        1,
        "well-formed extended CONNECT must dial the WS backend exactly once"
    );
}

// ── Raw-H2-frame helpers (the `h2` client always synthesises `:path` for an
//    extended CONNECT — Pseudo::request defaults it to "/", h2-0.4.13
//    frame/headers.rs:561-577 — so a path-LESS extended CONNECT can only be
//    produced at the raw frame level with hand-rolled HPACK) ───────────────

async fn connect_tls(gw: SocketAddr, ta: CertificateDer<'static>) -> TlsStream<TcpStream> {
    let sock = TcpStream::connect(gw).await.unwrap();
    let connector = TlsConnector::from(build_client_cfg(ta));
    let sn = ServerName::try_from(SAN_HOST).unwrap();
    connector.connect(sn, sock).await.unwrap()
}

async fn write_frame(
    s: &mut TlsStream<TcpStream>,
    frame_type: u8,
    flags: u8,
    stream_id: u32,
    payload: &[u8],
) {
    let len = payload.len() as u32;
    let sid = stream_id & 0x7fff_ffff;
    let hdr = [
        ((len >> 16) & 0xff) as u8,
        ((len >> 8) & 0xff) as u8,
        (len & 0xff) as u8,
        frame_type,
        flags,
        ((sid >> 24) & 0xff) as u8,
        ((sid >> 16) & 0xff) as u8,
        ((sid >> 8) & 0xff) as u8,
        (sid & 0xff) as u8,
    ];
    s.write_all(&hdr).await.unwrap();
    s.write_all(payload).await.unwrap();
    s.flush().await.unwrap();
}

/// HPACK "Literal Header Field without Indexing" with an integer name index
/// (4-bit prefix `0000`) + a literal (non-Huffman) string value.
fn hpack_lit_indexed_name(name_index: u8, value: &str) -> Vec<u8> {
    assert!(name_index < 15, "single-byte 4-bit index only");
    let mut out = vec![name_index]; // 0x0n
    push_str(&mut out, value);
    out
}

/// HPACK "Literal Header Field without Indexing - New Name" (0x00) with both
/// name and value as literal (non-Huffman) strings.
fn hpack_lit_new_name(name: &str, value: &str) -> Vec<u8> {
    let mut out = vec![0x00];
    push_str(&mut out, name);
    push_str(&mut out, value);
    out
}

fn push_str(out: &mut Vec<u8>, s: &str) {
    // 7-bit length prefix, H=0 (not Huffman). Strings here are short (<127).
    assert!(s.len() < 127, "test strings are short");
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
}

// ── Test 2: missing :path → rejected, no backend dial (raw frames) ─────

#[tokio::test]
async fn extended_connect_missing_path_rejected_no_dial() {
    let accepts = Arc::new(AtomicU64::new(0));
    let backend = spawn_echo_backend_counting(Arc::clone(&accepts)).await;
    let (gw, ta) = spawn_gateway(backend).await;

    let mut tls = connect_tls(gw, ta).await;
    tls.write_all(H2_PREFACE).await.unwrap();
    // Client SETTINGS (empty).
    write_frame(&mut tls, 0x04, 0x00, 0, &[]).await;

    // HPACK-encode an extended CONNECT WITHOUT a `:path` pseudo-header:
    //   :method CONNECT  (literal w/o indexing, name idx 2 = `:method`)
    //   :scheme  https   (indexed static entry 7)               = 0x87
    //   :authority host  (literal w/o indexing, name idx 1)
    //   :protocol websocket (literal w/o indexing, new name)
    // NO :path on purpose.
    let mut block = Vec::new();
    block.extend_from_slice(&hpack_lit_indexed_name(2, "CONNECT")); // :method
    block.push(0x87); // :scheme https (static idx 7, indexed)
    block.extend_from_slice(&hpack_lit_indexed_name(1, SAN_HOST)); // :authority
    block.extend_from_slice(&hpack_lit_new_name("protocol", "websocket")); // :protocol

    // HEADERS on stream 1, END_HEADERS (0x04) but NOT END_STREAM — a tunnel
    // would keep the stream open; we are proving it is rejected instead.
    write_frame(&mut tls, 0x01, 0x04, 1, &block).await;

    // Read the server's frames for a bounded window and look for a HEADERS
    // frame on stream 1 carrying `:status 400` (HPACK static index 13 =>
    // indexed header field byte 0x8d) — robust against hpack response detail.
    // Also accept a RST_STREAM (0x03) as "not tunneled". The forbidden
    // outcome is the stream being accepted as a tunnel (which would require a
    // backend dial, asserted separately).
    let mut buf = [0u8; 4096];
    let mut saw_400 = false;
    let mut saw_rst = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, tls.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => {
                let chunk = &buf[..n];
                // Crude scan: a HEADERS (type 0x01) response for a 400 carries
                // the indexed `:status 400` octet 0x8d in its block.
                if chunk.contains(&0x8d) {
                    saw_400 = true;
                    break;
                }
                // RST_STREAM frame type is 0x03 in the 4th header octet.
                for w in chunk.windows(9) {
                    if w[3] == 0x03 {
                        saw_rst = true;
                    }
                }
                if saw_rst {
                    break;
                }
            }
            Ok(Err(_)) | Err(_) => break,
        }
    }

    assert!(
        saw_400 || saw_rst,
        "missing-:path extended CONNECT must be rejected (400 or RST), not tunneled"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        0,
        "a malformed (missing :path) extended CONNECT must NOT dial the backend"
    );
    eprintln!("missing :path: saw_400={saw_400} saw_rst={saw_rst}");
}

// ── Test 3: missing :scheme → 400, no backend dial ─────────────────────

#[tokio::test]
async fn extended_connect_missing_scheme_is_400() {
    let accepts = Arc::new(AtomicU64::new(0));
    let backend = spawn_echo_backend_counting(Arc::clone(&accepts)).await;
    let (gw, ta) = spawn_gateway(backend).await;
    let mut h2c = ready_h2(gw, ta).await;

    // Path-absolute URI with NO scheme but WITH a path. Combined with a Host
    // header the request has :authority + :path but no :scheme pseudo-header.
    let uri: http::Uri = "/chat".parse().unwrap();
    let mut req = http::Request::builder()
        .method(http::Method::CONNECT)
        .uri(uri)
        .header(http::header::HOST, SAN_HOST)
        .body(())
        .unwrap();
    req.extensions_mut()
        .insert(h2::ext::Protocol::from_static("websocket"));
    let status = match h2c.send_request(req, false) {
        Ok((resp_fut, _send)) => match tokio::time::timeout(STEP_TIMEOUT, resp_fut).await {
            Ok(Ok(resp)) => Ok(resp.status()),
            Ok(Err(e)) => Err(format!("response stream error: {e}")),
            Err(_) => Err("response timed out".to_owned()),
        },
        Err(e) => Err(format!("send_request error: {e}")),
    };
    // Either the gateway rejects it 400 (reached the handler without :scheme),
    // or the h2 layer rejects the malformed CONNECT before the handler. The
    // forbidden outcome is a 2xx tunnel; and the backend must never be dialed.
    match &status {
        Ok(s) => assert_eq!(
            *s,
            http::StatusCode::BAD_REQUEST,
            "extended CONNECT missing :scheme must be 400 (RFC 8441 §4), got {s}"
        ),
        Err(marker) => {
            // A stream-level protocol error is also acceptable (the request
            // never became a tunnel). Just make sure it's not a hang masking
            // a 200 — the timeout case is reported distinctly.
            assert!(
                !marker.contains("timed out"),
                "missing-:scheme extended CONNECT neither rejected nor errored (possible hang/tunnel)"
            );
        }
    }
    eprintln!("missing :scheme outcome: {status:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        0,
        "a malformed (missing :scheme) extended CONNECT must NOT dial the backend"
    );
}

// ── Test 4 (INC-4 Fix 2 — ROUND8-OPS-06 R12 parity): the H2 upstream WS
//    handshake carries the child W3C trace context, mirroring H1's
//    `upstream_receives_child_traceparent`. ───────────────────────────────

/// A client traceparent: trace-id 0af7..319c, parent-id b7ad..3331.
const CLIENT_TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// A WS backend that records the inbound handshake request's `traceparent`
/// (+ `tracestate`) via `accept_hdr_async`'s callback, then completes the WS
/// handshake normally so the gateway's inline dial succeeds (→ client 200).
async fn spawn_trace_recording_backend(seen_traceparent: Arc<Mutex<Option<String>>>) -> SocketAddr {
    use tokio_tungstenite::tungstenite::handshake::server::{
        ErrorResponse, Request as HsRequest, Response as HsResponse,
    };
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let slot = Arc::clone(&seen_traceparent);
            tokio::spawn(async move {
                let cb = |req: &HsRequest, resp: HsResponse| -> Result<HsResponse, ErrorResponse> {
                    if let Some(tp) = req
                        .headers()
                        .get("traceparent")
                        .and_then(|v| v.to_str().ok())
                    {
                        *slot.lock() = Some(tp.to_owned());
                    }
                    Ok(resp)
                };
                let ws = match tokio_tungstenite::accept_hdr_async(sock, cb).await {
                    Ok(w) => w,
                    Err(_) => return,
                };
                // Hold the connection briefly so the gateway sees a clean 101.
                let _hold = ws;
                tokio::time::sleep(Duration::from_secs(1)).await;
            });
        }
    });
    local
}

#[tokio::test]
async fn upstream_ws_handshake_carries_child_traceparent() {
    let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let backend = spawn_trace_recording_backend(Arc::clone(&seen)).await;
    let (gw, ta) = spawn_gateway(backend).await;
    let mut h2c = ready_h2(gw, ta).await;

    // Send a well-formed extended CONNECT carrying the client traceparent.
    let uri: http::Uri = format!("https://{SAN_HOST}/chat").parse().unwrap();
    let mut req = http::Request::builder()
        .method(http::Method::CONNECT)
        .uri(uri)
        .header("traceparent", CLIENT_TRACEPARENT)
        .body(())
        .unwrap();
    req.extensions_mut()
        .insert(h2::ext::Protocol::from_static("websocket"));
    let (resp_fut, _send) = h2c.send_request(req, false).unwrap();
    let resp = tokio::time::timeout(STEP_TIMEOUT, resp_fut)
        .await
        .expect("extended CONNECT response timed out")
        .expect("extended CONNECT errored");
    assert_eq!(resp.status(), http::StatusCode::OK, "tunnel must establish");

    // The backend's recorded upstream handshake must carry a traceparent
    // whose trace-id == the client's, but whose parent-id != the client's
    // verbatim (the LB span is the new parent — ROUND8-OPS-06).
    let recorded = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(tp) = seen.lock().clone() {
                break tp;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("upstream WS handshake never carried a traceparent");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    eprintln!("upstream handshake traceparent: {recorded}");
    assert!(
        recorded.contains("0af7651916cd43dd8448eb211c80319c"),
        "trace-id must be preserved end-to-end onto the H2 upstream WS handshake; got {recorded:?}"
    );
    assert!(
        !recorded.contains("b7ad6b7169203331"),
        "parent-id must be replaced by the LB span id, not forwarded verbatim; got {recorded:?}"
    );
}
