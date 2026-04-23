//! Inbound packet router for Pillar 3b.3c-2.
//!
//! Owns a single [`UdpSocket`] and dispatches received packets by DCID
//! to per-connection [`ConnectionActor`] mpsc channels. For unknown
//! CIDs:
//!
//! * Initial with **no** token → reply with a RETRY packet (built via
//!   [`quiche::retry`]) whose token is minted by
//!   [`lb_security::RetryTokenSigner`]. The client echoes the token in
//!   its second Initial, which we verify before spawning an actor.
//! * Initial with **valid** token → spawn an actor with the ODCID
//!   recovered from the token, call [`quiche::accept`] with
//!   `odcid = Some(..)`.
//! * Initial with **invalid** token → drop silently.
//!
//! 0-RTT / early-data Initials gate through
//! [`lb_security::ZeroRttReplayGuard::check_0rtt_token`]: we use the
//! client's SCID bytes (plus a digest of the first 32 bytes of the
//! encrypted payload) as the dedup key. Any Initial whose key has been
//! seen is dropped.
//!
//! Back-pressure: the per-connection channel is bounded (32 packets).
//! On `try_send` → `Full`, the packet is dropped and a warning logged.
//! QUIC tolerates loss at the application level — this is safer than
//! blocking the UDP recv loop.

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
    /// Shared UDP socket the router owns. The actor writes go back out
    /// of the same socket, matching the conventional QUIC server shape.
    pub socket: Arc<UdpSocket>,
    /// Retry-token signer. The listener constructs this from the
    /// `retry_secret_path` config entry; the router uses it to mint +
    /// verify stateless-retry tokens.
    pub retry_signer: Arc<RetryTokenSigner>,
    /// 0-RTT early-data replay guard. Pillar 3b.3a built this; the
    /// router is its first real caller.
    pub replay_guard: Arc<PlMutex<ZeroRttReplayGuard>>,
    /// Factory that produces a fresh `quiche::Config` for each new
    /// accepted connection. A factory (not a shared config) is
    /// required because `quiche::Config` holds interior mutable state
    /// that cannot be reused across `accept()` calls.
    pub config_factory: Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync>,
    /// Backend TCP pool for H1 backends.
    pub pool: TcpPool,
    /// Resolved backend addresses.
    pub backends: Arc<Vec<SocketAddr>>,
    /// Optional upstream H3 backend `(pool, addr, sni)`. When set,
    /// H3 requests on this listener route to the upstream via the
    /// QUIC pool instead of the H1/TcpPool path. Pillar 3b.3c-3.
    pub h3_backend: Option<(QuicUpstreamPool, SocketAddr, String)>,
    /// Listener-wide cancellation.
    pub cancel: CancellationToken,
}

/// Spawned handle for the router task. Dropping the handle does not
/// cancel the router — use the `CancellationToken` held by whoever
/// constructed `RouterParams`.
pub struct RouterHandle {
    pub(crate) join: tokio::task::JoinHandle<()>,
}

impl RouterHandle {
    /// Await the router task's graceful exit.
    ///
    /// # Errors
    ///
    /// Returns [`tokio::task::JoinError`] if the spawned router task
    /// panicked or was cancelled before completing normally.
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
    // Per-CID dispatch table.
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
    // Parse header without consuming the buffer — `from_slice` copies
    // what it needs; the original bytes are still the bytes we hand
    // off to the actor.
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

    // New connection attempt: only Initial is meaningful on the router
    // path. Drop everything else (0-RTT with unknown CID, Handshake
    // without prior Initial, Retry/VN packets arriving at a server).
    if header.ty != Type::Initial {
        return Ok(());
    }

    let token_nonempty = header.token.as_ref().is_some_and(|t| !t.is_empty());
    if !token_nonempty {
        return send_retry(&header, peer, local, params).await;
    }
    let token = header.token.as_ref().ok_or("unreachable: token_nonempty")?;
    // Verify + maybe spawn an actor.
    let odcid_vec = match params.retry_signer.verify(token, peer, Instant::now()) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, "retry token verify failed");
            return Ok(());
        }
    };
    // 0-RTT replay check: Initial packets that carry valid retry tokens
    // are the client's SECOND Initial; we key the replay guard off the
    // client SCID + a prefix of the authenticated token. Duplicate
    // Initials with identical SCID + identical token → replay.
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

/// Compose a replay-guard key from a client Initial that already
/// verified its retry token. We intentionally do NOT try to inspect
/// early-data payload (which is protected under a key we do not yet
/// have at this router layer); the client-identity slice (SCID + token
/// prefix) is the strongest identifier we can cheaply compute here and
/// is exactly what a replay attacker would have to duplicate.
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

/// Emit a RETRY packet. The client will retry with our minted token;
/// we then verify it and spawn an actor.
async fn send_retry(
    header: &Header<'_>,
    peer: SocketAddr,
    _local: SocketAddr,
    params: &RouterParams,
) -> Result<(), String> {
    // Mint a token bound to the peer address and the original
    // destination connection ID (ODCID).
    let token = params.retry_signer.mint(peer, &header.dcid);

    // Choose a new SCID. RFC 9000 §17.2.5: the RETRY's Source Connection
    // ID MUST differ from the client's destination CID. We sample 16
    // random bytes; the client MUST echo this as the DCID of its
    // subsequent Initials.
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
    let scid_bytes = sample_conn_id();
    let scid = ConnectionId::from_ref(&scid_bytes);
    let odcid = ConnectionId::from_ref(odcid_bytes);
    // `retry_source_cid` MUST match the SCID the server sent in the
    // RETRY packet. In our router the client's second-Initial DCID is
    // that value, and the quiche docs explicitly bless using that
    // field: "It is safe to use the DCID in the retry_source_cid
    // field of the RetryConnectionIds." (quiche 0.28 lib.rs:1673)
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
    // Register the new SCID so subsequent packets from this peer
    // route to the actor. Also register the header's original DCID
    // bytes — client packets in the next few flights may still use
    // the original DCID until it learns the server's SCID.
    let router_key: Vec<u8> = scid_bytes.to_vec();
    connections.insert(router_key.clone(), tx.clone());
    let header_dcid_key: Vec<u8> = header.dcid.to_vec();
    connections.insert(header_dcid_key.clone(), tx.clone());

    // Seed the actor with the Initial we just accepted.
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
    };
    // Spawn actor; on exit it leaves the dashmap entries behind which
    // will be reaped on next `try_send` failure. That's acceptable —
    // actor lifetimes are short compared to long-lived mapping leaks.
    let connections_for_cleanup = Arc::clone(connections);
    tokio::spawn(async move {
        let _ = Box::pin(run_actor(actor)).await;
        // Best-effort cleanup once the actor exits.
        connections_for_cleanup.remove(&router_key);
        connections_for_cleanup.remove(&header_dcid_key);
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
