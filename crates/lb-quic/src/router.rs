//! Inbound packet router: owns one [`UdpSocket`] and dispatches packets by DCID to per-connection
//! [`ConnectionActor`] mpsc channels.
//!
//! For unknown CIDs: an Initial with **no** token gets a RETRY whose token is minted by
//! [`lb_security::RetryTokenSigner`]; an Initial with a **valid** token spawns an actor via
//! [`quiche::accept`] with the ODCID recovered from the token; an **invalid** token is dropped.
//! 0-RTT Initials gate through [`lb_security::ZeroRttReplayGuard::check_0rtt_token`].
//!
//! The per-connection channel is bounded; a `Full` `try_send` DROPS the packet, which QUIC
//! tolerates at the application level and is safer than blocking the UDP recv loop.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex as PlMutex;
use quiche::{ConnectionId, Header, Type};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;
use lb_security::{RetryTokenSigner, ZeroRttReplayGuard};

use crate::conn_actor::{ActorParams, InboundPacket, run_actor};

/// Channel depth per connection actor.
const ACTOR_CHANNEL_DEPTH: usize = 32;

/// Max UDP datagram we'll read or emit.
const MAX_UDP: usize = 65_535;

/// Construction parameters for [`InboundPacketRouter::spawn`].
pub struct RouterParams {
    /// Shared UDP socket the router owns; actor writes go back out through it.
    pub socket: Arc<UdpSocket>,
    /// Retry-token signer, built by the listener from the on-disk secret.
    pub retry_signer: Arc<RetryTokenSigner>,
    /// 0-RTT early-data replay guard.
    pub replay_guard: Arc<PlMutex<ZeroRttReplayGuard>>,
    /// Factory producing a fresh `quiche::Config` per connection — it is not `Sync`.
    pub config_factory: Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync>,
    /// Backend TCP pool for H1 backends.
    pub pool: TcpPool,
    /// Resolved backend addresses.
    pub backends: Arc<Vec<SocketAddr>>,
    /// Optional upstream H3 backend `(pool, addr, sni)`; takes precedence.
    pub h3_backend: Option<(QuicUpstreamPool, SocketAddr, String)>,
    /// Optional upstream H2 backend `(pool, addr)`.
    pub h2_backend: Option<(lb_io::http2_pool::Http2Pool, SocketAddr)>,
    /// Mode B seam. `Some` ⇒ every accepted connection goes to [`crate::raw_proxy`], not the H3 actor.
    pub raw_quic_backend: Option<crate::raw_proxy::RawBackend>,
    /// Mode B `quic_modeb_*` observability handles.
    pub quic_modeb_metrics: Option<lb_observability::QuicModeBMetrics>,
    /// WS-over-H3 Stage A: whether this listener accepts extended CONNECT.
    pub ws_enabled: bool,
    /// WS-over-H3 Stage C: the injected WebSocket relay launcher.
    pub ws_relay_launcher: Option<crate::ws_tunnel::WsRelayLauncher>,
    /// S36-A: per-connection H3 request cap, threaded to the actor.
    pub max_requests_per_h3_connection: u32,
    /// S36-A: the `h3_*` recycle metric handles.
    pub h3_recycle_metrics: Option<lb_observability::QuicH3RecycleMetrics>,
    /// Maximum concurrent QUIC connections; at the cap new Initials are DROPPED, so a
    /// memory-exhaustion attacker finds the bound finite while legitimate clients retry. Each
    /// connection occupies TWO dispatch entries, so the map cap is `2 * max_connections`.
    pub max_connections: usize,
    /// Listener-wide cancellation.
    pub cancel: CancellationToken,
}

/// Spawned handle for the router task. Dropping it does NOT stop the router — cancel the token.
pub struct RouterHandle {
    pub(crate) join: tokio::task::JoinHandle<()>,
}

impl RouterHandle {
    /// Await the router task's graceful exit.
    pub async fn join(self) -> Result<(), tokio::task::JoinError> {
        self.join.await
    }
}

/// Spawn the packet router.
#[must_use]
pub fn spawn(params: RouterParams) -> RouterHandle {
    let join = tokio::spawn(async move {
        if let Err(e) = Box::pin(router_main(params)).await {
            tracing::error!(error = %e, "QUIC router task exited with error");
        }
    });
    RouterHandle { join }
}

