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

use crate::H3_ALPN_PROTOS;
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
    /// SESSION 19 / Mode B (B6): optional raw-QUIC re-origination backend.
    /// When `Some`, this listener runs Mode B (terminate-and-re-originate)
    /// — every accepted connection is handed to the raw-proxy actor — and
    /// the client-facing `quiche::Config` enables QUIC DATAGRAM support
    /// (`enable_datagrams = true`). When `None` (every existing caller) the
    /// listener runs H3-termination EXACTLY as today and DATAGRAM support
    /// is NOT advertised, so the transport parameters are byte-identical
    /// (R3). Threaded into [`RouterParams::raw_quic_backend`].
    pub raw_quic_backend: Option<crate::raw_proxy::RawBackend>,
    /// SESSION 19 / Mode B (B6): DATAGRAM recv/send queue length to
    /// advertise to peers via `enable_dgram(true, cap, cap)` on a Mode-B
    /// listener. Only consulted when `raw_quic_backend` is `Some`; ignored
    /// (DATAGRAM disabled) on the H3 path. Mirrors the backend-leg
    /// `dgram_queue_cap`.
    pub dgram_queue_cap: usize,
    /// SESSION 19 / Mode B (B6): `quic_modeb_*` metric handles, threaded to
    /// [`RouterParams::quic_modeb_metrics`]. `None` ⇒ no Mode-B metrics
    /// (and always `None` on the H3 path).
    pub quic_modeb_metrics: Option<lb_observability::QuicModeBMetrics>,
    /// SESSION 27 / WS-over-H3 (RFC 9220) Stage A: whether this listener
    /// opted into WebSocket (the binary sets this from a present
    /// `[listeners.websocket]` block). Threaded to
    /// [`crate::router::RouterParams::ws_enabled`] → each actor. Gates the
    /// `SETTINGS_ENABLE_CONNECT_PROTOCOL` advertisement + the `:protocol`
    /// Extended-CONNECT accept path. `false` by default (set via
    /// [`Self::with_websocket`]) keeps the H3 front byte-identical (R3).
    pub ws_enabled: bool,
    /// SESSION 28 / WS-over-H3 (RFC 9220) Stage C: the injected WebSocket
    /// relay launcher (dependency inversion — the `lb` binary builds it
    /// because `lb-quic` cannot import `lb_l7::ws_proxy::proxy_frames`).
    /// Set via [`Self::with_ws_relay_launcher`] alongside
    /// [`Self::with_websocket`]; threaded into
    /// [`crate::router::RouterParams::ws_relay_launcher`] → each actor.
    /// `None` (every non-WS listener) keeps the H3 front byte-identical
    /// (R3). Mirrors how the `config_factory` closure is threaded.
    pub ws_relay_launcher: Option<crate::ws_tunnel::WsRelayLauncher>,
    /// S36-A: per-connection H3 request cap. When non-zero, the H3 actor
    /// sends an H3 GOAWAY once `max_requests_per_h3_connection` request
    /// streams have been seen on a single connection, drains the in-flight
    /// requests, then gracefully closes — so the connection recycles and
    /// quiche's per-connection `StreamMap::collected` set is freed (the
    /// S32 CF-GRPC-H3-CHURN-RSS leak / DoS vector). `0` (set via
    /// [`Self::new`]) disables the cap — byte-identical to the pre-S36 H3
    /// front (no GOAWAY, no recycle, R3). The binary sets it from
    /// `[runtime].max_requests_per_h3_connection` via
    /// [`Self::with_h3_request_cap`]. Threaded to
    /// [`crate::router::RouterParams::max_requests_per_h3_connection`] →
    /// each actor. Ignored on Mode B (`run_raw_proxy_actor` returns before
    /// any H3 state is built).
    pub max_requests_per_h3_connection: u32,
    /// S36-A: the `h3_*` recycle metric handles, threaded verbatim into
    /// [`crate::router::RouterParams::h3_recycle_metrics`] → each actor.
    /// `Some` once a non-Mode-B QUIC listener is spawned with a metrics
    /// registry; the actor bumps `goaway_sent_total` at the cap and
    /// `connections_recycled_total` after the drain-close. `None` (the
    /// smoke / transport-only path with no registry) ⇒ the actor still
    /// recycles, it just does not record (R3-neutral). Cheap to clone (an
    /// `Arc`-backed `prometheus` bundle).
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
            // SESSION 19 / Mode B (B6): H3-terminate by default — Mode B
            // is opted in via `with_raw_backend`. DATAGRAM stays disabled
            // (R3) until then.
            raw_quic_backend: None,
            dgram_queue_cap: 1_024,
            quic_modeb_metrics: None,
            // SESSION 27 / WS-over-H3: WebSocket opt-in is OFF by default
            // — enabled via `with_websocket`. R3: the H3 settings frame +
            // pseudo-header acceptance stay byte-identical until then.
            ws_enabled: false,
            // SESSION 28 / WS-over-H3 Stage C: no relay launcher by default
            // — injected via `with_ws_relay_launcher` on a WS listener.
            ws_relay_launcher: None,
            // S36-A: H3 request cap DISABLED by default — the binary opts
            // in via `with_h3_request_cap` from
            // `[runtime].max_requests_per_h3_connection`. `0` keeps the H3
            // front byte-identical (no GOAWAY, no recycle, R3) for every
            // smoke / transport-only caller.
            max_requests_per_h3_connection: 0,
            h3_recycle_metrics: None,
        }
    }

    /// S36-A: set the per-connection H3 request cap (+ the recycle metric
    /// handles). The binary calls this from
    /// `[runtime].max_requests_per_h3_connection`; `cap == 0` disables the
    /// cap (the field stays at its `new()` default, byte-identical to the
    /// pre-S36 H3 front, R3). `metrics` is the registered `h3_*` recycle
    /// family, threaded into [`crate::router::RouterParams`] → each actor.
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

    /// SESSION 19 / Mode B (B6): switch this listener to
    /// terminate-and-re-originate Mode B. Sets the raw-QUIC re-origination
    /// `backend`, the DATAGRAM queue cap to advertise to peers
    /// (`enable_dgram(true, cap, cap)`), and the optional `quic_modeb_*`
    /// metric handles. Calling this is the ONLY thing that flips the
    /// client-facing config's `enable_datagrams` to `true` and sets
    /// [`RouterParams::raw_quic_backend`]; without it the listener is
    /// byte-identical H3-terminate (R3).
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

    /// SESSION 27 / WS-over-H3 (RFC 9220) Stage A: opt this listener into
    /// WebSocket. Sets `ws_enabled`, which the listener threads into
    /// [`crate::router::RouterParams::ws_enabled`] → each per-connection
    /// actor. With it set, the H3 server advertises
    /// `SETTINGS_ENABLE_CONNECT_PROTOCOL` and
    /// [`crate::h3_bridge::validate_request_pseudo_headers`] accepts an
    /// RFC 8441/9220 Extended CONNECT (`:method=CONNECT` + `:protocol`).
    /// Without it the H3 front is byte-identical to a non-WS listener
    /// (R3). The actual frame relay is wired in a later stage; this knob
    /// only governs the handshake-acceptance surface.
    #[must_use]
    pub const fn with_websocket(mut self, enabled: bool) -> Self {
        self.ws_enabled = enabled;
        self
    }

    /// SESSION 28 / WS-over-H3 (RFC 9220) Stage C: inject the WebSocket
    /// relay launcher. The `lb` binary builds it (it sees both `lb-quic`
    /// and `lb-l7`, the latter holding the single-sourced
    /// `proxy_frames`); the listener threads it into
    /// [`crate::router::RouterParams::ws_relay_launcher`] → each actor.
    /// Set this alongside [`Self::with_websocket(true)`] on a
    /// WebSocket-opted-in listener; without it a validated extended
    /// CONNECT has no relay to run (the actor answers `502`). `None`
    /// listeners are byte-identical to the pre-S28 H3 front (R3).
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
        // SESSION 19 / Mode B (B6): enable QUIC DATAGRAM support on the
        // client-facing config ONLY for a Mode-B listener. On the H3 path
        // (`raw_quic_backend` is `None`) `enable_datagrams` is `false`, so
        // `build_server_config` does NOT call `enable_dgram` and the
        // advertised transport parameters are byte-identical to today (R3).
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

        // Pool is required for real traffic; if the caller did not
        // supply one (3b.3c-1 smoke path), build a transient in-memory
        // pool so the router's actor can be constructed.
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
            // PROMPT.md §6 target conntrack scale. Auditor-suggested
            // bound (2026-04-23) — finite-memory defence against Initial
            // flooding behind legitimate source-address retry tokens.
            max_connections: 100_000,
            cancel: shutdown.clone(),
            // SESSION 16 / Mode B + SESSION 19 (B6): thread the configured
            // raw-QUIC backend through. `None` keeps this listener on the
            // H3-termination path byte-for-byte (R3); `Some` (set via
            // `QuicListenerParams::with_raw_backend`) hands every accepted
            // connection to the raw-proxy actor.
            raw_quic_backend: params.raw_quic_backend.clone(),
            // SESSION 19 / Mode B (B6): the `quic_modeb_*` handles (always
            // `None` on the H3 path — no metric churn, R3).
            quic_modeb_metrics: params.quic_modeb_metrics.clone(),
            // SESSION 27 / WS-over-H3 Stage A: the WebSocket opt-in.
            ws_enabled: params.ws_enabled,
            // SESSION 28 / WS-over-H3 Stage C: the injected relay launcher
            // (cloned so the `QuicListenerParams` — which is `Clone` — can
            // outlive `spawn`). `None` ⇒ byte-identical H3 front (R3).
            ws_relay_launcher: params.ws_relay_launcher.clone(),
            // S36-A: the per-connection H3 request cap + recycle metrics,
            // threaded verbatim into each actor. `0` ⇒ no cap (R3).
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

