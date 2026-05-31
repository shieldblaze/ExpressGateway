//! SESSION 23 / INC-1 — THROWAWAY go/no-go experiment for the
//! quiche::h3 migration. **This file proves an assumption; it is not a
//! production increment and wires into no live path.** It will be
//! deleted (or folded into the real suite) once the migration decision
//! is made. Nothing in `lb-h3` or the hand-rolled stack is touched.
//!
//! ## What it must answer (owner gate, `s23-migration-plan.md` INC-1)
//!
//! 1. **Interop in practice (not API-on-paper):** does
//!    `quiche::h3::Connection::with_transport` on a `quiche::accept`
//!    server connection actually drive a full request→response with a
//!    **conformant** `quiche::h3` client (the production counterparty —
//!    a real browser/curl opens a control stream + SETTINGS, which the
//!    hand-rolled test clients do NOT; so the GO bar is judged against a
//!    real h3 client, and the test clients themselves are in-scope to
//!    migrate). Bytes must round-trip verbatim through a **real TCP
//!    backend**.
//! 2. **R8 bound attaches:** the bounded-incremental relay
//!    (`recv_body` caller-sized → bounded mpsc → backend, and backend →
//!    bounded mpsc → `send_body`) keeps server-held bytes **bounded and
//!    body-size-INDEPENDENT** — NOT draining `recv_body` into an
//!    unbounded buffer (that "success" is a NO-GO, the buffering trap in
//!    new clothes).
//! 3. **R8 backpressure attaches:** when the server stops calling
//!    `recv_body`, the QUIC flow-control window is not extended and the
//!    client's `send_body` is paused (cannot push the whole body).
//!
//! The relay below mirrors the production shape EXACTLY: per-request
//! bounded `mpsc::channel(DEPTH)` for the request body and the response
//! body, `recv_body` into a fixed `CHUNK` buffer with a "stop reading
//! when the channel is full" gate, and `send_body` partial-write/`Done`
//! retry — i.e. the same primitives `conn_actor`/`h3_bridge` use, but on
//! quiche::h3 instead of the hand-rolled framing.
//!
//! ## Transport fidelity
//!
//! The QUIC legs run over **real loopback UDP sockets** with a real
//! quiche handshake, real H3 framing, real QPACK, and real QUIC flow
//! control (the backpressure mechanism). RETRY is a transport-layer
//! concern orthogonal to `with_transport` (an established connection is
//! identical with or without it), already covered by the existing
//! real-UDP H3 suite; this experiment uses `accept`/`connect` directly
//! to isolate the H3-layer question.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::too_many_lines)]

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use quiche::h3;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;

const H3_CHUNK: usize = 16 * 1024;
const REQ_DEPTH: usize = 8;
const RESP_DEPTH: usize = 8;
const MAX_DGRAM: usize = 1350;

// ----------------------------------------------------------------------
// Certs (self-signed loopback, same rcgen pattern as the existing suite)
// ----------------------------------------------------------------------

struct Certs {
    dir: PathBuf,
    cert: PathBuf,
    key: PathBuf,
}
impl Drop for Certs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn gen_certs() -> Certs {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("inc1-h3-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["inc1.test".to_string()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
    let key_pair = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, cert.pem().as_bytes()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem().as_bytes()).unwrap();
    Certs {
        dir,
        cert: cert_path,
        key: key_path,
    }
}

fn scid() -> quiche::ConnectionId<'static> {
    use ring::rand::SecureRandom;
    let mut b = vec![0u8; quiche::MAX_CONN_ID_LEN];
    ring::rand::SystemRandom::new().fill(&mut b).unwrap();
    quiche::ConnectionId::from_vec(b)
}

fn server_cfg(certs: &Certs) -> quiche::Config {
    let mut c = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    c.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
        .unwrap();
    c.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
        .unwrap();
    c.set_application_protos(h3::APPLICATION_PROTOCOL).unwrap();
    c.set_max_idle_timeout(10_000);
    c.set_max_recv_udp_payload_size(MAX_DGRAM);
    c.set_max_send_udp_payload_size(MAX_DGRAM);
    // Deliberately MODEST windows so "server stops reading" caps the
    // client quickly (the backpressure proof). quiche auto-extends as
    // the app consumes, so the drain path still flows a full 1 MiB body.
    c.set_initial_max_data(256 * 1024);
    c.set_initial_max_stream_data_bidi_local(256 * 1024);
    c.set_initial_max_stream_data_bidi_remote(64 * 1024);
    c.set_initial_max_stream_data_uni(64 * 1024);
    c.set_initial_max_streams_bidi(16);
    c.set_initial_max_streams_uni(8);
    c.set_disable_active_migration(true);
    c
}

fn client_cfg() -> quiche::Config {
    let mut c = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    c.verify_peer(false);
    c.set_application_protos(h3::APPLICATION_PROTOCOL).unwrap();
    c.set_max_idle_timeout(10_000);
    c.set_max_recv_udp_payload_size(MAX_DGRAM);
    c.set_max_send_udp_payload_size(MAX_DGRAM);
    c.set_initial_max_data(4 * 1024 * 1024);
    c.set_initial_max_stream_data_bidi_local(4 * 1024 * 1024);
    c.set_initial_max_stream_data_bidi_remote(256 * 1024);
    c.set_initial_max_stream_data_uni(256 * 1024);
    c.set_initial_max_streams_bidi(16);
    c.set_initial_max_streams_uni(8);
    c.set_disable_active_migration(true);
    c
}

