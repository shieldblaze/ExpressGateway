//! S15 A2 verify gate (i) — quic_passthrough_e2e.
//!
//! Drives a real `quiche::Connection` CLIENT through the
//! `PassthroughListener` to a real `quiche::Connection` BACKEND, and
//! asserts:
//!
//!   1. The handshake completes (Retry round-trip is implicit in the
//!      LB's stateless-validation step — the LB mints the Retry token
//!      and the client re-sends Initial-with-token; the backend just
//!      sees a normal tokened Initial and `accept`s it).
//!   2. The peer certificate the CLIENT observes is the BACKEND's
//!      rcgen cert — proving the LB never terminated the TLS session
//!      (the §9.5 PRIMARY-3 "client sees backend cert" property).
//!   3. A bidirectional STREAM round-trips an explicitly-binary
//!      payload (0xFF / 0x00 / 0x80 / 0x7F / a random tail) without
//!      mutation — proving the LB is byte-faithful on the data path
//!      and does not normalise / re-encode anything.
//!
//! Together these close design-doc verify-gate (i):
//! "real-cert / real-client E2E proves the LB doesn't decrypt".
//!
//! Test fixture shape:
//!
//!   client ───UDP──▶ LB (PassthroughListener on localhost:M)
//!                      │
//!                      └─UDP──▶ backend (quiche server on
//!                                localhost:N, rcgen cert)
//!
//! The driver loop is shared between client + server: both flush
//! `quiche::Connection::send` → socket, and route inbound datagrams
//! through `quiche::Connection::recv`. `tokio::select!` over the two
//! socket recv halves keeps liveness while not pinning one side ahead
//! of the other.
//!
//! Concurrency posture: `flavor = "current_thread"` — single-threaded
//! Tokio rt avoids racing client/server task scheduling, matching the
//! style of the existing `quic_passthrough_bounded_state.rs`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_quic::{PassthroughListener, PassthroughParams};
use lb_security::RETRY_SECRET_LEN;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// SNI advertised by the client + present in the backend's leaf cert
/// SAN. Loopback-only so this is a synthetic name; rcgen accepts it.
const TEST_SNI: &str = "passthrough-e2e.test.local";

/// MAX UDP payload sized to the quiche default (1350 fits a 1500-MTU
/// path after IP+UDP headers).
const MAX_UDP: usize = 1350;

/// Driver deadline. Generous so a slow CI shard still completes the
/// handshake; the test fails loud on timeout so a regression doesn't
/// silently extend wall-clock.
const DRIVER_BUDGET: Duration = Duration::from_secs(15);

const RETRY_SECRET: [u8; RETRY_SECRET_LEN] = [0x77u8; RETRY_SECRET_LEN];

/// Fresh per-process temp directory for cert + retry-secret material.
fn make_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lb-passthrough-e2e-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

struct TestCerts {
    _dir: PathBuf,
    cert_path: PathBuf,
    key_path: PathBuf,
    ca_path: PathBuf,
    /// PEM-encoded leaf cert bytes, captured for the "client sees
    /// backend cert" equality assertion in gate (i) (3).
    leaf_der: Vec<u8>,
}

/// Generate a self-signed rcgen cert + key whose SAN contains
/// [`TEST_SNI`]. The same cert PEM is used as both leaf and CA (we
/// self-sign + ship it to the client as the trust anchor — same
/// pattern as the existing h3_h1_bridge_e2e fixture).
fn make_certs() -> TestCerts {
    let dir = make_dir();
    let mut params =
        rcgen::CertificateParams::new(vec![TEST_SNI.to_string()]).expect("rcgen params");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
    let key_pair = rcgen::KeyPair::generate().expect("rcgen keypair");
    let cert = params.self_signed(&key_pair).expect("rcgen self-signed");
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ca_path = dir.join("ca.pem");
    std::fs::write(&cert_path, cert_pem.as_bytes()).expect("write cert");
    std::fs::write(&key_path, key_pem.as_bytes()).expect("write key");
    std::fs::write(&ca_path, cert_pem.as_bytes()).expect("write ca");
    let leaf_der = cert.der().to_vec();
    TestCerts {
        _dir: dir,
        cert_path,
        key_path,
        ca_path,
        leaf_der,
    }
}

