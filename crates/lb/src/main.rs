//! `ExpressGateway` — L4/L7 load balancer entry point.
//!
//! Boots a multi-threaded Tokio runtime, loads TOML configuration, binds TCP
//! listeners, and proxies connections across upstream backends using
//! round-robin load balancing.
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

use std::io::BufReader;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use parking_lot::Mutex as PlMutex;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{self, AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::signal;
use tokio_rustls::TlsAcceptor;

use lb_balancer::round_robin::RoundRobin;
use lb_balancer::{Backend, LoadBalancer};
use lb_config::TlsConfig;
use lb_io::Runtime;
use lb_io::dns::{DnsResolver, ResolverConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::{BackendSockOpts, ListenerSockOpts};
use lb_observability::MetricsRegistry;
use lb_security::{TicketRotator, build_server_config};

// ── shared gateway state ────────────────────────────────────────────────

/// How a listener terminates inbound traffic.
enum ListenerMode {
    /// Plain TCP — no TLS, forward the socket directly.
    PlainTcp,
    /// TLS over TCP — terminate with the shared rustls acceptor and
    /// forward the decrypted stream. The `_rotator` handle is held so
    /// the background ticket-rotation ticker stays alive as long as the
    /// listener does (the ticker exits when its last `Arc` drops).
    Tls {
        acceptor: TlsAcceptor,
        _rotator: Arc<PlMutex<TicketRotator>>,
    },
}

/// Per-listener runtime state.
struct ListenerState {
    /// Balancer backends built from the config.
    backends: Vec<Backend>,
    /// Round-robin load balancer instance.
    balancer: parking_lot::Mutex<RoundRobin>,
    /// Resolved backend addresses for connecting.
    addresses: Vec<SocketAddr>,
    /// Shared metrics registry.
    metrics: Arc<MetricsRegistry>,
    /// Active connection gauge.
    active_connections: AtomicU64,
    /// Shared lb-io runtime (auto-detects io_uring vs epoll).
    io_runtime: Runtime,
    /// Shared TCP connection pool for backend dials.
    pool: TcpPool,
    /// Shared DNS resolver with positive/negative caching. Used to
    /// pre-resolve backend hostnames today; TcpPool will consume it for
    /// on-demand re-resolution in a follow-up.
    #[allow(dead_code)]
    resolver: DnsResolver,
    /// Listener termination mode (plain TCP or TLS over TCP).
    mode: ListenerMode,
}

/// Listener socket options matching PROMPT.md §7 for listener sockets.
const fn listener_opts() -> ListenerSockOpts {
    ListenerSockOpts {
        reuseaddr: true,
        reuseport: true,
        rcvbuf: Some(262_144),
        sndbuf: Some(262_144),
        nodelay: true,
        quickack: false,
        keepalive: true,
        tcp_fastopen: None,
        backlog: Some(50_000),
    }
}

/// Backend socket options matching PROMPT.md §7 for backend sockets.
const fn backend_opts() -> BackendSockOpts {
    BackendSockOpts {
        nodelay: true,
        keepalive: true,
        rcvbuf: Some(262_144),
        sndbuf: Some(262_144),
        quickack: false,
        tcp_fastopen_connect: false,
    }
}

/// Split a backend address of the form `host:port`, `[v6]:port`, or
/// `1.2.3.4:port` into its components. Returns an error if the port is
/// missing or malformed.
fn split_host_port(s: &str) -> anyhow::Result<(&str, u16)> {
    // IPv6 literal: `[addr]:port`.
    if let Some(rest) = s.strip_prefix('[') {
        if let Some((host, tail)) = rest.split_once(']') {
            let port_str = tail
                .strip_prefix(':')
                .ok_or_else(|| anyhow::anyhow!("missing port after IPv6 literal"))?;
            let port: u16 = port_str
                .parse()
                .with_context(|| format!("invalid port: {port_str}"))?;
            return Ok((host, port));
        }
        anyhow::bail!("unterminated IPv6 literal");
    }
    let (host, port_str) = s
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("missing port in {s}"))?;
    let port: u16 = port_str
        .parse()
        .with_context(|| format!("invalid port: {port_str}"))?;
    Ok((host, port))
}

// ── TLS helpers ─────────────────────────────────────────────────────────

/// Load a PEM-encoded certificate chain from `path`. Returns an error if
/// the file cannot be read or contains no certificates.
fn load_cert_chain(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("cannot open TLS cert {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parse TLS cert PEM from {}", path.display()))?;
    if certs.is_empty() {
        anyhow::bail!("no certificates found in {}", path.display());
    }
    Ok(certs)
}

