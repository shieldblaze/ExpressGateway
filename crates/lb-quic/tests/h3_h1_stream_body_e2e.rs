//! Incremental H3→H1 request-body streaming e2e, driving the REAL [`lb_quic::QuicListener`] with
//! a real quiche H3 client (HEADERS without fin, then DATA frames, then fin). T5 is the
//! memory-bound proof: a body sent as ONE LARGE DATA frame (≥ 8× the in-flight window) through a
//! STALLED upstream must keep the retained-per-stream gauge far below the body size, which FAILS
//! on the pre-fix whole-frame-buffering decoder.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_h3_testcodec::{H3Frame, QpackEncoder, decode_frame, encode_frame};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::{QuicListener, QuicListenerParams};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;

const H3_ALPN: &[u8] = b"h3";
const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;
const REQUEST_AUTHORITY: &str = "h3-stream.test:4433";
const REQUEST_PATH: &str = "/p1a/echo";

/// Huffman-capable decode; the hand-rolled `lb_h3_testcodec::QpackDecoder` is raw-only.
#[allow(dead_code)]
fn decode_resp_qpack(header_block: &[u8]) -> Result<Vec<(String, String)>, String> {
    use quiche::h3::NameValue;
    let hdrs = quiche::h3::qpack::Decoder::new()
        .decode(header_block, u64::MAX)
        .map_err(|e| format!("qpack decode: {e:?}"))?;
    Ok(hdrs
        .iter()
        .map(|h| {
            (
                String::from_utf8_lossy(h.name()).into_owned(),
                String::from_utf8_lossy(h.value()).into_owned(),
            )
        })
        .collect())
}
const UPSTREAM_STATUS: u16 = 201;
const UPSTREAM_BODY: &[u8] = b"p1a-resp-body";

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
        "lb-quic-h3h1-stream-{}-{}-{counter}",
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
    cfg.set_initial_max_data(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
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

/// Read a full HTTP/1.1 request, de-chunking if present → (raw head string, reassembled body).
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
            if dechunk_complete(&body) {
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

fn dechunk_complete(buf: &[u8]) -> bool {
    buf.windows(5).any(|w| w == b"0\r\n\r\n")
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
        i += size + 2; // skip data + CRLF
    }
    out
}

/// Captures (head, body) of the first request, optionally stalling before reading the body.
async fn spawn_backend(
    stall: Option<Arc<Notify>>,
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
            let stall = stall.clone();
            tokio::spawn(async move {
                if let Some(n) = &stall {
                    // Do NOT read anything yet: the proxy writes the head plus as many body
                    // chunks as its bounded in-flight window allows, then blocks on `write_all`
                    // — its body channel fills, poll_h3 stops `stream_recv`, and QUIC flow
                    // control is not extended.
                    n.notified().await;
                    // From a clean socket (nothing consumed), read the entire chunked request.
                    let (_h, _b) = read_h1_request(&mut sock).await;
                    let resp = format!(
                        "HTTP/1.1 {UPSTREAM_STATUS} Created\r\nContent-Length: {}\r\n\r\n",
                        UPSTREAM_BODY.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.write_all(UPSTREAM_BODY).await;
                    let _ = sock.shutdown().await;
                    return;
                }
                let (head, body) = read_h1_request(&mut sock).await;
                if let Some(tx) = cap {
                    let _ = tx.send((head, body));
                }
                let resp = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} Created\r\nContent-Length: {}\r\n\r\n",
                    UPSTREAM_BODY.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(UPSTREAM_BODY).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (addr, rx, handle)
}

/// Drive a quiche H3 client through HEADERS (no fin) + DATA + fin → (status, body).
#[allow(clippy::too_many_lines)]
async fn drive_h3_body_request(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    data_frames: Vec<Vec<u8>>,
    extra_headers: Vec<(String, String)>,
    deadline: tokio::time::Instant,
) -> Result<(u16, Vec<u8>), String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let stream_id: u64 = 0;
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    let encoder = QpackEncoder::new();
    let mut headers = vec![
        (":method".to_string(), "POST".to_string()),
        (":scheme".to_string(), "https".to_string()),
        (":authority".to_string(), REQUEST_AUTHORITY.to_string()),
        (":path".to_string(), REQUEST_PATH.to_string()),
    ];
    headers.extend(extra_headers);
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

    let mut sent = 0usize;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut status: Option<u16> = None;
    let mut body: Vec<u8> = Vec::new();
    let mut expected_len: Option<usize> = None;
    let mut done = false;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "deadline; established={}, sent={}/{}, status={status:?}",
                conn.is_established(),
                sent,
                wire.len()
            ));
        }
        if conn.is_closed() {
            return Err(format!(
                "conn closed: peer={:?} local={:?} timed_out={} sent={}/{}",
                conn.peer_error(),
                conn.local_error(),
                conn.is_timed_out(),
                sent,
                wire.len()
            ));
        }

        if conn.is_established() && sent < wire.len() {
            let chunk = &wire[sent..];
            let fin = true; // fin only matters once all bytes accepted
            match conn.stream_send(stream_id, chunk, false) {
                Ok(0) => {}
                Ok(n) => {
                    sent += n;
                    if sent == wire.len() {
                        let _ = conn.stream_send(stream_id, &[], true);
                        let _ = fin;
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
                        // Huffman-capable decode of the quiche-encoded head; the 413 path below
                        // still uses the raw hand-rolled decoder.
                        let hdrs = decode_resp_qpack(&header_block)?;
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
                    Err(lb_h3_testcodec::H3Error::Incomplete) => break,
                    Err(e) => return Err(format!("decode_frame: {e}")),
                }
            }
        }

        if let Some(s) = status {
            if done || (expected_len == Some(0)) {
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

        // Cap the recv wait hard: quiche's `timeout()` can be the full idle timeout when nothing
        // is pending, which would block the driver between RTTs and starve the request.
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

#[tokio::test]
async fn t1_multi_data_frame_binary_body_forwarded_byte_identical() {
    let certs = generate_loopback_certs();
    let (backend_addr, body_rx, backend_h) = spawn_backend(None).await;
    let (listener, server, _sd) = start_listener(&certs, backend_addr).await;

    // ≥3 frames, total ≥100 KB, each carrying the non-UTF-8 marker so a string conversion corrupts.
    let mut frames = Vec::new();
    let mut expected = Vec::new();
    for f in 0..4u8 {
        let mut frame = Vec::with_capacity(27_000);
        for i in 0..27_000usize {
            frame.push((i as u8) ^ f);
        }
        frame.extend_from_slice(NON_UTF8);
        expected.extend_from_slice(&frame);
        frames.push(frame);
    }
    assert!(frames.len() >= 3, "≥3 DATA frames");
    assert!(expected.len() >= 100 * 1024, "body must be ≥100 KB");

    let (conn, sock) = client_conn(server, &certs.ca);
    // Generous deadline: this real-QUIC suite is CPU-heavy and starved under the parallel runner
    // on a 2-CPU box. Correctness, not latency, is under test.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let res = drive_h3_body_request(conn, &sock, frames, vec![], deadline).await;

    let captured = tokio::time::timeout(Duration::from_secs(3), body_rx)
        .await
        .ok()
        .and_then(Result::ok);
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();

    let (status, _body) = res.expect("T1 e2e failed");
    assert_eq!(status, UPSTREAM_STATUS);
    let (head, body) = captured.expect("backend captured no request");
    assert!(
        head.to_ascii_lowercase()
            .contains("transfer-encoding: chunked"),
        "no client content-length ⇒ chunked egress; head:\n{head}"
    );
    assert_eq!(
        body.len(),
        expected.len(),
        "reassembled body length mismatch"
    );
    assert_eq!(body, expected, "reassembled body must be byte-identical");
}

#[tokio::test]
async fn t2_empty_body_is_byte_identical_to_s1_bodyless_head() {
    let certs = generate_loopback_certs();
    let (backend_addr, body_rx, backend_h) = spawn_backend(None).await;
    let (listener, server, _sd) = start_listener(&certs, backend_addr).await;

    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let res = drive_h3_body_request(conn, &sock, vec![], vec![], deadline).await;

    let captured = tokio::time::timeout(Duration::from_secs(3), body_rx)
        .await
        .ok()
        .and_then(Result::ok);
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();

    let (status, _) = res.expect("T2 e2e failed");
    assert_eq!(status, UPSTREAM_STATUS);
    let (head, body) = captured.expect("backend captured no request");
    let expected_head = format!(
        "POST {REQUEST_PATH} HTTP/1.1\r\n\
         Host: {REQUEST_AUTHORITY}\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\r\n"
    );
    assert_eq!(head, expected_head, "bodyless head must be byte-identical");
    assert!(body.is_empty(), "bodyless request must have empty body");
}

#[tokio::test]
async fn t3_zero_length_data_frame_then_fin_no_spurious_chunk() {
    let certs = generate_loopback_certs();
    let (backend_addr, body_rx, backend_h) = spawn_backend(None).await;
    let (listener, server, _sd) = start_listener(&certs, backend_addr).await;

    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let res = drive_h3_body_request(conn, &sock, vec![Vec::new()], vec![], deadline).await;

    let captured = tokio::time::timeout(Duration::from_secs(3), body_rx)
        .await
        .ok()
        .and_then(Result::ok);
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();

    let (status, _) = res.expect("T3 e2e failed");
    assert_eq!(status, UPSTREAM_STATUS);
    let (_head, body) = captured.expect("backend captured no request");
    assert!(
        body.is_empty(),
        "zero-length DATA frame must yield an empty backend body, got {} bytes",
        body.len()
    );
}

#[cfg(feature = "test-gauges")]
#[tokio::test]
async fn t5_single_large_data_frame_is_memory_bounded_through_stalled_upstream() {
    use lb_quic::conn_actor::H3_BODY_CHANNEL_DEPTH;
    use lb_quic::h3_bridge::{H3_BODY_CHUNK_MAX, MAX_RETAINED_BODY_BYTES};

    MAX_RETAINED_BODY_BYTES.store(0, Ordering::SeqCst);

    let certs = generate_loopback_certs();
    let resume = Arc::new(Notify::new());
    let (backend_addr, _rx, backend_h) = spawn_backend(Some(resume.clone())).await;
    let (listener, server, _sd) = start_listener(&certs, backend_addr).await;

    // THE point: ONE SINGLE LARGE DATA frame, ≥16× the in-flight window. The pre-fix decoder
    // required the ENTIRE frame payload buffered before yielding anything, so its buffer would
    // grow to the full body while the upstream stalls.
    let total_body = 1024 * 1024usize;
    let mut single = vec![0u8; total_body];
    for (i, b) in single.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    // Marker at head, middle and tail so a lossy conversion or off-by-window splice corrupts it.
    single[..3].copy_from_slice(NON_UTF8);
    let mid = total_body / 2;
    single[mid..mid + 3].copy_from_slice(NON_UTF8);
    let n = total_body;
    single[n - 3..].copy_from_slice(NON_UTF8);
    let expected = single.clone();
    let frames = vec![single]; // exactly ONE DATA frame.

    let (conn, sock) = client_conn(server, &certs.ca);
    // Resume only after a grace period long enough that a whole-frame-buffering proxy would
    // already have tripped the gauge.
    let resume_c = resume.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1200)).await;
        resume_c.notify_waiters();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let res = drive_h3_body_request(conn, &sock, frames, vec![], deadline).await;

    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();

    let max_retained = MAX_RETAINED_BODY_BYTES.load(Ordering::SeqCst);
    // (a) Tight bound: total retained per-stream body memory stays within a small multiple of the
    // in-flight window and UNCONDITIONALLY ≪ the body. A whole-frame-buffering decoder blows this.
    let window = H3_BODY_CHANNEL_DEPTH * H3_BODY_CHUNK_MAX; // 64 KiB
    let bound = 4 * window; // 256 KiB — small multiple of the window
    assert!(
        bound * 4 <= total_body,
        "sanity: bound ({bound}) must be `<<` the body ({total_body})"
    );
    assert!(
        max_retained > 0,
        "gauge must have observed in-flight retained bytes"
    );
    assert!(
        max_retained <= bound,
        "max retained per-stream body bytes = {max_retained}; must stay \
         <= {bound} (4 * depth*chunk-max) and `<<` the {total_body}-byte \
         single DATA frame — proves the whole frame is NOT buffered \
         (pre-fix whole-buffer decoder would retain ~{total_body})"
    );

    // (b) Liveness + correctness: after resume the full body arrived byte-identical.
    let (status, _) = res.expect("backpressured request never completed after resume");
    assert_eq!(status, UPSTREAM_STATUS);

    // The stalled backend drains post-resume locally, so re-verify through a SECOND request on a
    // non-stalled backend with the identical single large DATA frame.
    let (backend2, body_rx2, backend_h2) = spawn_backend(None).await;
    let (listener2, server2, _sd2) = start_listener(&certs, backend2).await;
    let (conn2, sock2) = client_conn(server2, &certs.ca);
    let deadline2 = tokio::time::Instant::now() + Duration::from_secs(90);
    let res2 =
        drive_h3_body_request(conn2, &sock2, vec![expected.clone()], vec![], deadline2).await;
    let captured = tokio::time::timeout(Duration::from_secs(5), body_rx2)
        .await
        .ok()
        .and_then(Result::ok);
    let _ = tokio::time::timeout(Duration::from_secs(3), listener2.shutdown()).await;
    backend_h2.abort();
    let (status2, _) = res2.expect("byte-identity request failed");
    assert_eq!(status2, UPSTREAM_STATUS);
    let (_h, body) = captured.expect("backend captured no request");
    assert_eq!(
        body.len(),
        expected.len(),
        "single-large-DATA-frame body length mismatch"
    );
    assert_eq!(
        body, expected,
        "single large DATA frame must arrive byte-identical (incl. \
         0xFF/0x00/0x80 markers)"
    );
}
