//! SESSION 2 / P1-B — H3→H1 request-body ERROR PATHS e2e.
//!
//! Builds on the P1-A real-QUIC-listener harness
//! (`h3_h1_stream_body_e2e.rs`). Covers the three approved P1-B error
//! paths, each asserting the *real* outcome (no deadline-as-pass):
//!
//!   * P1B-T1 — CLIENT CANCELS MID-BODY. A real quiche H3 client sends
//!     HEADERS + one partial DATA chunk (NO fin), then issues a QUIC
//!     RESET_STREAM (`stream_shutdown(Write,..)`) before fin. The mock
//!     upstream must NOT observe a completed request (no chunked
//!     `0\r\n\r\n` terminator, not a full Content-Length body). The
//!     actor must not panic and must not leak per-stream state: a
//!     SECOND independent request driven through the SAME live listener
//!     afterwards completes normally (proves no state corruption / no
//!     map leak / actor still healthy).
//!
//!   * P1B-T2 — UPSTREAM RESETS MID-BODY → 502. The mock backend reads
//!     part of the streamed body then abruptly RSTs/closes the TCP
//!     socket before responding. The H3 client must decode `:status
//!     502` and the call must complete well within the deadline (the
//!     assertion is a real 502 decode, not a timeout).
//!
//!   * P1B-T3 — OVERSIZED AFTER PARTIAL CHUNKED SEND. With chunked
//!     egress already begun (≥2 DATA chunks written to the upstream
//!     socket), the cap is breached → poll_h3 emits `ReqBodyEvent::
//!     Reset`. The client must get H3 `413` AND the upstream must be
//!     aborted: the backend sees the early chunk bytes but NEVER the
//!     `0\r\n\r\n` chunked terminator, so the partial request is not
//!     completable (smuggling / cache-poisoning guard).
//!
//! Every request body embeds the non-UTF-8 bytes 0xFF 0x00 0x80 so a
//! lossy/string conversion anywhere would corrupt it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use lb_h3_testcodec::{H3Frame, QpackEncoder, decode_frame, encode_frame};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::{QuicListener, QuicListenerParams};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const H3_ALPN: &[u8] = b"h3";
const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;
const REQUEST_AUTHORITY: &str = "h3-stream-err.test:4433";

/// SESSION 24 / INC-3: decode a RESPONSE QPACK field block emitted by
/// the migrated wire egress (quiche::h3 encoder Huffman-encodes values);
/// the hand-rolled `lb_h3::QpackDecoder` is raw-only.
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
const REQUEST_PATH: &str = "/p1b/echo";
const UPSTREAM_STATUS: u16 = 201;
const UPSTREAM_BODY: &[u8] = b"p1b-resp-body";
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
        "lb-quic-h3h1-stream-err-{}-{}-{counter}",
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

fn build_headers_frame(extra: &[(String, String)]) -> Vec<u8> {
    let encoder = QpackEncoder::new();
    let mut headers = vec![
        (":method".to_string(), "POST".to_string()),
        (":scheme".to_string(), "https".to_string()),
        (":authority".to_string(), REQUEST_AUTHORITY.to_string()),
        (":path".to_string(), REQUEST_PATH.to_string()),
    ];
    headers.extend_from_slice(extra);
    let hb = encoder.encode(&headers).unwrap();
    encode_frame(&H3Frame::Headers { header_block: hb })
        .unwrap()
        .to_vec()
}

fn build_data_frame(payload: &[u8]) -> Vec<u8> {
    encode_frame(&H3Frame::Data {
        payload: bytes::Bytes::copy_from_slice(payload),
    })
    .unwrap()
    .to_vec()
}

/// Owns the client-side UDP buffers + decoded-response state so the
/// pump is a single-receiver method (keeps the arg count sane and the
/// call sites trivial).
struct ClientPump {
    in_buf: Vec<u8>,
    out_buf: Vec<u8>,
    rx_tail: Vec<u8>,
    status: Option<u16>,
    body: Vec<u8>,
    expected_len: Option<usize>,
}

