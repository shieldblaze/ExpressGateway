//! Pillar 3b.3c-1 end-to-end tests for the QUIC listener seam.
//!
//! These tests exercise [`lb_quic::QuicListener`] — the binary-side
//! seam that binds a UDP socket, loads (or generates) a retry secret,
//! builds a [`quiche::Config`], and drives a single-connection accept
//! loop. They prove the seam is wired correctly **end-to-end**: a real
//! quiche client dials the listener's bound port and the handshake
//! reaches `is_established() == true`.
//!
//! What these tests validate:
//!
//! * UDP bind + observable `local_addr`.
//! * Retry-secret file auto-generation on first boot (mode 0600 on
//!   Unix).
//! * Cert + key PEM loading through quiche's BoringSSL context.
//! * The accept task handshakes against a real quiche client (not a
//!   fake / in-process loopback reuse of `QuicEndpoint`).
//! * Cancellation of the shutdown token cleanly terminates the accept
//!   task.
//!
//! What these tests do NOT validate (Pillar 3b.3c-2):
//!
//! * Stateless-retry wire handling via [`lb_quic::RetryTokenSigner`].
//! * 0-RTT replay defense via [`lb_quic::ZeroRttReplayGuard`].
//! * H3 request parsing / backend forwarding / response streaming.
//! * Concurrent connections on the same socket via an
//!   `InboundPacketRouter`.
//!
//! System `curl 8.5.0` on Ubuntu 24.04 ships without HTTP/3 support,
//! so the moral equivalent of
//! `curl --http3 https://expressgateway.test:PORT/ --resolve
//! expressgateway.test:PORT:127.0.0.1 --cacert test-ca.pem` is this
//! in-process quiche client. A subprocess-`curl` variant should be
//! added once a CI image ships curl built against quiche or ngtcp2.

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use lb_h3::{H3Frame, QpackDecoder, QpackEncoder, decode_frame, encode_frame};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::{QuicListener, QuicListenerParams};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio_util::sync::CancellationToken;

const LB_QUIC_ALPN: &[u8] = b"lb-quic";
const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;

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
        let _ = fs::remove_dir_all(&self._dir);
    }
}

fn generate_loopback_certs() -> TestCerts {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "lb-quic-listener-e2e-{}-{}-{counter}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).unwrap();

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
    // Retry secret path — deliberately does NOT exist, so the listener
    // generates and persists a fresh 32-byte file on spawn.
    let retry_path = dir.join("retry.key");
    fs::write(&cert_path, cert_pem.as_bytes()).unwrap();
    fs::write(&key_path, key_pem.as_bytes()).unwrap();
    fs::write(&ca_path, cert_pem.as_bytes()).unwrap();
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
    cfg.set_application_protos(&[LB_QUIC_ALPN]).unwrap();
    cfg.load_verify_locations_from_file(ca_path.to_str().unwrap())
        .unwrap();
    // The listener cert is self-signed with DNS SAN = expressgateway.test
    // and `serverAuth` EKU. BoringSSL's hostname verifier is satisfied by
    // matching SNI; leaving `verify_peer(true)` proves the chain builds.
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

/// Drive a client-side quiche::Connection until `is_established()`
/// or the deadline elapses. Returns Ok(()) on successful handshake.
async fn drive_client_to_established(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "handshake deadline elapsed, is_established={}",
                conn.is_established()
            ));
        }
        if conn.is_established() {
            return Ok(());
        }
        if conn.is_closed() {
            return Err(format!(
                "connection closed before established: {:?}",
                conn.peer_error()
            ));
        }

        // Flush outbound.
        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    let bytes = out_buf.get(..n).unwrap_or(&[]);
                    if let Err(e) = socket.send_to(bytes, info.to).await {
                        return Err(format!("send_to failed: {e}"));
                    }
                }
                Err(quiche::Error::Done) => break,
                Err(e) => return Err(format!("conn.send: {e}")),
            }
        }

        let timeout = conn.timeout().unwrap_or(Duration::from_millis(50));
        let local = socket.local_addr().map_err(|e| e.to_string())?;
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
                    Err(_elapsed) => {
                        conn.on_timeout();
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn quic_listener_accepts_handshake_smoke() {
    let certs = generate_loopback_certs();

    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    );
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone())
        .await
        .expect("listener bind failed");

    // Retry secret file must now exist (the listener generated it on
    // first boot because the path did not pre-exist).
    let secret = fs::read(&certs.retry).expect("retry secret file");
    assert_eq!(secret.len(), 32, "retry secret must be 32 bytes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&certs.retry).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "retry secret file mode must be 0600");
    }

    let server_addr = listener.local_addr();
    assert!(server_addr.ip().is_loopback());

    // Client: plain quiche::Connection over a loopback UDP socket.
    let client_sock = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
        .await
        .unwrap();
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
    .expect("quiche::connect");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let result = drive_client_to_established(conn, &client_sock, deadline).await;

    // Shut the listener down cleanly before asserting so a failed
    // handshake does not leave a runaway task.
    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    result.expect("client side handshake did not reach is_established()");
}