async fn router_main(params: RouterParams) -> std::io::Result<()> {
    let local_addr = params.socket.local_addr()?;
    let connections: Arc<dashmap::DashMap<Vec<u8>, mpsc::Sender<InboundPacket>>> =
        Arc::new(dashmap::DashMap::new());

    let mut in_buf = vec![0u8; MAX_UDP];

    loop {
        tokio::select! {
            biased;
            () = params.cancel.cancelled() => {
                tracing::debug!("QUIC router received shutdown signal");
                return Ok(());
            }
            r = params.socket.recv_from(&mut in_buf) => {
                let (n, peer) = match r {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::debug!(error = %e, "router recv_from");
                        continue;
                    }
                };
                if let Err(e) = dispatch_packet(
                    in_buf.get_mut(..n).unwrap_or(&mut []),
                    peer,
                    local_addr,
                    &params,
                    &connections,
                )
                .await
                {
                    tracing::debug!(error = %e, peer = %peer, "router dispatch");
                }
            }
        }
    }
}

async fn dispatch_packet(
    pkt: &mut [u8],
    peer: SocketAddr,
    local: SocketAddr,
    params: &RouterParams,
    connections: &Arc<dashmap::DashMap<Vec<u8>, mpsc::Sender<InboundPacket>>>,
) -> Result<(), String> {
    // Parse without consuming the buffer — `from_slice` copies the CID bytes it needs.
    let header = match Header::from_slice(pkt, quiche::MAX_CONN_ID_LEN) {
        Ok(h) => h,
        Err(e) => return Err(format!("header parse: {e}")),
    };
    let dcid_key: Vec<u8> = header.dcid.to_vec();

    // Short-header & any already-routed CID go to the actor directly.
    if let Some(sender) = connections.get(&dcid_key) {
        forward_to_actor(&sender, pkt.to_vec(), peer, local, connections, &dcid_key);
        return Ok(());
    }

    if header.ty != Type::Initial {
        return Ok(());
    }

    let token_nonempty = header.token.as_ref().is_some_and(|t| !t.is_empty());
    if !token_nonempty {
        return send_retry(&header, peer, local, params).await;
    }
    let token = header.token.as_ref().ok_or("unreachable: token_nonempty")?;
    let odcid_vec = match params.retry_signer.verify(token, peer, Instant::now()) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, "retry token verify failed");
            return Ok(());
        }
    };
    // 0-RTT replay check: an Initial carrying a valid retry token may also carry early data, so
    // it must clear the dedup guard before we accept.
    let replay_key = build_replay_key(&header, token);
    let replay_result = params.replay_guard.lock().check_0rtt_token(&replay_key);
    if let Err(e) = replay_result {
        tracing::debug!(error = %e, "0-RTT replay dropped");
        return Ok(());
    }
    spawn_new_connection(
        &header,
        &odcid_vec,
        pkt.to_vec(),
        peer,
        local,
        params,
        connections,
    )
}

/// Compose a replay-guard key from a client Initial whose retry token already verified. We
/// deliberately do NOT inspect the early-data payload (protected under a key this layer does not
/// have); SCID + token prefix is exactly what a replay attacker would have to duplicate.
fn build_replay_key(header: &Header<'_>, token: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(header.scid.len() + 32);
    key.extend_from_slice(&header.scid);
    let tail = token.get(..token.len().min(32)).unwrap_or(&[]);
    key.extend_from_slice(tail);
    key
}

/// Drop packets into the actor's mpsc. Channel-full → log + discard.
fn forward_to_actor(
    sender: &mpsc::Sender<InboundPacket>,
    data: Vec<u8>,
    from: SocketAddr,
    to: SocketAddr,
    connections: &Arc<dashmap::DashMap<Vec<u8>, mpsc::Sender<InboundPacket>>>,
    dcid_key: &[u8],
) {
    let pkt = InboundPacket { data, from, to };
    match sender.try_send(pkt) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::debug!("actor channel full, dropping packet");
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            connections.remove(dcid_key);
        }
    }
}