/// Load the first PEM-encoded private key from `path`. Accepts PKCS#8,
/// SEC1 (EC), or RSA PKCS#1 formats.
fn load_private_key(path: &Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("cannot open TLS key {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let key = rustls_pemfile::private_key(&mut reader)
        .with_context(|| format!("parse TLS key PEM from {}", path.display()))?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", path.display()))?;
    Ok(key)
}

/// Build the shared rustls `ServerConfig` for a listener, wiring the
/// per-listener `TicketRotator` through `lb_security::build_server_config`.
fn build_tls_stack(
    tls_cfg: &TlsConfig,
) -> anyhow::Result<(Arc<rustls::ServerConfig>, Arc<PlMutex<TicketRotator>>)> {
    let cert_chain = load_cert_chain(Path::new(&tls_cfg.cert_path))?;
    let key = load_private_key(Path::new(&tls_cfg.key_path))?;
    let interval = Duration::from_secs(tls_cfg.ticket_rotation_interval_seconds);
    let overlap = Duration::from_secs(tls_cfg.ticket_rotation_overlap_seconds);
    let rotator = TicketRotator::new(interval, overlap)
        .map_err(|e| anyhow::anyhow!("ticket rotator init failed: {e}"))?;
    let rot_arc = Arc::new(PlMutex::new(rotator));
    let server_cfg = build_server_config(Arc::clone(&rot_arc), cert_chain, key)
        .map_err(|e| anyhow::anyhow!("rustls ServerConfig build failed: {e}"))?;
    Ok((server_cfg, rot_arc))
}

/// Spawn a background task that nudges `rotator.rotate_if_due(now)` once
/// per minute. The task stops when the rotator's `Arc` strong count
/// drops to 1 (i.e. when the listener is gone).
fn spawn_rotator_ticker(rotator: Arc<PlMutex<TicketRotator>>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        // The first tick fires immediately; skip it so we don't rotate
        // a freshly-minted key.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if Arc::strong_count(&rotator) <= 1 {
                return;
            }
            let mut guard = rotator.lock();
            match guard.rotate_if_due(Instant::now()) {
                Ok(true) => {
                    tracing::info!("TLS ticket key rotated");
                }
                Ok(false) => {}
                Err(e) => tracing::error!("TLS ticket rotation failed: {e}"),
            }
        }
    });
}

// ── main ────────────────────────────────────────────────────────────────