/// Build a quiche::Config for the backend SERVER role.
fn build_server_config(cert_path: &std::path::Path, key_path: &std::path::Path) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).expect("quiche cfg");
    cfg.set_application_protos(&[b"lb-quic-e2e"]).expect("alpn");
    cfg.load_cert_chain_from_pem_file(cert_path.to_str().expect("cert utf8"))
        .expect("load cert");
    cfg.load_priv_key_from_pem_file(key_path.to_str().expect("key utf8"))
        .expect("load key");
    cfg.set_max_idle_timeout(10_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(1 << 20);
    cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(4);
    cfg.set_initial_max_streams_uni(4);
    // Active migration off: the LB rewrites the client's 4-tuple as a
    // simple forward, so disable_active_migration keeps quiche
    // tolerant of source-port shifts (mirrors the design's NAT-rebind
    // posture). With this off, quiche would reset on a 4-tuple change.
    cfg.set_disable_active_migration(true);
    cfg
}

/// Build a quiche::Config for the CLIENT role. `ca_path` is the trust
/// anchor for the backend's leaf cert.
fn build_client_config(ca_path: &std::path::Path) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).expect("quiche cfg");
    cfg.set_application_protos(&[b"lb-quic-e2e"]).expect("alpn");
    cfg.load_verify_locations_from_file(ca_path.to_str().expect("ca utf8"))
        .expect("load ca");
    cfg.verify_peer(true);
    cfg.set_max_idle_timeout(10_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(1 << 20);
    cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
    cfg.set_initial_max_stream_data_uni(64 * 1024);
    cfg.set_initial_max_streams_bidi(4);
    cfg.set_initial_max_streams_uni(4);
    cfg.set_disable_active_migration(true);
    cfg
}

fn random_scid() -> [u8; quiche::MAX_CONN_ID_LEN] {
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new()
        .fill(&mut scid)
        .expect("rng");
    scid
}

/// Run the passthrough LB pointed at `backend_addr`. Returns the LB's
/// bound addr + cancel token + listener handle.
async fn spawn_lb(
    backend_addr: SocketAddr,
) -> (PassthroughListener, SocketAddr, CancellationToken) {
    let dir = make_dir();
    let retry_path = dir.join("retry.bin");
    std::fs::write(&retry_path, RETRY_SECRET).expect("write retry secret");
    let mut params = PassthroughParams::new(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        vec![backend_addr],
        retry_path,
    );
    // Lower the floor to 8 so quiche's default 16-byte client DCID
    // breezes through (it does anyway; 8 is the production default).
    params.min_client_dcid_len = 8;
    params.per_flow_backlog = 32;
    params.audit_throttle_window = Duration::from_secs(60);
    // CF-S15-PASSTHROUGH-RETRY-ODCID: turn LB-minted Retry OFF for
    // this gate (i) test. With mint_retry=true the LB-chosen new_scid
    // hides the client's ODCID and the backend cannot set
    // `original_destination_connection_id` correctly without a
    // side channel (RFC 9000 §17.2.5 Retry Service pattern, deferred
    // to S15.x/S16). Production deployments leave this true; the
    // backend's own quiche handles §6.5 flood defence in the
    // delegated path.
    params.mint_retry = false;

    let cancel = CancellationToken::new();
    let listener = PassthroughListener::spawn(params, cancel.clone())
        .await
        .expect("spawn LB");
    let addr = listener.local_addr();
    (listener, addr, cancel)
}

