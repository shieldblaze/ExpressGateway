//! QUIC transport layer backed by [`quiche`] 0.28 over `BoringSSL`.
//!
//! This crate exposes two layers:
//!
//! 1. The typed data model — [`QuicDatagram`] and [`QuicStream`] — that
//!    the rest of the gateway (L7 pipeline, HTTP/3 codec, observability)
//!    passes around. The shapes are transport-independent and stable
//!    across the Pillar 3a (quinn) → Pillar 3b (quiche) migration.
//! 2. A real UDP + TLS 1.3 transport, hosted inside [`QuicEndpoint`],
//!    with [`roundtrip_datagram`] and [`roundtrip_stream`] exercising
//!    quiche's unreliable-datagram and unidirectional-stream APIs
//!    end-to-end. These are the functions that drive the manifest-locked
//!    `tests/quic_native.rs` coverage.
//!
//! The free functions [`forward_datagram`] and [`forward_stream`] remain
//! as thin synchronous validators: they do **no** network I/O. They
//! guard the typed model against empty payloads and return a clone.
//!
//! ## Stack
//!
//! Pillar 3b.1 migrated this crate from quinn 0.11 + rustls/ring to
//! [quiche] 0.28 + `BoringSSL`, tracking the decision in
//! `docs/decisions/quinn-to-quiche-migration.md`. The rationale is
//! alignment with Cloudflare's production HTTP/3 stack, measurable
//! throughput advantage over quinn, and vendor-scale CVE response via
//! Cloudflare's networking team. `BoringSSL` links alongside rustls/ring
//! (which remains in use on the TLS-over-TCP listener path and by the
//! Step 5b `TicketRotator`) — the same rustls + `BoringSSL` pairing that
//! Pingora ships in production.
//!
//! [`tokio_quiche::ConnectionParams`] is re-exported so callers wiring
//! Pillar 3b.2's listener into `crates/lb/src/main.rs` can reach the
//! shared configuration without depending on tokio-quiche directly.
//!
//! ## Security hardening (Pillar 3b.3a)
//!
//! * The loopback client path now honors `verify_peer(true)` and
//!   loads the trust anchor via
//!   `quiche::Config::load_verify_locations_from_file`; the tests
//!   build a proper self-signed cert with SAN + `serverAuth` EKU so
//!   `BoringSSL`'s hostname verifier accepts it. The Pillar 3b.1
//!   `verify_peer(false)` workaround is gone.
//! * [`QuicEndpoint::with_retry_signer`] installs a
//!   [`lb_security::RetryTokenSigner`]. quiche 0.28 does not expose
//!   `Config::enable_retry(bool)` — the RETRY handshake is driven by
//!   the application via the free function [`quiche::retry`] in a
//!   custom accept loop. The signer is stored on the endpoint as the
//!   public surface for operator secret rotation; Pillar 3b.3c's
//!   custom accept loop consumes it against the on-wire flow per
//!   RFC 9000 §8.1.3.
//! * [`QuicEndpoint::with_replay_filter`] installs a
//!   [`lb_security::ZeroRttReplayGuard`]. The filter exposes
//!   [`check_0rtt_token`](lb_security::ZeroRttReplayGuard::check_0rtt_token),
//!   which the Pillar 3b.3c accept loop will call before handing any
//!   0-RTT early-data bytes to the application.
//!
//! Deferred to later Pillar 3b steps: Alt-Svc injection on H2/H1
//! responses (3b.3b); CID-routed upstream pool + wiring the QUIC
//! listener into `crates/lb/src/main.rs` (3b.3c); `curl --http3` and
//! `h3i` interop (3b.4).
//!
//! [quiche]: https://github.com/cloudflare/quiche
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![warn(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex as PlMutex;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

pub use lb_security::{RetryTokenSigner, ZeroRttReplayGuard};

/// Re-exported from `tokio-quiche`. Pillar 3b.2 wires a listener in the
/// root binary that consumes a [`ConnectionParams`]; lifting the symbol
/// here keeps downstream crates decoupled from `tokio-quiche` versioning.
pub use tokio_quiche::ConnectionParams;

/// ALPN identifier advertised by the built-in [`QuicEndpoint`] helpers.
///
/// Real HTTP/3 listeners will advertise `h3` (see RFC 9114); this value
/// exists for the Pillar 3b.1 loopback transport test. Pillar 3b.2 will
/// upgrade this to a proper ALPN policy driven by the control plane.
pub const LB_QUIC_ALPN: &[u8] = b"lb-quic";

/// SNI the loopback client presents.
///
/// Pillar 3b.3a turned client peer-cert verification back on;
/// `BoringSSL`'s hostname verifier rejects an iPAddress-type SAN even
/// with a `serverAuth` EKU, so the loopback test cert uses a DNS SAN
/// of `expressgateway.test` and the client sends that name as SNI
/// while still targeting `SocketAddr(127.0.0.1, <port>)`. Pillar
/// 3b.3c will accept an SNI override via the endpoint builder when
/// the real listener lands; for now this constant is the default.
pub const LB_QUIC_TEST_SNI: &str = "expressgateway.test";

/// Maximum size of one datagram we accept over the UDP socket.
const MAX_UDP_DATAGRAM_SIZE: usize = 65_535;

/// Budget for how long the loopback driver will keep spinning before
/// treating a test as hung. Loopback handshake + one-shot roundtrip
/// completes well under 200 ms on idle hardware.
const LOOPBACK_DRIVER_BUDGET: Duration = Duration::from_secs(5);

/// Safely take the first `n` bytes of `buf`, returning `&[]` if `n`
/// exceeds the slice length. Avoids `clippy::indexing_slicing` panics
/// while remaining a no-op on the hot path where `n <= buf.len()` by
/// construction.
fn prefix(buf: &[u8], n: usize) -> &[u8] {
    buf.get(..n).unwrap_or(&[])
}

/// Errors from the QUIC layer.
#[derive(Debug, thiserror::Error)]
pub enum QuicError {
    /// Datagram payload is empty.
    #[error("empty datagram payload")]
    EmptyPayload,

    /// Stream data is empty.
    #[error("empty stream data")]
    EmptyStreamData,

    /// Invalid connection ID.
    #[error("invalid connection id: {0}")]
    InvalidConnectionId(u64),

    /// I/O error binding or driving the underlying UDP socket / endpoint.
    #[error("quic i/o error: {0}")]
    Io(#[from] io::Error),

    /// Error from the underlying quiche state machine (handshake,
    /// framing, flow control, TLS, datagram / stream I/O). Wraps
    /// [`quiche::Error`].
    #[error("quiche error: {0}")]
    Quiche(#[from] quiche::Error),

    /// No incoming connection was accepted before the driver budget ran
    /// out.
    #[error("no incoming connection")]
    NoIncoming,

    /// The driver observed no progress for longer than
    /// [`LOOPBACK_DRIVER_BUDGET`]. In production this never fires; it
    /// is a defensive test-harness guard.
    #[error("quic driver timed out making progress")]
    DriverTimeout,

    /// Internal task-join failure (server accept task panicked or was
    /// cancelled). Should not occur in practice.
    #[error("quic internal error: {0}")]
    Internal(String),
}

/// QUIC datagram — a connection-scoped, unreliable, bounded payload.
#[derive(Debug, Clone)]
pub struct QuicDatagram {
    /// Connection ID this datagram belongs to. quiche datagrams do not
    /// carry a per-datagram connection identifier on the wire, so this
    /// field is caller-tracked: [`roundtrip_datagram`] returns the
    /// original `connection_id` verbatim on the echoed struct.
    pub connection_id: u64,
    /// Raw payload bytes.
    pub data: Vec<u8>,
}

/// QUIC stream frame — a unidirectional stream slice with a FIN marker.
#[derive(Debug, Clone)]
pub struct QuicStream {
    /// Stream ID within the connection.
    pub stream_id: u64,
    /// Stream payload bytes.
    pub data: Vec<u8>,
    /// Whether this is the final frame on the stream.
    pub fin: bool,
}

/// Back-compat synchronous datagram validator. Does **no** network I/O.
/// Use [`roundtrip_datagram`] for real transport.
///
/// # Errors
///
/// Returns [`QuicError::EmptyPayload`] if the datagram has no data.
pub fn forward_datagram(dg: &QuicDatagram) -> Result<QuicDatagram, QuicError> {
    if dg.data.is_empty() {
        return Err(QuicError::EmptyPayload);
    }
    Ok(dg.clone())
}

/// Back-compat synchronous stream-frame validator. Does **no** network
/// I/O. Use [`roundtrip_stream`] for real transport.
///
/// # Errors
///
/// Returns [`QuicError::EmptyStreamData`] if the stream has no data.
pub fn forward_stream(stream: &QuicStream) -> Result<QuicStream, QuicError> {
    if stream.data.is_empty() {
        return Err(QuicError::EmptyStreamData);
    }
    Ok(stream.clone())
}

/// Role of a [`QuicEndpoint`]: either a server that accepts inbound
/// connections, or a client that initiates outbound ones.
#[allow(clippy::pub_underscore_fields)]
enum Role {
    Server {
        cert_pem_path: PathBuf,
        key_pem_path: PathBuf,
    },
    Client {
        ca_pem_path: PathBuf,
    },
}

/// A quiche-backed QUIC endpoint bound to `127.0.0.1`.
///
/// Owns the UDP socket, the material needed to build a fresh
/// [`quiche::Config`] for one connection, and (for server endpoints)
/// the optional [`RetryTokenSigner`] + [`ZeroRttReplayGuard`]
/// configured via the [`with_retry_signer`](Self::with_retry_signer)
/// and [`with_replay_filter`](Self::with_replay_filter) builder
/// methods.
pub struct QuicEndpoint {
    socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    role: Role,
    retry_signer: Option<Arc<RetryTokenSigner>>,
    replay_filter: Option<Arc<PlMutex<ZeroRttReplayGuard>>>,
}

impl std::fmt::Debug for QuicEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let role = match &self.role {
            Role::Server { .. } => "server",
            Role::Client { .. } => "client",
        };
        f.debug_struct("QuicEndpoint")
            .field("local_addr", &self.local_addr)
            .field("role", &role)
            .finish_non_exhaustive()
    }
}

impl QuicEndpoint {
    /// Build a server endpoint bound to an ephemeral `127.0.0.1` port.
    /// `cert_pem_path` must point at a PEM-encoded certificate chain
    /// and `key_pem_path` at its PKCS#8 PEM private key — quiche's
    /// `BoringSSL` context loads both by filesystem path.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] if the UDP socket fails to bind.
    pub async fn server_on_loopback(
        cert_pem_path: PathBuf,
        key_pem_path: PathBuf,
    ) -> io::Result<Self> {
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
        let socket = UdpSocket::bind(addr).await?;
        let local_addr = socket.local_addr()?;
        Ok(Self {
            socket: Arc::new(socket),
            local_addr,
            role: Role::Server {
                cert_pem_path,
                key_pem_path,
            },
            retry_signer: None,
            replay_filter: None,
        })
    }

    /// Build a client endpoint bound to an ephemeral `127.0.0.1` port.
    /// `ca_pem_path` must point at a PEM-encoded CA bundle trusted for
    /// peer-certificate verification.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] if the UDP socket fails to bind.
    pub async fn client_on_loopback(ca_pem_path: PathBuf) -> io::Result<Self> {
        let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
        let socket = UdpSocket::bind(addr).await?;
        let local_addr = socket.local_addr()?;
        Ok(Self {
            retry_signer: None,
            replay_filter: None,
            socket: Arc::new(socket),
            local_addr,
            role: Role::Client { ca_pem_path },
        })
    }

    /// The socket address the endpoint is bound to.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Install a [`RetryTokenSigner`] on this (server) endpoint.
    ///
    /// The signer is stored so that — once the listener moves to a
    /// custom accept loop in Pillar 3b.3c — the RETRY-packet wire
    /// handling can mint and verify tokens against a secret that the
    /// operator rotates. quiche 0.28 does not ship a
    /// `Config::enable_retry(bool)` toggle, so there is no
    /// built-in path to plug into today; the signer is the stable
    /// public surface (unit-tested in `lb_security::retry`) and the
    /// wire integration lands with the custom accept loop.
    #[must_use]
    pub fn with_retry_signer(mut self, signer: Arc<RetryTokenSigner>) -> Self {
        self.retry_signer = Some(signer);
        self
    }

    /// Install a [`ZeroRttReplayGuard`] on this (server) endpoint.
    ///
    /// The filter is held so the server accept path can call
    /// [`ZeroRttReplayGuard::check_0rtt_token`] before handing any
    /// 0-RTT early-data bytes to the application. Pillar 3b.3a ships
    /// the wiring seam; a custom accept loop that feeds the filter
    /// each 0-RTT token lands in Pillar 3b.3c alongside the real QUIC
    /// listener.
    #[must_use]
    pub fn with_replay_filter(mut self, filter: Arc<PlMutex<ZeroRttReplayGuard>>) -> Self {
        self.replay_filter = Some(filter);
        self
    }

    /// Access the installed retry signer, if any. Used by integration
    /// tests and upcoming Pillar 3b.3c wiring.
    #[must_use]
    pub fn retry_signer(&self) -> Option<Arc<RetryTokenSigner>> {
        self.retry_signer.as_ref().map(Arc::clone)
    }

    /// Access the installed replay filter, if any. Used by integration
    /// tests and upcoming Pillar 3b.3c wiring.
    #[must_use]
    pub fn replay_filter(&self) -> Option<Arc<PlMutex<ZeroRttReplayGuard>>> {
        self.replay_filter.as_ref().map(Arc::clone)
    }
}

/// Build a fresh `quiche::Config` for the endpoint's role with the
/// transport parameters our loopback tests require.
///
/// The peer-cert verification posture is the usual server-TLS
/// default: servers do not verify clients (no mTLS yet), clients DO
/// verify servers against the supplied trust anchor.
///
/// `_enable_retry` is a placeholder for Pillar 3b.3c's custom accept
/// loop. quiche 0.28 does NOT expose a `Config::enable_retry(bool)`
/// toggle — the RETRY handshake is driven by the application via the
/// free function [`quiche::retry`] in a custom accept loop. The flag
/// is accepted here so the call-site expresses intent, and so the
/// argument exists when the loop lands.
#[allow(clippy::needless_pass_by_value)]
fn build_config(role: &Role, _enable_retry: bool) -> Result<quiche::Config, QuicError> {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
    cfg.set_application_protos(&[LB_QUIC_ALPN])?;
    cfg.set_max_idle_timeout(5_000);
    cfg.set_max_recv_udp_payload_size(1_350);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(10 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(1024 * 1024);
    cfg.set_initial_max_stream_data_uni(1024 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(16);
    cfg.set_disable_active_migration(true);
    cfg.enable_dgram(true, 1024, 1024);
    match role {
        Role::Server {
            cert_pem_path,
            key_pem_path,
        } => {
            let cert = cert_pem_path
                .to_str()
                .ok_or_else(|| QuicError::Io(io::Error::other("cert path is not valid UTF-8")))?;
            let key = key_pem_path
                .to_str()
                .ok_or_else(|| QuicError::Io(io::Error::other("key path is not valid UTF-8")))?;
            cfg.load_cert_chain_from_pem_file(cert)?;
            cfg.load_priv_key_from_pem_file(key)?;
            // Server does not require client certs — no mTLS yet. The
            // quiche default is to not verify the peer, so we leave
            // that explicit toggle off; flipping to `verify_peer(true)`
            // happens when mTLS lands.
        }
        Role::Client { ca_pem_path } => {
            let ca = ca_pem_path
                .to_str()
                .ok_or_else(|| QuicError::Io(io::Error::other("ca path is not valid UTF-8")))?;
            cfg.load_verify_locations_from_file(ca)?;
            // Pillar 3b.3a: peer verification ENABLED. The test rig
            // builds a proper CA + leaf cert via rcgen with SAN +
            // `serverAuth` EKU so BoringSSL's hostname verifier
            // accepts the loopback peer. The old `verify_peer(false)`
            // workaround is gone.
            cfg.verify_peer(true);
        }
    }
    Ok(cfg)
}

/// Generate a 16-byte source connection id. Loopback test only needs
/// uniqueness, not unpredictability, so a nanos+pid mix is adequate.
#[allow(clippy::cast_possible_truncation)]
fn random_scid() -> [u8; quiche::MAX_CONN_ID_LEN] {
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    for (i, byte) in scid.iter_mut().enumerate() {
        let idx = u32::try_from(i).unwrap_or(0);
        let rot = idx & 31;
        let mix = nanos
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add(pid.wrapping_mul(idx.wrapping_add(1)))
            .rotate_left(rot);
        *byte = (mix & 0xFF) as u8;
    }
    scid
}

/// Flush any outbound packets queued on the connection into the socket.
async fn flush_out(
    conn: &Arc<Mutex<quiche::Connection>>,
    socket: &Arc<UdpSocket>,
    out: &mut [u8],
) -> Result<(), QuicError> {
    loop {
        let send = {
            let mut guard = conn.lock().await;
            guard.send(out)
        };
        match send {
            Ok((n, info)) => {
                socket.send_to(prefix(out, n), info.to).await?;
            }
            Err(quiche::Error::Done) => return Ok(()),
            Err(e) => return Err(QuicError::from(e)),
        }
    }
}

/// Best-effort flush used on shutdown. Errors are swallowed — the peer
/// has already been told to close.
async fn flush_out_best_effort(
    conn: &Arc<Mutex<quiche::Connection>>,
    socket: &Arc<UdpSocket>,
    out: &mut [u8],
) {
    loop {
        let send = {
            let mut guard = conn.lock().await;
            guard.send(out)
        };
        match send {
            Ok((n, info)) => {
                let _ = socket.send_to(prefix(out, n), info.to).await;
            }
            Err(_) => return,
        }
    }
}

/// Drive a quiche connection until either `ready` returns
/// `Ok(Some(v))`, the connection closes, or the loopback driver budget
/// expires.
#[allow(clippy::redundant_pub_crate)] // `tokio::select!` macro expansion
async fn drive<F, T>(
    conn: &Arc<Mutex<quiche::Connection>>,
    socket: &Arc<UdpSocket>,
    local_addr: SocketAddr,
    mut ready: F,
) -> Result<T, QuicError>
where
    F: FnMut(&mut quiche::Connection) -> Result<Option<T>, QuicError>,
{
    let deadline = tokio::time::Instant::now() + LOOPBACK_DRIVER_BUDGET;
    let mut out = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
    let mut in_buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(QuicError::DriverTimeout);
        }
        flush_out(conn, socket, &mut out).await?;

        {
            let mut guard = conn.lock().await;
            if let Some(v) = ready(&mut guard)? {
                return Ok(v);
            }
            if guard.is_closed() {
                return Err(QuicError::Internal(
                    "connection closed before condition was met".to_string(),
                ));
            }
        }

        let next_timeout = {
            let guard = conn.lock().await;
            guard.timeout()
        };
        let budget = deadline.saturating_duration_since(tokio::time::Instant::now());
        let wait = next_timeout.map_or(budget, |t| t.min(budget));

        tokio::select! {
            recv = socket.recv_from(&mut in_buf) => {
                let (n, from) = recv?;
                let info = quiche::RecvInfo { from, to: local_addr };
                let mut guard = conn.lock().await;
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                match guard.recv(slice, info) {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(e) => return Err(QuicError::from(e)),
                }
            }
            () = tokio::time::sleep(wait) => {
                let mut guard = conn.lock().await;
                guard.on_timeout();
            }
        }
    }
}

/// Minimal server-side accept loop: receive one inbound QUIC Initial,
/// build an `accept`ed connection, hand back the driven handle and the
/// peer's socket address.
async fn server_accept_one(
    socket: &Arc<UdpSocket>,
    local_addr: SocketAddr,
    config: &mut quiche::Config,
) -> Result<(Arc<Mutex<quiche::Connection>>, SocketAddr), QuicError> {
    let deadline = tokio::time::Instant::now() + LOOPBACK_DRIVER_BUDGET;
    let mut in_buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
    let mut out = vec![0u8; MAX_UDP_DATAGRAM_SIZE];

    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let (n, peer) = tokio::time::timeout(remaining, socket.recv_from(&mut in_buf))
        .await
        .map_err(|_| QuicError::NoIncoming)??;

    let scid_bytes = random_scid();
    let scid = quiche::ConnectionId::from_ref(&scid_bytes);
    let mut conn = quiche::accept(&scid, None, local_addr, peer, config)?;
    let info = quiche::RecvInfo {
        from: peer,
        to: local_addr,
    };
    let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
    match conn.recv(slice, info) {
        Ok(_) | Err(quiche::Error::Done) => {}
        Err(e) => return Err(QuicError::from(e)),
    }

    loop {
        match conn.send(&mut out) {
            Ok((sent, info)) => {
                socket.send_to(prefix(&out, sent), info.to).await?;
            }
            Err(quiche::Error::Done) => break,
            Err(e) => return Err(QuicError::from(e)),
        }
    }

    Ok((Arc::new(Mutex::new(conn)), peer))
}

/// Close the connection and flush the resulting `CONNECTION_CLOSE`
/// packets. Errors are best-effort by design — the peer is leaving.
async fn graceful_close(conn: &Arc<Mutex<quiche::Connection>>, socket: &Arc<UdpSocket>) {
    {
        let mut guard = conn.lock().await;
        let _ = guard.close(true, 0, b"ok");
    }
    let mut out = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
    flush_out_best_effort(conn, socket, &mut out).await;
}

/// Spawn the server side of a datagram roundtrip.
#[allow(clippy::future_not_send)]
async fn server_datagram_task(
    server_socket: Arc<UdpSocket>,
    server_local: SocketAddr,
    mut server_config: quiche::Config,
) -> Result<Vec<u8>, QuicError> {
    let (conn, _peer) = server_accept_one(&server_socket, server_local, &mut server_config).await?;
    let bytes = drive(&conn, &server_socket, server_local, |c| {
        if !c.is_established() {
            return Ok(None);
        }
        let mut buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
        match c.dgram_recv(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                Ok(Some(buf))
            }
            Err(quiche::Error::Done) => Ok(None),
            Err(e) => Err(QuicError::from(e)),
        }
    })
    .await?;
    graceful_close(&conn, &server_socket).await;
    Ok(bytes)
}

/// Spawn the server side of a unidirectional-stream roundtrip.
#[allow(clippy::future_not_send)]
async fn server_stream_task(
    server_socket: Arc<UdpSocket>,
    server_local: SocketAddr,
    mut server_config: quiche::Config,
) -> Result<(u64, Vec<u8>), QuicError> {
    let (conn, _peer) = server_accept_one(&server_socket, server_local, &mut server_config).await?;
    let result = drive(&conn, &server_socket, server_local, |c| {
        if !c.is_established() {
            return Ok(None);
        }
        let Some(sid) = c.readable().next() else {
            return Ok(None);
        };
        let mut collected: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match c.stream_recv(sid, &mut buf) {
                Ok((n, fin)) => {
                    let chunk = buf.get(..n).unwrap_or(&[]);
                    collected.extend_from_slice(chunk);
                    if fin {
                        return Ok(Some((sid, collected)));
                    }
                }
                Err(quiche::Error::Done) => return Ok(None),
                Err(e) => return Err(QuicError::from(e)),
            }
        }
    })
    .await?;
    graceful_close(&conn, &server_socket).await;
    Ok(result)
}