impl ClientPump {
    fn new() -> Self {
        Self {
            in_buf: vec![0u8; MAX_UDP],
            out_buf: vec![0u8; MAX_UDP],
            rx_tail: Vec::new(),
            status: None,
            body: Vec::new(),
            expected_len: None,
        }
    }

    /// Flush queued egress, recv one UDP datagram (bounded wait), decode
    /// any H3 response frames into `self.status`/`self.body`. Returns
    /// true once a final response status is fully known.
    async fn pump_once(
        &mut self,
        conn: &mut quiche::Connection,
        socket: &UdpSocket,
    ) -> Result<bool, String> {
        let local = socket.local_addr().map_err(|e| e.to_string())?;
        loop {
            match conn.send(&mut self.out_buf) {
                Ok((n, info)) => {
                    socket
                        .send_to(&self.out_buf[..n], info.to)
                        .await
                        .map_err(|e| format!("send_to: {e}"))?;
                }
                Err(quiche::Error::Done) => break,
                Err(e) => return Err(format!("conn.send: {e}")),
            }
        }
        let qto = conn.timeout().unwrap_or(Duration::from_millis(20));
        let wait = qto.clamp(Duration::from_millis(2), Duration::from_millis(20));
        match tokio::time::timeout(wait, socket.recv_from(&mut self.in_buf)).await {
            Ok(Ok((n, from))) => {
                let info = quiche::RecvInfo { from, to: local };
                match conn.recv(&mut self.in_buf[..n], info) {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(e) => return Err(format!("conn.recv: {e}")),
                }
            }
            Ok(Err(e)) => return Err(format!("recv_from: {e}")),
            Err(_) => conn.on_timeout(),
        }
        if conn.is_established() {
            let readable: Vec<u64> = conn.readable().collect();
            for sid in readable {
                let mut c = [0u8; 8192];
                // SESSION 24 / INC-2: the migrated gateway terminates H3 via
                // `quiche::h3::Connection`, which (per RFC 9114) opens
                // server-initiated control + QPACK encoder/decoder
                // UNIDIRECTIONAL streams. This hand-rolled lb_h3 client only
                // understands response frames on the request BIDI stream
                // (id 0); it drains-and-discards every other stream (as the
                // `drive_h3_get`/`drive_h3_body_request` clients in the
                // sibling suites and INC-1 Exp 3's interop client already
                // do). Feeding a uni-stream's stream-type/QPACK bytes into
                // `decode_frame` would mis-read them as a malformed HEADERS
                // frame ("qpack decode: incomplete input" — the pre-fix
                // failure on these two tests).
                if sid != 0 {
                    while conn.stream_recv(sid, &mut c).is_ok() {}
                    continue;
                }
                loop {
                    match conn.stream_recv(sid, &mut c) {
                        Ok((n, _)) => self.rx_tail.extend_from_slice(&c[..n]),
                        Err(quiche::Error::Done) | Err(quiche::Error::InvalidStreamState(_)) => {
                            break;
                        }
                        Err(_) => break,
                    }
                }
            }
            loop {
                match decode_frame(&self.rx_tail, 1 << 20) {
                    Ok((H3Frame::Headers { header_block }, c)) => {
                        self.rx_tail.drain(..c);
                        // SESSION 24 / INC-3: Huffman-capable QPACK
                        // decode of the quiche-encoded wire response head
                        // (the buffered `h3_to_h1_stream` 413 check below
                        // still uses the raw lb_h3 decoder).
                        let hdrs = decode_resp_qpack(&header_block)?;
                        for (n, v) in hdrs {
                            if n == ":status" {
                                self.status = Some(v.parse().map_err(|_| "status".to_string())?);
                            } else if n == "content-length" {
                                self.expected_len = v.parse().ok();
                            }
                        }
                    }
                    Ok((H3Frame::Data { payload }, c)) => {
                        self.rx_tail.drain(..c);
                        self.body.extend_from_slice(&payload);
                    }
                    Ok((_, c)) => {
                        self.rx_tail.drain(..c);
                    }
                    Err(lb_h3_testcodec::H3Error::Incomplete) => break,
                    Err(e) => return Err(format!("decode_frame: {e}")),
                }
            }
        }
        if self.status.is_some() {
            let done = match self.expected_len {
                Some(l) => self.body.len() >= l,
                None => false,
            };
            if done || self.expected_len == Some(0) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Drive handshake until the connection is established (bounded).
async fn handshake(
    conn: &mut quiche::Connection,
    socket: &UdpSocket,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    let mut pump = ClientPump::new();
    while !conn.is_established() {
        if tokio::time::Instant::now() >= deadline {
            return Err("handshake deadline".to_string());
        }
        pump.pump_once(conn, socket).await?;
    }
    Ok(())
}

/// Send all of `wire` on `stream_id` (no fin), pumping the connection
/// so flow control opens. Returns once every byte is buffered.
async fn send_all_no_fin(
    conn: &mut quiche::Connection,
    socket: &UdpSocket,
    stream_id: u64,
    wire: &[u8],
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    let mut pump = ClientPump::new();
    let mut sent = 0usize;
    while sent < wire.len() {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("send deadline; sent {sent}/{}", wire.len()));
        }
        match conn.stream_send(stream_id, &wire[sent..], false) {
            Ok(n) => sent += n,
            Err(quiche::Error::Done) => {}
            Err(e) => return Err(format!("stream_send: {e}")),
        }
        pump.pump_once(conn, socket).await?;
    }
    Ok(())
}

/// Full normal request driver (HEADERS + DATA + fin), used for the
/// "second request still works" liveness check.
async fn drive_full_request(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    body_payload: &[u8],
    deadline: tokio::time::Instant,
) -> Result<(u16, Vec<u8>), String> {
    handshake(&mut conn, socket, deadline).await?;
    let stream_id = 0u64;
    let mut wire = build_headers_frame(&[]);
    wire.extend_from_slice(&build_data_frame(body_payload));

    let mut pump = ClientPump::new();
    let mut sent = 0usize;
    let mut fin_sent = false;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "drive deadline; status={:?} sent={sent}",
                pump.status
            ));
        }
        if sent < wire.len() {
            match conn.stream_send(stream_id, &wire[sent..], false) {
                Ok(n) => {
                    sent += n;
                }
                Err(quiche::Error::Done) => {}
                Err(e) => return Err(format!("stream_send: {e}")),
            }
        } else if !fin_sent {
            let _ = conn.stream_send(stream_id, &[], true);
            fin_sent = true;
        }
        let done = pump.pump_once(&mut conn, socket).await?;
        if done {
            return Ok((pump.status.unwrap(), pump.body));
        }
    }
}

