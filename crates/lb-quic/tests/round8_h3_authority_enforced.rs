//! ROUND8-L7-16 proof — the protocol-neutral authority validator is
//! ON THE H3/QUIC REQUEST PATH, not just H1/H2.
//!
//! Reference: HAProxy `BUG/MAJOR: http: forbid comma character in
//! authority value` + `BUG/MEDIUM: h1: Enforce the authority
//! validation during H1 request parsing`. L7-09 closed H1/H2 with a
//! single `lb_l7::authority::validate_request` choke point; the H3
//! datapath lives in the separate `lb-quic` crate
//! (`conn_actor::poll_h3`) which never reaches that choke point.
//! ROUND8-L7-16 wires `lb_core::authority::validate` (the EXACT same
//! predicate the H1/H2 path uses) into the H3 ingress dispatch BEFORE
//! any of the three upstream branches.
//!
//! This test drives a REAL loopback QUIC handshake, runs the REAL
//! [`lb_quic::run_actor`] with a REAL accept-counting TCP probe
//! backend, and sends a REAL lb-h3 HEADERS frame on a client bidi
//! stream, asserting:
//!   * comma-in-`:authority` → H3 `:status 400` AND the probe backend
//!     records ZERO connections (the validator tripped BEFORE
//!     upstream selection — the exact HAProxy lesson, H3 leg);
//!   * a well-formed `:authority` is NOT rejected — it reaches the
//!     probe backend (non-zero connection count), proving the gate is
//!     value-sanitisation only and does not over-reject.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lb_h3::{H3Frame, QpackDecoder, QpackEncoder, decode_frame, encode_frame};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_quic::conn_actor::{ActorParams, InboundPacket, run_actor};

const MAX_UDP: usize = 65_535;
const TEST_SNI: &str = "expressgateway.test";
const H3_ALPN_PROTOS: &[&[u8]] = &[b"h3", b"h3-29"];
const HANDSHAKE_BUDGET: Duration = Duration::from_secs(3);
const RESPONSE_BUDGET: Duration = Duration::from_secs(4);

struct CertTempFile(PathBuf);

impl Drop for CertTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_test_cert() -> (CertTempFile, CertTempFile) {
    let generated =
        rcgen::generate_simple_self_signed(vec![TEST_SNI.to_string()]).expect("rcgen self-signed");
    let cert_pem = generated.cert.pem();
    let key_pem = generated.key_pair.serialize_pem();
    let dir = std::env::temp_dir();
    let subsec = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    // ROUND8 Phase-E test-harness micro-fix (verify, task#79): the prior
    // nonce was `pid * K + subsec_nanos`. `std::process::id()` is constant
    // across every `#[test]` in this binary, so two tests running in
    // parallel could land on the same `subsec_nanos()` (or collide through
    // the wrapping mul/add) and therefore the SAME cert path — one test's
    // `CertTempFile::drop` then `remove_file`s the cert another test is
    // still loading, yielding the intermittent "load cert" parallel flake.
    // A process-global monotonic counter makes every cert path unique by
    // construction (no parallel collision possible, no serial-only crutch).
    static CERT_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = CERT_SEQ.fetch_add(1, Ordering::Relaxed);
    let nonce = std::process::id()
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(subsec);
    let cert_path = dir.join(format!("lb-quic-l7-16-cert-{nonce}-{seq}.pem"));
    let key_path = dir.join(format!("lb-quic-l7-16-key-{nonce}-{seq}.pem"));
    std::fs::write(&cert_path, cert_pem).expect("write cert");
    std::fs::write(&key_path, key_pem).expect("write key");
    (CertTempFile(cert_path), CertTempFile(key_path))
}

fn build_config(server: bool, cert_path: &str, key_path: &str) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).expect("Config::new");
    cfg.set_application_protos(H3_ALPN_PROTOS).expect("alpn");
    cfg.set_max_idle_timeout(8_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(10 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(1024 * 1024);
    cfg.set_initial_max_stream_data_uni(1024 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(16);
    cfg.set_disable_active_migration(true);
    if server {
        cfg.load_cert_chain_from_pem_file(cert_path)
            .expect("load cert");
        cfg.load_priv_key_from_pem_file(key_path).expect("load key");
    } else {
        cfg.load_verify_locations_from_file(cert_path)
            .expect("trust cert");
        cfg.verify_peer(true);
    }
    cfg
}

fn scid_bytes(salt: u32) -> [u8; quiche::MAX_CONN_ID_LEN] {
    let mut b = [0u8; quiche::MAX_CONN_ID_LEN];
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
        .wrapping_add(salt);
    for (i, byte) in b.iter_mut().enumerate() {
        *byte = nanos
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add(u32::try_from(i).unwrap_or(0))
            .to_le_bytes()[i % 4];
    }
    b
}

/// A real listening backend that counts inbound TCP connections. If
/// the H3 authority gate is bypassed, the actor's picker dials this
/// address and `count` goes non-zero.
async fn spawn_probe_backend() -> (SocketAddr, Arc<AtomicU32>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicU32::new(0));
    let c = Arc::clone(&count);
    tokio::spawn(async move {
        loop {
            if let Ok((sock, _)) = listener.accept().await {
                c.fetch_add(1, Ordering::SeqCst);
                // Minimal well-formed H1 reply so the valid-authority
                // case completes cleanly rather than 502-ing on read.
                use tokio::io::AsyncWriteExt;
                let mut s = sock;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = s.flush().await;
            }
        }
    });
    (addr, count)
}

fn pool() -> TcpPool {
    TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts::default(),
        lb_io::Runtime::new(),
    )
}