/// Emit a RETRY packet; the client retries with our minted token.
async fn send_retry(
    header: &Header<'_>,
    peer: SocketAddr,
    _local: SocketAddr,
    params: &RouterParams,
) -> Result<(), String> {
    let token = params.retry_signer.mint(peer, &header.dcid);

    // RFC 9000 §17.2.5: the RETRY's Source Connection ID MUST differ from the client's
    // destination CID, and the client MUST echo this as its next DCID.
    let new_scid_bytes = sample_conn_id();
    let new_scid = ConnectionId::from_ref(&new_scid_bytes);

    let mut out = vec![0u8; MAX_UDP];
    let written = match quiche::retry(
        &header.scid,
        &header.dcid,
        &new_scid,
        &token,
        header.version,
        &mut out,
    ) {
        Ok(n) => n,
        Err(e) => return Err(format!("quiche::retry: {e}")),
    };
    let bytes = out.get(..written).unwrap_or(&[]);
    if let Err(e) = params.socket.send_to(bytes, peer).await {
        return Err(format!("retry send_to: {e}"));
    }
    Ok(())
}

fn spawn_new_connection(
    header: &Header<'_>,
    odcid_bytes: &[u8],
    first_packet: Vec<u8>,
    peer: SocketAddr,
    local: SocketAddr,
    params: &RouterParams,
    connections: &Arc<dashmap::DashMap<Vec<u8>, mpsc::Sender<InboundPacket>>>,
) -> Result<(), String> {
    // Memory-DoS cap: each accepted connection adds TWO dispatch entries, so the bound is
    // `2 * max_connections`. At the cap, drop the Initial and let the peer retry.
    let cap_entries = params.max_connections.saturating_mul(2);
    if connections.len() >= cap_entries {
        tracing::warn!(
            current = connections.len(),
            cap = cap_entries,
            max_connections = params.max_connections,
            %peer,
            "QUIC router at connection cap; dropping new Initial"
        );
        return Err("router at max_connections".to_owned());
    }
    let scid_bytes = sample_conn_id();
    let scid = ConnectionId::from_ref(&scid_bytes);
    let odcid = ConnectionId::from_ref(odcid_bytes);
    // `retry_source_cid` MUST match the SCID the server sent in the RETRY; the client's
    // second-Initial DCID is that value, and quiche explicitly blesses using it here.
    let retry_src_dcid = ConnectionId::from_ref(&header.dcid);

    let mut config = (params.config_factory)().map_err(|e| format!("config_factory: {e}"))?;
    let conn = quiche::accept_with_retry(
        &scid,
        quiche::RetryConnectionIds {
            original_destination_cid: &odcid,
            retry_source_cid: &retry_src_dcid,
        },
        local,
        peer,
        &mut config,
    )
    .map_err(|e| format!("quiche::accept_with_retry: {e}"))?;

    let (tx, rx) = mpsc::channel::<InboundPacket>(ACTOR_CHANNEL_DEPTH);
    // Register the new SCID AND the header's original DCID: the client's next few flights may
    // still use the original until it learns the server's SCID.
    let router_key: Vec<u8> = scid_bytes.to_vec();
    connections.insert(router_key.clone(), tx.clone());
    let header_dcid_key: Vec<u8> = header.dcid.to_vec();
    connections.insert(header_dcid_key.clone(), tx.clone());

    let _ = tx.try_send(InboundPacket {
        data: first_packet,
        from: peer,
        to: local,
    });

    let actor = ActorParams {
        conn,
        socket: Arc::clone(&params.socket),
        inbound: rx,
        cancel: params.cancel.clone(),
        pool: params.pool.clone(),
        backends: Arc::clone(&params.backends),
        h3_backend: params.h3_backend.clone(),
        h2_backend: params.h2_backend.clone(),
        raw_quic_backend: params.raw_quic_backend.clone(),
        quic_modeb_metrics: params.quic_modeb_metrics.clone(),
        ws_enabled: params.ws_enabled,
        ws_relay_launcher: params.ws_relay_launcher.clone(),
        max_requests_per_h3_connection: params.max_requests_per_h3_connection,
        h3_recycle_metrics: params.h3_recycle_metrics.clone(),
    };
    // CODE-2-08: wrap the two DashMap entries in a `CidEntryGuard` so cleanup runs unconditionally
    // — clean exit, async-cancel future-drop, OR panic unwind. Pre-fix the explicit
    // `connections.remove(...)` calls below the await were skipped on unwind, pinning two entries
    // per panicked actor. Dead code under `panic = "abort"`; kept for dev/test, where unwind is preserved.
    let guard = crate::cleanup_guard::CidEntryGuard::new(
        Arc::clone(connections),
        router_key,
        header_dcid_key,
    );
    tokio::spawn(async move {
        // Move the guard into the task so Drop runs when the future ends, including on cancel-drop.
        let _guard = guard;
        let _ = Box::pin(run_actor(actor)).await;
    });
    Ok(())
}

