//! Binary-side QUIC listener seam (Pillar 3b.3c-1).
//!
//! Exposes a [`QuicListener`] that `crates/lb/src/main.rs` spawns per
//! `protocol = "quic"` entry in the config. The seam owns:
//!
//! * one [`UdpSocket`] bound to the configured address,
//! * a [`quiche::Config`] built from the configured cert/key paths and
//!   tuning,
//! * an [`Arc<RetryTokenSigner>`] and [`Arc<Mutex<ZeroRttReplayGuard>>`]
//!   — constructed from the `retry_secret_path` on disk, held for the
//!   inbound packet router that lands in Pillar 3b.3c-2,
//! * a [`CancellationToken`] the binary cancels on shutdown.
//!
//! 3b.3c-1 drives a **single-connection accept loop**: one inbound
//! Initial packet spawns one `quiche::accept` call, drives send/recv/
//! timeout until the connection closes or the token cancels, then the
//! loop returns to waiting for the next Initial. Per-CID routing with a
//! `DashMap<ConnectionId, Sender>` is Pillar 3b.3c-2's
//! `InboundPacketRouter`, which replaces this module's accept loop
//! wholesale.
//!
//! What 3b.3c-1 proves that a log-line smoke test cannot:
//!
//! * the UDP socket is bound, receives from an external client, and
//!   sends responses back to that client's observed address;
//! * the PEM-loaded cert+key drive a real `BoringSSL` handshake;
//! * the [`quiche::Connection`] event loop (`recv` / `send` /
//!   `on_timeout`) advances state to `is_established() == true`;
//! * the cancellation token unblocks the accept loop even while a
//!   connection is mid-flight.
//!
//! What 3b.3c-1 does **not** do: RETRY packet minting via
//! [`RetryTokenSigner`] on the wire (the signer is held but not yet
//! called), 0-RTT replay checking via [`ZeroRttReplayGuard`] on the
//! wire (same), H3 bridging to backends, concurrent connections on the
//! same socket. All of those land in 3b.3c-2.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex as PlMutex;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use lb_security::{RetryTokenSigner, ZeroRttReplayGuard};

use crate::LB_QUIC_ALPN;

const MAX_UDP_DATAGRAM: usize = 65_535;

/// 32-byte retry-secret file size on disk.
const RETRY_SECRET_LEN: usize = 32;

/// Inputs to [`QuicListener::spawn`]. Mirrors the shape of
/// `lb_config::QuicListenerConfig` but flattened into owned values so
/// `lb-quic` does not depend on `lb-config`.
#[derive(Debug, Clone)]
pub struct QuicListenerParams {
    /// Bind address, e.g. `127.0.0.1:0`.
    pub bind_addr: SocketAddr,
    /// PEM-encoded certificate chain path (server leaf).
    pub cert_pem_path: PathBuf,
    /// PEM-encoded private-key path.
    pub key_pem_path: PathBuf,
    /// Path to a 32-byte retry-secret file. Generated with mode 0600
    /// if missing.
    pub retry_secret_path: PathBuf,
    /// Connection idle timeout advertised to peers.
    pub max_idle_timeout: Duration,
    /// Maximum UDP payload the endpoint will accept; clamped to
    /// `>= 1200` (RFC 9000 §14) by the caller's validator.
    pub max_recv_udp_payload_size: u64,
    /// Replay-guard capacity. Defaults to 1024 recent tokens.
    pub replay_capacity: usize,
}

impl QuicListenerParams {
    /// Build a parameter bundle with reasonable defaults for the
    /// non-tunable knobs.
    #[must_use]
    pub const fn new(
        bind_addr: SocketAddr,
        cert_pem_path: PathBuf,
        key_pem_path: PathBuf,
        retry_secret_path: PathBuf,
    ) -> Self {
        Self {
            bind_addr,
            cert_pem_path,
            key_pem_path,
            retry_secret_path,
            max_idle_timeout: Duration::from_secs(30),
            max_recv_udp_payload_size: 1_350,
            replay_capacity: 1_024,
        }
    }
}

/// A running QUIC listener spawned by [`QuicListener::spawn`].
///
/// Holds the tokio `JoinHandle` for the accept task, the cancellation
/// token that triggers graceful shutdown, and the socket's observed
/// local address (important when `bind_addr` used port 0).
pub struct QuicListener {
    local_addr: SocketAddr,
    shutdown: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
    /// Held so the signer survives at least as long as the listener.
    /// Pillar 3b.3c-2's router consumes this via `retry_signer()`.
    retry_signer: Arc<RetryTokenSigner>,
    /// Same rationale as `retry_signer`.
    replay_guard: Arc<PlMutex<ZeroRttReplayGuard>>,
}

