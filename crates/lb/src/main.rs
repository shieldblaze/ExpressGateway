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

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;

use lb_balancer::round_robin::RoundRobin;
use lb_balancer::{Backend, LoadBalancer};
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::{BackendSockOpts, ListenerSockOpts};
use lb_observability::MetricsRegistry;

// ── shared gateway state ────────────────────────────────────────────────

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

        // Resolve backend addresses at startup.
        let mut addresses = Vec::with_capacity(listener_cfg.backends.len());
        let mut backends = Vec::with_capacity(listener_cfg.backends.len());

        for (i, b) in listener_cfg.backends.iter().enumerate() {
            let addr: SocketAddr = b
                .address
                .parse()
                .with_context(|| format!("invalid backend address: {}", b.address))?;
            addresses.push(addr);
            backends.push(Backend::new(format!("backend-{i}"), b.weight));
        }

        let state = Arc::new(ListenerState {
            backends,
            balancer: parking_lot::Mutex::new(RoundRobin::new()),
            addresses,
            metrics: Arc::clone(&metrics),
            active_connections: AtomicU64::new(0),
            io_runtime,
            pool: pool.clone(),
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

            if let Err(e) =
                proxy_connection(client_stream, backend_addr, &st.metrics, &st.pool).await
            {
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

async fn proxy_connection(
    mut client: TcpStream,
    backend_addr: SocketAddr,
    metrics: &MetricsRegistry,
    pool: &TcpPool,
) -> anyhow::Result<()> {
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
