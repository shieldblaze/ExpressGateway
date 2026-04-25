//! Binary-side QUIC listener (Pillar 3b.3c-1 seam / Pillar 3b.3c-2 router).
//!
//! `QuicListener::spawn` binds a `UdpSocket`, loads (or generates) the
//! 32-byte retry-token secret, constructs a
//! [`lb_security::RetryTokenSigner`] and
//! [`lb_security::ZeroRttReplayGuard`], builds a `quiche::Config`
//! factory, and spawns the Pillar 3b.3c-2
//! [`crate::router::InboundPacketRouter`] which handles per-CID
//! dispatch, RETRY wire handling, 0-RTT replay checks, and per-actor
//! H3 termination.
//!
//! The 3b.3c-1 [`QuicListenerParams::new`] constructor still builds a
//! listener with no backends — useful for transport-only smoke tests.
//! Once backends are supplied via
//! [`QuicListenerParams::with_backends`], the listener becomes a real
//! reverse proxy: established connections flow through the router →
//! actor → [`crate::h3_bridge`] → [`lb_io::pool::TcpPool`] → H1
//! backend, with the response streamed back.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex as PlMutex;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use lb_io::http2_pool::Http2Pool;
use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;
use lb_security::{RetryTokenSigner, ZeroRttReplayGuard};

use crate::LB_QUIC_ALPN;
use crate::router::{self, RouterParams};

/// 32-byte retry-secret file size on disk.
const RETRY_SECRET_LEN: usize = 32;

/// Inputs to [`QuicListener::spawn`]. Mirrors the shape of
/// `lb_config::QuicListenerConfig` but flattened into owned values so
/// `lb-quic` does not depend on `lb-config`.
#[derive(Clone)]
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
    /// Resolved backend addresses. When empty, the listener still
    /// accepts connections and completes handshakes but H3 requests
    /// that reach established state are answered with 502 (no
    /// backends available). 3b.3c-1 smoke tests deliberately leave
    /// this empty; 3b.3c-2 e2e tests populate it.
    pub backends: Vec<SocketAddr>,
    /// Shared TCP pool for H1 backend dials.
    pub pool: Option<TcpPool>,
    /// Optional upstream H3 backend `(pool, addr, sni)` — when set,
    /// H3 requests on this listener route via the QUIC upstream pool
    /// instead of the H1/TcpPool path (Pillar 3b.3c-3).
    pub h3_backend: Option<(QuicUpstreamPool, SocketAddr, String)>,
    /// Optional upstream H2 backend `(pool, addr)` — when set,
    /// H3 requests on this listener route via the HTTP/2 upstream pool
    /// (PROTO-001 H3→H2 path). Takes precedence over `h3_backend` when
    /// both are configured; mixed-protocol routing is not supported in
    /// v1.
    pub h2_backend: Option<(Http2Pool, SocketAddr)>,
}

impl std::fmt::Debug for QuicListenerParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicListenerParams")
            .field("bind_addr", &self.bind_addr)
            .field("cert_pem_path", &self.cert_pem_path)
            .field("key_pem_path", &self.key_pem_path)
            .field("retry_secret_path", &self.retry_secret_path)
            .field("max_idle_timeout", &self.max_idle_timeout)
            .field("max_recv_udp_payload_size", &self.max_recv_udp_payload_size)
            .field("replay_capacity", &self.replay_capacity)
            .field("backends", &self.backends)
            .field("pool_set", &self.pool.is_some())
            .field("h3_backend_set", &self.h3_backend.is_some())
            .field("h2_backend_set", &self.h2_backend.is_some())
            .finish()
    }
}

impl QuicListenerParams {
    /// Build a parameter bundle with reasonable defaults for the
    /// non-tunable knobs. No backends, no pool — 3b.3c-1 smoke shape.
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
            backends: Vec::new(),
            pool: None,
            h3_backend: None,
            h2_backend: None,
        }
    }

    /// Attach a backend list + TCP pool for H3→H1 forwarding
    /// (Pillar 3b.3c-2).
    #[must_use]
    pub fn with_backends(mut self, backends: Vec<SocketAddr>, pool: TcpPool) -> Self {
        self.backends = backends;
        self.pool = Some(pool);
        self
    }

    /// Attach an upstream H3 backend for H3→H3 forwarding
    /// (Pillar 3b.3c-3). `addr` is the backend's UDP address; `sni`
    /// is the TLS name presented on the upstream handshake.
    #[must_use]
    pub fn with_h3_backend(
        mut self,
        pool: QuicUpstreamPool,
        addr: SocketAddr,
        sni: impl Into<String>,
    ) -> Self {
        self.h3_backend = Some((pool, addr, sni.into()));
        self
    }

    /// Attach an upstream H2 backend for H3→H2 forwarding
    /// (PROTO-001). `addr` is the backend's TCP address. Takes
    /// precedence over `h3_backend`.
    #[must_use]
    pub fn with_h2_backend(mut self, pool: Http2Pool, addr: SocketAddr) -> Self {
        self.h2_backend = Some((pool, addr));
        self
    }
}

/// A running QUIC listener spawned by [`QuicListener::spawn`].
pub struct QuicListener {
    local_addr: SocketAddr,
    shutdown: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
    /// Held so the signer survives at least as long as the listener.
    retry_signer: Arc<RetryTokenSigner>,
    /// Same rationale as `retry_signer`.
    replay_guard: Arc<PlMutex<ZeroRttReplayGuard>>,
}