fn sample_conn_id() -> [u8; quiche::MAX_CONN_ID_LEN] {
    use ring::rand::SecureRandom;
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    if ring::rand::SystemRandom::new().fill(&mut scid).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        for (i, b) in scid.iter_mut().enumerate() {
            let idx = u32::try_from(i).unwrap_or(0);
            #[allow(clippy::cast_possible_truncation)]
            {
                *b = (nanos
                    .wrapping_mul(0x9E37_79B9)
                    .wrapping_add(idx.wrapping_mul(0x0100_0193))
                    & 0xFF) as u8;
            }
        }
    }
    scid
}

#[cfg(test)]
mod tests {
    //! TEST-001: the cap-drop branch in [`spawn_new_connection`] had no dedicated test.
    use super::*;
    use lb_io::Runtime;
    use lb_io::pool::PoolConfig;
    use lb_io::sockopts::BackendSockOpts;
    use std::net::Ipv4Addr;

    /// Drives the `connections.len() >= max_connections * 2` branch with a `config_factory` that
    /// MUST NEVER be called — a call would mean the cap-check was skipped.
    #[tokio::test]
    async fn router_drops_initial_when_cap_reached() {
        let socket = Arc::new(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .expect("bind udp"),
        );
        let local = socket.local_addr().expect("local_addr");
        let peer: SocketAddr = "127.0.0.1:65535".parse().expect("parse peer");

        let retry_signer = Arc::new(RetryTokenSigner::new_with_secret([0xa5u8; 32]));
        let replay_guard = Arc::new(PlMutex::new(ZeroRttReplayGuard::new(64)));

        // Fails loudly: reaching the factory means the cap-check fell through.
        let config_factory: Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> =
            Arc::new(|| Err(quiche::Error::TlsFail));

        let runtime = Runtime::new();
        let pool = TcpPool::new(PoolConfig::default(), BackendSockOpts::default(), runtime);

        let params = RouterParams {
            socket,
            retry_signer,
            replay_guard,
            config_factory,
            pool,
            backends: Arc::new(Vec::new()),
            h3_backend: None,
            h2_backend: None,
            raw_quic_backend: None,
            quic_modeb_metrics: None,
            ws_enabled: false,
            ws_relay_launcher: None,
            max_requests_per_h3_connection: 0,
            h3_recycle_metrics: None,
            // Reduced cap so the dashmap only needs 4 entries to be full.
            max_connections: 2,
            cancel: CancellationToken::new(),
        };

        let connections: Arc<dashmap::DashMap<Vec<u8>, mpsc::Sender<InboundPacket>>> =
            Arc::new(dashmap::DashMap::new());
        for i in 0u8..4 {
            let (tx, _rx) = mpsc::channel::<InboundPacket>(1);
            connections.insert(vec![i; 8], tx);
        }
        assert_eq!(
            connections.len(),
            4,
            "fixture: dashmap should be at 2 * max_connections == 4"
        );

        let mut client_cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).expect("client cfg new");
        client_cfg
            .set_application_protos(&[crate::LB_QUIC_TEST_ALPN])
            .expect("alpn");
        client_cfg.verify_peer(false);
        client_cfg.set_max_idle_timeout(5_000);
        client_cfg.set_max_recv_udp_payload_size(1_350);
        client_cfg.set_max_send_udp_payload_size(1_350);
        client_cfg.set_initial_max_data(1024);
        client_cfg.set_initial_max_stream_data_bidi_local(1024);
        client_cfg.set_initial_max_streams_bidi(1);
        client_cfg.set_disable_active_migration(true);