fn h3_headers_frame(authority: &str) -> Vec<u8> {
    let headers = vec![
        (":method".to_string(), "GET".to_string()),
        (":scheme".to_string(), "https".to_string()),
        (":path".to_string(), "/p".to_string()),
        (":authority".to_string(), authority.to_string()),
    ];
    let block = QpackEncoder::new().encode(&headers).expect("qpack encode");
    encode_frame(&H3Frame::Headers {
        header_block: block,
    })
    .expect("h3 headers frame")
    .to_vec()
}

async fn flush(conn: &mut quiche::Connection, socket: &UdpSocket, out: &mut [u8]) {
    loop {
        match conn.send(out) {
            Ok((n, info)) => {
                socket
                    .send_to(out.get(..n).unwrap_or(&[]), info.to)
                    .await
                    .expect("send_to");
            }
            Err(quiche::Error::Done) => break,
            Err(e) => panic!("conn.send: {e:?}"),
        }
    }
}

async fn try_recv_one(
    conn: &mut quiche::Connection,
    socket: &UdpSocket,
    local: SocketAddr,
    in_buf: &mut [u8],
    wait: Duration,
) -> bool {
    match tokio::time::timeout(wait, socket.recv_from(in_buf)).await {
        Ok(Ok((n, from))) => {
            let info = quiche::RecvInfo { from, to: local };
            let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
            matches!(conn.recv(slice, info), Ok(_) | Err(quiche::Error::Done))
        }
        Ok(Err(_)) | Err(_) => false,
    }
}

