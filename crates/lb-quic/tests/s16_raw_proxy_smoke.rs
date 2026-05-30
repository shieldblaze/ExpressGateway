//! SESSION 16 / Mode B — B1 MINIMAL smoke test (author's self-check).
//!
//! Scope is deliberately narrow (author ≠ verifier): this proves the
//! NEW dedicated-dial path + the new public seam types, NOT the full
//! two-connections client⇄LB⇄backend wire proof or the H3-regression
//! suite (both are the VERIFIER's job, plan §5 / increment B1).
//!
//! What this asserts on the wire:
//!
//! 1. [`lb_io::quic_pool::QuicUpstreamPool::dial_dedicated`] reaches
//!    `is_established()` against a throwaway REAL quiche server, mirrors
//!    the requested ALPN onto the per-dial config, and returns a
//!    [`lb_io::quic_pool::DedicatedQuic`] whose socket is a fresh
//!    dedicated UDP socket (distinct local port). This exercises the
//!    R12-extracted `connect_and_drive` handshake loop that `dial_new`
//!    also uses.
//! 2. The new [`lb_quic::RawBackend`] seam type is constructible (the
//!    field threaded into `RouterParams` / `ActorParams`).

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use lb_io::quic_pool::{QuicPoolConfig, QuicUpstreamPool};
use tokio::net::UdpSocket;

const TEST_SNI: &str = "expressgateway.test";
const MAX_UDP: usize = 65_535;
const H3_ALPN: &[u8] = b"h3";

/// Generate an in-memory self-signed cert/key PEM pair (SAN = TEST_SNI,
/// serverAuth EKU) so BoringSSL's hostname verifier accepts the
/// loopback peer. Written to a unique temp dir; cleaned on drop.
struct TestCerts {
    dir: std::path::PathBuf,
    cert: std::path::PathBuf,
    key: std::path::PathBuf,
    ca: std::path::PathBuf,
}

impl Drop for TestCerts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn generate_loopback_certs() -> TestCerts {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir =
        std::env::temp_dir().join(format!("lb-quic-s16-smoke-{}-{nanos}", std::process::id()));
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
    std::fs::write(&cert_path, cert.pem().as_bytes()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem().as_bytes()).unwrap();
    std::fs::write(&ca_path, cert.pem().as_bytes()).unwrap();
    TestCerts {
        dir,
        cert: cert_path,
        key: key_path,
        ca: ca_path,
    }
}

/// Client-side config factory for the dedicated-dial pool (verifies the
/// throwaway server's cert against the generated CA).
fn client_config_factory(
    ca_path: std::path::PathBuf,
) -> Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> {
    Arc::new(move || {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
        // Factory installs a DIFFERENT ALPN on purpose so the test can
        // prove dial_dedicated's ALPN override actually takes effect (we
        // pass [h3] to dial_dedicated; the server only speaks h3).
        cfg.set_application_protos(&[b"factory-default-alpn"])?;
        let ca = ca_path.to_str().ok_or(quiche::Error::TlsFail)?;
        cfg.load_verify_locations_from_file(ca)
            .map_err(|_| quiche::Error::TlsFail)?;
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
        cfg.enable_dgram(true, 1024, 1024);
        Ok(cfg)
    })
}

fn server_config(certs: &TestCerts) -> quiche::Config {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    cfg.set_application_protos(&[H3_ALPN]).unwrap();
    cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
        .unwrap();
    cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
        .unwrap();
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
    cfg.enable_dgram(true, 1024, 1024);
    cfg
}