        let client_scid_bytes = sample_conn_id();
        let client_scid = ConnectionId::from_ref(&client_scid_bytes);
        let mut conn = quiche::connect(Some("test"), &client_scid, peer, local, &mut client_cfg)
            .expect("quiche::connect");
        let mut send_buf = vec![0u8; MAX_UDP];
        let (n, _info) = conn.send(&mut send_buf).expect("client conn.send");

        // Clone the wire bytes BEFORE parsing — the header borrows from them.
        let first_packet = send_buf.get(..n).unwrap_or(&[]).to_vec();
        let header_buf = send_buf.get_mut(..n).unwrap_or(&mut []);
        let header = Header::from_slice(header_buf, quiche::MAX_CONN_ID_LEN).expect("parse header");
        assert_eq!(header.ty, Type::Initial, "wire pkt should be Initial");
        let odcid = header.dcid.to_vec();

        let result = spawn_new_connection(
            &header,
            &odcid,
            first_packet,
            peer,
            local,
            &params,
            &connections,
        );

        // The cap-drop branch returns Err with this exact message.
        match result {
            Err(msg) => assert_eq!(
                msg, "router at max_connections",
                "expected cap-drop Err, got a different error: {msg}"
            ),
            Ok(()) => panic!("expected cap-drop Err, but spawn_new_connection returned Ok"),
        }
        // The table size must be unchanged — the early-return is the point.
        assert_eq!(
            connections.len(),
            4,
            "cap-drop must not grow the dispatch table"
        );
    }

    /// A minimal but REAL server config, so `accept_with_retry` completes rather than stubbing.
    fn flood_server_config_factory()
    -> Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> {
        let mut params =
            rcgen::CertificateParams::new(vec!["flood.test".to_string()]).expect("cert params");
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params
            .extended_key_usages
            .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
        let key_pair = rcgen::KeyPair::generate().expect("keypair");
        let cert = params.self_signed(&key_pair).expect("self-signed");
        let cert_pem = cert.pem();
        let key_pem = key_pair.serialize_pem();
        Arc::new(move || {
            let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
            cfg.set_application_protos(&[crate::LB_QUIC_TEST_ALPN])?;
            cfg.load_cert_chain_from_pem_file(&write_tmp(&cert_pem, "cert"))
                .map_err(|_| quiche::Error::TlsFail)?;
            cfg.load_priv_key_from_pem_file(&write_tmp(&key_pem, "key"))
                .map_err(|_| quiche::Error::TlsFail)?;
            // Long idle timeout so admitted actors survive the whole flood.
            cfg.set_max_idle_timeout(30_000);
            cfg.set_max_recv_udp_payload_size(1_350);
            cfg.set_max_send_udp_payload_size(1_350);
            cfg.set_initial_max_data(1024);
            cfg.set_initial_max_stream_data_bidi_local(1024);
            cfg.set_initial_max_streams_bidi(1);
            cfg.set_disable_active_migration(true);
            Ok(cfg)
        })
    }

    /// Write `pem` to a unique temp file and return its path.
    fn write_tmp(pem: &str, kind: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "lb-quic-s19-b5-flood-{kind}-{}-{nanos}-{seq}.pem",
            std::process::id()
        ));
        std::fs::write(&path, pem.as_bytes()).expect("write tmp pem");
        path.to_string_lossy().into_owned()
    }

    /// Mint a fresh, DISTINCT real Initial (its own random SCID, hence a distinct DCID per call).
    fn mint_distinct_initial() -> Vec<u8> {
        let local: SocketAddr = "127.0.0.1:4433".parse().expect("local");
        let mut client_cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).expect("client cfg");
        client_cfg
            .set_application_protos(&[crate::LB_QUIC_TEST_ALPN])
            .expect("alpn");
        client_cfg.verify_peer(false);
        client_cfg.set_max_idle_timeout(5_000);
        client_cfg.set_max_recv_udp_payload_size(1_350);
        client_cfg.set_max_send_udp_payload_size(1_350);
        client_cfg.set_initial_max_data(1024);
        client_cfg.set_initial_max_stream_data_bidi_local(1024);
        client_cfg.set_initial_max_streams_bidi(1);
        client_cfg.set_disable_active_migration(true);

        let scid_bytes = sample_conn_id();
        let scid = ConnectionId::from_ref(&scid_bytes);
        let mut conn =
            quiche::connect(Some("test"), &scid, local, local, &mut client_cfg).expect("connect");
        let mut send_buf = vec![0u8; MAX_UDP];
        let (n, _info) = conn.send(&mut send_buf).expect("client send");
        send_buf.get(..n).unwrap_or(&[]).to_vec()
    }

    /// S19 B5 — flood an EMPTY router with distinct Initials under a small `max_connections`: the
    /// table must fill to EXACTLY `2 * max_connections` and every further Initial is DROPPED.
    /// Load-bearing: this exercises the admit→saturate→drop TRANSITION. Remove the
    /// `connections.len() >= cap_entries` guard and the table grows to `2 * FLOOD`.
    #[tokio::test]
    async fn router_drops_flood_of_distinct_initials_at_cap() {
        const MAX_CONNECTIONS: usize = 4;
        const CAP_ENTRIES: usize = MAX_CONNECTIONS * 2; // 8
        const FLOOD: usize = 40; // ≫ MAX_CONNECTIONS so the cap is crossed

        let socket = Arc::new(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .expect("bind udp"),
        );

        let retry_signer = Arc::new(RetryTokenSigner::new_with_secret([0x5au8; 32]));
        let replay_guard = Arc::new(PlMutex::new(ZeroRttReplayGuard::new(64)));
        let runtime = Runtime::new();
        let pool = TcpPool::new(PoolConfig::default(), BackendSockOpts::default(), runtime);

        let cancel = CancellationToken::new();
        let params = RouterParams {
            socket,
            retry_signer,
            replay_guard,
            config_factory: flood_server_config_factory(),
            pool,
            backends: Arc::new(Vec::new()),
            h3_backend: None,
            h2_backend: None,
            raw_quic_backend: None,
            quic_modeb_metrics: None,
            ws_enabled: false,
            ws_relay_launcher: None,
            max_requests_per_h3_connection: 0,
            h3_recycle_metrics: None,
            max_connections: MAX_CONNECTIONS,
            cancel: cancel.clone(),
        };

        let connections: Arc<dashmap::DashMap<Vec<u8>, mpsc::Sender<InboundPacket>>> =
            Arc::new(dashmap::DashMap::new());

        let peer: SocketAddr = "127.0.0.1:4433".parse().expect("peer");
        let local = peer;
        let mut admitted = 0usize;
        let mut dropped = 0usize;
        for _ in 0..FLOOD {
            let wire = mint_distinct_initial();
            let first_packet = wire.clone();
            let mut hdr_buf = wire;
            let header =
                Header::from_slice(&mut hdr_buf, quiche::MAX_CONN_ID_LEN).expect("parse header");
            let odcid = header.dcid.to_vec();
            let before = connections.len();
            let result = spawn_new_connection(
                &header,
                &odcid,
                first_packet,
                peer,
                local,
                &params,
                &connections,
            );
            match result {
                Ok(()) => {
                    admitted += 1;
                    // Each admit inserts EXACTLY two entries.
                    assert_eq!(
                        connections.len(),
                        before + 2,
                        "an admitted connection must add exactly 2 dispatch entries"
                    );
                }
                Err(msg) => {
                    dropped += 1;
                    assert_eq!(
                        msg, "router at max_connections",
                        "an over-cap Initial must be dropped with the cap-drop Err"
                    );
                    // A drop must NOT change the table.
                    assert_eq!(
                        connections.len(),
                        before,
                        "a cap-drop must not grow the dispatch table"
                    );
                }
            }
            // THE BOUND: the table must NEVER exceed 2 * max_connections.
            assert!(
                connections.len() <= CAP_ENTRIES,
                "dispatch table ({}) must never exceed 2 * max_connections ({CAP_ENTRIES})",
                connections.len()
            );
        }

        // Exactly MAX_CONNECTIONS admitted, the rest dropped at the bound.
        assert_eq!(
            connections.len(),
            CAP_ENTRIES,
            "the table must be saturated at exactly 2 * max_connections"
        );
        assert_eq!(
            admitted, MAX_CONNECTIONS,
            "exactly max_connections Initials may be admitted"
        );
        assert_eq!(
            dropped,
            FLOOD - MAX_CONNECTIONS,
            "every Initial beyond the cap must be dropped"
        );

        cancel.cancel();
    }
}