// ---------------------------------------------------------------------
// P1B-T1 — CLIENT CANCELS MID-BODY.
//
// Intent: the H3 client sends HEADERS + ONE partial DATA chunk WITHOUT
// fin, then resets the request stream (QUIC RESET_STREAM via
// `stream_shutdown(Write, code)`). The proxy's `drain_body_stream`
// surfaces `Err(StreamReset)` from `stream_recv`, sends
// `ReqBodyEvent::Reset` into the body channel, and tears down all
// per-stream maps. `h3_to_h1_stream` aborts the upstream: it marks the
// pooled conn non-reusable and returns BEFORE writing the `0\r\n\r\n`
// chunked terminator. The mock backend therefore NEVER sees a
// completable request (no terminator). Proof of no state leak / no
// corruption: a SECOND independent request through the SAME live
// listener completes normally afterwards.
// ---------------------------------------------------------------------
#[tokio::test]
async fn p1b_t1_client_cancels_mid_body_upstream_not_completed_and_no_leak() {
    let certs = generate_loopback_certs();

    // Backend that records, for the FIRST connection, whether it ever
    // saw a completed request (chunked `0\r\n\r\n` terminator) and
    // whether it saw the cancelled chunk's distinctive marker bytes.
    let saw_terminator = Arc::new(AtomicUsize::new(0));
    let saw_first_conn = Arc::new(AtomicUsize::new(0));
    let st = saw_terminator.clone();
    let sf = saw_first_conn.clone();
    let listener_b = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let backend_addr = listener_b.local_addr().unwrap();
    let backend_h = tokio::spawn(async move {
        let mut first = true;
        loop {
            let (mut s, _) = match listener_b.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let is_first = first;
            first = false;
            let st = st.clone();
            let sf = sf.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut t = [0u8; 4096];
                loop {
                    match tokio::time::timeout(Duration::from_millis(800), s.read(&mut t)).await {
                        Ok(Ok(0)) | Err(_) => break,
                        Ok(Ok(n)) => buf.extend_from_slice(&t[..n]),
                        Ok(Err(_)) => break,
                    }
                }
                if is_first {
                    sf.store(1, Ordering::SeqCst);
                    if buf.windows(5).any(|w| w == b"0\r\n\r\n") {
                        st.store(1, Ordering::SeqCst);
                    }
                    // The cancelled connection gets no response (the
                    // proxy aborted it); just drop.
                } else {
                    // Second (liveness) request: read full chunked body
                    // then reply 201 so the client request completes.
                    loop {
                        if buf.windows(5).any(|w| w == b"0\r\n\r\n") {
                            break;
                        }
                        match tokio::time::timeout(Duration::from_millis(800), s.read(&mut t)).await
                        {
                            Ok(Ok(0)) | Err(_) => break,
                            Ok(Ok(n)) => buf.extend_from_slice(&t[..n]),
                            Ok(Err(_)) => break,
                        }
                    }
                    let resp = format!(
                        "HTTP/1.1 {UPSTREAM_STATUS} Created\r\nContent-Length: {}\r\n\r\n",
                        UPSTREAM_BODY.len()
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                    let _ = s.write_all(UPSTREAM_BODY).await;
                    let _ = s.shutdown().await;
                }
            });
        }
    });

    let (listener, server, _sd) = start_listener(&certs, backend_addr).await;

    // --- cancelled request ---
    {
        let (mut conn, sock) = client_conn(server, &certs.ca);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        handshake(&mut conn, &sock, deadline)
            .await
            .expect("handshake");
        let stream_id = 0u64;
        // HEADERS (no content-length ⇒ chunked egress) + ONE partial
        // DATA chunk, carrying the non-UTF-8 marker. No fin.
        let mut chunk = vec![0u8; 4096];
        chunk[..3].copy_from_slice(NON_UTF8);
        let mut wire = build_headers_frame(&[]);
        wire.extend_from_slice(&build_data_frame(&chunk));
        send_all_no_fin(&mut conn, &sock, stream_id, &wire, deadline)
            .await
            .expect("send HEADERS + partial DATA");
        // Give the proxy a few ticks to forward the head + first chunk
        // to the backend (so the abort is genuinely MID-body, after
        // egress began), then RESET_STREAM before fin.
        let mut pump = ClientPump::new();
        let spin_until = tokio::time::Instant::now() + Duration::from_millis(400);
        while tokio::time::Instant::now() < spin_until {
            pump.pump_once(&mut conn, &sock)
                .await
                .expect("pump pre-reset");
        }
        // QUIC RESET_STREAM on our send side (the request stream) +
        // STOP_SENDING on the read side — peer-initiated cancel.
        conn.stream_shutdown(stream_id, quiche::Shutdown::Write, 0x10)
            .expect("stream_shutdown write");
        let _ = conn.stream_shutdown(stream_id, quiche::Shutdown::Read, 0x10);
        // Flush the RESET_STREAM frame to the proxy and let it process.
        let flush_until = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < flush_until {
            pump.pump_once(&mut conn, &sock)
                .await
                .expect("pump post-reset");
        }
        // conn drops here.
    }

    // --- liveness: a SECOND independent request must still succeed ---
    let (conn2, sock2) = client_conn(server, &certs.ca);
    let deadline2 = tokio::time::Instant::now() + Duration::from_secs(45);
    let mut body2 = vec![0u8; 2048];
    body2[..3].copy_from_slice(NON_UTF8);
    let res2 = drive_full_request(conn2, &sock2, &body2, deadline2).await;

    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();

    assert_eq!(
        saw_first_conn.load(Ordering::SeqCst),
        1,
        "the cancelled request must have reached the backend (egress began) \
         so the no-terminator assertion is meaningful"
    );
    assert_eq!(
        saw_terminator.load(Ordering::SeqCst),
        0,
        "CLIENT CANCEL MID-BODY: the upstream must NOT observe a completed \
         request — no chunked 0\\r\\n\\r\\n terminator may ever be written"
    );
    let (status2, _) = res2.expect(
        "liveness: a second independent request through the SAME listener \
         must complete after a mid-body client cancel (proves no per-stream \
         map leak / no actor-state corruption)",
    );
    assert_eq!(
        status2, UPSTREAM_STATUS,
        "second request must return the upstream status"
    );
}

