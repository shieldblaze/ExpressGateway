//! SESSION 2 / P1-C — H3→H1 request-trailer no-regression + large
//! binary RESPONSE-body correctness, both driven through the REAL
//! [`lb_quic::QuicListener`] (UDP bind → router per-CID dispatch →
//! `conn_actor::poll_h3` → `h3_bridge::h3_to_h1_stream`) with a real
//! quiche H3 client. Harness idioms mirror `h3_h1_stream_body_e2e.rs`
//! (P1-A) and `h3_h1_stream_body_errors_e2e.rs` (P1-B); no existing
//! test is touched, relaxed, or `#[ignore]`d.
//!
//! Coverage:
//!   * PC1 — REQUEST TRAILERS NO-REGRESSION. The H3 client sends
//!     HEADERS (no fin) → DATA frames → a POST-DATA HEADERS frame
//!     (the RFC 9114 §4.1 trailing field section) → fin. The body
//!     embeds 0xFF/0x00/0x80 at head, middle and tail. Asserts:
//!     - the request body arrives BYTE-IDENTICAL at the H1
//!       backend (the body-phase `BodyItem::Trailers` parser
//!       path does not crash or corrupt the DATA stream);
//!     - the H1 request is well-formed + COMPLETE (chunked
//!       terminator present and de-chunks to exactly the body);
//!     - the request trailer fields are *intentionally NOT*
//!       smuggled into the H1 request head/body (documented
//!       RFC-acceptable downgrade — see the `ReqBodyEvent::End`
//!       arm doc-comment in `h3_bridge.rs`);
//!     - the H3 client receives the backend's real `:status`.
//!   * PC2 — LARGE BINARY RESPONSE body. The H1 backend returns a
//!     \>= 256 KiB binary body (> one comfortable DATA frame) with
//!     a correct `Content-Length`, embedding 0xFF/0x00/0x80 at
//!     head, middle and tail. The real H3 client must reassemble
//!     the response body BYTE-IDENTICAL and observe the backend
//!     `:status`. Locks in the upstream→H3-client response DATA
//!     path for big binary bodies.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_h3::{H3Frame, QpackDecoder, QpackEncoder, decode_frame, encode_frame};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::{QuicListener, QuicListenerParams};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const H3_ALPN: &[u8] = b"h3";
const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;
const REQUEST_AUTHORITY: &str = "h3-trailers.test:4433";
const REQUEST_PATH: &str = "/p1c/echo";

/// Distinctive non-UTF-8 marker embedded at head/mid/tail of every
/// binary payload (request body and response body).
const NON_UTF8: &[u8] = &[0xFF, 0x00, 0x80];

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        "lb-quic-h3h1-trailers-{}-{}-{counter}",
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
    let retry_path = dir.join("retry.key");
    std::fs::write(&cert_path, cert.pem().as_bytes()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem().as_bytes()).unwrap();
    std::fs::write(&ca_path, cert.pem().as_bytes()).unwrap();
    TestCerts {
        _dir: dir,
        cert: cert_path,
        key: key_path,
        ca: ca_path,
        retry: retry_path,
    }
}

