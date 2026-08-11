//! Binary-side QUIC listener: bind a `UdpSocket`, load (or generate) the 32-byte retry-token
//! secret, build the [`lb_security::RetryTokenSigner`] + [`lb_security::ZeroRttReplayGuard`] and a
//! `quiche::Config` factory, then spawn the [`crate::router::InboundPacketRouter`].

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

use crate::H3_ALPN_PROTOS;
use crate::router::{self, RouterParams};

/// 32-byte retry-secret file size on disk.
const RETRY_SECRET_LEN: usize = 32;

/// Inputs to [`QuicListener::spawn`].
#[derive(Clone)]
pub struct QuicListenerParams {
    /// Bind address, e.g. `127.0.0.1:0`.
    pub bind_addr: SocketAddr,
    /// PEM-encoded certificate chain path (server leaf).
    pub cert_pem_path: PathBuf,
    /// PEM-encoded private-key path.
    pub key_pem_path: PathBuf,
    /// Path to a 32-byte retry-secret file, generated 0600 if absent.
    pub retry_secret_path: PathBuf,
    /// Connection idle timeout advertised to peers.
    pub max_idle_timeout: Duration,
    /// Maximum UDP payload accepted, clamped to the QUIC packet ceiling.
    pub max_recv_udp_payload_size: u64,
    /// Replay-guard capacity. Defaults to 1024 recent tokens.
    pub replay_capacity: usize,
    /// Resolved backend addresses. Empty ⇒ the listener terminates QUIC but forwards nothing.
    pub backends: Vec<SocketAddr>,
    /// Shared TCP pool for H1 backend dials.
    pub pool: Option<TcpPool>,
    /// Optional upstream H3 backend `(pool, addr, sni)` — takes precedence over the H1 list.
    pub h3_backend: Option<(QuicUpstreamPool, SocketAddr, String)>,
    /// Optional upstream H2 backend `(pool, addr)`.
    pub h2_backend: Option<(Http2Pool, SocketAddr)>,
    /// Mode B: optional raw-QUIC re-origination backend, switching from H3 termination.
    pub raw_quic_backend: Option<crate::raw_proxy::RawBackend>,
    /// Mode B: DATAGRAM queue length advertised to peers, single-sourced with the relay's cap.
    pub dgram_queue_cap: usize,
    /// Mode B `quic_modeb_*` metric handles.
    pub quic_modeb_metrics: Option<lb_observability::QuicModeBMetrics>,
    /// WS-over-H3 Stage A; `false` ⇒ `:protocol` is rejected byte-identically to a pre-WS listener.
    pub ws_enabled: bool,
    /// WS-over-H3 Stage C: the injected relay launcher (see [`crate::ws_tunnel::WsRelayLauncher`]).
    pub ws_relay_launcher: Option<crate::ws_tunnel::WsRelayLauncher>,
    /// S36-A: per-connection H3 request cap; `0` disables recycling entirely.
    pub max_requests_per_h3_connection: u32,
    /// S36-A: the `h3_*` recycle metric handles.
    pub h3_recycle_metrics: Option<lb_observability::QuicH3RecycleMetrics>,
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
            .field("raw_quic_backend_set", &self.raw_quic_backend.is_some())
            .field("dgram_queue_cap", &self.dgram_queue_cap)
            .field("quic_modeb_metrics_set", &self.quic_modeb_metrics.is_some())
            .field("ws_enabled", &self.ws_enabled)
            .field("ws_relay_launcher_set", &self.ws_relay_launcher.is_some())
            .field(
                "max_requests_per_h3_connection",
                &self.max_requests_per_h3_connection,
            )
            .field("h3_recycle_metrics_set", &self.h3_recycle_metrics.is_some())
            .finish()
    }
}

