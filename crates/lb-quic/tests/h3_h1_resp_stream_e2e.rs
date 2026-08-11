//! Incremental H3 RESPONSE-body streaming e2e over the REAL [`lb_quic::QuicListener`]
//! (UDP bind → router → `conn_actor` → `h3_bridge::stream_h1_response`): a backend streams
//! a body and the quiche H3 client must receive it byte-identical, memory-bounded.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(dead_code)] // scaffold: some helpers wired by the R-tests at P1-B.

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
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const H3_ALPN: &[u8] = b"h3";
const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;
const REQUEST_AUTHORITY: &str = "h3-resp-stream.test:4433";
const REQUEST_PATH: &str = "/p1/resp-echo";
const UPSTREAM_STATUS: u16 = 200;

/// Marker embedded at head/mid/tail of every fixture, so a lossy string conversion
/// anywhere in the path is caught.
const NON_UTF8: &[u8] = &[0xFF, 0x00, 0x80];

/// The §1.5 C5 per-stream retained-bytes bound with ×4 slack. Sound occupancy is
/// `depth × (chunk_max + frame_hdr_max)`, NOT `depth × chunk_max`: a response event carries
/// the frame's type+length varints too. Test ceiling MUST equal the gauge bound.
fn resp_retained_ceiling(depth: usize, chunk_max: usize, frame_hdr_max: usize) -> usize {
    4 * (depth * (chunk_max + frame_hdr_max))
}

/// The migrated egress Huffman-encodes values; `lb_h3_testcodec::QpackDecoder` is raw-only.
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
        "lb-quic-h3h1-respstream-{}-{}-{counter}",
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

#[derive(Clone)]
enum RespBody {
    ContentLength(Vec<u8>),
    /// CF-H3-HEAD — a CL-framed body whose head also carries regular headers PLUS a
    /// hop-by-hop `Connection: close`. Load-bearing both ways: forward the former, strip the latter.
    ContentLengthWithHeaders {
        body: Vec<u8>,
        extra: Vec<(&'static str, &'static str)>,
    },
    Chunked {
        body: Vec<u8>,
        chunk_sizes: Vec<usize>,
    },
    /// Chunked body with an RFC 9112 §7.1.2 trailer section. `coalesce` puts the zero-size
    /// line, the trailer fields and the final CRLF in ONE socket write (PC-2 coalesced) rather
    /// than separate ones. Empty `trailers` ⇒ a bare terminator, so NO trailing HEADERS frame.
    ChunkedWithTrailers {
        body: Vec<u8>,
        chunk_sizes: Vec<usize>,
        trailers: Vec<(String, String)>,
        coalesce: bool,
    },
    /// EOF-delimited: no CL, no TE, `Connection: close` — length unknown, client relies on FIN.
    EofDelimited(Vec<u8>),
    /// Head + partial body, then a graceful close: a premature EOF before Content-Length.
    ResetMidBody {
        declared_len: usize,
        partial: Vec<u8>,
    },
    /// Head + partial body, then a hard TCP RST (SO_LINGER 0) — not a graceful FIN.
    RstMidBody {
        declared_len: usize,
        partial: Vec<u8>,
    },
    /// `Content-Length` declared LARGER than the proxy's cap ⇒ `OverCap` ⇒ RESET_STREAM 0x0102,
    /// never a body presented as complete.
    OverCap { declared_len: usize },
    /// Endless body (huge `Content-Length`), writing until the proxy stops reading.
    /// `read_done` fires once the backend's read returns 0/err (the proxy dropped the pooled
    /// upstream); `bytes_written` records how much was pushed before that.
    Endless {
        read_closed: Arc<Notify>,
        bytes_written: Arc<std::sync::atomic::AtomicUsize>,
    },
}