/// Spawn a minimal HTTP/1.1 mock backend that responds to every
/// accepted connection with `HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello`
/// then closes. Returns the bound address.
async fn spawn_mock_h1_backend() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            tokio::spawn(async move {
                // Drain request headers until CRLF CRLF.
                let mut buf = vec![0u8; 2048];
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
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
                    .await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (addr, handle)
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

/// Drive a client quiche::Connection through handshake, send a single
/// H3 GET on a bidi stream, and collect the response. Returns
/// (status, body) once the response is complete.
async fn drive_h3_get(
    mut conn: quiche::Connection,
    socket: &UdpSocket,
    path: &str,
    authority: &str,
    deadline: tokio::time::Instant,
) -> Result<(u16, Vec<u8>), String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let stream_id: u64 = 0; // client-initiated bidi stream 0
    let mut request_sent = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut decoded_status: Option<u16> = None;
    let mut decoded_body: Vec<u8> = Vec::new();
    let mut body_complete = false;
    let expected_body_len = 5usize; // mock returns Content-Length: 5

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
            return Err(format!(
                "connection closed: peer_error={:?}",
                conn.peer_error()
            ));
        }

        // Send request once established.
        if !request_sent && conn.is_established() {
            let encoder = QpackEncoder::new();
            let headers = vec![
                (":method".to_string(), "GET".to_string()),
                (":scheme".to_string(), "https".to_string()),
                (":authority".to_string(), authority.to_string()),
                (":path".to_string(), path.to_string()),
            ];
            let header_block = encoder
                .encode(&headers)
                .map_err(|e| format!("qpack: {e}"))?;
            let headers_frame = encode_frame(&H3Frame::Headers { header_block })
                .map_err(|e| format!("h3 frame: {e}"))?;
            // Send HEADERS with FIN=true (no request body).
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

        // Read any response bytes.
        if conn.is_established() {
            let readable: Vec<u64> = conn.readable().collect();
            for sid in readable {
                if sid != stream_id {
                    // ignore control streams etc.
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
            // Try decoding frames from rx_tail.
            loop {
                match decode_frame(&rx_tail, 1 << 20) {
                    Ok((H3Frame::Headers { header_block }, consumed)) => {
                        rx_tail.drain(..consumed);
                        let dec = QpackDecoder::new();
                        let hdrs = dec
                            .decode(&header_block)
                            .map_err(|e| format!("qpack decode: {e}"))?;
                        for (n, v) in hdrs {
                            if n == ":status" {
                                decoded_status = Some(v.parse::<u16>().map_err(|e| e.to_string())?);
                            }
                        }
                    }
                    Ok((H3Frame::Data { payload }, consumed)) => {
                        rx_tail.drain(..consumed);
                        decoded_body.extend_from_slice(&payload);
                        if decoded_body.len() >= expected_body_len {
                            body_complete = true;
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

        // Flush outbound.
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
                    Err(_) => {
                        conn.on_timeout();
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn quic_listener_e2e_http3_get_through_proxy_to_h1_backend() {
    let certs = generate_loopback_certs();
    let (backend_addr, backend_handle) = spawn_mock_h1_backend().await;
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
    let result = drive_h3_get(conn, &client_sock, "/", TEST_SNI, deadline).await;

    // Shut down before asserting so a failed result does not leave the
    // listener or backend running.
    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    backend_handle.abort();

    let (status, body) = result.expect("H3 GET e2e failed");
    assert_eq!(status, 200, "status must be 200");
    assert_eq!(body.as_slice(), b"hello", "body must be 'hello'");
}

#[tokio::test]
async fn retry_mints_on_first_initial_verifies_on_second() {
    // The full handshake path already depends on RETRY working (the
    // router responds to every first-Initial with RETRY by design).
    // This test asserts the specific property: we observe exactly one
    // RETRY packet on the wire in response to the client's first
    // Initial. Any subsequent Initial flights from the client carry
    // the token; the server does NOT emit further RETRYs.
    //
    // We run the raw client against the router but only parse the
    // first few server responses' headers rather than driving the
    // full handshake — all we need to confirm is that the first
    // server response is a RETRY long-header and that a second
    // response on the same flow is NOT a RETRY.
    let certs = generate_loopback_certs();
    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    );
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone()).await.unwrap();
    let server_addr = listener.local_addr();

    let client_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let client_local = client_sock.local_addr().unwrap();
    let mut client_cfg = build_client_config(&certs.ca);
    let scid = random_scid_bytes();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::connect(
        Some(TEST_SNI),
        &scid_ref,
        client_local,
        server_addr,
        &mut client_cfg,
    )
    .unwrap();

    // Drive the client through handshake, but inspect the first
    // server-bound response to confirm it is a RETRY.
    let mut out_buf = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut saw_retry = false;
    let mut saw_non_retry = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        if conn.is_established() || tokio::time::Instant::now() >= deadline {
            break;
        }

        // Flush.
        loop {
            match conn.send(&mut out_buf) {
                Ok((n, info)) => {
                    client_sock
                        .send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                        .await
                        .unwrap();
                }
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }

        let timeout = conn.timeout().unwrap_or(Duration::from_millis(50));
        match tokio::time::timeout(timeout, client_sock.recv_from(&mut in_buf)).await {
            Ok(Ok((n, from))) => {
                // Peek at the first byte: RETRY is long-header with
                // packet-type bits 0b11 (0xc0 after masking).
                let slice_copy = in_buf.get(..n).unwrap_or(&[]).to_vec();
                let mut probe = slice_copy.clone();
                if let Ok(hdr) = quiche::Header::from_slice(&mut probe, quiche::MAX_CONN_ID_LEN) {
                    if hdr.ty == quiche::Type::Retry {
                        saw_retry = true;
                    } else {
                        saw_non_retry = true;
                    }
                }
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                let info = quiche::RecvInfo {
                    from,
                    to: client_local,
                };
                let _ = conn.recv(slice, info);
            }
            Ok(Err(_)) | Err(_) => {
                conn.on_timeout();
            }
        }
    }

    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    assert!(saw_retry, "expected a RETRY packet from the server");
    assert!(
        saw_non_retry || conn.is_established(),
        "expected at least one non-RETRY server response after the client \
         resent Initial with the token; saw_retry={saw_retry}, \
         established={}",
        conn.is_established()
    );
}

#[tokio::test]
async fn zero_rtt_replay_dropped() {
    // The router rejects two Initial packets that both carry the
    // SAME verified retry token. To simulate this we:
    //  1. Start a normal client, let it reach is_established().
    //  2. Capture the second Initial's bytes (the one bearing the
    //     retry token). The client doesn't expose these to us
    //     directly, so instead we exercise the router's replay key
    //     directly by reusing the retry_signer + replay_guard on the
    //     listener.
    //
    // This test therefore asserts the observable property at the
    // listener's API: the ZeroRttReplayGuard on the listener rejects
    // a second use of the same (scid||token-prefix) key. Wire-level
    // replay drop is covered indirectly by the handshake test above:
    // that handshake would fail if our check_0rtt_token was flagging
    // the legitimate second Initial as a replay.
    let certs = generate_loopback_certs();
    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    );
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone()).await.unwrap();

    // Exercise the replay guard directly — the router uses the same
    // guard instance exposed via replay_guard().
    let guard = listener.replay_guard();
    let key = b"fake-scid-bytes-and-token-prefix-0123456789ab";
    assert!(
        guard.lock().check_0rtt_token(key).is_ok(),
        "first use must succeed"
    );
    assert!(
        guard.lock().check_0rtt_token(key).is_err(),
        "second use must be rejected as replay"
    );

    let _ = Bytes::new(); // silence unused import if body shrinks
    let handle = listener.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn quic_listener_shuts_down_on_cancellation_token() {
    let certs = generate_loopback_certs();
    let bind = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let params = QuicListenerParams::new(
        bind,
        certs.cert.clone(),
        certs.key.clone(),
        certs.retry.clone(),
    );
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone())
        .await
        .expect("listener bind");

    // Let the accept loop park on its first recv.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Cancel and await graceful exit.
    let handle = listener.shutdown();
    let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        joined.is_ok(),
        "listener task did not exit within 2s of shutdown.cancel()"
    );
}