/// F-INFRA-01 (S38): on the LOAD path, refuse (strict) or warn (lax) when an
/// existing retry-secret file is group/world-accessible. The secret is the
/// HMAC key behind Retry-token address validation; a world-readable secret
/// lets any local reader forge tokens and bypass the QUIC source-address
/// check. This MIRRORS the TLS-key advisory (`assert_key_perm_advisory` in
/// `lb/src/main.rs`): strict on release, lax (warn-only) on debug builds. The
/// generate path already writes mode 0600 via `write_secret_file`; this
/// closes the asymmetry on the read path.
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

/// Build the client-facing `quiche::Config`.
///
/// SESSION 19 / Mode B (B6): `enable_datagrams` is `true` ONLY for a
/// Mode-B listener — it calls `cfg.enable_dgram(true, dgram_queue_cap,
/// dgram_queue_cap)` so QUIC DATAGRAM (RFC 9221) is advertised to clients
/// (the B4 relay needs it negotiated). When `false` (the H3-termination
/// path) `enable_dgram` is NOT called, so the advertised transport
/// parameters — and therefore the wire-visible config — are byte-identical
/// to before B6 (R3 no-regression). `dgram_queue_cap` is ignored when
/// `enable_datagrams` is `false`.
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
    // R3: only a Mode-B listener advertises DATAGRAM support. The H3 path
    // skips this branch entirely ⇒ unchanged transport params.
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
    //! F-INFRA-01 (S38) regression — the retry-secret LOAD path must
    //! perm-check an existing file (strict=reject, lax=warn), closing the
    //! asymmetry vs the generate path (which already writes 0600).
    use super::{check_retry_secret_perms, load_or_generate_retry_secret, RETRY_SECRET_LEN};
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

    // NEGATIVE CONTROL: a world-readable (0644) existing secret is REJECTED
    // in strict mode. Pre-fix this loaded silently; post-fix it errors.
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

    // A world-readable secret is WARNED (not rejected) in lax mode — load
    // still succeeds (debug-build / operator-convenience parity with TLS key).
    #[test]
    fn world_readable_secret_warns_lax() {
        let p = temp_secret("0644-lax", 0o644);
        let res = check_retry_secret_perms(&p, /* strict */ false);
        let _ = std::fs::remove_file(&p);
        assert!(res.is_ok(), "lax mode must warn-and-continue, not error");
    }

    // A correctly-permissioned (0600) secret passes in strict mode AND the
    // full load path returns a signer (proves the gate doesn't block the
    // legitimate case).
    #[test]
    fn owner_only_secret_passes_strict_and_loads() {
        let p = temp_secret("0600-strict", 0o600);
        assert!(check_retry_secret_perms(&p, true).is_ok());
        let signer = load_or_generate_retry_secret(&p);
        let _ = std::fs::remove_file(&p);
        assert!(signer.is_ok(), "0600 secret must load cleanly");
    }
}