/// Accept one connection, read the bodyless request head, then stream the configured response.
/// `stall` waits BETWEEN head and body so the proxy's in-flight window fills for the gauge.
async fn spawn_resp_backend(
    body: RespBody,
    stall: Option<Arc<Notify>>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut sock, _) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut tmp = [0u8; 4096];
        let mut req = Vec::new();
        loop {
            if req.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            match sock.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => req.extend_from_slice(&tmp[..n]),
            }
        }

        match body {
            RespBody::ContentLength(b) => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    b.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                if let Some(n) = &stall {
                    n.notified().await;
                }
                let _ = sock.write_all(&b).await;
                let _ = sock.shutdown().await;
            }
            RespBody::ContentLengthWithHeaders { body: b, extra } => {
                let mut head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {}\r\nConnection: close\r\n",
                    b.len()
                );
                for (n, v) in &extra {
                    head.push_str(n);
                    head.push_str(": ");
                    head.push_str(v);
                    head.push_str("\r\n");
                }
                head.push_str("\r\n");
                let _ = sock.write_all(head.as_bytes()).await;
                if let Some(n) = &stall {
                    n.notified().await;
                }
                let _ = sock.write_all(&b).await;
                let _ = sock.shutdown().await;
            }
            RespBody::Chunked { body, chunk_sizes } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                if let Some(n) = &stall {
                    n.notified().await;
                }
                let mut off = 0;
                for sz in chunk_sizes {
                    let end = (off + sz).min(body.len());
                    let piece = &body[off..end];
                    let _ = sock
                        .write_all(format!("{:x}\r\n", piece.len()).as_bytes())
                        .await;
                    let _ = sock.write_all(piece).await;
                    let _ = sock.write_all(b"\r\n").await;
                    off = end;
                }
                let _ = sock.write_all(b"0\r\n\r\n").await;
                let _ = sock.shutdown().await;
            }
            RespBody::ChunkedWithTrailers {
                body,
                chunk_sizes,
                trailers,
                coalesce,
            } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                if let Some(n) = &stall {
                    n.notified().await;
                }
                let mut off = 0;
                for sz in chunk_sizes {
                    let end = (off + sz).min(body.len());
                    let piece = &body[off..end];
                    let _ = sock
                        .write_all(format!("{:x}\r\n", piece.len()).as_bytes())
                        .await;
                    let _ = sock.write_all(piece).await;
                    let _ = sock.write_all(b"\r\n").await;
                    off = end;
                }
                // Zero-size chunk + trailer section + terminating CRLF, either in ONE write
                // (PC-2 coalesced — parsed from the SAME read as the size line) or split.
                let mut tail = Vec::from(&b"0\r\n"[..]);
                for (n, v) in &trailers {
                    tail.extend_from_slice(format!("{n}: {v}\r\n").as_bytes());
                }
                tail.extend_from_slice(b"\r\n");
                if coalesce {
                    let _ = sock.write_all(&tail).await;
                } else {
                    for byte in &tail {
                        let _ = sock.write_all(std::slice::from_ref(byte)).await;
                        let _ = sock.flush().await;
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                }
                let _ = sock.shutdown().await;
            }
            RespBody::EofDelimited(b) => {
                let head = format!("HTTP/1.1 {UPSTREAM_STATUS} OK\r\nConnection: close\r\n\r\n");
                let _ = sock.write_all(head.as_bytes()).await;
                if let Some(n) = &stall {
                    n.notified().await;
                }
                let _ = sock.write_all(&b).await;
                let _ = sock.shutdown().await;
            }
            RespBody::ResetMidBody {
                declared_len,
                partial,
            } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&partial).await;
                // Premature EOF after only `partial` bytes: the proxy MUST reset, never present
                // the truncated body as complete (response-splitting guard).
                let _ = sock.shutdown().await;
                drop(sock);
            }
            RespBody::RstMidBody {
                declared_len,
                partial,
            } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&partial).await;
                let _ = sock.flush().await;
                // SO_LINGER 0 then drop ⇒ the peer's next read returns ECONNRESET, exercising
                // the read-ERROR arm rather than the EOF (read==0) arm.
                #[allow(deprecated)]
                let _ = sock.set_linger(Some(Duration::ZERO));
                drop(sock);
            }
            RespBody::OverCap { declared_len } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                // Stream a repeating pattern until the proxy aborts; never allocate `declared_len`.
                let chunk = vec![0xABu8; 64 * 1024];
                let mut written = 0usize;
                while written < declared_len {
                    if sock.write_all(&chunk).await.is_err() {
                        break; // proxy reset the upstream (OverCap).
                    }
                    written += chunk.len();
                }
                let _ = sock.shutdown().await;
                drop(sock);
            }
            RespBody::Endless {
                read_closed,
                bytes_written,
            } => {
                let head = format!(
                    "HTTP/1.1 {UPSTREAM_STATUS} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    1usize << 40 // 1 TiB — never satisfied
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let chunk = vec![0x5Au8; 32 * 1024];
                // The clippy `while let Ok(()) = write_all` rewrite would DROP the post-match
                // teardown probe below (the 1 ms read detecting the proxy's closed read half).
                #[allow(clippy::while_let_loop)]
                loop {
                    match sock.write_all(&chunk).await {
                        Ok(()) => {
                            bytes_written.fetch_add(chunk.len(), Ordering::Relaxed);
                        }
                        Err(_) => break, // proxy stopped reading + closed.
                    }
                    // Probe whether the proxy closed its read half: Ok(0) on FIN, Err on RST.
                    let mut probe = [0u8; 1];
                    match tokio::time::timeout(Duration::from_millis(1), sock.read(&mut probe))
                        .await
                    {
                        Ok(Ok(0)) | Ok(Err(_)) => break, // upstream torn down.
                        _ => {}
                    }
                }
                read_closed.notify_waiters();
                drop(sock);
            }
        }
    });
    (addr, handle)
}

#[derive(Debug, Default)]
struct ClientOutcome {
    status: Option<u16>,
    body: Vec<u8>,
    fin: bool,
    reset_code: Option<u64>,
    /// Fields of the post-DATA trailing HEADERS frame (RFC 9114 §4.1); empty when none.
    trailers: Vec<(String, String)>,
    /// Non-`:status` fields of the response HEAD frame, so the CF-H3-HEAD round-trip can assert
    /// regular headers survive H3→H1→H3 and hop-by-hop is stripped.
    head_fields: Vec<(String, String)>,
}