fn build_client_config(ca_path: &std::path::Path) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_verify_locations_from_file(ca_path.to_str().unwrap())
        .unwrap();
    cfg.verify_peer(true);
    cfg.set_max_idle_timeout(30_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(4 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(512 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(8);
    cfg.set_initial_max_streams_uni(8);
    cfg.set_disable_active_migration(true);
    cfg
}

fn random_scid_bytes() -> [u8; quiche::MAX_CONN_ID_LEN] {
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new().fill(&mut scid).unwrap();
    scid
}

fn build_tcp_pool() -> TcpPool {
    TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts {
            nodelay: true,
            keepalive: true,
            rcvbuf: Some(65_536),
            sndbuf: Some(65_536),
            quickack: false,
            tcp_fastopen_connect: false,
        },
        Runtime::new(),
    )
}

/// Read a full HTTP/1.1 request (head + body) from the backend socket,
/// de-chunking `Transfer-Encoding: chunked` if present. Returns the raw
/// head string and the reassembled body bytes. (Same idiom as P1-A.)
async fn read_h1_request(sock: &mut TcpStream) -> (String, Vec<u8>) {
    let mut all = Vec::with_capacity(4096);
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        if let Some(p) = all.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        let n = sock.read(&mut tmp).await.unwrap();
        if n == 0 {
            break all.len();
        }
        all.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&all[..head_end.min(all.len())]).into_owned();
    let lower = head.to_ascii_lowercase();
    let mut body = all[head_end.min(all.len())..].to_vec();

    if lower.contains("transfer-encoding: chunked") {
        loop {
            if body.windows(5).any(|w| w == b"0\r\n\r\n") {
                break;
            }
            let n = sock.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        (head, dechunk(&body))
    } else if let Some(cl) = content_length(&lower) {
        while body.len() < cl {
            let n = sock.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        body.truncate(cl);
        (head, body)
    } else {
        (head, body)
    }
}

fn content_length(lower_head: &str) -> Option<usize> {
    for line in lower_head.split("\r\n") {
        if let Some(v) = line.strip_prefix("content-length:") {
            return v.trim().parse().ok();
        }
    }
    None
}

fn dechunk(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let Some(nl) = buf[i..].windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let size_str = std::str::from_utf8(&buf[i..i + nl]).unwrap_or("0");
        let size = usize::from_str_radix(size_str.trim(), 16).unwrap_or(0);
        i += nl + 2;
        if size == 0 {
            break;
        }
        out.extend_from_slice(&buf[i..i + size]);
        i += size + 2;
    }
    out
}

/// Mock H1 backend. Captures `(head, body)` of the first request, then
/// replies with `status` and a fixed `resp_body` (Content-Length
/// framed). One-shot.
async fn spawn_backend(
    status: u16,
    resp_body: Vec<u8>,
) -> (
    SocketAddr,
    oneshot::Receiver<(String, Vec<u8>)>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    let handle = tokio::spawn(async move {
        let mut tx = Some(tx);
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let cap = tx.take();
            let resp_body = resp_body.clone();
            tokio::spawn(async move {
                let (head, body) = read_h1_request(&mut sock).await;
                if let Some(tx) = cap {
                    let _ = tx.send((head, body));
                }
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n",
                    resp_body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(&resp_body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (addr, rx, handle)
}

/// Drive a quiche H3 client: handshake, send HEADERS (no fin) +
/// `data_frames` DATA frames + an OPTIONAL post-DATA HEADERS
/// (trailers) frame + fin, then collect the response `:status` and the
/// fully-reassembled response body.
#[allow(clippy::too_many_lines)]
async fn drive_h3(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    data_frames: Vec<Vec<u8>>,
    trailers: Option<Vec<(String, String)>>,
    deadline: tokio::time::Instant,
) -> Result<(u16, Vec<u8>), String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let stream_id: u64 = 0;
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    let encoder = QpackEncoder::new();
    let headers = vec![
        (":method".to_string(), "POST".to_string()),
        (":scheme".to_string(), "https".to_string()),
        (":authority".to_string(), REQUEST_AUTHORITY.to_string()),
        (":path".to_string(), REQUEST_PATH.to_string()),
    ];
    let hb = encoder
        .encode(&headers)
        .map_err(|e| format!("qpack: {e}"))?;
    let headers_frame = encode_frame(&H3Frame::Headers { header_block: hb })
        .map_err(|e| format!("h3 frame: {e}"))?;

    let mut wire: Vec<u8> = Vec::new();
    wire.extend_from_slice(&headers_frame);
    for df in &data_frames {
        let f = encode_frame(&H3Frame::Data {
            payload: bytes::Bytes::copy_from_slice(df),
        })
        .map_err(|e| format!("h3 data: {e}"))?;
        wire.extend_from_slice(&f);
    }
    // RFC 9114 §4.1 trailing field section: a HEADERS frame AFTER DATA.
    if let Some(tr) = trailers {
        let tb = QpackEncoder::new()
            .encode(&tr)
            .map_err(|e| format!("qpack trailer: {e}"))?;
        let tf = encode_frame(&H3Frame::Headers { header_block: tb })
            .map_err(|e| format!("h3 trailer frame: {e}"))?;
        wire.extend_from_slice(&tf);
    }

    let mut sent = 0usize;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut status: Option<u16> = None;
    let mut body: Vec<u8> = Vec::new();
    let mut expected_len: Option<usize> = None;
    let mut done = false;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "deadline; established={}, sent={}/{}, status={status:?}, body={}",
                conn.is_established(),
                sent,
                wire.len(),
                body.len()
            ));
        }
        if conn.is_closed() {
            return Err(format!(
                "conn closed: peer={:?} local={:?} timed_out={}",
                conn.peer_error(),
                conn.local_error(),
                conn.is_timed_out(),
            ));
        }

        if conn.is_established() && sent < wire.len() {
            let chunk = &wire[sent..];
            match conn.stream_send(stream_id, chunk, false) {
                Ok(0) => {}
                Ok(n) => {
                    sent += n;
                    if sent == wire.len() {
                        let _ = conn.stream_send(stream_id, &[], true);
                    }
                }
                Err(quiche::Error::Done) => {}
                Err(e) => return Err(format!("stream_send: {e}")),
            }
        }

        if conn.is_established() {
            let readable: Vec<u64> = conn.readable().collect();
            for sid in readable {
                if sid != stream_id {
                    continue;
                }
                let mut c = [0u8; 8192];
                loop {
                    match conn.stream_recv(sid, &mut c) {
                        Ok((n, _)) => rx_tail.extend_from_slice(&c[..n]),
                        Err(quiche::Error::Done) | Err(quiche::Error::InvalidStreamState(_)) => {
                            break;
                        }
                        Err(e) => return Err(format!("stream_recv: {e}")),
                    }
                }
            }
            loop {
                match decode_frame(&rx_tail, 1 << 20) {
                    Ok((H3Frame::Headers { header_block }, c)) => {
                        rx_tail.drain(..c);
                        let hdrs = QpackDecoder::new()
                            .decode(&header_block)
                            .map_err(|e| format!("qpack decode: {e}"))?;
                        for (n, v) in hdrs {
                            if n == ":status" {
                                status = Some(v.parse().map_err(|_| "status".to_string())?);
                            } else if n == "content-length" {
                                expected_len = v.parse().ok();
                            }
                        }
                    }
                    Ok((H3Frame::Data { payload }, c)) => {
                        rx_tail.drain(..c);
                        body.extend_from_slice(&payload);
                        if let Some(l) = expected_len {
                            if body.len() >= l {
                                done = true;
                            }
                        }
                    }
                    Ok((_, c)) => {
                        rx_tail.drain(..c);
                    }
                    Err(lb_h3::H3Error::Incomplete) => break,
                    Err(e) => return Err(format!("decode_frame: {e}")),
                }
            }
        }

        if let Some(s) = status {
            if done || expected_len == Some(0) {
                return Ok((s, body));
            }
        }

        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    socket
                        .send_to(&out_buf[..n], info.to)
                        .await
                        .map_err(|e| format!("send_to: {e}"))?;
                }
                Err(quiche::Error::Done) => break,
                Err(e) => return Err(format!("conn.send: {e}")),
            }
        }

        let qto = conn.timeout().unwrap_or(Duration::from_millis(20));
        let wait = qto.clamp(Duration::from_millis(2), Duration::from_millis(20));
        match tokio::time::timeout(wait, socket.recv_from(&mut in_buf)).await {
            Ok(Ok((n, from))) => {
                let info = quiche::RecvInfo { from, to: local };
                match conn.recv(&mut in_buf[..n], info) {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(e) => return Err(format!("conn.recv: {e}")),
                }
            }
            Ok(Err(e)) => return Err(format!("recv_from: {e}")),
            Err(_) => conn.on_timeout(),
        }
    }
}

