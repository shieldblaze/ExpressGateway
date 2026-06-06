//! S36-A — H3 CONNECTION RECYCLING (cap → GOAWAY → drain → recycle) e2e.
//!
//! Proves the `max_requests_per_h3_connection` cap on the SHARED H3 actor
//! (`lb_quic::run_actor` / `poll_h3`, R12 single-source) actually fires —
//! the F-CAP-1 trap ("cap masked by backpressure") is avoided by asserting
//! the GOAWAY/recycle EVENTS, not merely an invariant:
//!
//!   * `cap_trips_goaway_and_recycles` — drive `> cap` requests on ONE real
//!     quiche::h3 client connection and assert (a) the client observes an
//!     `Event::GoAway`, (b) the recycle metric incremented
//!     (`h3_goaway_sent_total == 1`, `h3_connections_recycled_total == 1`),
//!     (c) the in-flight request at the GOAWAY boundary (the cap-th admitted
//!     request) COMPLETES with a real `:status 200` (NOT a RST), and (d)
//!     after the GOAWAY the real client refuses to open a new request on the
//!     recycling connection (`send_request` → `FrameUnexpected`, RFC 9114
//!     §5.2) — i.e. a fresh connection is required for subsequent work.
//!
//!   * `cap_zero_is_byte_identical` — the load-bearing R3 negative control:
//!     with `max_requests_per_h3_connection = 0` the actor NEVER sends a
//!     GOAWAY no matter how many requests ride one connection, and the
//!     connection stays open (no recycle). Byte-identical to the pre-S36 H3
//!     front.
//!
//! The client is a REAL `quiche::h3::Connection` (not the hand-rolled
//! testcodec) precisely because GOAWAY handling lives in quiche's h3 layer:
//! it surfaces `Event::GoAway` and enforces the "no new requests after a
//! peer GOAWAY" rule, so the test exercises the exact behaviour a
//! production H3 client would.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_observability::{MetricsRegistry, QuicH3RecycleMetrics};
use lb_quic::conn_actor::{ActorParams, InboundPacket, run_actor};
use lb_quic::{QuicListener, QuicListenerParams};

const MAX_UDP: usize = 65_535;
const TEST_SNI: &str = "expressgateway.test";
const H3_ALPN_PROTOS: &[&[u8]] = &[b"h3"];
const HANDSHAKE_BUDGET: Duration = Duration::from_secs(5);

// --------------------------------------------------------------------
// cert + config helpers (mirror round8_h3_authority_enforced.rs)
// --------------------------------------------------------------------

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
    let key_pem = generated.signing_key.serialize_pem();
    let dir = std::env::temp_dir();
    let subsec = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    static CERT_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = CERT_SEQ.fetch_add(1, Ordering::Relaxed);
    let nonce = std::process::id()
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(subsec);
    let cert_path = dir.join(format!("lb-quic-s36-recycle-cert-{nonce}-{seq}.pem"));
    let key_path = dir.join(format!("lb-quic-s36-recycle-key-{nonce}-{seq}.pem"));
    std::fs::write(&cert_path, cert_pem).expect("write cert");
    std::fs::write(&key_path, key_pem).expect("write key");
    (CertTempFile(cert_path), CertTempFile(key_path))
}