// ----------------------------------------------------------------------
// Real TCP echo backend: reads until the peer half-closes, then writes
// every received byte back, then closes. A genuine separate socket in
// the proxy topology.
// ----------------------------------------------------------------------

async fn spawn_echo_backend() -> SocketAddr {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                // Read to EOF (the proxy half-closes write when the H3
                // request FINs).
                let _ = s.read_to_end(&mut buf).await;
                let _ = s.write_all(&buf).await;
                let _ = s.shutdown().await;
            });
        }
    });
    addr
}

// ----------------------------------------------------------------------
// UDP pump helpers (loopback, no loss → drain-all each tick).
// ----------------------------------------------------------------------

fn flush(conn: &mut quiche::Connection, sock: &UdpSocket, out: &mut [u8]) {
    loop {
        match conn.send(out) {
            Ok((n, info)) => {
                // Blocking try_send on loopback is fine; datagram is small.
                let _ = sock.try_send_to(&out[..n], info.to);
            }
            Err(quiche::Error::Done) => break,
            Err(_) => break,
        }
    }
}

fn drain_into(conn: &mut quiche::Connection, sock: &UdpSocket, local: SocketAddr) {
    let mut buf = [0u8; 2048];
    loop {
        match sock.try_recv_from(&mut buf) {
            Ok((n, from)) => {
                let info = quiche::RecvInfo { from, to: local };
                let _ = conn.recv(&mut buf[..n], info);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

// ----------------------------------------------------------------------
// EXPERIMENT 1 — GO/NO-GO: full round-trip through a real backend +
// the R8 body-size-INDEPENDENT memory bound (non-vacuous: the whole
// body must transit).
// ----------------------------------------------------------------------

/// Run one request of `body_len` bytes through:
///   conformant quiche::h3 client → with_transport server (R8 bounded
///   relay) → real TCP echo backend → back to the client.
/// Returns `(echo_received_by_client, server_peak_retained_bytes)`.
async fn run_one(body_len: usize, backend: SocketAddr) -> (usize, usize) {
    let certs = gen_certs();
    let mut s_cfg = server_cfg(&certs);
    let mut c_cfg = client_cfg();

    let s_sock = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
    let c_sock = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
    let s_addr = s_sock.local_addr().unwrap();
    let c_addr = c_sock.local_addr().unwrap();

    let s_scid = scid();
    let c_scid = scid();
    let mut server = quiche::accept(&s_scid, None, s_addr, c_addr, &mut s_cfg).unwrap();
    let mut client =
        quiche::connect(Some("inc1.test"), &c_scid, c_addr, s_addr, &mut c_cfg).unwrap();

    let mut s_h3: Option<h3::Connection> = None;
    let mut c_h3: Option<h3::Connection> = None;
    let h3_cfg = lb_quic::h3_config::build_server_h3_config().unwrap();
    let h3_cfg_c = h3::Config::new().unwrap();

    // Request body: a deterministic, position-dependent pattern so the
    // echo comparison catches any reorder/truncation.
    let req_body: Vec<u8> = (0..body_len).map(|i| (i % 251) as u8).collect();

    // --- server-side R8 relay state (mirrors conn_actor exactly) ---
    let mut req_tx: Option<mpsc::Sender<Vec<u8>>> = None;
    let mut resp_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
    let mut req_stream_id: Option<u64> = None;
    let mut resp_started = false;
    // Unsent response tail carried across ticks (the production
    // `StreamTx::Progressive` queue): send_body returning Done leaves the
    // remainder here so the client can drain + extend the window before
    // we retry next tick. Never an inner force-drain.
    let mut resp_buf: Vec<u8> = Vec::new();
    let mut server_recv_buf = vec![0u8; H3_CHUNK];
    let peak = Arc::new(AtomicUsize::new(0));

    // --- client-side state ---
    let mut sent = 0usize; // request body bytes accepted by send_body
    let mut request_id: Option<u64> = None;
    let mut req_fin_sent = false;
    let mut echo: Vec<u8> = Vec::new();
    let mut client_done = false;

    let mut s_out = vec![0u8; 65535];
    let mut c_out = vec![0u8; 65535];

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if tokio::time::Instant::now() > deadline {
            panic!(
                "inc1 run_one timed out (body_len={body_len}, sent={sent}, echo={})",
                echo.len()
            );
        }

        flush(&mut server, &s_sock, &mut s_out);
        flush(&mut client, &c_sock, &mut c_out);
        tokio::time::sleep(Duration::from_millis(1)).await;
        drain_into(&mut server, &s_sock, s_addr);
        drain_into(&mut client, &c_sock, c_addr);

        // Bring up H3 once each side is established.
        if s_h3.is_none() && server.is_established() {
            s_h3 = Some(h3::Connection::with_transport(&mut server, &h3_cfg).unwrap());
        }
        if c_h3.is_none() && client.is_established() {
            let mut h = h3::Connection::with_transport(&mut client, &h3_cfg_c).unwrap();
            // Send the request head (body follows → fin=false).
            let req = vec![
                h3::Header::new(b":method", b"POST"),
                h3::Header::new(b":scheme", b"https"),
                h3::Header::new(b":authority", b"inc1.test"),
                h3::Header::new(b":path", b"/echo"),
            ];
            let id = h.send_request(&mut client, &req, false).unwrap();
            request_id = Some(id);
            c_h3 = Some(h);
        }

        // ---- CLIENT: push request body (bounded by flow control), read response ----
        if let (Some(h), Some(id)) = (c_h3.as_mut(), request_id) {
            // Push as much body as the window allows.
            while sent < body_len {
                let end = (sent + H3_CHUNK).min(body_len);
                match h.send_body(&mut client, id, &req_body[sent..end], false) {
                    Ok(n) if n > 0 => sent += n,
                    Ok(_) | Err(h3::Error::Done) => break,
                    Err(e) => panic!("client send_body: {e:?}"),
                }
            }
            if sent == body_len && !req_fin_sent {
                // Empty fin send closes the request body.
                match h.send_body(&mut client, id, &[], true) {
                    Ok(_) | Err(h3::Error::Done) => req_fin_sent = true,
                    Err(e) => panic!("client fin send_body: {e:?}"),
                }
            }
            // Drain response events.
            loop {
                match h.poll(&mut client) {
                    Ok((_sid, h3::Event::Headers { .. })) => {}
                    Ok((sid, h3::Event::Data)) => {
                        let mut b = [0u8; H3_CHUNK];
                        while let Ok(n) = h.recv_body(&mut client, sid, &mut b) {
                            if n == 0 {
                                break;
                            }
                            echo.extend_from_slice(&b[..n]);
                        }
                    }
                    Ok((_sid, h3::Event::Finished)) => client_done = true,
                    Ok((_sid, h3::Event::Reset(_))) => panic!("client got unexpected Reset"),
                    Ok(_) => {}
                    Err(h3::Error::Done) => break,
                    Err(e) => panic!("client poll: {e:?}"),
                }
            }
        }

        // ---- SERVER: poll, recv_body (bounded relay), send echoed body ----
        if let Some(h) = s_h3.as_mut() {
            loop {
                match h.poll(&mut server) {
                    Ok((sid, h3::Event::Headers { .. })) => {
                        // Spawn the bounded R8 relay to the real TCP backend.
                        let (rtx, mut rrx) = mpsc::channel::<Vec<u8>>(REQ_DEPTH);
                        let (ptx, prx) = mpsc::channel::<Vec<u8>>(RESP_DEPTH);
                        req_tx = Some(rtx);
                        resp_rx = Some(prx);
                        req_stream_id = Some(sid);
                        tokio::spawn(async move {
                            let mut be = TcpStream::connect(backend).await.unwrap();
                            // Forward request body → backend, bounded.
                            while let Some(chunk) = rrx.recv().await {
                                if be.write_all(&chunk).await.is_err() {
                                    return;
                                }
                            }
                            // Request FIN → half-close so the echo completes.
                            let _ = be.shutdown().await;
                            // Read echo → bounded response channel.
                            let mut buf = vec![0u8; H3_CHUNK];
                            loop {
                                match be.read(&mut buf).await {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        if ptx.send(buf[..n].to_vec()).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(_) => return,
                                }
                            }
                            // Drop ptx → response EOF.
                        });
                    }
                    Ok((sid, h3::Event::Data)) => {
                        if let Some(tx) = req_tx.as_ref() {
                            // R8 GATE: only read while the bounded channel
                            // has capacity. If full, STOP calling recv_body
                            // → quiche does not extend flow control → client
                            // paused. Never drain into an unbounded buffer.
                            while tx.capacity() > 0 {
                                match h.recv_body(&mut server, sid, &mut server_recv_buf) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        // Record peak server-held bytes:
                                        // occupied channel slots × chunk +
                                        // this read. Mirrors record_retained.
                                        let used = REQ_DEPTH - tx.capacity();
                                        let retained = used * H3_CHUNK + n;
                                        peak.fetch_max(retained, Ordering::Relaxed);
                                        let _ = tx.try_send(server_recv_buf[..n].to_vec());
                                    }
                                    Err(h3::Error::Done) => break,
                                    Err(e) => panic!("server recv_body: {e:?}"),
                                }
                            }
                        }
                    }
                    Ok((_sid, h3::Event::Finished)) => {
                        // Client finished the request body → drop req_tx so
                        // the backend task half-closes and the echo starts.
                        req_tx = None;
                    }
                    Ok((_sid, h3::Event::Reset(_))) => panic!("server got unexpected Reset"),
                    Ok(_) => {}
                    Err(h3::Error::Done) => break,
                    Err(e) => panic!("server poll: {e:?}"),
                }
            }

            // Drain the bounded response channel → send_body (egress R8).
            if let (Some(rx), Some(sid)) = (resp_rx.as_mut(), req_stream_id) {
                if !resp_started {
                    let resp = vec![h3::Header::new(b":status", b"200")];
                    match h.send_response(&mut server, sid, &resp, false) {
                        Ok(()) | Err(h3::Error::Done) => resp_started = true,
                        Err(e) => panic!("server send_response: {e:?}"),
                    }
                }
                if resp_started {
                    // (a) Flush the carried tail first (egress R8 gate:
                    //     only pull a new chunk once the tail is drained).
                    if !resp_buf.is_empty() {
                        match h.send_body(&mut server, sid, &resp_buf, false) {
                            Ok(n) if n > 0 => {
                                resp_buf.drain(..n);
                            }
                            Ok(_) | Err(h3::Error::Done) => { /* blocked → retry next tick */ }
                            Err(e) => panic!("server send_body: {e:?}"),
                        }
                    }
                    // (b) Tail drained → pull the next bounded-channel chunk.
                    if resp_buf.is_empty() {
                        match rx.try_recv() {
                            Ok(chunk) => {
                                // Record peak server-held RESPONSE bytes
                                // (bounded channel occupancy + this chunk).
                                let used = RESP_DEPTH - rx.capacity();
                                let retained = used * H3_CHUNK + chunk.len();
                                peak.fetch_max(retained, Ordering::Relaxed);
                                resp_buf = chunk;
                            }
                            Err(mpsc::error::TryRecvError::Empty) => {}
                            Err(mpsc::error::TryRecvError::Disconnected) => {
                                // Backend EOF + tail flushed → FIN the
                                // response stream.
                                let _ = h.send_body(&mut server, sid, &[], true);
                                resp_rx = None;
                            }
                        }
                    }
                }
            }
        }

        server.on_timeout();
        client.on_timeout();

        if client_done {
            break;
        }
    }

    let p = peak.load(Ordering::Relaxed);
    (echo.len(), p)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inc1_go_roundtrip_and_r8_bounded() {
    let backend = spawn_echo_backend().await;

    // Two bodies that BOTH saturate the bounded relay (both ≫ the
    // channel capacity 8×16 KiB = 128 KiB), so the peak comparison is a
    // true body-size-INDEPENDENCE proof: quadrupling the body must leave
    // the peak unchanged. A sub-capacity body (e.g. 64 KiB) would under-
    // fill the channel and is NOT a valid independence point.
    let body_a = 1024 * 1024; // 1 MiB
    let body_b = 4 * 1024 * 1024; // 4 MiB
    let (echo_a, peak_a) = run_one(body_a, backend).await;
    let (echo_b, peak_b) = run_one(body_b, backend).await;
    eprintln!(
        "INC-1 R8 EVIDENCE: body 1MiB→echo {echo_a}B peak {peak_a}B | \
         body 4MiB→echo {echo_b}B peak {peak_b}B | ceiling {}B | delta {}B",
        (REQ_DEPTH + 1) * H3_CHUNK,
        peak_a.abs_diff(peak_b)
    );

    // (1) INTEROP + CORRECTNESS: the whole body round-tripped verbatim
    // through with_transport → real backend → back (non-vacuous).
    assert_eq!(echo_a, body_a, "1 MiB body must fully round-trip");
    assert_eq!(echo_b, body_b, "4 MiB body must fully round-trip");

    // (2) R8 BOUND: peak server-held bytes is capped by the channel
    // depth × chunk (≈ 9 × 16 KiB = 144 KiB), NOT by body size. A
    // buffering relay would show peak ≈ body (1 MiB / 4 MiB).
    let ceiling = (REQ_DEPTH + 1) * H3_CHUNK; // 9 × 16 KiB = 144 KiB
    assert!(
        peak_a <= ceiling,
        "1 MiB peak {peak_a} exceeds R8 ceiling {ceiling} — \
         recv_body drained into an unbounded buffer (NO-GO)"
    );
    assert!(
        peak_b <= ceiling,
        "4 MiB peak {peak_b} exceeds R8 ceiling {ceiling} — \
         recv_body drained into an unbounded buffer (NO-GO)"
    );
    // (3) BODY-SIZE INDEPENDENCE: 4× the body → peak within one chunk
    // (it does NOT scale with the body — the load-bearing R8 property).
    let delta = peak_a.abs_diff(peak_b);
    assert!(
        delta <= H3_CHUNK,
        "peak scaled with body size (1MiB={peak_a}, 4MiB={peak_b}, \
         delta={delta}) — the relay is NOT body-size-independent (NO-GO)"
    );
    // Non-vacuous: the relay actually held bytes (the gauge fired and the
    // bounded channel genuinely saturated, not a 0-byte no-op).
    assert!(
        peak_b >= REQ_DEPTH * H3_CHUNK,
        "peak {peak_b} below a full channel — the bounded relay never \
         saturated, so the bound proof is vacuous"
    );
}

// ----------------------------------------------------------------------
// EXPERIMENT 2 — R8 BACKPRESSURE: when the server NEVER calls recv_body,
// the QUIC flow-control window is not extended and the client cannot
// push the whole body. (The drain control is EXPERIMENT 1, which sends a
// full 1 MiB through the same server window.)
// ----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inc1_go_backpressure_unread_pauses_peer() {
    let certs = gen_certs();
    let mut s_cfg = server_cfg(&certs);
    let mut c_cfg = client_cfg();

    let s_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let c_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let s_addr = s_sock.local_addr().unwrap();
    let c_addr = c_sock.local_addr().unwrap();

    let s_scid = scid();
    let c_scid = scid();
    let mut server = quiche::accept(&s_scid, None, s_addr, c_addr, &mut s_cfg).unwrap();
    let mut client =
        quiche::connect(Some("inc1.test"), &c_scid, c_addr, s_addr, &mut c_cfg).unwrap();

    let mut s_h3: Option<h3::Connection> = None;
    let mut c_h3: Option<h3::Connection> = None;
    let h3_cfg = lb_quic::h3_config::build_server_h3_config().unwrap();
    let h3_cfg_c = h3::Config::new().unwrap();

    // 1 MiB body; server NEVER reads it.
    let body_len = 1024 * 1024;
    let body = vec![0xABu8; body_len];
    let mut sent = 0usize;
    let mut request_id = None;

    let mut s_out = vec![0u8; 65535];
    let mut c_out = vec![0u8; 65535];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut stalled_ticks = 0u32;

    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        flush(&mut server, &s_sock, &mut s_out);
        flush(&mut client, &c_sock, &mut c_out);
        tokio::time::sleep(Duration::from_millis(1)).await;
        drain_into(&mut server, &s_sock, s_addr);
        drain_into(&mut client, &c_sock, c_addr);

        if s_h3.is_none() && server.is_established() {
            s_h3 = Some(h3::Connection::with_transport(&mut server, &h3_cfg).unwrap());
        }
        if c_h3.is_none() && client.is_established() {
            let mut h = h3::Connection::with_transport(&mut client, &h3_cfg_c).unwrap();
            let req = vec![
                h3::Header::new(b":method", b"POST"),
                h3::Header::new(b":scheme", b"https"),
                h3::Header::new(b":authority", b"inc1.test"),
                h3::Header::new(b":path", b"/sink"),
            ];
            let id = h.send_request(&mut client, &req, false).unwrap();
            request_id = Some(id);
            c_h3 = Some(h);
        }

        if let (Some(h), Some(id)) = (c_h3.as_mut(), request_id) {
            let before = sent;
            while sent < body_len {
                let end = (sent + H3_CHUNK).min(body_len);
                match h.send_body(&mut client, id, &body[sent..end], false) {
                    Ok(n) if n > 0 => sent += n,
                    Ok(_) | Err(h3::Error::Done) => break,
                    Err(e) => panic!("client send_body: {e:?}"),
                }
            }
            if sent == before {
                stalled_ticks += 1;
            } else {
                stalled_ticks = 0;
            }
        }

        // SERVER: poll for events but DELIBERATELY never recv_body — the
        // request body must stay unconsumed.
        if let Some(h) = s_h3.as_mut() {
            loop {
                match h.poll(&mut server) {
                    Ok((_sid, h3::Event::Data)) => { /* INTENTIONALLY do NOT read */ }
                    Ok(_) => {}
                    Err(h3::Error::Done) => break,
                    Err(e) => panic!("server poll: {e:?}"),
                }
            }
        }

        server.on_timeout();
        client.on_timeout();

        // The client has been unable to make progress for many ticks →
        // it is flow-control-blocked by the un-reading server.
        if stalled_ticks > 50 && sent > 0 {
            break;
        }
    }

    eprintln!(
        "INC-1 BACKPRESSURE EVIDENCE: server never read; client pushed \
         {sent}B of a {body_len}B body before flow-control blocked it \
         (cap ≈ server window)"
    );
    // BACKPRESSURE PROVEN: with the server never reading, the client
    // could NOT push the whole 1 MiB body — it is paused by flow control
    // (capped near the server's advertised window, ≤ a few hundred KiB).
    assert!(
        sent < body_len,
        "client pushed the ENTIRE {body_len}-byte body despite the server \
         never reading — backpressure did NOT engage (NO-GO). sent={sent}"
    );
    assert!(
        sent > 0,
        "client never sent anything — vacuous (handshake/setup failed?)"
    );
    // The cap should be near the server's window (256 KiB conn / 64 KiB
    // stream), not the full 1 MiB.
    assert!(
        sent <= 512 * 1024,
        "client sent {sent} (> 512 KiB) with the server not reading — \
         the flow-control window is not bounding the peer as expected"
    );
}