/// Application entry point.
///
/// Builds a Tokio runtime manually (avoiding `#[tokio::main]` which
/// generates an internal `.unwrap()`).
fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;

    rt.block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    // ── tracing ─────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("ExpressGateway v{}", env!("CARGO_PKG_VERSION"));

    // ── config ──────────────────────────────────────────────────────
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config/default.toml".to_owned());

    let config_str = std::fs::read_to_string(&config_path)
        .with_context(|| format!("cannot read config file: {config_path}"))?;

    let config = lb_config::parse_config(&config_str).context("config parse error")?;
    lb_config::validate_config(&config).context("config validation error")?;

    tracing::info!(
        listeners = config.listeners.len(),
        "configuration loaded from {config_path}"
    );

    // ── lb-io runtime ───────────────────────────────────────────────
    let io_runtime = Runtime::new();
    tracing::info!(
        backend = %io_runtime.backend(),
        high_water = Runtime::high_water_mark(),
        low_water = Runtime::low_water_mark(),
        "lb-io runtime ready"
    );

    // ── backend connection pool ─────────────────────────────────────
    let pool = TcpPool::new(PoolConfig::default(), backend_opts(), io_runtime);
    tracing::info!("TCP backend pool ready (defaults from PROMPT.md §21)");

    // ── DNS resolver ────────────────────────────────────────────────
    let resolver = DnsResolver::new(ResolverConfig::default());
    tracing::info!("DNS resolver ready (positive cap 300s, negative TTL 5s)");

    // ── metrics ─────────────────────────────────────────────────────
    let metrics = Arc::new(MetricsRegistry::new());

    // ── spawn listeners ─────────────────────────────────────────────
    let mut listener_handles = Vec::new();

    for listener_cfg in &config.listeners {
        if listener_cfg.backends.is_empty() {
            tracing::warn!(
                address = %listener_cfg.address,
                "listener has no backends configured — skipping"
            );
            continue;
        }

        // Resolve backend addresses at startup via the DNS cache. This
        // makes hostname-configured backends (e.g. `origin.example:443`)
        // work and warms the cache so the first proxied connection does
        // not pay the getaddrinfo cost. IP-literal backends go down the
        // same path; the resolver forwards to `(host, port).to_socket_addrs()`
        // which handles both.
        let mut addresses = Vec::with_capacity(listener_cfg.backends.len());
        let mut backends = Vec::with_capacity(listener_cfg.backends.len());

        for (i, b) in listener_cfg.backends.iter().enumerate() {
            let (host, port) = split_host_port(&b.address)
                .with_context(|| format!("invalid backend address: {}", b.address))?;
            let resolved = resolver
                .resolve(host, port)
                .await
                .with_context(|| format!("cannot resolve backend: {}", b.address))?;
            let Some(first) = resolved.first().copied() else {
                anyhow::bail!("resolver returned no addresses for {}", b.address);
            };
            tracing::debug!(
                backend = %b.address,
                resolved_count = resolved.len(),
                chosen = %first,
                "backend resolved via DnsResolver"
            );
            addresses.push(first);
            backends.push(Backend::new(format!("backend-{i}"), b.weight));
        }

        let mode = match listener_cfg.protocol.as_str() {
            "tls" => {
                let Some(tls_cfg) = listener_cfg.tls.as_ref() else {
                    anyhow::bail!(
                        "listener {} has protocol=tls but no [listeners.tls] block \
                         (validate_config should have caught this)",
                        listener_cfg.address
                    );
                };
                let (server_cfg, rotator) = build_tls_stack(tls_cfg)
                    .with_context(|| format!("TLS setup failed for {}", listener_cfg.address))?;
                let acceptor = TlsAcceptor::from(server_cfg);
                spawn_rotator_ticker(Arc::clone(&rotator));
                tracing::info!(
                    address = %listener_cfg.address,
                    protocol = "tls",
                    cert = %tls_cfg.cert_path,
                    rotation_interval_s = tls_cfg.ticket_rotation_interval_seconds,
                    overlap_s = tls_cfg.ticket_rotation_overlap_seconds,
                    "listener configured with TLS termination"
                );
                ListenerMode::Tls {
                    acceptor,
                    _rotator: rotator,
                }
            }
            _ => ListenerMode::PlainTcp,
        };

        let state = Arc::new(ListenerState {
            backends,
            balancer: parking_lot::Mutex::new(RoundRobin::new()),
            addresses,
            metrics: Arc::clone(&metrics),
            active_connections: AtomicU64::new(0),
            io_runtime,
            pool: pool.clone(),
            resolver: resolver.clone(),
            mode,
        });

        let bind_addr = listener_cfg.address.clone();
        let handle = tokio::spawn(run_listener(bind_addr, state));
        listener_handles.push(handle);
    }

    if listener_handles.is_empty() {
        anyhow::bail!("no listeners started — check your configuration");
    }

    // ── shutdown ────────────────────────────────────────────────────
    shutdown_signal().await;
    tracing::info!("shutdown signal received — draining connections");

    // Cancel listener tasks (they will stop accepting new connections).
    for h in &listener_handles {
        h.abort();
    }

    // Allow a brief drain period.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let total = metrics.get("connections_total").unwrap_or(0);
    let bytes_in = metrics.get("bytes_client_to_backend").unwrap_or(0);
    let bytes_out = metrics.get("bytes_backend_to_client").unwrap_or(0);
    tracing::info!(
        total_connections = total,
        bytes_in,
        bytes_out,
        "ExpressGateway stopped"
    );

    Ok(())
}

// ── listener loop ───────────────────────────────────────────────────────