/// Drive one full case: handshake the client against a server actor,
/// send an H3 HEADERS frame with `authority`, and return the decoded
/// H3 `:status` (if any response arrives) plus the probe-backend
/// connection count.
async fn run_case(authority: &str) -> (Option<u16>, u32) {
    let (cert_file, key_file) = write_test_cert();
    let cert_path = cert_file.0.to_str().unwrap().to_string();
    let key_path = key_file.0.to_str().unwrap().to_string();

    let (backend, hits) = spawn_probe_backend().await;

    let server_socket = Arc::new(
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap(),
    );
    let server_local = server_socket.local_addr().unwrap();
    let client_socket = Arc::new(
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap(),
    );
    let client_local = client_socket.local_addr().unwrap();

    let mut server_cfg = build_config(true, &cert_path, &key_path);
    let mut client_cfg = build_config(false, &cert_path, &key_path);

    let s_scid_bytes = scid_bytes(1);
    let s_scid = quiche::ConnectionId::from_ref(&s_scid_bytes);
    let c_scid_bytes = scid_bytes(2);
    let c_scid = quiche::ConnectionId::from_ref(&c_scid_bytes);

    let mut server_conn =
        quiche::accept(&s_scid, None, server_local, client_local, &mut server_cfg).unwrap();
    let mut client_conn = quiche::connect(
        Some(TEST_SNI),
        &c_scid,
        client_local,
        server_local,
        &mut client_cfg,
    )
    .unwrap();

    // Handshake pump (both ends driven inline until established).
    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];
    let deadline = tokio::time::Instant::now() + HANDSHAKE_BUDGET;
    while !(server_conn.is_established() && client_conn.is_established()) {
        if tokio::time::Instant::now() > deadline {
            panic!("handshake did not establish");
        }
        flush(&mut client_conn, &client_socket, &mut out).await;
        flush(&mut server_conn, &server_socket, &mut out).await;
        try_recv_one(
            &mut server_conn,
            &server_socket,
            server_local,
            &mut in_buf,
            Duration::from_millis(30),
        )
        .await;
        try_recv_one(
            &mut client_conn,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(30),
        )
        .await;
    }

    // Send the request HEADERS on client bidi stream 0 BEFORE handing
    // the server conn to the actor (the actor's poll_h3 picks it up
    // once it observes the readable stream).
    let frame = h3_headers_frame(authority);
    client_conn
        .stream_send(0, &frame, true)
        .expect("stream_send");
    flush(&mut client_conn, &client_socket, &mut out).await;

    // Hand the established server conn to the real actor. The router
    // is simulated by a forwarder task draining the server UDP socket
    // into the actor's inbound channel.
    let (tx, rx) = mpsc::channel::<InboundPacket>(64);
    let cancel = CancellationToken::new();
    let fwd_socket = Arc::clone(&server_socket);
    let fwd_cancel = cancel.clone();
    let forwarder = tokio::spawn(async move {
        let mut buf = vec![0u8; MAX_UDP];
        loop {
            tokio::select! {
                () = fwd_cancel.cancelled() => break,
                r = fwd_socket.recv_from(&mut buf) => {
                    if let Ok((n, from)) = r {
                        let pkt = InboundPacket {
                            data: buf.get(..n).unwrap_or(&[]).to_vec(),
                            from,
                            to: server_local,
                        };
                        if tx.send(pkt).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    });

    let params = ActorParams {
        conn: server_conn,
        socket: Arc::clone(&server_socket),
        inbound: rx,
        cancel: cancel.clone(),
        pool: pool(),
        backends: Arc::new(vec![backend]),
        h3_backend: None,
        h2_backend: None,
        // S16 Mode B seam: None keeps this on the H3 termination path.
        raw_quic_backend: None,
    };
    let actor = tokio::spawn(run_actor(params));

    // Drive the client until a response HEADERS frame is decoded or
    // the budget expires.
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut status: Option<u16> = None;
    let resp_deadline = tokio::time::Instant::now() + RESPONSE_BUDGET;
    while status.is_none() && tokio::time::Instant::now() < resp_deadline {
        flush(&mut client_conn, &client_socket, &mut out).await;
        try_recv_one(
            &mut client_conn,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(30),
        )
        .await;
        let mut chunk = [0u8; 8192];
        while let Ok((n, _fin)) = client_conn.stream_recv(0, &mut chunk) {
            rx_tail.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
        }
        loop {
            match decode_frame(&rx_tail, 1 << 20) {
                Ok((H3Frame::Headers { header_block }, consumed)) => {
                    rx_tail.drain(..consumed);
                    if let Ok(hdrs) = QpackDecoder::new().decode(&header_block) {
                        for (k, v) in hdrs {
                            if k == ":status" {
                                status = v.parse::<u16>().ok();
                            }
                        }
                    }
                }
                Ok((_other, consumed)) => {
                    rx_tail.drain(..consumed);
                }
                Err(_) => break,
            }
        }
    }

    // Give any (erroneously) dispatched upstream dial a beat to land.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let count = hits.load(Ordering::SeqCst);

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), actor).await;
    forwarder.abort();

    (status, count)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h3_comma_in_authority_rejected_before_upstream() {
    let (status, hits) = run_case("victim.example,attacker.example").await;
    assert_eq!(
        status,
        Some(400),
        "H3: comma-in-:authority MUST be rejected with :status 400 by \
         the SAME authority predicate the H1/H2 path uses, BEFORE \
         upstream selection; got {status:?}"
    );
    assert_eq!(
        hits, 0,
        "H3: probe backend was reached — the H3 dispatch bypassed the \
         authority choke point (ROUND8-L7-16 / HAProxy BUG/MAJOR \
         comma class)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h3_whitespace_in_authority_rejected_before_upstream() {
    let (status, hits) = run_case("victim.example attacker").await;
    assert_eq!(
        status,
        Some(400),
        "H3: whitespace-in-:authority MUST be 400 before upstream; \
         got {status:?}"
    );
    assert_eq!(hits, 0, "H3: whitespace case reached the backend");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h3_valid_authority_passes_validator() {
    // A well-formed authority must NOT be rejected: the request
    // reaches the probe backend (proving the gate is value-
    // sanitisation only and does not over-reject the H3 path).
    let (status, hits) = run_case("example.test:8080").await;
    // The load-bearing proof: a well-formed `:authority` is NOT
    // rejected by the gate — the actor's picker dials the probe
    // backend (hits >= 1). This is the H3 mirror of the L7-09 H1/H2
    // "valid authority reaches upstream" assertion.
    assert!(
        hits >= 1,
        "H3: a valid :authority must pass the validator and reach the \
         probe backend; got hits={hits} status={status:?}"
    );
    // The upstream status is a secondary signal. Reaching the
    // (real) probe backend yields 200 when the minimal H3→H1
    // harness completes the read, or 502 when the minimal
    // single-shot harness's read races the backend's
    // Connection: close — EITHER value proves the request got PAST
    // the authority validator (a reject would be a 400, like the
    // comma/whitespace cases above). This is the exact same
    // "502 proves it passed the validator" inference the L7-09
    // H1/H2 proof (`lb-l7/tests/round8_authority_enforced.rs`) uses.
    assert!(
        matches!(status, Some(200) | Some(502)),
        "H3: a valid :authority must NOT be rejected — expected the \
         upstream's 200 (or 502 from the minimal harness read race), \
         NOT a 400 from the validator; got {status:?}"
    );
}