impl QuicListener {
    /// Bind a UDP socket to `params.bind_addr`, load (or generate) the
    /// retry secret, build a `quiche::Config`, and spawn the accept
    /// task. Returns once the socket is bound; the task runs in the
    /// background until `shutdown` is cancelled.
    ///
    /// # Errors
    ///
    /// * `std::io::ErrorKind::AddrInUse` / similar on bind failure.
    /// * `std::io::ErrorKind::Other` wrapping a [`quiche::Error`] if
    ///   cert/key loading fails, or a [`lb_security::TicketError`]
    ///   equivalent if the retry-secret file cannot be read or
    ///   generated.
    pub async fn spawn(
        params: QuicListenerParams,
        shutdown: CancellationToken,
    ) -> std::io::Result<Self> {
        // Bind UDP.
        let socket = UdpSocket::bind(params.bind_addr).await?;
        let local_addr = socket.local_addr()?;
        let socket = Arc::new(socket);

        // Load-or-generate the retry secret.
        let retry_signer = Arc::new(load_or_generate_retry_secret(&params.retry_secret_path)?);
        let replay_guard = Arc::new(PlMutex::new(ZeroRttReplayGuard::new(
            params.replay_capacity,
        )));

        tracing::info!(
            address = %local_addr,
            protocol = "quic",
            cert = %params.cert_pem_path.display(),
            retry_secret = %params.retry_secret_path.display(),
            "QUIC listener bound (3b.3c-1 seam; full router lands in 3b.3c-2)"
        );

        // Spawn the accept task.
        let task_socket = Arc::clone(&socket);
        let task_params = params.clone();
        let task_shutdown = shutdown.clone();
        let handle = tokio::spawn(async move {
            let fut = Box::pin(accept_loop(task_socket, task_params, task_shutdown.clone()));
            if let Err(e) = fut.await {
                tracing::error!(error = %e, "QUIC accept loop terminated with error");
            }
            tracing::info!(address = %local_addr, "QUIC listener drained");
        });

        Ok(Self {
            local_addr,
            shutdown,
            handle,
            retry_signer,
            replay_guard,
        })
    }

    /// The socket address the listener is bound to.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Retry-token signer installed on this listener. Pillar 3b.3c-2
    /// consumes this for wire-level RETRY handling.
    #[must_use]
    pub fn retry_signer(&self) -> Arc<RetryTokenSigner> {
        Arc::clone(&self.retry_signer)
    }

    /// 0-RTT replay guard installed on this listener. Pillar 3b.3c-2
    /// consumes this for wire-level early-data replay checks.
    #[must_use]
    pub fn replay_guard(&self) -> Arc<PlMutex<ZeroRttReplayGuard>> {
        Arc::clone(&self.replay_guard)
    }

    /// Trigger graceful shutdown. The underlying task exits once the
    /// current connection (if any) closes. Returns the task's
    /// `JoinHandle` so the caller can `.await` completion.
    #[must_use]
    pub fn shutdown(self) -> tokio::task::JoinHandle<()> {
        self.shutdown.cancel();
        self.handle
    }
}

fn load_or_generate_retry_secret(path: &Path) -> std::io::Result<RetryTokenSigner> {
    match std::fs::read(path) {
        Ok(bytes) => {
            if bytes.len() != RETRY_SECRET_LEN {
                return Err(std::io::Error::other(format!(
                    "retry secret file {} has wrong length: expected {} bytes, got {}",
                    path.display(),
                    RETRY_SECRET_LEN,
                    bytes.len()
                )));
            }
            let mut secret = [0u8; RETRY_SECRET_LEN];
            secret.copy_from_slice(
                bytes
                    .get(..RETRY_SECRET_LEN)
                    .unwrap_or(&[0u8; RETRY_SECRET_LEN]),
            );
            Ok(RetryTokenSigner::new_with_secret(secret))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Generate a fresh secret, persist it with mode 0600.
            let signer = RetryTokenSigner::new_random().map_err(std::io::Error::other)?;
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            // We cannot read the 32 bytes back out of `signer` (the
            // signer never exposes its key), so mint a parallel secret
            // here and use it. The signer and the on-disk secret agree
            // because both are derived from the same byte array.
            let mut secret = [0u8; RETRY_SECRET_LEN];
            ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut secret)
                .map_err(|e| std::io::Error::other(format!("rng: {e}")))?;
            write_secret_file(path, &secret)?;
            // Return the signer derived from the written bytes so the
            // on-disk file and the in-memory signer match.
            let _ = signer; // earlier signer discarded on purpose
            Ok(RetryTokenSigner::new_with_secret(secret))
        }
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn write_secret_file(path: &Path, secret: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(secret)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secret_file(path: &Path, secret: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, secret)
}

/// Single-connection accept loop. Pillar 3b.3c-2 replaces this with an
/// `InboundPacketRouter` that handles many concurrent connections on
/// the same socket.
async fn accept_loop(
    socket: Arc<UdpSocket>,
    params: QuicListenerParams,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        // Each iteration: wait for an Initial, accept, drive, close.
        match tokio::select! {
            () = shutdown.cancelled() => {
                return Ok(());
            }
            res = accept_one_connection(&socket, &params, &shutdown) => res,
        } {
            Ok(()) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                // Malformed / unknown-version packets are expected
                // from the network at large; log at debug and resume.
                tracing::debug!(error = %e, "QUIC listener dropped malformed packet");
                continue;
            }
            Err(e) => {
                tracing::warn!(error = %e, "QUIC listener connection ended with error");
                continue;
            }
        }
    }
}