/// FIN-aware H3 response client driver. Unlike the request-side driver (which returns as soon
/// as `content-length` is satisfied), this completes on stream FIN OR RESET_STREAM — required
/// for chunked / EOF-delimited framings where the length is unknown, and for the abort paths.
///
/// `cancel_after`: send STOP_SENDING + RESET_STREAM once `n` body bytes have arrived.
#[allow(clippy::too_many_lines)]
async fn drive_h3_response_client(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    extra_headers: Vec<(String, String)>,
    cancel_after: Option<usize>,
    deadline: tokio::time::Instant,
) -> Result<ClientOutcome, String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let stream_id: u64 = 0;
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    let encoder = QpackEncoder::new();
    let mut headers = vec![
        (":method".to_string(), "GET".to_string()),
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

    let mut sent = 0usize;
    let mut header_done = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut status: Option<u16> = None;
    let mut body: Vec<u8> = Vec::new();
    let mut fin = false;
    let mut reset_code: Option<u64> = None;
    let mut cancelled = false;
    let mut trailers: Vec<(String, String)> = Vec::new();
    let mut head_fields: Vec<(String, String)> = Vec::new();

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "deadline; established={} sent={}/{} status={status:?} body={} fin={fin}",
                conn.is_established(),
                sent,
                headers_frame.len(),
                body.len()
            ));
        }
        if conn.is_closed() {
            if fin || reset_code.is_some() {
                return Ok(ClientOutcome {
                    status,
                    body,
                    fin,
                    reset_code,
                    trailers,
                    head_fields,
                });
            }
            return Err(format!(
                "conn closed early: peer={:?} local={:?} status={status:?} body={}",
                conn.peer_error(),
                conn.local_error(),
                body.len()
            ));
        }

        if conn.is_established() && !header_done {
            match conn.stream_send(stream_id, &headers_frame[sent..], false) {
                Ok(0) | Err(quiche::Error::Done) => {}
                Ok(n) => {
                    sent += n;
                    if sent == headers_frame.len() {
                        let _ = conn.stream_send(stream_id, &[], true);
                        header_done = true;
                    }
                }
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
                        Ok((n, stream_fin)) => {
                            rx_tail.extend_from_slice(&c[..n]);
                            if stream_fin {
                                fin = true;
                            }
                        }
                        Err(quiche::Error::Done) => break,
                        Err(quiche::Error::InvalidStreamState(_)) => break,
                        Err(quiche::Error::StreamReset(code)) => {
                            reset_code = Some(code);
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
                        let hdrs = decode_resp_qpack(&header_block)?;
                        if hdrs.iter().any(|(n, _)| n == ":status") {
                            for (n, v) in hdrs {
                                if n == ":status" {
                                    status = Some(v.parse().map_err(|_| "status".to_string())?);
                                } else {
                                    head_fields.push((n, v));
                                }
                            }
                        } else {
                            // A post-DATA HEADERS frame with no `:status` is the RFC 9114
                            // §4.1 trailing field section.
                            trailers.extend(hdrs);
                        }
                    }
                    Ok((H3Frame::Data { payload }, c)) => {
                        rx_tail.drain(..c);
                        body.extend_from_slice(&payload);
                        if let Some(after) = cancel_after {
                            if !cancelled && body.len() >= after {
                                let _ =
                                    conn.stream_shutdown(stream_id, quiche::Shutdown::Read, 0x10);
                                cancelled = true;
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

        if (fin && status.is_some()) || reset_code.is_some() || (cancelled && fin) {
            return Ok(ClientOutcome {
                status,
                body,
                fin,
                reset_code,
                trailers,
                head_fields,
            });
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

/// Like `start_listener` but with a SINGLE-slot `TcpPool` whose handle is returned, so a test
/// can observe pool parking (C2: a poisoned upstream must be dropped, never parked).
async fn start_listener_single_slot_pool(
    certs: &TestCerts,
    backend: SocketAddr,
) -> (QuicListener, SocketAddr, CancellationToken, TcpPool) {
    let pool = TcpPool::new(
        PoolConfig {
            per_peer_max: 1,
            total_max: 1,
            ..PoolConfig::default()
        },
        BackendSockOpts {
            nodelay: true,
            keepalive: true,
            rcvbuf: Some(65_536),
            sndbuf: Some(65_536),
            quickack: false,
            tcp_fastopen_connect: false,
        },
        Runtime::new(),
    );
    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    )
    .with_backends(vec![backend], pool.clone());
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone()).await.unwrap();
    let addr = listener.local_addr();
    (listener, addr, shutdown, pool)
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

/// Stalled/slow FIN-aware response client driver — the R2/R3 memory + backpressure proof. It
/// drives the QUIC connection so the handshake and ACKs progress but does NOT `stream_recv` the
/// response stream for `stall`, so quiche grants the proxy no credit, the `Progressive` queue
/// stays non-empty, the bounded channel fills, and the producer's `tx.send().await` blocks —
/// pausing the upstream socket read. `sample` runs once mid-stall, at the largest-retained instant.
#[allow(clippy::too_many_lines)]
async fn drive_h3_response_client_stalled(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    stall: Duration,
    deadline: tokio::time::Instant,
    mut sample: impl FnMut(),
) -> Result<ClientOutcome, String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let stream_id: u64 = 0;
    let local = socket.local_addr().map_err(|e| e.to_string())?;

    let encoder = QpackEncoder::new();
    let headers = vec![
        (":method".to_string(), "GET".to_string()),
        (":scheme".to_string(), "https".to_string()),
        (":authority".to_string(), REQUEST_AUTHORITY.to_string()),
        (":path".to_string(), REQUEST_PATH.to_string()),
    ];
    let hb = encoder
        .encode(&headers)
        .map_err(|e| format!("qpack: {e}"))?;
    let headers_frame = encode_frame(&H3Frame::Headers { header_block: hb })
        .map_err(|e| format!("h3 frame: {e}"))?;

    let mut sent = 0usize;
    let mut header_done = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut status: Option<u16> = None;
    let mut body: Vec<u8> = Vec::new();
    let mut fin = false;
    let mut reset_code: Option<u64> = None;

    let mut stream_seen_readable = false;
    let mut stall_until: Option<tokio::time::Instant> = None;
    let mut sampled = false;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "deadline; established={} status={status:?} body={} fin={fin}",
                conn.is_established(),
                body.len()
            ));
        }
        if conn.is_closed() {
            if fin || reset_code.is_some() {
                return Ok(ClientOutcome {
                    status,
                    body,
                    fin,
                    reset_code,
                    trailers: Vec::new(),
                    head_fields: Vec::new(),
                });
            }
            return Err(format!(
                "conn closed early: peer={:?} local={:?} status={status:?} body={}",
                conn.peer_error(),
                conn.local_error(),
                body.len()
            ));
        }

        if conn.is_established() && !header_done {
            match conn.stream_send(stream_id, &headers_frame[sent..], false) {
                Ok(0) | Err(quiche::Error::Done) => {}
                Ok(n) => {
                    sent += n;
                    if sent == headers_frame.len() {
                        let _ = conn.stream_send(stream_id, &[], true);
                        header_done = true;
                    }
                }
                Err(e) => return Err(format!("stream_send: {e}")),
            }
        }

        let in_stall = match stall_until {
            Some(t) => tokio::time::Instant::now() < t,
            None => false,
        };

        if conn.is_established() {
            let readable: Vec<u64> = conn.readable().collect();
            if readable.contains(&stream_id) && !stream_seen_readable {
                stream_seen_readable = true;
                stall_until = Some(tokio::time::Instant::now() + stall);
            }
            // Do NOT consume the response stream while stalling — that is the whole point.
            // UDP recv + ACKs continue so the connection lives.
            if !in_stall {
                for sid in readable {
                    if sid != stream_id {
                        continue;
                    }
                    let mut c = [0u8; 8192];
                    loop {
                        match conn.stream_recv(sid, &mut c) {
                            Ok((n, stream_fin)) => {
                                rx_tail.extend_from_slice(&c[..n]);
                                if stream_fin {
                                    fin = true;
                                }
                            }
                            Err(quiche::Error::Done) => break,
                            Err(quiche::Error::InvalidStreamState(_)) => break,
                            Err(quiche::Error::StreamReset(code)) => {
                                reset_code = Some(code);
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
                            let hdrs = decode_resp_qpack(&header_block)?;
                            for (n, v) in hdrs {
                                if n == ":status" {
                                    status = Some(v.parse().map_err(|_| "status".to_string())?);
                                }
                            }
                        }
                        Ok((H3Frame::Data { payload }, c)) => {
                            rx_tail.drain(..c);
                            body.extend_from_slice(&payload);
                        }
                        Ok((_, c)) => {
                            rx_tail.drain(..c);
                        }
                        Err(lb_h3_testcodec::H3Error::Incomplete) => break,
                        Err(e) => return Err(format!("decode_frame: {e}")),
                    }
                }
            }
        }

        // Sample the gauge once mid-stall, after the proxy has had wall-time to fill its
        // bounded in-flight window against us.
        if in_stall && !sampled {
            if let Some(t) = stall_until {
                if t.saturating_duration_since(tokio::time::Instant::now()) <= stall / 2 {
                    sample();
                    sampled = true;
                }
            }
        }

        if (fin && status.is_some()) || reset_code.is_some() {
            return Ok(ClientOutcome {
                status,
                body,
                fin,
                reset_code,
                trailers: Vec::new(),
                head_fields: Vec::new(),
            });
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

/// R1/R2/R3 binary body: `n` deterministic bytes with the non-UTF-8 marker at head/mid/tail.
fn binary_body(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    for (i, x) in b.iter_mut().enumerate() {
        *x = (i % 251) as u8;
    }
    b[..3].copy_from_slice(NON_UTF8);
    let mid = n / 2;
    b[mid..mid + 3].copy_from_slice(NON_UTF8);
    b[n - 3..].copy_from_slice(NON_UTF8);
    b
}

#[test]
fn fixture_r1_binary_body_is_large_and_non_utf8_marked() {
    let b = binary_body(120 * 1024);
    assert!(b.len() >= 100 * 1024, "R1 body must be ≥100 KB");
    assert_eq!(&b[..3], NON_UTF8, "head marker");
    let mid = b.len() / 2;
    assert_eq!(&b[mid..mid + 3], NON_UTF8, "mid marker");
    assert_eq!(&b[b.len() - 3..], NON_UTF8, "tail marker");
    assert!(
        std::str::from_utf8(&b).is_err(),
        "fixture must be non-UTF-8 so a lossy conversion is caught"
    );
}

#[test]
fn fixture_r4_empty_body() {
    let b = Vec::<u8>::new();
    assert!(b.is_empty(), "R4 empty-body fixture");
}

#[test]
fn fixture_r7_chunked_split_reassembles() {
    let body = binary_body(50_000);
    let chunk_sizes = vec![1, 7, 4096, 8192, 1, 100, 99_999];
    let mut off = 0;
    let mut reassembled = Vec::new();
    for sz in &chunk_sizes {
        let end = (off + sz).min(body.len());
        reassembled.extend_from_slice(&body[off..end]);
        off = end;
    }
    assert_eq!(reassembled, body, "chunk split must cover the whole body");
}

#[test]
fn c5_resp_retained_ceiling_is_sound_and_much_less_than_1mib() {
    // C5 soundness: the ceiling expression must use depth × (chunk + hdr), NOT depth × chunk —
    // under-counting the frame header would make the proof unsound.
    let depth = 8;
    let chunk_max = 8 * 1024;
    let frame_hdr_max = 16;
    let ceiling = resp_retained_ceiling(depth, chunk_max, frame_hdr_max);
    assert!(
        ceiling > 4 * (depth * chunk_max),
        "C5: ceiling must include the frame-header term"
    );
    let one_mib = 1024 * 1024;
    // "≪" = at least ~3× headroom below the body (the real margin is ≈4×), so the threshold is
    // provably non-vacuous without being brittle.
    assert!(
        ceiling * 3 <= one_mib,
        "non-vacuous: ceiling ({ceiling}) must be ≪ the 1 MiB R2 body \
         (got {:.2}× headroom, need ≥3×)",
        one_mib as f64 / ceiling as f64
    );
}

// FEATURE GATE (load-bearing): R2 and R3 reference `lb_quic::h3_bridge::MAX_RETAINED_RESP_BYTES`,
// a `#[cfg(any(test, feature = "test-gauges"))]` static, so this crate only COMPILES the
// memory/backpressure proofs under `--features test-gauges`. A CI gate that omits the flag
// SILENTLY DROPS the only non-vacuous memory assertions — any R8 gate for this cell MUST pass it.

#[tokio::test]
async fn r1_multi_data_binary_response_byte_identical() {
    let certs = generate_loopback_certs();
    let expected = binary_body(120 * 1024);
    let (backend, backend_h) =
        spawn_resp_backend(RespBody::ContentLength(expected.clone()), None).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();
    let out = out.expect("R1 e2e failed");
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R1 must end with a clean FIN");
    assert_eq!(
        out.body, expected,
        "R1 response body must be byte-identical"
    );
}

/// CF-H3-HEAD — the H3→H1 response leg MUST forward the FULL non-hop-by-hop header set (pre-S12
/// it dropped everything but `:status` + content-length). LOAD-BEARING both ways: temp-revert the
/// head re-encode to the `:status`+CL projection and this FAILS.
#[tokio::test]
async fn cf_h3_head_h3_to_h1_full_response_headers_round_trip() {
    let certs = generate_loopback_certs();
    let expected = b"h3h1-full-head-body".to_vec();
    let (backend, backend_h) = spawn_resp_backend(
        RespBody::ContentLengthWithHeaders {
            body: expected.clone(),
            extra: vec![
                ("content-type", "application/json"),
                ("cache-control", "no-store"),
                ("x-eg-resp", "round-trip"),
            ],
        },
        None,
    )
    .await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();
    let out = out.expect("CF-H3-HEAD H3→H1 e2e failed");

    assert_eq!(
        out.status,
        Some(UPSTREAM_STATUS),
        "must be the upstream status"
    );
    assert!(out.fin, "clean FIN expected after the full-header response");
    assert_eq!(out.body, expected, "body must be byte-identical");
    let has = |name: &str, val: &str| out.head_fields.iter().any(|(n, v)| n == name && v == val);
    assert!(
        has("content-type", "application/json"),
        "content-type MUST round-trip H3→H1 (CF-H3-HEAD); got head fields {:?}",
        out.head_fields
    );
    assert!(
        has("cache-control", "no-store"),
        "cache-control MUST round-trip H3→H1 (CF-H3-HEAD); got head fields {:?}",
        out.head_fields
    );
    assert!(
        has("x-eg-resp", "round-trip"),
        "a custom response header MUST round-trip H3→H1 (CF-H3-HEAD); got head fields {:?}",
        out.head_fields
    );
    // Hop-by-hop MUST be stripped — load-bearing the other way.
    assert!(
        !out.head_fields.iter().any(|(n, _)| n == "connection"),
        "the hop-by-hop `connection` header MUST NOT be forwarded to the H3 \
         client (got head fields {:?})",
        out.head_fields
    );
}

/// R2 — NON-VACUOUS memory bound: a ~4 MiB response through a STALLED H3 client keeps
/// `MAX_RETAINED_RESP_BYTES`, sampled at the largest-retained instant, ≤ the C5 ceiling
/// (262 656 B, ≈16× below the body), and the body still resumes byte-identical with a clean FIN.
#[tokio::test]
async fn r2_response_memory_bounded_through_stalled_client() {
    use lb_quic::h3_bridge::{
        H3_FRAME_HDR_MAX, H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, MAX_RETAINED_RESP_BYTES,
    };

    MAX_RETAINED_RESP_BYTES.store(0, Ordering::SeqCst);

    // The EXACT C5 bound: depth × (chunk + hdr), NOT depth × chunk. Same expression
    // `drain_resp_channels` feeds `record_resp_retained`.
    let c5_channel_bound = H3_RESP_CHANNEL_DEPTH * (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX);
    let ceiling = resp_retained_ceiling(H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, H3_FRAME_HDR_MAX);
    assert_eq!(
        ceiling,
        4 * c5_channel_bound,
        "R2 ceiling MUST equal the §1.5 C5 sound bound (4 × depth×(chunk+hdr))"
    );
    assert_eq!(
        c5_channel_bound, 65_664,
        "C5 channel bound authoritative value"
    );
    assert_eq!(ceiling, 262_656, "R2 ceiling authoritative value");

    let total_body = 4 * 1024 * 1024usize; // 4 MiB
    assert!(
        ceiling * 8 <= total_body,
        "non-vacuous: ceiling ({ceiling}) must be ≪ body ({total_body}) \
         at ≥8× (got {:.2}×)",
        total_body as f64 / ceiling as f64
    );

    let certs = generate_loopback_certs();
    let expected = binary_body(total_body);
    let resume = Arc::new(Notify::new());
    let (backend, backend_h) = spawn_resp_backend(
        RespBody::ContentLength(expected.clone()),
        Some(resume.clone()),
    )
    .await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);

    // Release the body only after a grace period long enough that a whole-body buffering proxy
    // would have tripped the gauge far above the ceiling first.
    let resume_c = resume.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        resume_c.notify_waiters();
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let out =
        drive_h3_response_client_stalled(conn, &sock, Duration::from_millis(1600), deadline, || {})
            .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();

    let out = out.expect("R2 e2e failed (liveness: must complete after resume)");
    let max_retained = MAX_RETAINED_RESP_BYTES.load(Ordering::SeqCst);

    eprintln!(
        "R2-EVIDENCE: max_retained={max_retained} B  ceiling={ceiling} B \
         (C5 channel bound={c5_channel_bound} B)  body={total_body} B  \
         margin={:.2}x  retained/ceiling={:.4}",
        total_body as f64 / ceiling as f64,
        max_retained as f64 / ceiling as f64
    );
    assert!(
        max_retained > 0,
        "gauge must have observed in-flight retained response bytes \
         (else the proof is vacuous)"
    );
    assert!(
        max_retained <= ceiling,
        "R2 memory bound BREACHED: max retained = {max_retained} B, \
         C5 ceiling = {ceiling} B (body = {total_body} B). A buffering \
         proxy would retain ≈ body size."
    );
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(
        out.fin,
        "R2 must end with a clean FIN after resume (liveness)"
    );
    assert_eq!(
        out.body, expected,
        "R2 body must be byte-identical after the stall+resume"
    );
}

/// R3 — slow-client backpressure. R2 proves the memory CEILING; R3 proves the CAUSAL CHAIN: the
/// gauge holding at the ceiling for a body 16× it, with a backend willing to firehose, is only
/// possible if the producer's `tx.send().await` blocked the upstream read.
#[tokio::test]
async fn r3_slow_client_backpressures_upstream_read() {
    use lb_quic::h3_bridge::{
        H3_FRAME_HDR_MAX, H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, MAX_RETAINED_RESP_BYTES,
    };

    MAX_RETAINED_RESP_BYTES.store(0, Ordering::SeqCst);
    let ceiling = resp_retained_ceiling(H3_RESP_CHANNEL_DEPTH, H3_RESP_CHUNK_MAX, H3_FRAME_HDR_MAX);

    let total_body = 4 * 1024 * 1024usize; // 4 MiB, 16× the ceiling
    let certs = generate_loopback_certs();
    let expected = binary_body(total_body);
    // No backend stall: it sends as fast as TCP allows, so backpressure is the ONLY thing that
    // can hold retained bytes at the ceiling.
    let (backend, backend_h) =
        spawn_resp_backend(RespBody::ContentLength(expected.clone()), None).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);

    let gauge_mid = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let gauge_mid_c = gauge_mid.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let out = drive_h3_response_client_stalled(
        conn,
        &sock,
        Duration::from_millis(2000),
        deadline,
        move || {
            gauge_mid_c.store(
                MAX_RETAINED_RESP_BYTES.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
        },
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();

    let out = out.expect("R3 e2e failed (must complete after the slow client resumes)");
    let mid = gauge_mid.load(Ordering::SeqCst);
    let max_retained = MAX_RETAINED_RESP_BYTES.load(Ordering::SeqCst);

    eprintln!(
        "R3-EVIDENCE: mid_stall_retained={mid} B  peak_retained={max_retained} B  \
         ceiling={ceiling} B  body={total_body} B  \
         mid/ceiling={:.4}  peak/ceiling={:.4}",
        mid as f64 / ceiling as f64,
        max_retained as f64 / ceiling as f64
    );
    assert!(
        mid > 0,
        "gauge must have observed in-flight bytes mid-stall (proof not vacuous)"
    );
    assert!(
        mid <= ceiling,
        "BACKPRESSURE FAILED: mid-stall retained = {mid} B exceeds the \
         C5 ceiling {ceiling} B for a {total_body} B body — the upstream \
         socket read was NOT paused (it ran ahead of the stalled client)"
    );
    assert!(
        max_retained <= ceiling,
        "BACKPRESSURE FAILED: peak retained = {max_retained} B > ceiling \
         {ceiling} B for a {total_body} B body"
    );
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R3 must complete with a clean FIN after resume");
    assert_eq!(
        out.body, expected,
        "R3 body must be byte-identical despite the backpressure stall"
    );
}

#[tokio::test]
async fn r4_empty_response_body_clean_fin() {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_resp_backend(RespBody::ContentLength(Vec::new()), None).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();
    let out = out.expect("R4 e2e failed");
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R4 empty body must still end with a clean FIN");
    assert!(out.body.is_empty(), "R4 body must be empty");
}

async fn run_abort_scenario(body: RespBody) -> ClientOutcome {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_resp_backend(body, None).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();
    out.expect("abort-scenario e2e failed")
}

/// Assert the C1 invariant for an abort: RESET_STREAM with `H3_INTERNAL_ERROR` (0x0102), NOT
/// `H3_NO_ERROR` (0x0100), and never a clean FIN — a truncated body is never presentable.
fn assert_c1_reset(out: &ClientOutcome, label: &str) {
    assert!(
        !out.fin,
        "{label}: abort path must NOT end with a clean FIN \
         (truncated-as-complete guard) — got fin=true, body={}",
        out.body.len()
    );
    let code = out.reset_code.unwrap_or_else(|| {
        panic!(
            "{label}: client must observe RESET_STREAM on the abort \
             path; reset_code=None, fin={}, body={}",
            out.fin,
            out.body.len()
        )
    });
    assert_eq!(
        code,
        lb_quic::H3_INTERNAL_ERROR,
        "{label}: RESET_STREAM error_code must == H3_INTERNAL_ERROR \
         (0x0102); got {code:#x}"
    );
    assert_eq!(code, 0x0102, "{label}: explicit codepoint check");
    assert_ne!(
        code,
        lb_quic::H3_NO_ERROR,
        "{label}: RESET_STREAM error_code must NOT be the graceful \
         H3_NO_ERROR (0x0100) — that would let a cache treat the \
         truncated body as a complete response (binding C1)"
    );
}

/// R5 — upstream resets / fails mid-response ⇒ RESET_STREAM with the explicit C1 code: hard TCP
/// RST, premature EOF before Content-Length, and over-cap.
#[tokio::test]
async fn r5_upstream_reset_midresponse_yields_reset_stream() {
    let out = run_abort_scenario(RespBody::RstMidBody {
        declared_len: 200_000,
        partial: binary_body(16 * 1024),
    })
    .await;
    assert_c1_reset(&out, "R5a upstream-RST");

    let out = run_abort_scenario(RespBody::ResetMidBody {
        declared_len: 200_000,
        partial: binary_body(16 * 1024),
    })
    .await;
    assert_c1_reset(&out, "R5b premature-EOF-before-Content-Length");

    // (c) over-cap (declared > MAX_RESPONSE_BODY_BYTES, 64 MiB) ⇒ `RespAbort::OverCap`.
    let out = run_abort_scenario(RespBody::OverCap {
        declared_len: (64 * 1024 * 1024) + (4 * 1024 * 1024),
    })
    .await;
    assert_c1_reset(&out, "R5c over-cap");
}

/// R6 — a client cancel mid-response must stop the upstream read, proven two ways: the endless
/// backend's read closes (the proxy dropped the pooled connection) AND `bytes_written` stops
/// growing — the read provably halted rather than draining 1 TiB.
#[tokio::test]
async fn r6_client_cancel_midresponse_stops_upstream_read() {
    let certs = generate_loopback_certs();
    let read_closed = Arc::new(Notify::new());
    let bytes_written = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (backend, backend_h) = spawn_resp_backend(
        RespBody::Endless {
            read_closed: read_closed.clone(),
            bytes_written: bytes_written.clone(),
        },
        None,
    )
    .await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let drive = tokio::spawn(async move {
        drive_h3_response_client(conn, &sock, vec![], Some(32 * 1024), deadline).await
    });

    // A timeout here means the upstream read did NOT stop ⇒ a leak.
    let torn_down = tokio::time::timeout(Duration::from_secs(20), read_closed.notified()).await;

    let written_at_teardown = bytes_written.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let written_after = bytes_written.load(Ordering::Relaxed);

    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), drive).await;
    backend_h.abort();

    assert!(
        torn_down.is_ok(),
        "R6: proxy did NOT tear down the upstream after the client \
         cancelled (endless backend's read never closed within 20s) — \
         per-stream state leak / upstream read not stopped"
    );
    // The read provably stopped: a still-reading proxy on a 1 TiB body would keep the backend
    // writing indefinitely.
    assert_eq!(
        written_after, written_at_teardown,
        "R6: backend kept writing after teardown ({written_at_teardown} \
         → {written_after}) — the proxy was still reading the upstream"
    );
}

#[tokio::test]
async fn r7_chunked_upstream_response_byte_identical() {
    let certs = generate_loopback_certs();
    let expected = binary_body(50_000);
    let (backend, backend_h) = spawn_resp_backend(
        RespBody::Chunked {
            body: expected.clone(),
            chunk_sizes: vec![1, 7, 4096, 8192, 1, 100, 99_999],
        },
        None,
    )
    .await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();
    let out = out.expect("R7 e2e failed");
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R7 chunked response must end with a clean FIN");
    assert_eq!(
        out.body, expected,
        "R7 chunked response must reassemble byte-identical (new decoder)"
    );
}

// C2 — pooled-upstream smuggling guard. For each `RespAbort` variant plus a ClientGone cancel:
// the poisoned connection is NOT parked (single-slot pool ⇒ idle == 0, so the next acquire must
// dial fresh) and the client saw RESET_STREAM 0x0102 — except ClientGone, where the proxy
// correctly does NOT reset but the upstream MUST still be dropped.

/// Drive one C2 abort scenario through a single-slot pool; returns the outcome + idle-count.
async fn run_c2_scenario(body: RespBody, cancel_after: Option<usize>) -> (ClientOutcome, usize) {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_resp_backend(body, None).await;
    let (listener, server, _sd, pool) = start_listener_single_slot_pool(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], cancel_after, deadline).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let idle = pool.idle_count_for(backend);
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();
    (out.expect("C2 scenario e2e failed"), idle)
}

#[tokio::test]
async fn c2_every_abort_variant_drops_pooled_upstream_and_resets() {
    let (out, idle) = run_c2_scenario(
        RespBody::RstMidBody {
            declared_len: 200_000,
            partial: binary_body(16 * 1024),
        },
        None,
    )
    .await;
    assert_c1_reset(&out, "C2/UpstreamReset");
    assert_eq!(
        idle, 0,
        "C2/UpstreamReset: poisoned upstream must NOT be parked"
    );

    let (out, idle) = run_c2_scenario(
        RespBody::ResetMidBody {
            declared_len: 200_000,
            partial: binary_body(16 * 1024),
        },
        None,
    )
    .await;
    assert_c1_reset(&out, "C2/PrematureEof");
    assert_eq!(
        idle, 0,
        "C2/PrematureEof: poisoned upstream must NOT be parked"
    );

    let (out, idle) = run_c2_raw_chunked(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\nZZ\r\nx\r\n",
    )
    .await;
    assert_c1_reset(&out, "C2/ChunkedDecode");
    assert_eq!(
        idle, 0,
        "C2/ChunkedDecode: poisoned upstream must NOT be parked"
    );

    let (out, idle) = run_c2_scenario(
        RespBody::OverCap {
            declared_len: (64 * 1024 * 1024) + (4 * 1024 * 1024),
        },
        None,
    )
    .await;
    assert_c1_reset(&out, "C2/OverCap");
    assert_eq!(idle, 0, "C2/OverCap: poisoned upstream must NOT be parked");

    // BadHead needs a dedicated backend: an empty EOF-delimited body is a *valid* 200.
    let (out, idle) = run_c2_bad_head().await;
    assert_c1_reset(&out, "C2/BadHead");
    assert_eq!(idle, 0, "C2/BadHead: poisoned upstream must NOT be parked");

    // The sixth C2 variant, ClientGone, is a separate test (a regression lock for a product
    // defect) so these five arms give a clean signal rather than being weakened or ignored.
}

/// C2 sixth variant — ClientGone: the regression lock for the defect where a client cancel did
/// not stop the upstream read. It asserts the REAL teardown (the endless backend's read half
/// closes ⇒ the pooled upstream was dropped), NOT merely `idle == 0`, which the defect would
/// spuriously satisfy because a never-finishing producer simply never parks the conn. Do NOT weaken.
#[tokio::test]
async fn c2_clientgone_drops_pooled_upstream() {
    let certs = generate_loopback_certs();
    let read_closed = Arc::new(Notify::new());
    let bytes_written = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (backend, backend_h) = spawn_resp_backend(
        RespBody::Endless {
            read_closed: read_closed.clone(),
            bytes_written: bytes_written.clone(),
        },
        None,
    )
    .await;
    let (listener, server, _sd, pool) = start_listener_single_slot_pool(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let drive = tokio::spawn(async move {
        drive_h3_response_client(conn, &sock, vec![], Some(32 * 1024), deadline).await
    });
    let torn_down = tokio::time::timeout(Duration::from_secs(20), read_closed.notified()).await;
    let idle = pool.idle_count_for(backend);
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), drive).await;
    backend_h.abort();
    assert!(
        torn_down.is_ok(),
        "C2/ClientGone: proxy did NOT drop the upstream after the \
         client cancelled (endless backend read never closed in 20s) — \
         binding C2 / §1.3.4 ClientGone violated"
    );
    assert_eq!(
        idle, 0,
        "C2/ClientGone: poisoned upstream must NOT be parked"
    );
}

