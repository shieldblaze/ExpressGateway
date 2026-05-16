//! SESSION 1 / S1-B.2 — crate-local H3→H1 bridge end-to-end proof.
//!
//! `s1-inventory.md` capability #1 records the H3→H1 path as BUILT but
//! proven only by `round8_h3_authority_enforced.rs`, which asserts a
//! backend **probe-hit count** (and bypasses `QuicListener`, driving
//! `run_actor` directly). It never asserts the response status/body
//! verbatim, nor that `:authority` reached the H1 upstream as `Host`.
//!
//! This test closes that regression-gate gap *inside* the crate's own
//! suite (so `cargo test -p lb-quic` catches H3→H1 bridge regressions,
//! not only the repo-root integration suite). It drives the **REAL**
//! [`lb_quic::QuicListener`] (UDP bind → router per-CID dispatch →
//! `conn_actor::poll_h3` → `h3_bridge::h3_to_h1_roundtrip`) — the exact
//! same harness shape as repo-root `tests/quic_listener_e2e.rs`'s
//! `quic_listener_e2e_http3_get_through_proxy_to_h1_backend`, chosen
//! over driving `run_actor` directly because the real listener path is
//! reachable crate-locally (it is `pub` from `lb_quic`) and yields a
//! true front-to-back e2e.
//!
//! Assertions that go strictly beyond round8:
//!   * response `:status` is the upstream's **exact** code (201, a
//!     non-default value the bridge must echo, not a hardcoded 200);
//!   * response body is the upstream's **exact** bytes, verbatim;
//!   * the H1 upstream received `Host: <:authority>` — proving the
//!     `h3_bridge::build_h1_request` `:authority`→`Host` translation
//!     actually lands on the wire at the backend.
//!
//! This test does NOT touch any existing test and does not relax any
//! round8 assertion.

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
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const H3_ALPN: &[u8] = b"h3";
const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;