/// Drive one unreliable-datagram roundtrip through real quiche endpoints.
///
/// The client connects to the server, sends `dg.data` as a QUIC DATAGRAM
/// frame, and the returned [`QuicDatagram`] carries the bytes the server
/// actually received plus the original `connection_id` (caller-tracked —
/// see [`QuicDatagram::connection_id`]).
///
/// # Errors
///
/// Any quiche-level failure — UDP I/O, TLS handshake, DATAGRAM send or
/// receive — is surfaced as a [`QuicError`] variant. An empty `dg.data`
/// is rejected up front with [`QuicError::EmptyPayload`].
pub async fn roundtrip_datagram(
    server: &QuicEndpoint,
    client: &QuicEndpoint,
    dg: QuicDatagram,
) -> Result<QuicDatagram, QuicError> {
    if dg.data.is_empty() {
        return Err(QuicError::EmptyPayload);
    }

    let server_config = build_config(&server.role, server.retry_signer.is_some())?;
    let mut client_config = build_config(&client.role, false)?;

    let server_socket = Arc::clone(&server.socket);
    let server_local = server.local_addr;
    let server_task = tokio::spawn(server_datagram_task(
        server_socket,
        server_local,
        server_config,
    ));

    let scid_bytes = random_scid();
    let scid = quiche::ConnectionId::from_ref(&scid_bytes);
    let client_conn = quiche::connect(
        Some(LB_QUIC_TEST_SNI),
        &scid,
        client.local_addr,
        server.local_addr,
        &mut client_config,
    )?;
    let client_conn = Arc::new(Mutex::new(client_conn));
    let client_socket = Arc::clone(&client.socket);
    let client_local = client.local_addr;

    let payload = dg.data.clone();
    let mut sent = false;
    drive(&client_conn, &client_socket, client_local, move |c| {
        if !sent && c.is_established() {
            c.dgram_send(&payload)?;
            sent = true;
            return Ok(None);
        }
        if sent && c.dgram_send_queue_len() == 0 {
            return Ok(Some(()));
        }
        Ok(None)
    })
    .await?;

    let received = server_task
        .await
        .map_err(|e| QuicError::Internal(e.to_string()))??;
    graceful_close(&client_conn, &client_socket).await;

    Ok(QuicDatagram {
        connection_id: dg.connection_id,
        data: received,
    })
}