/// Raw malformed-chunked backend through the single-slot pool: the poisoned upstream is dropped
/// AND the client saw RESET_STREAM 0x0102.
async fn run_c2_raw_chunked(raw: &str) -> (ClientOutcome, usize) {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_raw_backend(raw.as_bytes().to_vec()).await;
    let (l, server, _sd, pool) = start_listener_single_slot_pool(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let idle = pool.idle_count_for(backend);
    let _ = tokio::time::timeout(Duration::from_secs(3), l.shutdown()).await;
    backend_h.abort();
    (out.expect("C2/raw-chunked e2e failed"), idle)
}

/// Bad-head backend: not a valid status line and never a `CRLF CRLF` ⇒ `BadHead` ⇒ 0x0102.
async fn run_c2_bad_head() -> (ClientOutcome, usize) {
    let certs = generate_loopback_certs();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let backend = listener.local_addr().unwrap();
    let backend_h = tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut tmp = [0u8; 4096];
            let mut req = Vec::new();
            loop {
                if req.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                match sock.read(&mut tmp).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&tmp[..n]),
                }
            }
            let _ = sock.write_all(b"NOT-HTTP garbage no terminator").await;
            let _ = sock.shutdown().await;
        }
    });
    let (l, server, _sd, pool) = start_listener_single_slot_pool(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let idle = pool.idle_count_for(backend);
    let _ = tokio::time::timeout(Duration::from_secs(3), l.shutdown()).await;
    backend_h.abort();
    (out.expect("C2/BadHead e2e failed"), idle)
}