async fn accept_one_connection(
    socket: &Arc<UdpSocket>,
    params: &QuicListenerParams,
    shutdown: &CancellationToken,
) -> std::io::Result<()> {
    let local_addr = socket.local_addr()?;
    let mut in_buf = vec![0u8; MAX_UDP_DATAGRAM];
    let mut out_buf = vec![0u8; MAX_UDP_DATAGRAM];

    let (n, peer) = tokio::select! {
        () = shutdown.cancelled() => return Ok(()),
        r = socket.recv_from(&mut in_buf) => r?,
    };

    // Parse the QUIC header just enough to reject short-header packets
    // without an established connection (we don't route them yet).
    let header = {
        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
        match quiche::Header::from_slice(slice, quiche::MAX_CONN_ID_LEN) {
            Ok(h) => h,
            Err(e) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("quiche header parse: {e}"),
                ));
            }
        }
    };
    if header.ty != quiche::Type::Initial {
        // Not an Initial — without per-CID routing we can't do
        // anything with it in 3b.3c-1. Drop.
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "non-Initial packet arrived with no established connection",
        ));
    }

    let mut config = build_server_config(params).map_err(std::io::Error::other)?;
    let scid = random_scid();
    let scid_ref = quiche::ConnectionId::from_ref(&scid);
    let mut conn = quiche::accept(&scid_ref, None, local_addr, peer, &mut config)
        .map_err(|e| std::io::Error::other(format!("quiche::accept failed: {e}")))?;

    // Feed the Initial in.
    {
        let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
        let info = quiche::RecvInfo {
            from: peer,
            to: local_addr,
        };
        match conn.recv(slice, info) {
            Ok(_) | Err(quiche::Error::Done) => {}
            Err(e) => {
                return Err(std::io::Error::other(format!("quiche recv: {e}")));
            }
        }
    }

    // Drive send+recv+timeout until closed or cancelled.
    loop {
        // Flush any outbound packets.
        loop {
            match conn.send(&mut out_buf) {
                Ok((sent, info)) => {
                    let slice = out_buf.get(..sent).unwrap_or(&[]);
                    if let Err(e) = socket.send_to(slice, info.to).await {
                        tracing::debug!(error = %e, "QUIC send_to failed");
                        break;
                    }
                }
                Err(quiche::Error::Done) => break,
                Err(e) => {
                    tracing::debug!(error = %e, "QUIC send error");
                    break;
                }
            }
        }
        if conn.is_closed() {
            return Ok(());
        }

        let timeout = conn.timeout().unwrap_or(Duration::from_millis(100));
        tokio::select! {
            () = shutdown.cancelled() => {
                let _ = conn.close(false, 0x0, b"shutdown");
                // Flush CONNECTION_CLOSE.
                while let Ok((sent, info)) = conn.send(&mut out_buf) {
                    let slice = out_buf.get(..sent).unwrap_or(&[]);
                    let _ = socket.send_to(slice, info.to).await;
                }
                return Ok(());
            }
            r = socket.recv_from(&mut in_buf) => {
                let (rn, rfrom) = r?;
                let slice = in_buf.get_mut(..rn).unwrap_or(&mut []);
                let info = quiche::RecvInfo { from: rfrom, to: local_addr };
                match conn.recv(slice, info) {
                    Ok(_) | Err(quiche::Error::Done) => {}
                    Err(e) => {
                        tracing::debug!(error = %e, "QUIC recv error");
                    }
                }
            }
            () = tokio::time::sleep(timeout) => {
                conn.on_timeout();
            }
        }
    }
}

fn build_server_config(params: &QuicListenerParams) -> Result<quiche::Config, quiche::Error> {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
    cfg.set_application_protos(&[LB_QUIC_ALPN])?;
    cfg.set_max_idle_timeout(
        u64::try_from(params.max_idle_timeout.as_millis()).unwrap_or(u64::MAX),
    );
    // Clamp to a sane usize; validator already guaranteed >= 1200.
    let recv_payload = usize::try_from(params.max_recv_udp_payload_size).unwrap_or(1_350);
    cfg.set_max_recv_udp_payload_size(recv_payload);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(10 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(1024 * 1024);
    cfg.set_initial_max_stream_data_uni(1024 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(16);
    cfg.set_disable_active_migration(true);

    let cert = params
        .cert_pem_path
        .to_str()
        .ok_or(quiche::Error::TlsFail)?;
    let key = params.key_pem_path.to_str().ok_or(quiche::Error::TlsFail)?;
    cfg.load_cert_chain_from_pem_file(cert)?;
    cfg.load_priv_key_from_pem_file(key)?;
    // Server does not request client certs; quiche default is off.
    Ok(cfg)
}

fn random_scid() -> [u8; quiche::MAX_CONN_ID_LEN] {
    use ring::rand::SecureRandom;
    let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
    // Fallback to a deterministic mix if RNG fails — SCID uniqueness,
    // not unpredictability, is the only requirement for accept.
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