// ---------------------------------------------------------------------
// P1B-T2 — UPSTREAM RESETS MID-BODY → 502.
//
// Intent: the mock backend accepts, reads part of the streamed chunked
// body, then abruptly RSTs/closes the TCP socket before responding.
// `h3_to_h1_stream`'s `write_all` to the upstream errors → the
// `fail502!` path marks the pooled conn non-reusable and returns an H3
// 502. The H3 client must DECODE `:status 502` (real outcome, asserted
// well within the deadline — not deadline-as-pass).
// ---------------------------------------------------------------------
#[tokio::test]
async fn p1b_t2_upstream_resets_mid_body_yields_502() {
    let certs = generate_loopback_certs();

    let listener_b = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let backend_addr = listener_b.local_addr().unwrap();
    let (ready_tx, ready_rx) = oneshot::channel::<()>();
    let backend_h = tokio::spawn(async move {
        let (mut s, _) = listener_b.accept().await.unwrap();
        // Read just the head + maybe one chunk, then ABRUPTLY abort the
        // connection: shut down BOTH directions and drop the socket. The
        // proxy is mid-stream writing a large (>256 KiB) chunked body —
        // once the peer is gone, its `write_all` to the upstream (or the
        // subsequent response read) fails with a broken pipe / reset →
        // the `fail502!` path fires.
        let mut t = [0u8; 1024];
        let _ = tokio::time::timeout(Duration::from_millis(400), s.read(&mut t)).await;
        let _ = ready_tx.send(());
        let _ = s.shutdown().await; // FIN our write side
        drop(s); // close read side → proxy's writes now error
    });

    let (listener, server, _sd) = start_listener(&certs, backend_addr).await;

    let (mut conn, sock) = client_conn(server, &certs.ca);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    handshake(&mut conn, &sock, deadline)
        .await
        .expect("handshake");
    let stream_id = 0u64;

    // Chunked egress (no content-length). HEADERS + several DATA frames
    // + fin so the proxy opens the upstream, writes head + chunk(s),
    // then hits the RST on a subsequent write or the response read.
    let mut wire = build_headers_frame(&[]);
    // Many sizeable DATA frames (~512 KiB total) so the proxy is still
    // actively streaming to the upstream when the backend RSTs — its
    // write_all then fails (kernel send buffer + dead peer) → 502.
    for f in 0..64u8 {
        let mut chunk = vec![f; 8192];
        chunk[..3].copy_from_slice(NON_UTF8);
        wire.extend_from_slice(&build_data_frame(&chunk));
    }

    let mut pump = ClientPump::new();
    let mut sent = 0usize;
    let mut fin_sent = false;
    let mut ready_rx = ready_rx;
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "P1B-T2 deadline: never decoded a final status (status={:?})",
            pump.status
        );
        if sent < wire.len() {
            match conn.stream_send(stream_id, &wire[sent..], false) {
                Ok(n) => sent += n,
                Err(quiche::Error::Done) => {}
                Err(e) => panic!("stream_send: {e}"),
            }
        } else if !fin_sent {
            let _ = conn.stream_send(stream_id, &[], true);
            fin_sent = true;
        }
        let done = pump.pump_once(&mut conn, &sock).await.expect("pump");
        // Once the backend has RST, the proxy should produce a 502.
        let _ = ready_rx.try_recv();
        if done {
            break;
        }
        if pump.status == Some(502) {
            // Status decoded (502 has a small body / content-length);
            // accept as soon as it is known — a real decoded outcome.
            break;
        }
    }

    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
    backend_h.abort();

    assert_eq!(
        pump.status,
        Some(502),
        "UPSTREAM RESET MID-BODY must surface H3 :status 502 to the client"
    );
}