// ----------------------------------------------------------------------
// EXPERIMENT 3 — INC-2 FEASIBILITY: does a quiche::h3 with_transport
// SERVER interoperate with the EXISTING hand-rolled lb_h3 test-client
// wire (lb_h3 QPACK + encode_frame, NO client control stream / SETTINGS,
// lb_h3 response decode)? If YES, migrating conn_actor to quiche::h3
// keeps the 69 existing wire tests green (gate-green incremental INC-2).
// If NO, INC-2 must also migrate every test client (much larger scope).
// Either result is decision-relevant; this test asserts the answer.
// ----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inc1_interop_handrolled_lbh3_client_vs_quiche_server() {
    use lb_h3::{H3Frame, QpackDecoder, QpackEncoder, decode_frame, encode_frame};
    use quiche::h3::NameValue;

    let certs = gen_certs();
    let mut s_cfg = server_cfg(&certs);
    let mut c_cfg = client_cfg();

    let s_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let c_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let s_addr = s_sock.local_addr().unwrap();
    let c_addr = c_sock.local_addr().unwrap();

    let s_scid = scid();
    let c_scid = scid();
    let mut server = quiche::accept(&s_scid, None, s_addr, c_addr, &mut s_cfg).unwrap();
    let mut client =
        quiche::connect(Some("inc1.test"), &c_scid, c_addr, s_addr, &mut c_cfg).unwrap();

    let mut s_h3: Option<h3::Connection> = None;
    let h3_cfg = lb_quic::h3_config::build_server_h3_config().unwrap();

    // Hand-rolled client state (NO h3::Connection — raw stream I/O, the
    // exact shape of h3_h1_bridge_e2e's client).
    const STREAM_ID: u64 = 0;
    let mut request_sent = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut decoded_status: Option<u16> = None;
    let mut decoded_body: Vec<u8> = Vec::new();

    // Server observations.
    let mut server_saw_headers: Option<Vec<(String, String)>> = None;
    let mut response_sent = false;
    const RESP_BODY: &[u8] = b"interop-ok-handrolled";

    let mut s_out = vec![0u8; 65535];
    let mut c_out = vec![0u8; 65535];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);

    loop {
        if tokio::time::Instant::now() > deadline {
            panic!(
                "interop timed out: server_saw_headers={}, decoded_status={decoded_status:?}, \
                 body={}, client_closed={}, server_closed={}",
                server_saw_headers.is_some(),
                decoded_body.len(),
                client.is_closed(),
                server.is_closed(),
            );
        }
        flush(&mut server, &s_sock, &mut s_out);
        flush(&mut client, &c_sock, &mut c_out);
        tokio::time::sleep(Duration::from_millis(1)).await;
        drain_into(&mut server, &s_sock, s_addr);
        drain_into(&mut client, &c_sock, c_addr);

        if client.is_closed() {
            panic!(
                "client connection closed by server: {:?}",
                client.peer_error()
            );
        }

        // --- hand-rolled client: send HEADERS (GET + fin), decode resp ---
        if !request_sent && client.is_established() {
            let encoder = QpackEncoder::new();
            let headers = vec![
                (":method".to_string(), "GET".to_string()),
                (":scheme".to_string(), "https".to_string()),
                (":authority".to_string(), "inc1.test".to_string()),
                (":path".to_string(), "/interop".to_string()),
            ];
            let header_block = encoder.encode(&headers).unwrap();
            let frame = encode_frame(&H3Frame::Headers { header_block }).unwrap();
            let mut pos = 0;
            while pos < frame.len() {
                match client.stream_send(
                    STREAM_ID,
                    &frame[pos..],
                    pos + (frame.len() - pos) >= frame.len(),
                ) {
                    Ok(n) => pos += n,
                    Err(quiche::Error::Done) => break,
                    Err(e) => panic!("client stream_send: {e}"),
                }
            }
            request_sent = true;
        }
        if client.is_established() {
            for sid in client.readable().collect::<Vec<u64>>() {
                if sid != STREAM_ID {
                    // Drain+discard server uni streams (control/QPACK) —
                    // exactly what the hand-rolled test clients do.
                    let mut sink = [0u8; 4096];
                    while client.stream_recv(sid, &mut sink).is_ok() {}
                    continue;
                }
                let mut chunk = [0u8; 8192];
                while let Ok((n, _fin)) = client.stream_recv(sid, &mut chunk) {
                    rx_tail.extend_from_slice(&chunk[..n]);
                }
            }
            loop {
                match decode_frame(&rx_tail, 1 << 20) {
                    Ok((H3Frame::Headers { header_block }, consumed)) => {
                        rx_tail.drain(..consumed);
                        let hdrs = QpackDecoder::new().decode(&header_block).unwrap();
                        for (n, v) in hdrs {
                            if n == ":status" {
                                decoded_status = Some(v.parse().unwrap());
                            }
                        }
                    }
                    Ok((H3Frame::Data { payload }, consumed)) => {
                        rx_tail.drain(..consumed);
                        decoded_body.extend_from_slice(&payload);
                    }
                    Ok((_other, consumed)) => {
                        rx_tail.drain(..consumed);
                    }
                    Err(lb_h3::H3Error::Incomplete) => break,
                    Err(e) => panic!("client decode_frame: {e}"),
                }
            }
        }

        // --- quiche::h3 server: poll, respond ---
        if s_h3.is_none() && server.is_established() {
            s_h3 = Some(h3::Connection::with_transport(&mut server, &h3_cfg).unwrap());
        }
        if let Some(h) = s_h3.as_mut() {
            loop {
                match h.poll(&mut server) {
                    Ok((sid, h3::Event::Headers { list, .. })) => {
                        server_saw_headers = Some(
                            list.iter()
                                .map(|hdr| {
                                    (
                                        String::from_utf8_lossy(hdr.name()).into_owned(),
                                        String::from_utf8_lossy(hdr.value()).into_owned(),
                                    )
                                })
                                .collect(),
                        );
                        if !response_sent {
                            let resp = vec![h3::Header::new(b":status", b"200")];
                            h.send_response(&mut server, sid, &resp, false).unwrap();
                            let _ = h.send_body(&mut server, sid, RESP_BODY, true);
                            response_sent = true;
                        }
                    }
                    Ok(_) => {}
                    Err(h3::Error::Done) => break,
                    Err(e) => panic!("server poll: {e:?}"),
                }
            }
        }

        server.on_timeout();
        client.on_timeout();

        if decoded_status.is_some() && decoded_body.len() >= RESP_BODY.len() {
            break;
        }
    }

    // ANSWER: a quiche::h3 server DID parse the non-conformant hand-rolled
    // lb_h3 request (no control stream / SETTINGS) AND the hand-rolled
    // lb_h3 decoder read the quiche-encoded response. The 69 existing
    // wire tests can stay on their hand-rolled clients across INC-2.
    let hdrs = server_saw_headers.expect("server must have polled a Headers event");
    assert!(
        hdrs.iter().any(|(n, v)| n == ":path" && v == "/interop"),
        "server's quiche::h3 poll did not surface the lb_h3 request path: {hdrs:?}"
    );
    assert!(
        hdrs.iter().any(|(n, v)| n == ":method" && v == "GET"),
        "server's quiche::h3 poll did not surface the lb_h3 method: {hdrs:?}"
    );
    assert_eq!(
        decoded_status,
        Some(200),
        "lb_h3 client must decode quiche's :status"
    );
    assert_eq!(
        decoded_body, RESP_BODY,
        "lb_h3 client must decode quiche's response body verbatim"
    );
    eprintln!(
        "INC-1 INTEROP EVIDENCE: quiche::h3 server ↔ hand-rolled lb_h3 client \
         round-trips BOTH directions (req path/method parsed by quiche; \
         resp :status 200 + body decoded by lb_h3). 69 wire tests stay green across INC-2."
    );
}