/// Distinctive, non-default response so the assertions cannot pass on a
/// hardcoded 200 / canned body. `build_h1_request` is bodyless today so
/// the upstream must reply with a `Content-Length`-delimited body.
const UPSTREAM_STATUS: u16 = 201;
const UPSTREAM_BODY: &[u8] = b"s1b2-verbatim-body-payload";
const REQUEST_AUTHORITY: &str = "h3-bridge.test:4433";
const REQUEST_PATH: &str = "/s1b2/echo";

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
        "lb-quic-h3h1-e2e-{}-{}-{counter}",
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
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ca_path = dir.join("ca.pem");
    let retry_path = dir.join("retry.key");
    std::fs::write(&cert_path, cert_pem.as_bytes()).unwrap();
    std::fs::write(&key_path, key_pem.as_bytes()).unwrap();
    std::fs::write(&ca_path, cert_pem.as_bytes()).unwrap();
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
    cfg.set_max_idle_timeout(5_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(4);
    cfg.set_initial_max_streams_uni(4);
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

/// Minimal HTTP/1.1 upstream that *captures the raw request head* of
/// the first connection (so the test can assert the `Host` header the
/// bridge synthesised from `:authority`) and replies with a
/// distinctive status + body. Returns the bound addr plus a receiver
/// that yields the captured request-head string.
async fn spawn_capturing_h1_backend() -> (
    SocketAddr,
    oneshot::Receiver<String>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<String>();
    let handle = tokio::spawn(async move {
        let mut tx = Some(tx);
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let captured_tx = tx.take();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut total = 0;
                loop {
                    match sock.read(&mut buf[total..]).await {
                        Ok(0) => break,
                        Ok(n) => {
                            total += n;
                            if buf
                                .windows(4)
                                .take(total.saturating_sub(3))
                                .any(|w| w == b"\r\n\r\n")
                            {
                                break;
                            }
                            if total == buf.len() {
                                buf.resize(total * 2, 0);
                            }
                        }
                        Err(_) => return,
                    }
                }
                let head = String::from_utf8_lossy(&buf[..total]).into_owned();
                if let Some(tx) = captured_tx {
                    let _ = tx.send(head);
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

/// Drive a client quiche::Connection through handshake, send a single
/// H3 GET on bidi stream 0, and collect the response status + body.
async fn drive_h3_get(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    deadline: tokio::time::Instant,
) -> Result<(u16, Vec<u8>), String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let stream_id: u64 = 0;
    let mut request_sent = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut decoded_status: Option<u16> = None;
    let mut decoded_body: Vec<u8> = Vec::new();
    let mut expected_len: Option<usize> = None;
    let mut body_complete = false;

    let local = socket.local_addr().map_err(|e| e.to_string())?;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "deadline; established={}, status={:?}, body_len={}",
                conn.is_established(),
                decoded_status,
                decoded_body.len()
            ));
        }
        if conn.is_closed() {
            return Err(format!("connection closed: {:?}", conn.peer_error()));
        }

        if !request_sent && conn.is_established() {
            let encoder = QpackEncoder::new();
            let headers = vec![
                (":method".to_string(), "GET".to_string()),
                (":scheme".to_string(), "https".to_string()),
                (":authority".to_string(), REQUEST_AUTHORITY.to_string()),
                (":path".to_string(), REQUEST_PATH.to_string()),
            ];
            let header_block = encoder
                .encode(&headers)
                .map_err(|e| format!("qpack: {e}"))?;
            let headers_frame = encode_frame(&H3Frame::Headers { header_block })
                .map_err(|e| format!("h3 frame: {e}"))?;
            let mut pos = 0;
            while pos < headers_frame.len() {
                let chunk = headers_frame.get(pos..).unwrap_or(&[]);
                let fin = pos + chunk.len() >= headers_frame.len();
                match conn.stream_send(stream_id, chunk, fin) {
                    Ok(n) => pos += n,
                    Err(quiche::Error::Done) => break,
                    Err(e) => return Err(format!("stream_send: {e}")),
                }
            }
            request_sent = true;
        }

        if conn.is_established() {
            let readable: Vec<u64> = conn.readable().collect();
            for sid in readable {
                if sid != stream_id {
                    continue;
                }
                let mut chunk = [0u8; 8192];
                loop {
                    match conn.stream_recv(sid, &mut chunk) {
                        Ok((n, _fin)) => {
                            rx_tail.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                        }
                        Err(quiche::Error::Done) | Err(quiche::Error::InvalidStreamState(_)) => {
                            break;
                        }
                        Err(e) => return Err(format!("stream_recv: {e}")),
                    }
                }
            }
            loop {
                match decode_frame(&rx_tail, 1 << 20) {
                    Ok((H3Frame::Headers { header_block }, consumed)) => {
                        rx_tail.drain(..consumed);
                        let hdrs = QpackDecoder::new()
                            .decode(&header_block)
                            .map_err(|e| format!("qpack decode: {e}"))?;
                        for (n, v) in hdrs {
                            if n == ":status" {
                                decoded_status =
                                    Some(v.parse::<u16>().map_err(|e| e.to_string())?);
                            } else if n == "content-length" {
                                expected_len = v.parse::<usize>().ok();
                            }
                        }
                    }
                    Ok((H3Frame::Data { payload }, consumed)) => {
                        rx_tail.drain(..consumed);
                        decoded_body.extend_from_slice(&payload);
                        if let Some(cl) = expected_len {
                            if decoded_body.len() >= cl {
                                body_complete = true;
                            }
                        }
                    }
                    Ok((_other, consumed)) => {
                        rx_tail.drain(..consumed);
                    }
                    Err(lb_h3::H3Error::Incomplete) => break,
                    Err(e) => return Err(format!("decode_frame: {e}")),
                }
            }
        }

        if let Some(status) = decoded_status {
            if body_complete {
                return Ok((status, decoded_body));
            }
        }

        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    let bytes = out_buf.get(..n).unwrap_or(&[]);
                    if let Err(e) = socket.send_to(bytes, info.to).await {
                        return Err(format!("send_to: {e}"));
                    }
                }
                Err(quiche::Error::Done) => break,
                Err(e) => return Err(format!("conn.send: {e}")),
            }
        }

        let timeout = conn.timeout().unwrap_or(Duration::from_millis(50));
        tokio::select! {
            r = tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)) => {
                match r {
                    Ok(Ok((n, from))) => {
                        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                        let info = quiche::RecvInfo { from, to: local };
                        match conn.recv(slice, info) {
                            Ok(_) | Err(quiche::Error::Done) => {}
                            Err(e) => return Err(format!("conn.recv: {e}")),
                        }
                    }
                    Ok(Err(e)) => return Err(format!("recv_from: {e}")),
                    Err(_) => conn.on_timeout(),
                }
            }
        }
    }
}