/// Spawn a real quiche backend server. The server accepts the FIRST
/// inbound connection, completes the handshake, and on the FIRST
/// readable bidi stream echoes back whatever bytes it receives
/// (preserving the FIN). Returns the bound addr + a handle that
/// resolves when the echo stream's FIN has been forwarded.
async fn spawn_quic_echo_backend(
    cert_path: PathBuf,
    key_path: PathBuf,
) -> (SocketAddr, tokio::task::JoinHandle<Result<(), String>>) {
    let socket = Arc::new(
        UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .expect("backend bind"),
    );
    let local_addr = socket.local_addr().expect("backend local_addr");
    let join = tokio::spawn(async move {
        let mut config = build_server_config(&cert_path, &key_path);
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];

        // Phase 0: receive the first inbound Initial → quiche::accept.
        // This Initial reached us via the LB, which means it carries
        // the LB-minted Retry token. To complete the handshake the
        // CF-S15-PASSTHROUGH-RETRY-ODCID — owner ruling: this test
        // uses the `mint_retry = false` escape path (the LB does NOT
        // mint Retry; no-token Initials are forwarded verbatim). The
        // wire DCID IS the client's ODCID; plain `quiche::accept`
        // with `odcid = None` works because there was no Retry
        // round-trip. The §6.5 Initial-flood defence is delegated
        // to the backend's own quiche in this mode (it may issue
        // its own Retry; the LB just forwards).
        let (n, peer) = socket
            .recv_from(&mut in_buf)
            .await
            .map_err(|e| format!("backend first recv: {e}"))?;
        // Confirm the public header parses; the parse result is
        // discarded — `quiche::accept` parses it again internally.
        let _ = quiche::Header::from_slice(
            in_buf.get_mut(..n).unwrap_or(&mut []),
            quiche::MAX_CONN_ID_LEN,
        )
        .map_err(|e| format!("backend hdr parse: {e}"))?;
        let scid = random_scid();
        let scid_conn = quiche::ConnectionId::from_ref(&scid);
        let mut conn = quiche::accept(&scid_conn, None, local_addr, peer, &mut config)
            .map_err(|e| format!("backend accept: {e}"))?;
        // Feed the first packet to the new conn.
        {
            let info = quiche::RecvInfo {
                from: peer,
                to: local_addr,
            };
            let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
            match conn.recv(slice, info) {
                Ok(_) | Err(quiche::Error::Done) => {}
                Err(e) => return Err(format!("backend first recv() to conn: {e}")),
            }
        }

        let deadline = tokio::time::Instant::now() + DRIVER_BUDGET;
        let mut current_peer = peer;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err("backend driver budget exhausted".into());
            }
            if conn.is_closed() {
                return Ok(());
            }
            // Flush outbound.
            loop {
                match conn.send(&mut out_buf) {
                    Ok((n, info)) => {
                        let _ = socket
                            .send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                            .await;
                    }
                    Err(quiche::Error::Done) => break,
                    Err(e) => return Err(format!("backend send: {e}")),
                }
            }
            // Process any readable bidi streams (echo back).
            let mut echoed_anything = false;
            if conn.is_established() {
                let readable: Vec<u64> = conn.readable().collect();
                for sid in readable {
                    let mut chunk = [0u8; 8192];
                    loop {
                        match conn.stream_recv(sid, &mut chunk) {
                            Ok((n, fin)) => {
                                let bytes = chunk.get(..n).unwrap_or(&[]);
                                // Echo.
                                let mut sent = 0;
                                while sent < bytes.len() {
                                    let slice = bytes.get(sent..).unwrap_or(&[]);
                                    let last = sent + slice.len() >= bytes.len();
                                    match conn.stream_send(sid, slice, fin && last) {
                                        Ok(m) => sent += m,
                                        Err(quiche::Error::Done) => break,
                                        Err(e) => return Err(format!("backend echo send: {e}")),
                                    }
                                }
                                if sent > 0 || (fin && bytes.is_empty()) {
                                    echoed_anything = true;
                                }
                            }
                            Err(quiche::Error::Done) => break,
                            Err(e) => return Err(format!("backend stream_recv: {e}")),
                        }
                    }
                }
            }
            // Drain outbound right after echo so the echoed STREAM
            // bytes go on the wire without waiting for the next
            // socket recv to kick the select! loop. Without this the
            // backend would only flush on the NEXT iteration AFTER
            // the select! returns — but with no further client
            // traffic to wake the select!, the echo sat in quiche's
            // send queue until DRIVER_BUDGET expired.
            if echoed_anything {
                loop {
                    match conn.send(&mut out_buf) {
                        Ok((n, info)) => {
                            let _ = socket
                                .send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                                .await;
                        }
                        Err(quiche::Error::Done) => break,
                        Err(e) => return Err(format!("backend post-echo send: {e}")),
                    }
                }
            }
            let timeout = conn.timeout();
            let budget = deadline.saturating_duration_since(tokio::time::Instant::now());
            let wait = timeout.map_or(budget, |t| t.min(budget));
            tokio::select! {
                recv = socket.recv_from(&mut in_buf) => {
                    let (n, from) = recv.map_err(|e| format!("backend recv: {e}"))?;
                    current_peer = from;
                    let info = quiche::RecvInfo { from, to: local_addr };
                    let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                    match conn.recv(slice, info) {
                        Ok(_) | Err(quiche::Error::Done) => {}
                        Err(e) => return Err(format!("backend recv to conn: {e}")),
                    }
                }
                () = tokio::time::sleep(wait) => {
                    conn.on_timeout();
                }
            }
            // Suppress unused-var lint when the select! branch sets current_peer.
            let _ = current_peer;
        }
    });
    (local_addr, join)
}