async fn start_listener(
    certs: &TestCerts,
    backend: SocketAddr,
) -> (QuicListener, SocketAddr, CancellationToken) {
    let pool = build_tcp_pool();
    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    )
    .with_backends(vec![backend], pool);
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone()).await.unwrap();
    let addr = listener.local_addr();
    (listener, addr, shutdown)
}

fn client_conn(server: SocketAddr, ca: &std::path::Path) -> (quiche::Connection, UdpSocket) {
    let sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind client udp");
    sock.set_nonblocking(true).unwrap();
    let sock = UdpSocket::from_std(sock).unwrap();
    let local = sock.local_addr().unwrap();
    let mut cfg = build_client_config(ca);
    let scid = random_scid_bytes();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let conn = quiche::connect(Some(TEST_SNI), &scid_ref, local, server, &mut cfg).unwrap();
    (conn, sock)
}

// ---------------------------------------------------------------------
// PC1 — request trailers: no-regression + intentionally-dropped proof.
// ---------------------------------------------------------------------
#[tokio::test]
async fn pc1_request_with_trailing_field_section_completes_and_drops_trailers() {
    let certs = generate_loopback_certs();
    // Backend echoes a fixed status; we assert the client sees it.
    let (backend_addr, body_rx, backend_h) = spawn_backend(203, b"pc1-ok".to_vec()).await;
    let (listener, server, _sd) = start_listener(&certs, backend_addr).await;

    // Two DATA frames; non-UTF-8 marker at head/mid/tail of the body.
    let mut f0 = vec![0u8; 40_000];
    for (i, b) in f0.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    f0[..3].copy_from_slice(NON_UTF8);
    let mut f1 = vec![0u8; 40_000];
    for (i, b) in f1.iter_mut().enumerate() {
        *b = ((i + 7) % 251) as u8;
    }
    let mid = f1.len() / 2;
    f1[mid..mid + 3].copy_from_slice(NON_UTF8);
    let ln = f1.len();
    f1[ln - 3..].copy_from_slice(NON_UTF8);

    let mut expected_body = Vec::new();
    expected_body.extend_from_slice(&f0);
    expected_body.extend_from_slice(&f1);

    // A distinctive trailer field whose value MUST NOT appear anywhere
    // in the H1 request the backend receives (intentional drop).
    let trailer_sentinel = "x-p1c-trailer-sentinel-VALUE-09F1";
    let trailers = vec![
        ("x-checksum".to_string(), trailer_sentinel.to_string()),
        ("x-trace".to_string(), "trace-xyz".to_string()),
    ];

    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let res = drive_h3(conn, &sock, vec![f0, f1], Some(trailers), deadline).await;

    let captured = tokio::time::timeout(Duration::from_secs(3), body_rx)
        .await
        .ok()
        .and_then(Result::ok);
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();

    let (status, _resp) = res.expect("PC1 e2e failed");
    // The H3 client receives the backend's real status.
    assert_eq!(status, 203, "client must observe the backend status");

    let (head, body) = captured.expect("backend captured no request");

    // (1) Request body arrives byte-identical at the H1 backend — the
    // body-phase `BodyItem::Trailers` parser path did not crash/corrupt
    // the DATA stream.
    assert_eq!(
        body.len(),
        expected_body.len(),
        "reassembled request body length mismatch (trailers present)"
    );
    assert_eq!(
        body, expected_body,
        "request body must reach the H1 backend byte-identical even \
         when a trailing field section follows the DATA frames"
    );

    // (2) Well-formed COMPLETE request: no client content-length ⇒
    // chunked egress, and read_h1_request only returns once the
    // `0\r\n\r\n` terminator is seen (so the request is complete).
    assert!(
        head.to_ascii_lowercase()
            .contains("transfer-encoding: chunked"),
        "no client content-length ⇒ chunked egress; head:\n{head}"
    );

    // (3) The H3 request trailer fields are INTENTIONALLY NOT smuggled
    // into the H1 request. Neither the head nor the (de-chunked) body
    // may contain the trailer sentinel or the trailer field names.
    assert!(
        !head.contains(trailer_sentinel)
            && !head.to_ascii_lowercase().contains("x-checksum")
            && !head.to_ascii_lowercase().contains("x-trace"),
        "request trailer fields must NOT be smuggled into the H1 \
         request head; head:\n{head}"
    );
    assert!(
        !body
            .windows(trailer_sentinel.len())
            .any(|w| w == trailer_sentinel.as_bytes()),
        "request trailer value must NOT appear in the H1 request body"
    );
}