// C3 — chunked-decoder negative / smuggling tests end-to-end. The unit cases live in
// `h3_bridge.rs`; here we additionally prove the two they did not cover (declared-size overflow,
// junk after the zero-size terminator) and that a malformed chunked response ⇒ RESET_STREAM
// 0x0102, never a truncated or forwarded body presented as complete.

/// Raw backend emitting caller-supplied bytes verbatim after the request head, so arbitrarily
/// malformed chunked framing can be sent.
async fn spawn_raw_backend(raw_response: Vec<u8>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let h = tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut tmp = [0u8; 4096];
            let mut req = Vec::new();
            loop {
                if req.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                match sock.read(&mut tmp).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&tmp[..n]),
                }
            }
            let _ = sock.write_all(&raw_response).await;
            let _ = sock.shutdown().await;
        }
    });
    (addr, h)
}

async fn run_raw_chunked_abort(raw: &str) -> ClientOutcome {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_raw_backend(raw.as_bytes().to_vec()).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();
    out.expect("C3 raw-chunked e2e failed")
}

#[tokio::test]
async fn c3_malformed_chunked_responses_reset_never_forward_truncated() {
    let out = run_raw_chunked_abort(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nZZ\r\nabc\r\n0\r\n\r\n",
    )
    .await;
    assert_c1_reset(&out, "C3 non-hex-chunk-size");

    let out = run_raw_chunked_abort(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabcXX0\r\n\r\n",
    )
    .await;
    assert_c1_reset(&out, "C3 missing-CRLF-after-chunk");

    // (3) declared chunk-size larger than the data, then EOF.
    let out = run_raw_chunked_abort(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nFFFF\r\nshort",
    )
    .await;
    assert_c1_reset(&out, "C3 declared-size-overflow");

    // (4) junk after the zero-size terminator — a smuggled second "response": the well-formed
    // terminator is `0` CRLF CRLF, here the final CRLF is corrupted.
    let out = run_raw_chunked_abort(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n0\r\nXJUNK",
    )
    .await;
    // Either a clean completion with the junk ignored as trailing bytes OR a decode reset —
    // but NEVER the junk forwarded as body.
    if out.reset_code.is_some() {
        assert_c1_reset(&out, "C3 junk-after-terminator (reset)");
    } else {
        assert_eq!(
            out.body, b"abc",
            "C3 junk-after-terminator: only the valid chunk body may be \
             forwarded; trailing junk must NEVER be smuggled into the body"
        );
        assert!(out.fin, "C3 junk-after-terminator: clean completion");
    }
}