impl QuicListenerParams {
    /// Build a parameter bundle with defaults for the optional fields.
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
            // H3-terminate by default; Mode B is opt-in.
            raw_quic_backend: None,
            dgram_queue_cap: 1_024,
            quic_modeb_metrics: None,
            // WebSocket opt-in is OFF by default (R3).
            ws_enabled: false,
            ws_relay_launcher: None,
            // S36-A: H3 request cap DISABLED by default — the binary opts in.
            max_requests_per_h3_connection: 0,
            h3_recycle_metrics: None,
        }
    }

    /// S36-A: set the per-connection H3 request cap and recycle metrics.
    #[must_use]
    pub fn with_h3_request_cap(
        mut self,
        cap: u32,
        metrics: Option<lb_observability::QuicH3RecycleMetrics>,
    ) -> Self {
        self.max_requests_per_h3_connection = cap;
        self.h3_recycle_metrics = metrics;
        self
    }

    /// Attach a backend list + TCP pool for H3→H1 forwarding.
    #[must_use]
    pub fn with_backends(mut self, backends: Vec<SocketAddr>, pool: TcpPool) -> Self {
        self.backends = backends;
        self.pool = Some(pool);
        self
    }

    /// Attach an upstream H3 backend for H3→H3 forwarding.
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

    /// Attach an upstream H2 backend for H3→H2 forwarding.
    #[must_use]
    pub fn with_h2_backend(mut self, pool: Http2Pool, addr: SocketAddr) -> Self {
        self.h2_backend = Some((pool, addr));
        self
    }

    /// Mode B: switch this listener to terminate-and-re-originate raw QUIC.
    #[must_use]
    pub fn with_raw_backend(
        mut self,
        backend: crate::raw_proxy::RawBackend,
        dgram_queue_cap: usize,
        quic_modeb_metrics: Option<lb_observability::QuicModeBMetrics>,
    ) -> Self {
        self.raw_quic_backend = Some(backend);
        self.dgram_queue_cap = dgram_queue_cap;
        self.quic_modeb_metrics = quic_modeb_metrics;
        self
    }

    /// WS-over-H3 Stage A: opt this listener into extended CONNECT.
    #[must_use]
    pub const fn with_websocket(mut self, enabled: bool) -> Self {
        self.ws_enabled = enabled;
        self
    }

    /// WS-over-H3 Stage C: inject the WebSocket relay launcher.
    #[must_use]
    pub fn with_ws_relay_launcher(mut self, launcher: crate::ws_tunnel::WsRelayLauncher) -> Self {
        self.ws_relay_launcher = Some(launcher);
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
    /// Bind to `params.bind_addr`, load (or generate) the retry secret, and spawn the router.
    ///
    /// # Errors
    /// Bind failure, or a retry-secret load/generate/permission failure.
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

        // A config factory, not a shared config: quiche::Config is not `Sync`.
        let cert = params.cert_pem_path.clone();
        let key = params.key_pem_path.clone();
        let idle_ms = u64::try_from(params.max_idle_timeout.as_millis()).unwrap_or(u64::MAX);
        let recv_payload = usize::try_from(params.max_recv_udp_payload_size).unwrap_or(1_350);
        // Mode B: enable QUIC DATAGRAM only for a raw-QUIC listener, so the H3 path's advertised
        // transport params are unchanged (R3).
        let enable_datagrams = params.raw_quic_backend.is_some();
        let dgram_queue_cap = params.dgram_queue_cap;
        let config_factory: Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> =
            Arc::new(move || {
                build_server_config(
                    &cert,
                    &key,
                    idle_ms,
                    recv_payload,
                    enable_datagrams,
                    dgram_queue_cap,
                )
            });

        // A pool is required for real traffic; without one the listener forwards nothing.
        let pool = params.pool.clone().unwrap_or_else(|| {
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
        });

        let router_params = RouterParams {
            socket: Arc::clone(&socket),
            retry_signer: Arc::clone(&retry_signer),
            replay_guard: Arc::clone(&replay_guard),
            config_factory,
            pool,
            backends: Arc::new(params.backends.clone()),
            h3_backend: params.h3_backend.clone(),
            h2_backend: params.h2_backend.clone(),
            // Auditor-suggested default matching the conntrack scale target.
            max_connections: 100_000,
            cancel: shutdown.clone(),
            // Mode B: thread the configured raw-QUIC backend through.
            raw_quic_backend: params.raw_quic_backend.clone(),
            quic_modeb_metrics: params.quic_modeb_metrics.clone(),
            ws_enabled: params.ws_enabled,
            ws_relay_launcher: params.ws_relay_launcher.clone(),
            max_requests_per_h3_connection: params.max_requests_per_h3_connection,
            h3_recycle_metrics: params.h3_recycle_metrics.clone(),
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

/// F-INFRA-01 — on the LOAD path, refuse (strict) or warn (lax) when an existing retry-secret file
/// is group/world-accessible. The secret is the HMAC key behind Retry-token address validation, so
/// a world-readable one lets any local reader forge tokens and bypass the source-address check.
/// The generate path already writes 0600; this closes the read-path asymmetry.
#[cfg(unix)]
fn check_retry_secret_perms(path: &Path, strict: bool) -> std::io::Result<()> {
    match lb_security::assert_owner_only(path, strict) {
        Ok(lb_security::KeyPermAdvice::Ok | lb_security::KeyPermAdvice::NotApplicable) => Ok(()),
        Ok(lb_security::KeyPermAdvice::TooPermissive { mode }) => {
            tracing::warn!(
                retry_secret = %path.display(),
                mode = format!("{mode:o}"),
                "retry-secret file permissions wider than 0o600 — tighten with `chmod 600`"
            );
            Ok(())
        }
        Err(e) => Err(std::io::Error::other(format!(
            "retry-secret permission check failed for {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(not(unix))]
fn check_retry_secret_perms(_path: &Path, _strict: bool) -> std::io::Result<()> {
    Ok(())
}

fn load_or_generate_retry_secret(path: &Path) -> std::io::Result<RetryTokenSigner> {
    match std::fs::read(path) {
        Ok(bytes) => {
            // F-INFRA-01: perm-gate the existing-file load (strict on release).
            check_retry_secret_perms(path, !cfg!(debug_assertions))?;
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

/// Build the client-facing `quiche::Config`. `enable_datagrams` is `true` ONLY for a Mode-B
/// listener, which needs RFC 9221 DATAGRAM for the B4 relay; when `false` `enable_dgram` is NOT
/// called at all, so the H3 path's advertised transport parameters stay byte-identical (R3).
fn build_server_config(
    cert: &Path,
    key: &Path,
    idle_ms: u64,
    recv_payload: usize,
    enable_datagrams: bool,
    dgram_queue_cap: usize,
) -> Result<quiche::Config, quiche::Error> {
    let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
    cfg.set_application_protos(H3_ALPN_PROTOS)?;
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
    // R3: only a Mode-B listener advertises DATAGRAM support.
    if enable_datagrams {
        cfg.enable_dgram(true, dgram_queue_cap, dgram_queue_cap);
    }
    let cert = cert.to_str().ok_or(quiche::Error::TlsFail)?;
    let key = key.to_str().ok_or(quiche::Error::TlsFail)?;
    cfg.load_cert_chain_from_pem_file(cert)?;
    cfg.load_priv_key_from_pem_file(key)?;
    Ok(cfg)
}

#[cfg(test)]
#[cfg(unix)]
mod retry_secret_perm_tests {
    //! F-INFRA-01 regression — the retry-secret LOAD path must perm-check an existing file.
    use super::{RETRY_SECRET_LEN, check_retry_secret_perms, load_or_generate_retry_secret};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn temp_secret(name: &str, mode: u32) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        p.push(format!("lb-quic-retry-secret-{nanos}-{name}"));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(&[0u8; RETRY_SECRET_LEN]).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    // NEGATIVE CONTROL: a world-readable (0644) existing secret is REJECTED in strict mode.
    // Pre-fix this loaded silently.
    #[test]
    fn world_readable_secret_rejected_strict() {
        let p = temp_secret("0644-strict", 0o644);
        let res = check_retry_secret_perms(&p, /* strict */ true);
        let _ = std::fs::remove_file(&p);
        assert!(
            res.is_err(),
            "world-readable retry secret must be rejected in strict mode"
        );
    }

    // Lax mode WARNS but still loads (debug-build parity with the TLS key).
    #[test]
    fn world_readable_secret_warns_lax() {
        let p = temp_secret("0644-lax", 0o644);
        let res = check_retry_secret_perms(&p, /* strict */ false);
        let _ = std::fs::remove_file(&p);
        assert!(res.is_ok(), "lax mode must warn-and-continue, not error");
    }

    // A 0600 secret passes strict AND the full load path returns a signer, so the gate does not
    // block the legitimate case.
    #[test]
    fn owner_only_secret_passes_strict_and_loads() {
        let p = temp_secret("0600-strict", 0o600);
        assert!(check_retry_secret_perms(&p, true).is_ok());
        let signer = load_or_generate_retry_secret(&p);
        let _ = std::fs::remove_file(&p);
        assert!(signer.is_ok(), "0600 secret must load cleanly");
    }
}