/// A throwaway server that accepts ONE connection and drives it to
/// established, then idles (keeps responding to keep-alives) until the
/// task is dropped. Returns the bound server address. No RETRY (this is
/// a plain backend the LB dials, not the LB's own listener).
async fn spawn_throwaway_server(certs: &TestCerts) -> SocketAddr {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = socket.local_addr().unwrap();
    let mut config = server_config(certs);

    tokio::spawn(async move {
        let mut in_buf = vec![0u8; MAX_UDP];
        let mut out_buf = vec![0u8; MAX_UDP];
        let mut conn: Option<quiche::Connection> = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);

        loop {
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            // Flush any pending outbound on the accepted conn.
            if let Some(c) = conn.as_mut() {
                loop {
                    match c.send(&mut out_buf) {
                        Ok((n, info)) => {
                            let _ = socket
                                .send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                                .await;
                        }
                        Err(quiche::Error::Done) => break,
                        Err(_) => break,
                    }
                }
            }
            let timeout = conn
                .as_ref()
                .and_then(quiche::Connection::timeout)
                .unwrap_or(Duration::from_millis(100));
            match tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)).await {
                Ok(Ok((n, from))) => {
                    if conn.is_none() {
                        let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
                        use ring::rand::SecureRandom;
                        ring::rand::SystemRandom::new().fill(&mut scid).unwrap();
                        let scid_ref = quiche::ConnectionId::from_ref(&scid);
                        let c = quiche::accept(&scid_ref, None, addr, from, &mut config).unwrap();
                        conn = Some(c);
                    }
                    if let Some(c) = conn.as_mut() {
                        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                        let info = quiche::RecvInfo { from, to: addr };
                        let _ = c.recv(slice, info);
                    }
                }
                Ok(Err(_)) | Err(_) => {
                    if let Some(c) = conn.as_mut() {
                        c.on_timeout();
                    }
                }
            }
        }
    });

    addr
}

/// B1 smoke: `dial_dedicated` reaches established against a real quiche
/// server, returns a dedicated socket, and mirrors the requested ALPN.
#[tokio::test]
async fn dial_dedicated_reaches_established_with_mirrored_alpn() {
    let certs = generate_loopback_certs();
    let server_addr = spawn_throwaway_server(&certs).await;

    let pool = QuicUpstreamPool::new(
        QuicPoolConfig::default(),
        client_config_factory(certs.ca.clone()),
    );

    // Mirror ALPN [h3] — overriding the factory's "factory-default-alpn"
    // (which the server does NOT speak; a non-override would TLS-fail).
    let alpn: &[&[u8]] = &[H3_ALPN];
    let dialed = tokio::time::timeout(
        Duration::from_secs(8),
        pool.dial_dedicated(server_addr, TEST_SNI, alpn),
    )
    .await
    .expect("dial_dedicated must not hang")
    .expect("dial_dedicated must reach established");

    assert!(
        dialed.conn.is_established(),
        "dedicated upstream connection must be established"
    );
    assert!(
        !dialed.conn.is_closed(),
        "dedicated upstream connection must be open"
    );
    assert_eq!(
        dialed.peer, server_addr,
        "DedicatedQuic.peer must be the dialed backend address"
    );
    // The dedicated socket is its OWN ephemeral loopback socket, distinct
    // from the server's (one upstream conn per client conn, plan §2.1).
    assert_ne!(
        dialed.local.port(),
        server_addr.port(),
        "dedicated dial must own a distinct UDP socket"
    );
    // ALPN mirroring took effect: the negotiated protocol is the one we
    // passed, NOT the factory default. (Empty until established; we are
    // established here.)
    assert_eq!(
        dialed.conn.application_proto(),
        H3_ALPN,
        "dial_dedicated must mirror the requested ALPN onto the wire"
    );
    // The fresh-dial counter bumped (parity with dial_new).
    assert_eq!(
        pool.fresh_dials(),
        1,
        "dial_dedicated must count as a fresh dial"
    );
    // It must NOT have been pooled (dedicated == not idle-parked).
    assert_eq!(
        pool.idle_count(),
        0,
        "dedicated dial must not insert into the idle pool"
    );
}

/// B1 smoke: the new `RawBackend` seam type is constructible (the field
/// threaded into `RouterParams` / `ActorParams`). Cheap-clone + Debug.
#[tokio::test]
async fn raw_backend_is_constructible_and_clone() {
    let certs = generate_loopback_certs();
    let pool = QuicUpstreamPool::new(
        QuicPoolConfig::default(),
        client_config_factory(certs.ca.clone()),
    );
    let backend = lb_quic::RawBackend {
        pool,
        addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 4433)),
        sni: TEST_SNI.to_string(),
        // B6 (R14/R12): caps now carried on RawBackend; the const
        // defaults keep these tests byte-identical in behaviour.
        dgram_queue_cap: lb_quic::DGRAM_QUEUE_CAP,
        max_relay_streams: lb_quic::MAX_RELAY_STREAMS,
    };
    let cloned = backend.clone();
    assert_eq!(cloned.addr, backend.addr);
    assert_eq!(cloned.sni, TEST_SNI);
    // Debug must not leak internals beyond addr/sni.
    let dbg = format!("{backend:?}");
    assert!(dbg.contains("RawBackend"), "Debug should name the type");
    assert!(dbg.contains(TEST_SNI), "Debug should show the sni");
}