/// Drive one unidirectional-stream roundtrip through real quiche
/// endpoints.
///
/// The client opens a uni stream (stream id 2 under RFC 9000 §2.1),
/// writes `s.data` with FIN, and the server reads until EOF. The
/// returned [`QuicStream`] preserves `stream_id` and `fin` from the
/// input and carries the bytes the server actually received.
///
/// # Errors
///
/// Any quiche-level failure (UDP I/O, TLS handshake, stream I/O) is
/// surfaced as a [`QuicError`] variant. An empty `s.data` is rejected
/// up front with [`QuicError::EmptyStreamData`].
pub async fn roundtrip_stream(
    server: &QuicEndpoint,
    client: &QuicEndpoint,
    s: QuicStream,
) -> Result<QuicStream, QuicError> {
    if s.data.is_empty() {
        return Err(QuicError::EmptyStreamData);
    }

    let server_config = build_config(&server.role, server.retry_signer.is_some())?;
    let mut client_config = build_config(&client.role, false)?;

    let server_socket = Arc::clone(&server.socket);
    let server_local = server.local_addr;
    let server_task = tokio::spawn(server_stream_task(
        server_socket,
        server_local,
        server_config,
    ));

    let scid_bytes = random_scid();
    let scid = quiche::ConnectionId::from_ref(&scid_bytes);
    let client_conn = quiche::connect(
        Some(LB_QUIC_TEST_SNI),
        &scid,
        client.local_addr,
        server.local_addr,
        &mut client_config,
    )?;
    let client_conn = Arc::new(Mutex::new(client_conn));
    let client_socket = Arc::clone(&client.socket);
    let client_local = client.local_addr;

    // Client-initiated unidirectional streams have IDs 2, 6, 10, …
    // under RFC 9000 §2.1. Use id=2 for the one stream this test
    // exercises.
    let stream_id = 2u64;
    let payload = s.data.clone();
    let mut sent = false;
    drive(&client_conn, &client_socket, client_local, move |c| {
        if !sent && c.is_established() {
            c.stream_send(stream_id, &payload, true)?;
            sent = true;
            return Ok(None);
        }
        if sent && c.stream_finished(stream_id) {
            return Ok(Some(()));
        }
        Ok(None)
    })
    .await?;

    let (_sid, received) = server_task
        .await
        .map_err(|e| QuicError::Internal(e.to_string()))??;
    graceful_close(&client_conn, &client_socket).await;

    Ok(QuicStream {
        stream_id: s.stream_id,
        data: received,
        fin: s.fin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datagram_forward_ok() {
        let dg = QuicDatagram {
            connection_id: 42,
            data: b"hello".to_vec(),
        };
        let fwd = forward_datagram(&dg).unwrap();
        assert_eq!(fwd.connection_id, 42);
        assert_eq!(fwd.data, b"hello");
    }

    #[test]
    fn datagram_empty_rejected() {
        let dg = QuicDatagram {
            connection_id: 1,
            data: vec![],
        };
        assert!(forward_datagram(&dg).is_err());
    }

    #[test]
    fn stream_forward_ok() {
        let stream = QuicStream {
            stream_id: 1,
            data: b"payload".to_vec(),
            fin: true,
        };
        let fwd = forward_stream(&stream).unwrap();
        assert_eq!(fwd.stream_id, 1);
        assert!(fwd.fin);
    }

    #[test]
    fn stream_empty_rejected() {
        let stream = QuicStream {
            stream_id: 0,
            data: vec![],
            fin: false,
        };
        assert!(forward_stream(&stream).is_err());
    }
}