/// REAL front-to-back e2e: a quiche H3 client dials the production
/// `QuicListener`; the listener proxies through the H3→H1 bridge to a
/// real tokio TCP HTTP/1.1 upstream; the response flows back. Asserts
/// status + body verbatim AND that `:authority` arrived as `Host`.
#[tokio::test]
async fn h3_h1_bridge_status_body_and_host_verbatim_through_quic_listener() {
    let certs = generate_loopback_certs();
    let (backend_addr, head_rx, backend_handle) = spawn_capturing_h1_backend().await;
    let pool = build_tcp_pool();

    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    )
    .with_backends(vec![backend_addr], pool);

    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone()).await.unwrap();
    let server_addr = listener.local_addr();

    let client_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let client_local = client_sock.local_addr().unwrap();
    let mut client_cfg = build_client_config(&certs.ca);
    let scid = random_scid_bytes();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let conn = quiche::connect(
        Some(TEST_SNI),
        &scid_ref,
        client_local,
        server_addr,
        &mut client_cfg,
    )
    .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let result = drive_h3_get(conn, &client_sock, deadline).await;

    // Capture the request head the bridge sent to the H1 upstream
    // before tearing anything down.
    let captured_head = tokio::time::timeout(Duration::from_secs(2), head_rx)
        .await
        .ok()
        .and_then(Result::ok);

    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    backend_handle.abort();

    let (status, body) = result.expect("H3→H1 bridge e2e failed");

    // (1) Response STATUS verbatim — the upstream's exact non-default
    // code, not a hardcoded 200. Beyond round8 (which never checks
    // status on the valid path; it accepts 200 OR 502).
    assert_eq!(
        status, UPSTREAM_STATUS,
        "H3→H1 bridge must echo the H1 upstream's exact status; got {status}"
    );

    // (2) Response BODY verbatim. Beyond round8 (probe-hit count only).
    assert_eq!(
        body.as_slice(),
        UPSTREAM_BODY,
        "H3→H1 bridge must return the H1 upstream's body verbatim"
    );

    // (3) `:authority` → `Host` actually reached the H1 upstream.
    let head = captured_head.expect("H1 upstream did not capture a request head");
    let host_line = head
        .split("\r\n")
        .find(|l| l.to_ascii_lowercase().starts_with("host:"))
        .unwrap_or_else(|| panic!("no Host header in upstream request:\n{head}"));
    assert_eq!(
        host_line.trim(),
        format!("Host: {REQUEST_AUTHORITY}"),
        "H3 :authority must be translated to the H1 Host header verbatim"
    );
    // Request-line sanity: method + path the client sent arrived too.
    assert!(
        head.starts_with(&format!("GET {REQUEST_PATH} HTTP/1.1\r\n")),
        "H1 request line must carry the H3 :method + :path; got first line: {:?}",
        head.split("\r\n").next()
    );
}