// ----------------------------------------------------------------------
// EXPERIMENT 4 — INC-2 SHAPE: can the EXISTING hand-rolled egress (raw
// `conn.stream_send` of lb_h3-encoded HEADERS+DATA on the request bidi
// stream) coexist with quiche::h3 OWNING the ingress (poll/recv_body +
// its own server control/QPACK uni streams) on the SAME connection?
//
// If YES → INC-2 can migrate ONLY the ingress (poll_h3 recv/feed →
// quiche::h3 poll/recv_body), leaving the entire RespEvent/StreamTx/
// drain_streams_to_conn egress UNTOUCHED → a small, standalone, gate-
// green increment, with the egress restructure deferred to INC-3.
// If NO → INC-2 must fuse with INC-3 (migrate egress to send_response/
// send_body too) — a much larger single step.
//
// This mirrors the INC-2-ingress-only design EXACTLY: server quiche::h3
// for ingress, raw lb_h3 stream_send for egress, hand-rolled lb_h3
// client (the production-relevant existing test client).
// ----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inc1_hybrid_quiche_ingress_handrolled_egress() {
    use lb_h3::{H3Frame, QpackDecoder, QpackEncoder, decode_frame, encode_frame};
    use quiche::h3::NameValue;

    let certs = gen_certs();
    let mut s_cfg = server_cfg(&certs);
    let mut c_cfg = client_cfg();

    let s_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let c_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let s_addr = s_sock.local_addr().unwrap();
    let c_addr = c_sock.local_addr().unwrap();

    let s_scid = scid();
    let c_scid = scid();
    let mut server = quiche::accept(&s_scid, None, s_addr, c_addr, &mut s_cfg).unwrap();
    let mut client =
        quiche::connect(Some("inc1.test"), &c_scid, c_addr, s_addr, &mut c_cfg).unwrap();

    let mut s_h3: Option<h3::Connection> = None;
    let h3_cfg = lb_quic::h3_config::build_server_h3_config().unwrap();

    // Pre-build the hand-rolled response via the PRODUCTION egress
    // encoder (h3_bridge::encode_h3_response → HEADERS(:status 200) +
    // DATA) — exactly what the existing StreamTx egress emits, sent raw
    // via conn.stream_send (NOT quiche::h3 send_response).
    const RESP_BODY: &[u8] = b"hybrid-egress-ok";
    let resp_bytes: Vec<u8> = lb_quic::h3_bridge::encode_h3_response(200, RESP_BODY).unwrap();

    const STREAM_ID: u64 = 0;
    let mut request_sent = false;
    let mut rx_tail: Vec<u8> = Vec::new();
    let mut decoded_status: Option<u16> = None;
    let mut decoded_body: Vec<u8> = Vec::new();
    let mut server_saw_path = false;
    let mut egress_done = false;

    let mut s_out = vec![0u8; 65535];
    let mut c_out = vec![0u8; 65535];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);

    loop {
        if tokio::time::Instant::now() > deadline {
            panic!(
                "hybrid timed out: saw_path={server_saw_path}, status={decoded_status:?}, \
                 body={}, client_closed={}",
                decoded_body.len(),
                client.is_closed()
            );
        }
        flush(&mut server, &s_sock, &mut s_out);
        flush(&mut client, &c_sock, &mut c_out);
        tokio::time::sleep(Duration::from_millis(1)).await;
        drain_into(&mut server, &s_sock, s_addr);
        drain_into(&mut client, &c_sock, c_addr);
        if client.is_closed() {
            panic!(
                "client closed by server during hybrid egress: {:?}",
                client.peer_error()
            );
        }

        // hand-rolled lb_h3 client: send GET, decode response.
        if !request_sent && client.is_established() {
            let hb = QpackEncoder::new()
                .encode(&[
                    (":method".to_string(), "GET".to_string()),
                    (":scheme".to_string(), "https".to_string()),
                    (":authority".to_string(), "inc1.test".to_string()),
                    (":path".to_string(), "/hybrid".to_string()),
                ])
                .unwrap();
            let frame = encode_frame(&H3Frame::Headers { header_block: hb }).unwrap();
            let mut pos = 0;
            while pos < frame.len() {
                match client.stream_send(STREAM_ID, &frame[pos..], true) {
                    Ok(n) => pos += n,
                    Err(quiche::Error::Done) => break,
                    Err(e) => panic!("client stream_send: {e}"),
                }
            }
            request_sent = true;
        }
        if client.is_established() {
            for sid in client.readable().collect::<Vec<u64>>() {
                if sid != STREAM_ID {
                    let mut sink = [0u8; 4096];
                    while client.stream_recv(sid, &mut sink).is_ok() {}
                    continue;
                }
                let mut chunk = [0u8; 8192];
                while let Ok((n, _fin)) = client.stream_recv(sid, &mut chunk) {
                    rx_tail.extend_from_slice(&chunk[..n]);
                }
            }
            loop {
                match decode_frame(&rx_tail, 1 << 20) {
                    Ok((H3Frame::Headers { header_block }, consumed)) => {
                        rx_tail.drain(..consumed);
                        for (n, v) in QpackDecoder::new().decode(&header_block).unwrap() {
                            if n == ":status" {
                                decoded_status = Some(v.parse().unwrap());
                            }
                        }
                    }
                    Ok((H3Frame::Data { payload }, consumed)) => {
                        rx_tail.drain(..consumed);
                        decoded_body.extend_from_slice(&payload);
                    }
                    Ok((_o, consumed)) => {
                        rx_tail.drain(..consumed);
                    }
                    Err(lb_h3::H3Error::Incomplete) => break,
                    Err(e) => panic!("client decode_frame: {e}"),
                }
            }
        }

        // SERVER: quiche::h3 ingress + RAW lb_h3 egress on the same conn.
        if s_h3.is_none() && server.is_established() {
            s_h3 = Some(h3::Connection::with_transport(&mut server, &h3_cfg).unwrap());
        }
        let mut respond_to: Option<u64> = None;
        if let Some(h) = s_h3.as_mut() {
            loop {
                match h.poll(&mut server) {
                    Ok((sid, h3::Event::Headers { list, .. })) => {
                        if list
                            .iter()
                            .any(|hd| hd.name() == b":path" && hd.value() == b"/hybrid")
                        {
                            server_saw_path = true;
                        }
                        respond_to = Some(sid);
                    }
                    Ok(_) => {}
                    Err(h3::Error::Done) => break,
                    Err(e) => panic!("server poll: {e:?}"),
                }
            }
        }
        // RAW egress: write the hand-rolled response bytes directly on the
        // request stream via conn.stream_send (bypassing quiche::h3's
        // send_response/send_body). This is the hybrid under test.
        if let Some(sid) = respond_to {
            if !egress_done {
                let mut pos = 0;
                while pos < resp_bytes.len() {
                    let fin = true;
                    match server.stream_send(sid, &resp_bytes[pos..], fin) {
                        Ok(n) => pos += n,
                        Err(quiche::Error::Done) => break,
                        Err(e) => panic!("server RAW stream_send (hybrid egress): {e}"),
                    }
                }
                if pos >= resp_bytes.len() {
                    egress_done = true;
                }
            }
        }

        server.on_timeout();
        client.on_timeout();

        if decoded_status.is_some() && decoded_body.len() >= RESP_BODY.len() {
            break;
        }
    }

    // ANSWER: quiche::h3 ingress + raw hand-rolled stream_send egress
    // COEXIST on one connection. INC-2 can migrate ingress only.
    assert!(
        server_saw_path,
        "quiche::h3 ingress did not surface the request"
    );
    assert_eq!(
        decoded_status,
        Some(200),
        "hand-rolled raw egress response not decoded by the client — hybrid FAILED"
    );
    assert_eq!(decoded_body, RESP_BODY, "hybrid egress body corrupted");
    eprintln!(
        "INC-1 HYBRID EVIDENCE: quiche::h3 ingress (poll/recv_body + its own \
         control/QPACK streams) COEXISTS with raw lb_h3 conn.stream_send egress \
         on the same connection → INC-2 = ingress-only (small, standalone), \
         egress restructure deferred to INC-3."
    );
}