async fn run_listener(bind_addr: String, state: Arc<ListenerState>) -> anyhow::Result<()> {
    let parsed: SocketAddr = bind_addr
        .parse()
        .with_context(|| format!("invalid listen address: {bind_addr}"))?;

    let std_listener = state
        .io_runtime
        .listen(parsed, &listener_opts())
        .with_context(|| format!("failed to bind {bind_addr}"))?;
    std_listener
        .set_nonblocking(true)
        .with_context(|| format!("set_nonblocking on {bind_addr}"))?;
    let listener = TcpListener::from_std(std_listener)
        .with_context(|| format!("tokio from_std on {bind_addr}"))?;

    tracing::info!(
        address = %bind_addr,
        backends = state.addresses.len(),
        backend = %state.io_runtime.backend(),
        "listener started"
    );

    loop {
        let (client_stream, client_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!("accept error: {e}");
                continue;
            }
        };

        // Pick a backend.
        let backend_idx = {
            let mut balancer = state.balancer.lock();
            match balancer.pick(&state.backends) {
                Ok(idx) => idx,
                Err(e) => {
                    tracing::error!("balancer pick failed: {e}");
                    continue;
                }
            }
        };

        let Some(backend_addr) = state.addresses.get(backend_idx).copied() else {
            tracing::error!(idx = backend_idx, "backend index out of range");
            continue;
        };

        let st = Arc::clone(&state);
        tokio::spawn(async move {
            st.active_connections.fetch_add(1, Ordering::Relaxed);
            st.metrics.increment("connections_total", 1);

            let result = match &st.mode {
                ListenerMode::PlainTcp => {
                    proxy_connection(client_stream, backend_addr, &st.metrics, &st.pool).await
                }
                ListenerMode::Tls { acceptor, .. } => match acceptor.accept(client_stream).await {
                    Ok(tls_stream) => {
                        proxy_connection(tls_stream, backend_addr, &st.metrics, &st.pool).await
                    }
                    Err(e) => Err(e.into()),
                },
            };

            if let Err(e) = result {
                tracing::debug!(
                    client = %client_addr,
                    backend = %backend_addr,
                    "proxy session ended: {e}"
                );
            }

            st.active_connections.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

// ── TCP proxy ───────────────────────────────────────────────────────────

async fn proxy_connection<C>(
    mut client: C,
    backend_addr: SocketAddr,
    metrics: &MetricsRegistry,
    pool: &TcpPool,
) -> anyhow::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
{
    // Acquire a backend connection from the pool. On a hot pool this
    // returns an idle reuse; otherwise it dials a new socket via
    // Runtime::connect, which performs a blocking connect(2). Wrap in
    // spawn_blocking so we don't stall the tokio worker on the cold
    // path — pool.acquire is cheap on the hot path so the always-spawn
    // overhead is acceptable today and matches the prior baseline.
    let pool_for_dial = pool.clone();
    let mut pooled = tokio::task::spawn_blocking(move || pool_for_dial.acquire(backend_addr))
        .await
        .with_context(|| format!("backend acquire task joined {backend_addr}"))?
        .with_context(|| format!("cannot connect to backend {backend_addr}"))?;

    let copy_result = {
        let backend = pooled
            .stream_mut()
            .with_context(|| format!("pooled stream missing for {backend_addr}"))?;
        io::copy_bidirectional(&mut client, backend).await
    };

    match copy_result {
        Ok((client_to_backend, backend_to_client)) => {
            metrics.increment("bytes_client_to_backend", client_to_backend);
            metrics.increment("bytes_backend_to_client", backend_to_client);
            // Half-close from either side is normal at end-of-session;
            // the pool's liveness probe will reject the socket on the
            // next acquire if it is no longer reusable.
            Ok(())
        }
        Err(e) => {
            // Stream is in an unknown state; do not return it to the
            // pool to avoid serving the next request a broken socket.
            pooled.set_reusable(false);
            Err(e.into())
        }
    }
}

// ── signal handling ─────────────────────────────────────────────────────

#[allow(clippy::redundant_pub_crate)]
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .unwrap_or_else(|_| tracing::warn!("failed to listen for ctrl-c"));
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!("failed to listen for SIGTERM: {e}");
                // Fall back to waiting forever (ctrl_c will still work).
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