fn build_config(server: bool, cert_path: &str, key_path: &str) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).expect("Config::new");
    cfg.set_application_protos(H3_ALPN_PROTOS).expect("alpn");
    cfg.set_max_idle_timeout(10_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(10 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(1024 * 1024);
    cfg.set_initial_max_stream_data_uni(1024 * 1024);
    // Allow plenty of concurrent bidi streams so a > cap request burst is
    // not throttled by the transport flow-control window (the cap must be
    // what stops new requests, not stream-credit exhaustion — F-CAP-1).
    cfg.set_initial_max_streams_bidi(64);
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
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new().fill(&mut b).unwrap();
    // Mix the salt so two conns in one test never collide.
    b[0] ^= u8::try_from(salt & 0xff).unwrap_or(0);
    b
}

/// A real listening backend that returns a minimal well-formed H1 200 for
/// every accepted connection and counts the hits.
async fn spawn_probe_backend() -> (SocketAddr, Arc<AtomicU32>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicU32::new(0));
    let c = Arc::clone(&count);
    tokio::spawn(async move {
        loop {
            if let Ok((sock, _)) = listener.accept().await {
                c.fetch_add(1, Ordering::SeqCst);
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

/// A probe backend that delays `delay` before sending the 200 — so a
/// response that is still in flight when the cap GOAWAY fires lets us prove
/// the actor DRAINS it (completes the 200) rather than force-closing (RST).
async fn spawn_slow_probe_backend(delay: Duration) -> (SocketAddr, Arc<AtomicU32>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicU32::new(0));
    let c = Arc::clone(&count);
    tokio::spawn(async move {
        loop {
            if let Ok((sock, _)) = listener.accept().await {
                c.fetch_add(1, Ordering::SeqCst);
                let mut s = sock;
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = s
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        )
                        .await;
                    let _ = s.flush().await;
                });
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

fn req_headers() -> Vec<quiche::h3::Header> {
    vec![
        quiche::h3::Header::new(b":method", b"GET"),
        quiche::h3::Header::new(b":scheme", b"https"),
        quiche::h3::Header::new(b":path", b"/p"),
        quiche::h3::Header::new(b":authority", TEST_SNI.as_bytes()),
    ]
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

/// Per-stream client-side response accounting.
#[derive(Default, Clone)]
struct StreamOutcome {
    status: Option<u16>,
    body_len: usize,
    finished: bool,
    reset_code: Option<u64>,
}

/// What one `cap_trips_goaway_and_recycles`-style run observed.
struct RunResult {
    /// Per-stream outcomes keyed by client stream id.
    outcomes: std::collections::HashMap<u64, StreamOutcome>,
    /// The GOAWAY id the client observed (None ⇒ no GOAWAY seen).
    goaway_id: Option<u64>,
    /// `true` once the client's `send_request` was refused with
    /// `FrameUnexpected` (the post-GOAWAY "open a new connection" rule).
    send_refused_after_goaway: bool,
    /// Recycle metric snapshot AFTER the actor task joined.
    goaway_sent_total: u64,
    connections_recycled_total: u64,
    /// Probe-backend hit count (admitted requests that reached upstream).
    backend_hits: u32,
}

/// Drive `n_requests` GET requests over ONE real quiche::h3 client
/// connection against a `run_actor` server configured with
/// `max_requests_per_h3_connection = cap`. Returns everything the
/// assertions need.
async fn drive(cap: u32, n_requests: usize) -> RunResult {
    drive_with_backend_delay(cap, n_requests, Duration::ZERO).await
}

async fn drive_with_backend_delay(
    cap: u32,
    n_requests: usize,
    backend_delay: Duration,
) -> RunResult {
    let (cert_file, key_file) = write_test_cert();
    let cert_path = cert_file.0.to_str().unwrap().to_string();
    let key_path = key_file.0.to_str().unwrap().to_string();

    let (backend, hits) = if backend_delay.is_zero() {
        spawn_probe_backend().await
    } else {
        spawn_slow_probe_backend(backend_delay).await
    };

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

    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];

    // Handshake pump.
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
            Duration::from_millis(20),
        )
        .await;
        try_recv_one(
            &mut client_conn,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(20),
        )
        .await;
    }

    // Build the client h3 layer on the established transport.
    let h3_cfg = quiche::h3::Config::new().unwrap();
    let mut client_h3 = quiche::h3::Connection::with_transport(&mut client_conn, &h3_cfg).unwrap();

    // Hand the established server conn to the REAL actor; a forwarder task
    // drains the server UDP socket into the actor's inbound channel (the
    // router is simulated, the actor itself is real).
    let (tx, rx) = mpsc::channel::<InboundPacket>(256);
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

    let registry = MetricsRegistry::new();
    let metrics = QuicH3RecycleMetrics::register(&registry).expect("register recycle metrics");

    let params = ActorParams {
        conn: server_conn,
        socket: Arc::clone(&server_socket),
        inbound: rx,
        cancel: cancel.clone(),
        pool: pool(),
        backends: Arc::new(vec![backend]),
        h3_backend: None,
        h2_backend: None,
        raw_quic_backend: None,
        quic_modeb_metrics: None,
        ws_enabled: false,
        ws_relay_launcher: None,
        max_requests_per_h3_connection: cap,
        h3_recycle_metrics: Some(metrics.clone()),
    };
    let actor = tokio::spawn(run_actor(params));

    let mut outcomes: std::collections::HashMap<u64, StreamOutcome> =
        std::collections::HashMap::new();
    let mut goaway_id: Option<u64> = None;
    let mut send_refused_after_goaway = false;
    let mut sent = 0usize;
    let mut body_buf = [0u8; 4096];

    // Issue requests + pump until all sent requests reach a terminal state
    // (Finished or Reset) and a GOAWAY has been observed (when a cap is in
    // play), or a generous budget expires.
    let run_deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if tokio::time::Instant::now() > run_deadline {
            break;
        }

        // Open one more request per iteration until we've sent n_requests,
        // unless the client has seen a GOAWAY (then `send_request` is a
        // protocol error — capture that and stop opening new ones).
        if sent < n_requests {
            match client_h3.send_request(&mut client_conn, &req_headers(), true) {
                Ok(sid) => {
                    outcomes.entry(sid).or_default();
                    sent += 1;
                }
                Err(quiche::h3::Error::FrameUnexpected) if goaway_id.is_some() => {
                    // RFC 9114 §5.2: a client that received a GOAWAY MUST NOT
                    // open new requests on that connection. quiche enforces
                    // this — the proof that the client must reconnect.
                    send_refused_after_goaway = true;
                    sent = n_requests; // stop trying to open more
                }
                Err(quiche::h3::Error::StreamBlocked) | Err(quiche::h3::Error::Done) => {
                    // transient — retry next iteration
                }
                Err(e) => panic!("unexpected send_request error: {e:?}"),
            }
        }

        flush(&mut client_conn, &client_socket, &mut out).await;
        try_recv_one(
            &mut client_conn,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(15),
        )
        .await;

        // Drain client h3 events.
        loop {
            match client_h3.poll(&mut client_conn) {
                Ok((sid, quiche::h3::Event::Headers { list, .. })) => {
                    use quiche::h3::NameValue;
                    let o = outcomes.entry(sid).or_default();
                    for h in &list {
                        if h.name() == b":status" {
                            o.status = std::str::from_utf8(h.value())
                                .ok()
                                .and_then(|s| s.parse::<u16>().ok());
                        }
                    }
                }
                Ok((sid, quiche::h3::Event::Data)) => {
                    while let Ok(n) = client_h3.recv_body(&mut client_conn, sid, &mut body_buf) {
                        outcomes.entry(sid).or_default().body_len += n;
                        if n == 0 {
                            break;
                        }
                    }
                }
                Ok((sid, quiche::h3::Event::Finished)) => {
                    outcomes.entry(sid).or_default().finished = true;
                }
                Ok((sid, quiche::h3::Event::Reset(code))) => {
                    outcomes.entry(sid).or_default().reset_code = Some(code);
                }
                Ok((id, quiche::h3::Event::GoAway)) => {
                    goaway_id = Some(id);
                }
                Ok((_sid, _other)) => {}
                Err(quiche::h3::Error::Done) => break,
                Err(e) => {
                    // The connection may go away as the server recycles it;
                    // that is expected once a GOAWAY was seen.
                    let _ = e;
                    break;
                }
            }
        }

        // Termination: every opened request reached a terminal state
        // (Finished or Reset). For a capped run we also require the GOAWAY
        // to have been observed; for cap==0 no GOAWAY will ever come.
        let all_terminal = !outcomes.is_empty()
            && outcomes
                .values()
                .all(|o| o.finished || o.reset_code.is_some());
        let goaway_satisfied = cap == 0 || goaway_id.is_some();
        if sent >= n_requests && all_terminal && goaway_satisfied {
            break;
        }

        if client_conn.is_closed() {
            break;
        }
    }

    // Post-GOAWAY "must reconnect" proof, timing-independent: if the client
    // saw a GOAWAY, an explicit fresh `send_request` on THIS connection must
    // be refused with `FrameUnexpected` (RFC 9114 §5.2). (During the burst
    // above the client may have already queued every request before it
    // processed the GOAWAY, so this explicit probe is the load-bearing
    // assertion rather than the opportunistic in-loop capture.)
    if goaway_id.is_some() && !send_refused_after_goaway {
        match client_h3.send_request(&mut client_conn, &req_headers(), true) {
            Err(quiche::h3::Error::FrameUnexpected) => {
                send_refused_after_goaway = true;
            }
            other => panic!(
                "S36-A: after a GOAWAY the client's send_request MUST return \
                 FrameUnexpected (no new requests on a recycling connection); \
                 got {other:?}"
            ),
        }
    }

    // Let the server actor observe the drained, idle connection and run its
    // drain-then-recycle close; pump a little so the CONNECTION_CLOSE lands.
    let settle = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < settle && !client_conn.is_closed() {
        flush(&mut client_conn, &client_socket, &mut out).await;
        try_recv_one(
            &mut client_conn,
            &client_socket,
            client_local,
            &mut in_buf,
            Duration::from_millis(30),
        )
        .await;
        // Drain any late events (e.g. a trailing GoAway / Reset).
        loop {
            match client_h3.poll(&mut client_conn) {
                Ok((id, quiche::h3::Event::GoAway)) => goaway_id = Some(id),
                Ok((sid, quiche::h3::Event::Reset(code))) => {
                    outcomes.entry(sid).or_default().reset_code = Some(code);
                }
                Ok((sid, quiche::h3::Event::Finished)) => {
                    outcomes.entry(sid).or_default().finished = true;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    let backend_hits = hits.load(Ordering::SeqCst);

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(3), actor).await;
    forwarder.abort();

    RunResult {
        outcomes,
        goaway_id,
        send_refused_after_goaway,
        goaway_sent_total: metrics.goaway_sent_total.get(),
        connections_recycled_total: metrics.connections_recycled_total.get(),
        backend_hits,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cap_trips_goaway_and_recycles() {
    // cap = 3, attempt 10 requests on ONE connection. The 3rd admitted
    // request trips the cap → GOAWAY; once the client processes it (after a
    // sub-ms loopback round-trip, long before all 10 are opened) it refuses
    // to open the rest, and any racing in-flight opens past the GOAWAY id
    // are rejected by the server (H3_REQUEST_REJECTED).
    let cap = 3u32;
    let res = drive(cap, 10).await;

    // (a) the client observed a GOAWAY.
    assert!(
        res.goaway_id.is_some(),
        "S36-A: the client MUST observe an H3 GOAWAY once the connection \
         hit max_requests_per_h3_connection={cap}; outcomes={:?}",
        res.outcomes.len()
    );

    // (b) the recycle metric incremented — the branch is PROVABLY TAKEN
    // (not just an invariant; avoids the F-CAP-1 backpressure-masked trap).
    assert_eq!(
        res.goaway_sent_total, 1,
        "S36-A: exactly one GOAWAY must be recorded at the cap (got {})",
        res.goaway_sent_total
    );
    assert_eq!(
        res.connections_recycled_total, 1,
        "S36-A: the connection must record exactly one recycle after the \
         drain-then-close (got {})",
        res.connections_recycled_total
    );

    // (c) at least `cap` admitted requests completed with a real 200 (the
    // boundary request — the cap-th — drains correctly, it is NOT RST). The
    // probe backend returns 200; a recycle that truncated an in-flight
    // request would surface a Reset or a missing 200 here.
    let completed_200 = res
        .outcomes
        .values()
        .filter(|o| o.status == Some(200) && o.finished && o.reset_code.is_none())
        .count();
    assert!(
        completed_200 >= cap as usize,
        "S36-A: at least cap={cap} admitted requests must COMPLETE with a \
         clean :status 200 (the GOAWAY boundary request drains, not RST); \
         got {completed_200} clean 200s. outcomes: {:?}",
        res.outcomes
            .values()
            .map(|o| (o.status, o.finished, o.reset_code))
            .collect::<Vec<_>>()
    );

    // Every completed request that got a status got exactly 200 (no
    // per-request correctness regression on the admitted set).
    for (sid, o) in &res.outcomes {
        if let Some(st) = o.status {
            assert_eq!(
                st, 200,
                "S36-A: admitted request on stream {sid} must be 200, got {st}"
            );
        }
    }

    // (d) after the GOAWAY the real client refused to open new requests on
    // the recycling connection (must reconnect) — RFC 9114 §5.2.
    assert!(
        res.send_refused_after_goaway,
        "S36-A: after the GOAWAY the client MUST refuse new requests on the \
         recycling connection (FrameUnexpected) — a fresh connection is \
         required for subsequent work"
    );

    assert!(
        res.backend_hits >= cap,
        "S36-A: at least cap={cap} admitted requests must have reached the \
         upstream backend (got {})",
        res.backend_hits
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_flight_at_boundary_drains_not_rst() {
    // The pointed drain-correctness proof (R3): a SLOW backend holds every
    // response open ~400 ms. With cap=2, the GOAWAY fires (control stream)
    // while the admitted responses are STILL IN FLIGHT. The actor MUST drain
    // them — every admitted request completes with a clean 200 and NONE is
    // reset — and only THEN recycle (a force-close would surface a Reset /
    // missing 200 on the boundary request).
    let cap = 2u32;
    let res = drive_with_backend_delay(cap, 8, Duration::from_millis(400)).await;

    assert!(
        res.goaway_id.is_some(),
        "S36-A: GOAWAY must fire even while responses are in flight"
    );
    assert_eq!(
        res.connections_recycled_total, 1,
        "S36-A: the connection must recycle exactly once AFTER the in-flight \
         responses drained (got {})",
        res.connections_recycled_total
    );

    // No admitted request was reset, and at least `cap` completed with 200
    // (the in-flight boundary responses drained, not truncated/RST).
    let any_admitted_reset = res.outcomes.values().any(|o| {
        // An admitted request is one that got a status (reached upstream);
        // a post-GOAWAY rejected stream has H3_REQUEST_REJECTED and never a
        // status. Such a rejected stream IS allowed to carry a reset_code.
        o.status.is_some() && o.reset_code.is_some()
    });
    assert!(
        !any_admitted_reset,
        "S36-A: NO admitted (status-bearing) request may be reset — an \
         in-flight response at the GOAWAY boundary must drain, not RST. \
         outcomes: {:?}",
        res.outcomes
            .values()
            .map(|o| (o.status, o.finished, o.reset_code))
            .collect::<Vec<_>>()
    );
    let completed_200 = res
        .outcomes
        .values()
        .filter(|o| o.status == Some(200) && o.finished && o.reset_code.is_none())
        .count();
    assert!(
        completed_200 >= cap as usize,
        "S36-A: at least cap={cap} in-flight responses must drain to a clean \
         200 across the GOAWAY; got {completed_200}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cap_zero_is_byte_identical() {
    // R3 negative control: cap = 0 ⇒ NO GOAWAY, NO recycle, regardless of
    // how many requests ride one connection.
    let res = drive(0, 6).await;

    assert!(
        res.goaway_id.is_none(),
        "R3: with max_requests_per_h3_connection=0 the actor MUST NEVER \
         send a GOAWAY (byte-identical pre-S36 H3 front); client saw \
         goaway_id={:?}",
        res.goaway_id
    );
    assert_eq!(
        res.goaway_sent_total, 0,
        "R3: cap=0 must record zero GOAWAYs (got {})",
        res.goaway_sent_total
    );
    assert_eq!(
        res.connections_recycled_total, 0,
        "R3: cap=0 must record zero recycles (got {})",
        res.connections_recycled_total
    );
    assert!(
        !res.send_refused_after_goaway,
        "R3: cap=0 ⇒ the client is never refused (no GOAWAY ever sent)"
    );

    // All six requests should have completed cleanly with 200 — the
    // disabled path forwards every request exactly as before.
    let completed_200 = res
        .outcomes
        .values()
        .filter(|o| o.status == Some(200) && o.finished)
        .count();
    assert!(
        completed_200 >= 6,
        "R3: cap=0 must serve every request (>=6 clean 200s); got \
         {completed_200}. outcomes: {:?}",
        res.outcomes
            .values()
            .map(|o| (o.status, o.finished, o.reset_code))
            .collect::<Vec<_>>()
    );
}

// --------------------------------------------------------------------
// FRESH-CONNECTION-SERVES proof — through the REAL QuicListener (router +
// per-CID dispatch), so a SECOND client connection spawns a SECOND actor.
// This also exercises the full binary wiring path the unit tests bypass:
// QuicListenerParams::with_h3_request_cap → RouterParams → ActorParams.
// --------------------------------------------------------------------

/// Drive ONE client connection against an already-bound server addr (the
/// real listener owns its receive socket). Sends up to `n_requests` GET
/// requests, returns (clean-200 count, saw_goaway).
async fn drive_one_client_against(
    server_addr: SocketAddr,
    cert_path: &str,
    n_requests: usize,
    salt: u32,
) -> (usize, bool) {
    let client_socket = Arc::new(
        UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap(),
    );
    client_socket.connect(server_addr).await.unwrap();
    let client_local = client_socket.local_addr().unwrap();

    let mut client_cfg = build_config(false, cert_path, "");
    let c_scid_bytes = scid_bytes(salt);
    let c_scid = quiche::ConnectionId::from_ref(&c_scid_bytes);
    let mut client_conn = quiche::connect(
        Some(TEST_SNI),
        &c_scid,
        client_local,
        server_addr,
        &mut client_cfg,
    )
    .unwrap();

    let mut out = vec![0u8; MAX_UDP];
    let mut in_buf = vec![0u8; MAX_UDP];

    // Handshake.
    let deadline = tokio::time::Instant::now() + HANDSHAKE_BUDGET;
    while !client_conn.is_established() {
        if tokio::time::Instant::now() > deadline {
            panic!("client handshake did not establish");
        }
        loop {
            match client_conn.send(&mut out) {
                Ok((n, _)) => {
                    client_socket.send(out.get(..n).unwrap_or(&[])).await.ok();
                }
                Err(quiche::Error::Done) => break,
                Err(e) => panic!("client send: {e:?}"),
            }
        }
        if let Ok(Ok((n, from))) = tokio::time::timeout(
            Duration::from_millis(30),
            client_socket.recv_from(&mut in_buf),
        )
        .await
        {
            let info = quiche::RecvInfo {
                from,
                to: client_local,
            };
            let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
            let _ = client_conn.recv(slice, info);
        }
    }

    let h3_cfg = quiche::h3::Config::new().unwrap();
    let mut h3 = quiche::h3::Connection::with_transport(&mut client_conn, &h3_cfg).unwrap();

    let mut outcomes: std::collections::HashMap<u64, StreamOutcome> =
        std::collections::HashMap::new();
    let mut saw_goaway = false;
    let mut sent = 0usize;
    let mut body_buf = [0u8; 4096];

    let run_deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if tokio::time::Instant::now() > run_deadline {
            break;
        }
        if sent < n_requests && !saw_goaway {
            match h3.send_request(&mut client_conn, &req_headers(), true) {
                Ok(sid) => {
                    outcomes.entry(sid).or_default();
                    sent += 1;
                }
                Err(quiche::h3::Error::StreamBlocked) | Err(quiche::h3::Error::Done) => {}
                Err(quiche::h3::Error::FrameUnexpected) => saw_goaway = true,
                Err(e) => panic!("send_request: {e:?}"),
            }
        }
        loop {
            match client_conn.send(&mut out) {
                Ok((n, _)) => {
                    client_socket.send(out.get(..n).unwrap_or(&[])).await.ok();
                }
                Err(quiche::Error::Done) => break,
                Err(_) => break,
            }
        }
        if let Ok(Ok((n, from))) = tokio::time::timeout(
            Duration::from_millis(15),
            client_socket.recv_from(&mut in_buf),
        )
        .await
        {
            let info = quiche::RecvInfo {
                from,
                to: client_local,
            };
            let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
            let _ = client_conn.recv(slice, info);
        }
        loop {
            match h3.poll(&mut client_conn) {
                Ok((sid, quiche::h3::Event::Headers { list, .. })) => {
                    use quiche::h3::NameValue;
                    let o = outcomes.entry(sid).or_default();
                    for h in &list {
                        if h.name() == b":status" {
                            o.status = std::str::from_utf8(h.value())
                                .ok()
                                .and_then(|s| s.parse::<u16>().ok());
                        }
                    }
                }
                Ok((sid, quiche::h3::Event::Data)) => {
                    while let Ok(n) = h3.recv_body(&mut client_conn, sid, &mut body_buf) {
                        outcomes.entry(sid).or_default().body_len += n;
                        if n == 0 {
                            break;
                        }
                    }
                }
                Ok((sid, quiche::h3::Event::Finished)) => {
                    outcomes.entry(sid).or_default().finished = true;
                }
                Ok((sid, quiche::h3::Event::Reset(code))) => {
                    outcomes.entry(sid).or_default().reset_code = Some(code);
                }
                Ok((_id, quiche::h3::Event::GoAway)) => saw_goaway = true,
                Ok(_) => {}
                Err(quiche::h3::Error::Done) => break,
                Err(_) => break,
            }
        }
        let all_terminal = !outcomes.is_empty()
            && outcomes
                .values()
                .all(|o| o.finished || o.reset_code.is_some());
        if (sent >= n_requests || saw_goaway) && all_terminal {
            break;
        }
        if client_conn.is_closed() {
            break;
        }
    }

    let clean_200 = outcomes
        .values()
        .filter(|o| o.status == Some(200) && o.finished && o.reset_code.is_none())
        .count();
    (clean_200, saw_goaway)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_connection_serves_after_recycle() {
    let (cert_file, key_file) = write_test_cert();
    let cert_path = cert_file.0.to_str().unwrap().to_string();
    let key_path = key_file.0.to_str().unwrap().to_string();

    let (backend, _hits) = spawn_probe_backend().await;

    // A retry-secret path the listener generates if missing.
    let retry_path = std::env::temp_dir().join(format!(
        "lb-quic-s36-recycle-retry-{}-{}.key",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _retry_cleanup = CertTempFile(retry_path.clone());

    let cap = 2u32;
    let registry = MetricsRegistry::new();
    let metrics = QuicH3RecycleMetrics::register(&registry).expect("register recycle metrics");

    let params = QuicListenerParams::new(
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
        PathBuf::from(&cert_path),
        PathBuf::from(&key_path),
        retry_path,
    )
    .with_backends(vec![backend], pool())
    .with_h3_request_cap(cap, Some(metrics.clone()));

    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown.clone())
        .await
        .expect("listener must bind");
    let server_addr = listener.local_addr();

    // Connection #1: drive > cap requests → it recycles (GOAWAY).
    let (c1_200, c1_goaway) = drive_one_client_against(server_addr, &cert_path, 6, 11).await;
    assert!(
        c1_goaway,
        "S36-A: connection #1 (> cap) must be recycled — the client sees a GOAWAY"
    );
    assert!(
        c1_200 >= cap as usize,
        "S36-A: connection #1 must still serve its admitted requests cleanly \
         (>= cap={cap} 200s); got {c1_200}"
    );

    // Give the listener a beat to finish recycling connection #1.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Connection #2: a FRESH connection (new SCID ⇒ new actor) must serve
    // its requests normally — the recycle of #1 did not poison the listener.
    let (c2_200, _c2_goaway) = drive_one_client_against(server_addr, &cert_path, 2, 22).await;
    assert_eq!(
        c2_200, 2,
        "S36-A: a FRESH connection after a recycle MUST serve subsequent \
         requests (got {c2_200} clean 200s on connection #2)"
    );

    // The recycle metric saw at least connection #1's recycle.
    assert!(
        metrics.goaway_sent_total.get() >= 1,
        "S36-A: at least one GOAWAY recorded across the two connections (got {})",
        metrics.goaway_sent_total.get()
    );
    assert!(
        metrics.connections_recycled_total.get() >= 1,
        "S36-A: at least one connection recycled (got {})",
        metrics.connections_recycled_total.get()
    );

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(3), listener.shutdown()).await;
}