impl QuicListener {
    /// Bind a UDP socket to `params.bind_addr`, load (or generate) the
    /// retry secret, build the `quiche::Config` factory, and spawn
    /// the [`crate::router::InboundPacketRouter`].
    ///
    /// # Errors
    ///
    /// * `std::io::ErrorKind::AddrInUse` on bind failure.
    /// * `std::io::ErrorKind::Other` wrapping other failures.
    pub async fn spawn(
        params: QuicListenerParams,
        shutdown: CancellationToken,
    ) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(params.bind_addr).await?;
        let local_addr = socket.local_addr()?;
        let socket = Arc::new(socket);

        let retry_signer = Arc::new(load_or_generate_retry_secret(&params.retry_secret_path)?);
        let replay_guard = Arc::new(PlMutex::new(ZeroRttReplayGuard::new(
            params.replay_capacity,
        )));

        tracing::info!(
            address = %local_addr,
            protocol = "quic",
            cert = %params.cert_pem_path.display(),
            retry_secret = %params.retry_secret_path.display(),
            backends = params.backends.len(),
            "QUIC listener bound"
        );

        // Build a config factory that produces a fresh `quiche::Config`
        // per accepted connection. quiche::Config holds non-Send
        // internal state after cert loading, so cloning is not an
        // option; cert+key paths + the other knobs are Send+Sync and
        // cheap to re-load.
        let cert = params.cert_pem_path.clone();
        let key = params.key_pem_path.clone();
        let idle_ms = u64::try_from(params.max_idle_timeout.as_millis()).unwrap_or(u64::MAX);
        let recv_payload = usize::try_from(params.max_recv_udp_payload_size).unwrap_or(1_350);
        let config_factory: Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> =
            Arc::new(move || build_server_config(&cert, &key, idle_ms, recv_payload));

        // Pool is required for real traffic; if the caller did not
        // supply one (3b.3c-1 smoke path), build a transient in-memory
        // pool so the router's actor can be constructed.
        let pool = params.pool.clone().map_or_else(
            || {
                let runtime = lb_io::Runtime::new();
                TcpPool::new(
                    lb_io::pool::PoolConfig::default(),
                    lb_io::sockopts::BackendSockOpts {
                        nodelay: true,
                        keepalive: true,
                        rcvbuf: Some(262_144),
                        sndbuf: Some(262_144),
                        quickack: false,
                        tcp_fastopen_connect: false,
                    },
                    runtime,
                )
            },
            |p| p,
        );

        let router_params = RouterParams {
            socket: Arc::clone(&socket),
            retry_signer: Arc::clone(&retry_signer),
            replay_guard: Arc::clone(&replay_guard),
            config_factory,
            pool,
            backends: Arc::new(params.backends.clone()),
            h3_backend: params.h3_backend.clone(),
            h2_backend: params.h2_backend.clone(),
            // PROMPT.md §6 target conntrack scale. Auditor-suggested
            // bound (2026-04-23) — finite-memory defence against Initial
            // flooding behind legitimate source-address retry tokens.
            max_connections: 100_000,
            cancel: shutdown.clone(),
        };
        let router_handle = router::spawn(router_params);
        let handle = tokio::spawn(async move {
            if let Err(e) = router_handle.join().await {
                tracing::warn!(error = %e, "QUIC router join error");
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

    /// Retry-token signer installed on this listener.
    #[must_use]
    pub fn retry_signer(&self) -> Arc<RetryTokenSigner> {
        Arc::clone(&self.retry_signer)
    }

    /// 0-RTT replay guard installed on this listener.
    #[must_use]
    pub fn replay_guard(&self) -> Arc<PlMutex<ZeroRttReplayGuard>> {
        Arc::clone(&self.replay_guard)
    }

    /// Trigger graceful shutdown. Returns the task's `JoinHandle`.
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
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            let mut secret = [0u8; RETRY_SECRET_LEN];
            ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut secret)
                .map_err(|e| std::io::Error::other(format!("rng: {e}")))?;
            write_secret_file(path, &secret)?;
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

fn build_server_config(
    cert: &Path,
    key: &Path,
    idle_ms: u64,
    recv_payload: usize,
) -> Result<quiche::Config, quiche::Error> {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
    cfg.set_application_protos(&[LB_QUIC_ALPN])?;
    cfg.set_max_idle_timeout(idle_ms);
    cfg.set_max_recv_udp_payload_size(recv_payload);
    cfg.set_max_send_udp_payload_size(1_350);
    cfg.set_initial_max_data(10 * 1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_local(1024 * 1024);
    cfg.set_initial_max_stream_data_bidi_remote(1024 * 1024);
    cfg.set_initial_max_stream_data_uni(1024 * 1024);
    cfg.set_initial_max_streams_bidi(16);
    cfg.set_initial_max_streams_uni(16);
    cfg.set_disable_active_migration(true);
    let cert = cert.to_str().ok_or(quiche::Error::TlsFail)?;
    let key = key.to_str().ok_or(quiche::Error::TlsFail)?;
    cfg.load_cert_chain_from_pem_file(cert)?;
    cfg.load_priv_key_from_pem_file(key)?;
    Ok(cfg)
}