/// C3 coverage map: the two lead-named cases the unit test did not cover are proven END-TO-END
/// above; this test exists so the mapping is explicit and greppable.
#[test]
fn c3_unit_supplement_documents_coverage() {
    // Decoder-level malformed cases are asserted in `h3_bridge.rs`; the remaining two by cases
    // (3) and (4) of `c3_malformed_chunked_responses_reset_never_forward_truncated`.
}

// R8 / C4 — an upstream chunked response carrying an RFC 9112 §7.1.2 trailer section is
// delivered as a post-DATA RFC 9114 §4.1 trailing HEADERS frame, AFTER a byte-identical body and
// BEFORE a clean FIN. Content-Length / EOF framings emit NO trailer frame, and the no-trailer
// chunked sub-case must produce NO spurious empty trailing HEADERS.

/// Drive one chunked-with-trailers scenario end-to-end; returns status + body + trailers.
async fn run_r8_scenario(body: RespBody) -> ClientOutcome {
    let certs = generate_loopback_certs();
    let (backend, backend_h) = spawn_resp_backend(body, None).await;
    let (listener, server, _sd) = start_listener(&certs, backend).await;
    let (conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let out = drive_h3_response_client(conn, &sock, vec![], None, deadline).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
    backend_h.abort();
    out.expect("R8 e2e failed")
}

#[tokio::test]
async fn r8_chunked_response_trailers_delivered_to_h3_client() {
    let expected = binary_body(60_000);
    let trailers = vec![
        ("x-checksum".to_string(), "abc123-DEADBEEF".to_string()),
        ("x-trailer-two".to_string(), "second-value".to_string()),
    ];

    // (1) PC-2 coalesced: terminator and trailer fields arrive in ONE write, parsed from the
    //     SAME read.
    let out = run_r8_scenario(RespBody::ChunkedWithTrailers {
        body: expected.clone(),
        chunk_sizes: vec![1, 7, 4096, 8192, 1, 100, 99_999],
        trailers: trailers.clone(),
        coalesce: true,
    })
    .await;
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(
        out.fin,
        "R8 coalesced: clean FIN after the trailing HEADERS frame"
    );
    assert_eq!(
        out.body, expected,
        "R8 coalesced: body byte-identical (binary 0xFF/0x00/0x80)"
    );
    assert_eq!(
        out.trailers, trailers,
        "R8 coalesced: chunked trailer section delivered as the H3 \
         trailing HEADERS frame, byte-identical"
    );

    // (2) PC-2 split-across-reads: the same input in separate reads must decode identically.
    let out = run_r8_scenario(RespBody::ChunkedWithTrailers {
        body: expected.clone(),
        chunk_sizes: vec![8192, 8192, 43_616],
        trailers: trailers.clone(),
        coalesce: false,
    })
    .await;
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R8 split: clean FIN after the trailing HEADERS");
    assert_eq!(
        out.body, expected,
        "R8 split: body byte-identical across split-read trailer parse"
    );
    assert_eq!(
        out.trailers, trailers,
        "R8 split: trailer section byte-identical when split across reads"
    );

    // (3) Chunked WITHOUT a trailer section: the trailing HEADERS frame is CONDITIONAL.
    let out = run_r8_scenario(RespBody::ChunkedWithTrailers {
        body: expected.clone(),
        chunk_sizes: vec![4096, 55_904],
        trailers: vec![],
        coalesce: true,
    })
    .await;
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R8 no-trailers: clean FIN");
    assert_eq!(out.body, expected, "R8 no-trailers: body byte-identical");
    assert!(
        out.trailers.is_empty(),
        "R8 no-trailers: a chunked response with NO trailer section \
         MUST NOT produce a spurious trailing HEADERS frame"
    );

    // (4) Content-Length carries no trailer section — the "trailer frame is chunked-only" contract.
    let out = run_r8_scenario(RespBody::ContentLength(expected.clone())).await;
    assert_eq!(out.status, Some(UPSTREAM_STATUS));
    assert!(out.fin, "R8 CL: clean FIN");
    assert_eq!(out.body, expected, "R8 CL: body byte-identical");
    assert!(
        out.trailers.is_empty(),
        "R8 CL: Content-Length framing carries no trailer frame"
    );
}