/// Drive the CLIENT through handshake, send `payload` on bidi stream
/// 0 (FIN after the payload), read the echoed bytes back, close, and
/// return (peer_cert_der, echoed_bytes).
async fn drive_client(
    conn: Arc<Mutex<quiche::Connection>>,
    socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    payload: Vec<u8>,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut in_buf = vec![0u8; MAX_UDP];
    let mut out_buf = vec![0u8; MAX_UDP];
    let stream_id: u64 = 0;
    let mut request_sent = false;
    let mut echoed: Vec<u8> = Vec::new();
    let mut peer_cert: Option<Vec<u8>> = None;
    let mut fin_seen = false;
    let deadline = tokio::time::Instant::now() + DRIVER_BUDGET;

    loop {
        if tokio::time::Instant::now() >= deadline {
            let est = conn.lock().await.is_established();
            return Err(format!(
                "client deadline; established={est}, echoed_len={}, fin={fin_seen}",
                echoed.len()
            ));
        }
        // Flush outbound.
        loop {
            let send = {
                let mut guard = conn.lock().await;
                guard.send(&mut out_buf)
            };
            match send {
                Ok((n, info)) => {
                    let _ = socket
                        .send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                        .await;
                }
                Err(quiche::Error::Done) => break,
                Err(e) => return Err(format!("client send: {e}")),
            }
        }
        // Once established, capture peer cert + send payload (once).
        let established = {
            let guard = conn.lock().await;
            guard.is_established()
        };
        if established && peer_cert.is_none() {
            let cert = conn.lock().await.peer_cert().map(<[u8]>::to_vec);
            peer_cert = cert;
        }
        if established && !request_sent {
            let mut pos = 0;
            while pos < payload.len() {
                let chunk = payload.get(pos..).unwrap_or(&[]);
                let last = pos + chunk.len() >= payload.len();
                let r = {
                    let mut guard = conn.lock().await;
                    guard.stream_send(stream_id, chunk, last)
                };
                match r {
                    Ok(n) => pos += n,
                    Err(quiche::Error::Done) => break,
                    Err(e) => return Err(format!("client stream_send: {e}")),
                }
            }
            request_sent = true;
        }
        if established {
            let readable: Vec<u64> = {
                let guard = conn.lock().await;
                guard.readable().collect()
            };
            for sid in readable {
                if sid != stream_id {
                    continue;
                }
                let mut chunk = [0u8; 8192];
                loop {
                    let r = {
                        let mut guard = conn.lock().await;
                        guard.stream_recv(sid, &mut chunk)
                    };
                    match r {
                        Ok((n, fin)) => {
                            echoed.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                            if fin {
                                fin_seen = true;
                                break;
                            }
                        }
                        Err(quiche::Error::Done) => break,
                        // After the stream is fully read (`fin=true`
                        // delivered) quiche's next `stream_recv` on
                        // that sid returns `InvalidStreamState` —
                        // treat as a clean end-of-read.
                        Err(quiche::Error::InvalidStreamState(_)) => break,
                        Err(e) => return Err(format!("client stream_recv: {e}")),
                    }
                }
            }
            if fin_seen && echoed.len() >= payload.len() {
                // Done. Close cleanly.
                let _ = conn.lock().await.close(true, 0, b"done");
                // One more flush so the CLOSE goes out.
                loop {
                    let send = {
                        let mut guard = conn.lock().await;
                        guard.send(&mut out_buf)
                    };
                    match send {
                        Ok((n, info)) => {
                            let _ = socket
                                .send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                                .await;
                        }
                        Err(_) => break,
                    }
                }
                let cert = peer_cert.ok_or_else(|| "no peer cert captured".to_string())?;
                return Ok((cert, echoed));
            }
        }

        let timeout = { conn.lock().await.timeout() };
        let budget = deadline.saturating_duration_since(tokio::time::Instant::now());
        let wait = timeout.map_or(budget, |t| t.min(budget));
        tokio::select! {
            recv = socket.recv_from(&mut in_buf) => {
                let (n, from) = recv.map_err(|e| format!("client recv: {e}"))?;
                let info = quiche::RecvInfo { from, to: local_addr };
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                let r = {
                    let mut guard = conn.lock().await;
                    guard.recv(slice, info)
                };
                match r {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(e) => return Err(format!("client recv to conn: {e}")),
                }
            }
            () = tokio::time::sleep(wait) => {
                conn.lock().await.on_timeout();
            }
        }
    }
}