// ---------------------------------------------------------------------
// PC2 — large (>=256 KiB) binary response body, reassembled identical.
// ---------------------------------------------------------------------
#[tokio::test]
async fn pc2_large_binary_response_body_reassembled_byte_identical() {
    // >= 256 KiB so it exceeds one comfortable DATA frame; embeds the
    // non-UTF-8 marker at head, middle and tail so an off-by-window
    // splice or a lossy conversion anywhere would corrupt it.
    let total = 256 * 1024 + 777usize;
    let mut resp_body = vec![0u8; total];
    for (i, b) in resp_body.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    resp_body[..3].copy_from_slice(NON_UTF8);
    let mid = total / 2;
    resp_body[mid..mid + 3].copy_from_slice(NON_UTF8);
    resp_body[total - 3..].copy_from_slice(NON_UTF8);
    let expected = resp_body.clone();

    let certs = generate_loopback_certs();
    let (backend_addr, _body_rx, backend_h) = spawn_backend(206, resp_body).await;
    let (listener, server, _sd) = start_listener(&certs, backend_addr).await;

    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    // Bodyless request (HEADERS + FIN): focus is the RESPONSE path.
    let res = drive_h3(conn, &sock, vec![], None, deadline).await;

    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();

    let (status, body) = res.expect("PC2 e2e failed");
    assert_eq!(status, 206, ":status must match the backend");
    assert_eq!(
        body.len(),
        expected.len(),
        "reassembled response body length mismatch (got {}, want {})",
        body.len(),
        expected.len()
    );
    assert_eq!(
        body, expected,
        "large binary response body must reassemble byte-identical at \
         the H3 client (incl. 0xFF/0x00/0x80 at head/mid/tail)"
    );
}