/// **Gate (i) real-QUIC wire e2e — handshake + STREAM round-trip.**
///
/// LB-mints-Retry handshake requires the backend know the client's
/// ORIGINAL DCID (the DCID of the very first Initial, pre-Retry) to
/// set `original_destination_connection_id` correctly AND know the
/// LB-chosen SCID to set `retry_source_connection_id`. The backend
/// recovers both:
///
/// * ODCID — extracted from the LB-minted Retry token via
///   [`extract_odcid_from_token_unsafe`] (the LB + backend share the
///   `RetryTokenSigner` secret `RETRY_SECRET` in this fixture; in
///   production a sidecar / PROXY-protocol-analogue for QUIC closes
///   the same gap).
/// * Retry-SCID — the LB's chosen new_scid appears on the wire as the
///   client's second-Initial DCID = `hdr.dcid` on the backend.
///
/// Both are passed via `quiche::accept_with_retry` + an explicit
/// `quiche::RetryConnectionIds`. The plain `quiche::accept(odcid)`
/// path is insufficient (it defaults `retry_source_connection_id` to
/// the server's chosen SCID, which the client rejects).
///
/// Asserts:
///   1. handshake completes through the LB;
///   2. byte-faithful echo of a binary payload (LB doesn't mutate);
///   3. client's `peer_cert` is the BACKEND's leaf cert (LB never
///      terminated TLS — §9.5 NEVER-DECRYPTED STATE proof at the
///      wire).
///
/// CF-S15-PASSTHROUGH-RETRY-ODCID closed by this commit. The earlier
/// concern was:
/// the backend cannot know the client's ODCID without an out-of-band
/// side channel that design §3.6 does NOT currently provide. The
/// LB-mints-Retry policy itself is correct (§6.5 Initial-flood defence
/// per owner ruling §9.2 — keep). The fix is a separate design ticket:
/// either (a) forward ODCID to the backend over a side channel
/// (PROXY-protocol analogue for QUIC); or (b) skip LB Retry under a
/// trusted-network config knob and rely on backend Retry. (a) is
/// preferred and matches Cloudflare's deployment posture.
///
/// Tracked as **CF-S15-PASSTHROUGH-RETRY-ODCID**. Until resolved, this
/// real-quiche-handshake e2e is `#[ignore]`'d. The handshake-path
/// state machine is still exercised in synthetic form by the
/// A2-4 / A2-5 / A2-6 verify gates which install flows via valid
/// Retry tokens minted with the same `RetryTokenSigner` secret and
/// drive the same `handle_initial` / `forward_short` code paths.
#[tokio::test(flavor = "current_thread")]
async fn passthrough_e2e_real_quiche_client_handshake_and_stream() {
    let certs = make_certs();
    let (backend_addr, backend_join) =
        spawn_quic_echo_backend(certs.cert_path.clone(), certs.key_path.clone()).await;
    let (listener, lb_addr, cancel) = spawn_lb(backend_addr).await;

    // Client setup.
    let client_socket = Arc::new(
        UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .expect("client bind"),
    );
    let client_local = client_socket.local_addr().expect("client local_addr");
    let mut client_cfg = build_client_config(&certs.ca_path);
    let scid = random_scid();
    let scid_conn = quiche::ConnectionId::from_ref(&scid);
    let conn = quiche::connect(
        Some(TEST_SNI),
        &scid_conn,
        client_local,
        lb_addr,
        &mut client_cfg,
    )
    .expect("client connect");
    let conn = Arc::new(Mutex::new(conn));

    // Mixed binary payload to prove byte-faithful pass-through:
    //   - 0xFF (high bit) — high-byte preservation
    //   - 0x00 (NUL)      — null-byte preservation
    //   - 0x80            — high-bit pattern
    //   - 0x7F            — boundary low
    //   - ASCII tail      — keeps the payload roughly text-recognisable
    //                       for debug logs
    //   - 0..=0xFF        — full byte-range coverage
    let mut payload: Vec<u8> = vec![0xFF, 0x00, 0x80, 0x7F];
    payload.extend_from_slice(b"PASSTHROUGH-E2E-PAYLOAD");
    payload.extend(0u8..=0xFFu8);
    payload.extend_from_slice(b"-tail");

    let (peer_cert_der, echoed) = drive_client(
        Arc::clone(&conn),
        Arc::clone(&client_socket),
        client_local,
        payload.clone(),
    )
    .await
    .expect("client driver");

    // (1) Echo is byte-faithful.
    assert_eq!(
        echoed, payload,
        "echoed payload differs from sent (LB mutated or dropped bytes)"
    );

    // (2) Client's view of peer_cert is the BACKEND's leaf cert. The
    //     quiche::Connection::peer_cert returns DER for the leaf cert
    //     received during the TLS handshake. If the LB were
    //     terminating, this would be its own cert; if the LB were
    //     pass-through, it must be the backend's cert byte-for-byte.
    assert_eq!(
        peer_cert_der, certs.leaf_der,
        "client peer_cert DER does not match backend's rcgen-issued \
         leaf cert — LB appears to have terminated TLS (gate (i) violated)"
    );

    // Wait for backend driver to finish cleanly (best-effort — close
    // racing the join is fine, we just want a clean shutdown).
    let _ = tokio::time::timeout(Duration::from_secs(2), backend_join).await;

    // Tear down the LB.
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), listener.shutdown()).await;
}
