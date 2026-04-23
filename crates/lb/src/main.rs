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

use tokio_util::sync::CancellationToken;

use lb_balancer::round_robin::RoundRobin;
use lb_balancer::{Backend, LoadBalancer};
use lb_config::{AltSvcConfig, HttpTimeoutsConfig, QuicListenerConfig, TlsConfig};
use lb_io::Runtime;
use lb_io::dns::{DnsResolver, ResolverConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::{BackendSockOpts, ListenerSockOpts};
use lb_l7::h1_proxy::{AltSvcConfig as H1AltSvcConfig, H1Proxy, HttpTimeouts, RoundRobinAddrs};
use lb_l7::h2_proxy::H2Proxy;
use lb_observability::MetricsRegistry;
use lb_quic::{QuicListener, QuicListenerParams};
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
    /// Plain HTTP/1.1 — `lb-l7` `H1Proxy` over the raw TCP stream.
    H1 { proxy: Arc<H1Proxy> },
    /// HTTPS listener that offers HTTP/2 and HTTP/1.1 via ALPN. After
    /// `TlsAcceptor::accept`, the runtime inspects
    /// [`rustls::ServerConnection::alpn_protocol`] and dispatches to the
    /// matching proxy.
    H1s {
        h1_proxy: Arc<H1Proxy>,
        h2_proxy: Arc<H2Proxy>,
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
    /// Shared `lb-io` runtime (auto-detects `io_uring` vs epoll).
    io_runtime: Runtime,
    /// Shared TCP connection pool for backend dials.
    pool: TcpPool,
    /// Shared DNS resolver with positive/negative caching. Used to
    /// pre-resolve backend hostnames today; `TcpPool` will consume it for
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

// ── QUIC helpers ────────────────────────────────────────────────────────

fn quic_listener_params_from_config(
    bind_addr: SocketAddr,
    cfg: &QuicListenerConfig,
) -> QuicListenerParams {
    let mut params = QuicListenerParams::new(
        bind_addr,
        std::path::PathBuf::from(&cfg.cert_path),
        std::path::PathBuf::from(&cfg.key_path),
        std::path::PathBuf::from(&cfg.retry_secret_path),
    );
    params.max_idle_timeout = Duration::from_millis(cfg.max_idle_timeout_ms);
    params.max_recv_udp_payload_size = cfg.max_recv_udp_payload_size;
    params
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
    let server_cfg = build_server_config(Arc::clone(&rot_arc), cert_chain, key, &[])
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

// ── H1 / H1s helpers ────────────────────────────────────────────────────

/// Build a [`H1Proxy`] from a resolved backend address list and the
/// optional `[listeners.alt_svc]` / `[listeners.http]` blocks.
fn build_h1_proxy(
    pool: TcpPool,
    addresses: &[SocketAddr],
    alt_svc_cfg: Option<&AltSvcConfig>,
    http_cfg: Option<&HttpTimeoutsConfig>,
    is_https: bool,
) -> anyhow::Result<Arc<H1Proxy>> {
    let picker = RoundRobinAddrs::new(addresses.to_vec())
        .ok_or_else(|| anyhow::anyhow!("H1 listener requires at least one backend"))?;
    let alt_svc = alt_svc_cfg.map(|a| H1AltSvcConfig {
        h3_port: a.h3_port,
        max_age: a.max_age,
    });
    let timeouts = http_cfg.map_or_else(HttpTimeouts::default, |h| HttpTimeouts {
        header: Duration::from_millis(h.header_timeout_ms),
        body: Duration::from_millis(h.body_timeout_ms),
        total: Duration::from_millis(h.total_timeout_ms),
    });
    Ok(Arc::new(H1Proxy::new(
        pool,
        Arc::new(picker),
        alt_svc,
        timeouts,
        is_https,
    )))
}

/// Build a [`H2Proxy`] sharing the same picker/alt_svc/timeouts shape as
/// the matching [`H1Proxy`]. Used when the `h1s` listener negotiates
/// `h2` via ALPN.
fn build_h2_proxy(
    pool: TcpPool,
    addresses: &[SocketAddr],
    alt_svc_cfg: Option<&AltSvcConfig>,
    http_cfg: Option<&HttpTimeoutsConfig>,
    is_https: bool,
) -> anyhow::Result<Arc<H2Proxy>> {
    let picker = RoundRobinAddrs::new(addresses.to_vec())
        .ok_or_else(|| anyhow::anyhow!("H2 listener requires at least one backend"))?;
    let alt_svc = alt_svc_cfg.map(|a| H1AltSvcConfig {
        h3_port: a.h3_port,
        max_age: a.max_age,
    });
    let timeouts = http_cfg.map_or_else(HttpTimeouts::default, |h| HttpTimeouts {
        header: Duration::from_millis(h.header_timeout_ms),
        body: Duration::from_millis(h.body_timeout_ms),
        total: Duration::from_millis(h.total_timeout_ms),
    });
    Ok(Arc::new(H2Proxy::new(
        pool,
        Arc::new(picker),
        alt_svc,
        timeouts,
        is_https,
    )))
}

/// Build the TLS stack for an `h1s` listener. Same plumbing as the
/// `tls` listener (ticket rotator + cert + key) plus an ALPN advertisement
/// covering HTTP/2 (preferred) and HTTP/1.1 (fallback). The runtime
/// dispatches by negotiated protocol after `TlsAcceptor::accept`.
fn build_h1s_tls_stack(
    tls_cfg: &TlsConfig,
) -> anyhow::Result<(Arc<rustls::ServerConfig>, Arc<PlMutex<TicketRotator>>)> {
    let cert_chain = load_cert_chain(Path::new(&tls_cfg.cert_path))?;
    let key = load_private_key(Path::new(&tls_cfg.key_path))?;
    let interval = Duration::from_secs(tls_cfg.ticket_rotation_interval_seconds);
    let overlap = Duration::from_secs(tls_cfg.ticket_rotation_overlap_seconds);
    let rotator = TicketRotator::new(interval, overlap)
        .map_err(|e| anyhow::anyhow!("ticket rotator init failed: {e}"))?;
    let rot_arc = Arc::new(PlMutex::new(rotator));
    let alpn: &[&[u8]] = &[b"h2", b"http/1.1"];
    let server_cfg = build_server_config(Arc::clone(&rot_arc), cert_chain, key, alpn)
        .map_err(|e| anyhow::anyhow!("rustls ServerConfig build failed: {e}"))?;
    Ok((server_cfg, rot_arc))
}

/// Bind and spawn a [`QuicListener`]. Pulled out of `async_main` to
/// keep its body small enough to satisfy `clippy::too_many_lines`.
async fn spawn_quic(listener_cfg: &lb_config::ListenerConfig) -> anyhow::Result<QuicListener> {
    let Some(quic_cfg) = listener_cfg.quic.as_ref() else {
        anyhow::bail!(
            "listener {} has protocol=quic but no [listeners.quic] block",
            listener_cfg.address
        );
    };
    let bind_addr: SocketAddr = listener_cfg
        .address
        .parse()
        .with_context(|| format!("invalid listen address: {}", listener_cfg.address))?;
    let params = quic_listener_params_from_config(bind_addr, quic_cfg);
    let shutdown = CancellationToken::new();
    let listener = QuicListener::spawn(params, shutdown)
        .await
        .with_context(|| format!("QUIC listener bind failed for {bind_addr}"))?;
    tracing::info!(
        address = %listener.local_addr(),
        protocol = "quic",
        cert = %quic_cfg.cert_path,
        retry_secret = %quic_cfg.retry_secret_path,
        "QUIC listener started"
    );
    Ok(listener)
}

/// Resolve backends, build the listener state, and spawn the accept
/// loop for a TCP/TLS/H1/H1s listener.
async fn spawn_tcp(
    listener_cfg: &lb_config::ListenerConfig,
    pool: &TcpPool,
    resolver: &DnsResolver,
    io_runtime: Runtime,
    metrics: &Arc<MetricsRegistry>,
) -> anyhow::Result<tokio::task::JoinHandle<anyhow::Result<()>>> {
    let mut addresses = Vec::with_capacity(listener_cfg.backends.len());
    let mut backends = Vec::with_capacity(listener_cfg.backends.len());
    for (i, b) in listener_cfg.backends.iter().enumerate() {
        let (host, port) = split_host_port(&b.address)
            .with_context(|| format!("invalid backend address: {}", b.address))?;
        let lookup = resolver
            .resolve(host, port)
            .await
            .with_context(|| format!("cannot resolve backend: {}", b.address))?;
        let Some(first) = lookup.first().copied() else {
            anyhow::bail!("resolver returned no addresses for {}", b.address);
        };
        addresses.push(first);
        backends.push(Backend::new(format!("backend-{i}"), b.weight));
    }
    let mode = build_listener_mode(listener_cfg, pool, &addresses)?;
    let state = Arc::new(ListenerState {
        backends,
        balancer: parking_lot::Mutex::new(RoundRobin::new()),
        addresses,
        metrics: Arc::clone(metrics),
        active_connections: AtomicU64::new(0),
        io_runtime,
        pool: pool.clone(),
        resolver: resolver.clone(),
        mode,
    });
    Ok(tokio::spawn(run_listener(
        listener_cfg.address.clone(),
        state,
    )))
}

/// Build the per-listener [`ListenerMode`] from its config, dispatching
/// on `protocol`. Spawned per listener at startup; `addresses` are the
/// pre-resolved backend `SocketAddr`s for round-robin balancing.
fn build_listener_mode(
    listener_cfg: &lb_config::ListenerConfig,
    pool: &TcpPool,
    addresses: &[SocketAddr],
) -> anyhow::Result<ListenerMode> {
    match listener_cfg.protocol.as_str() {
        "tls" => {
            let Some(tls_cfg) = listener_cfg.tls.as_ref() else {
                anyhow::bail!(
                    "listener {} has protocol=tls but no [listeners.tls] block",
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
                "listener configured with TLS termination"
            );
            Ok(ListenerMode::Tls {
                acceptor,
                _rotator: rotator,
            })
        }
        "h1" => {
            let proxy = build_h1_proxy(
                pool.clone(),
                addresses,
                listener_cfg.alt_svc.as_ref(),
                listener_cfg.http.as_ref(),
                false,
            )
            .with_context(|| format!("H1 setup failed for {}", listener_cfg.address))?;
            tracing::info!(
                address = %listener_cfg.address,
                protocol = "h1",
                alt_svc = ?listener_cfg.alt_svc.as_ref().map(|a| format!("h3:{}", a.h3_port)),
                "listener configured for HTTP/1.1"
            );
            Ok(ListenerMode::H1 { proxy })
        }
        "h1s" => {
            let Some(tls_cfg) = listener_cfg.tls.as_ref() else {
                anyhow::bail!(
                    "listener {} has protocol=h1s but no [listeners.tls] block",
                    listener_cfg.address
                );
            };
            let (server_cfg, rotator) = build_h1s_tls_stack(tls_cfg)
                .with_context(|| format!("H1s TLS setup failed for {}", listener_cfg.address))?;
            let acceptor = TlsAcceptor::from(server_cfg);
            spawn_rotator_ticker(Arc::clone(&rotator));
            let h1_proxy = build_h1_proxy(
                pool.clone(),
                addresses,
                listener_cfg.alt_svc.as_ref(),
                listener_cfg.http.as_ref(),
                true,
            )
            .with_context(|| format!("H1s setup failed for {}", listener_cfg.address))?;
            let h2_proxy = build_h2_proxy(
                pool.clone(),
                addresses,
                listener_cfg.alt_svc.as_ref(),
                listener_cfg.http.as_ref(),
                true,
            )
            .with_context(|| format!("H2s setup failed for {}", listener_cfg.address))?;
            tracing::info!(
                address = %listener_cfg.address,
                protocol = "h1s",
                cert = %tls_cfg.cert_path,
                alpn = "h2,http/1.1",
                alt_svc = ?listener_cfg.alt_svc.as_ref().map(|a| format!("h3:{}", a.h3_port)),
                "listener configured for HTTPS with ALPN (h2 preferred, http/1.1 fallback)"
            );
            Ok(ListenerMode::H1s {
                h1_proxy,
                h2_proxy,
                acceptor,
                _rotator: rotator,
            })
        }
        _ => Ok(ListenerMode::PlainTcp),
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

    // ── DNS resolver ────────────────────────────────────────────────
    let resolver = DnsResolver::new(ResolverConfig::default());
    tracing::info!("DNS resolver ready (positive cap 300s, negative TTL 5s)");

    // ── metrics ─────────────────────────────────────────────────────
    let metrics = Arc::new(MetricsRegistry::new());

    // ── spawn listeners ─────────────────────────────────────────────
    let mut listener_handles = Vec::new();
    let mut quic_listeners: Vec<QuicListener> = Vec::new();

    for listener_cfg in &config.listeners {
        if listener_cfg.protocol == "quic" {
            quic_listeners.push(spawn_quic(listener_cfg).await?);
            continue;
        }
        if listener_cfg.backends.is_empty() {
            tracing::warn!(
                address = %listener_cfg.address,
                "listener has no backends configured — skipping"
            );
            continue;
        }
        let handle = spawn_tcp(listener_cfg, &pool, &resolver, io_runtime, &metrics).await?;
        listener_handles.push(handle);
    }

    if listener_handles.is_empty() && quic_listeners.is_empty() {
        anyhow::bail!("no listeners started — check your configuration");
    }

    // ── shutdown ────────────────────────────────────────────────────
    shutdown_signal().await;
    tracing::info!("shutdown signal received — draining connections");

    // Cancel TCP/TLS listener tasks (they will stop accepting new connections).
    for h in &listener_handles {
        h.abort();
    }

    // Cancel QUIC listeners — each holds a CancellationToken and returns
    // a JoinHandle on shutdown().
    let mut quic_drain_handles = Vec::with_capacity(quic_listeners.len());
    for listener in quic_listeners {
        quic_drain_handles.push(listener.shutdown());
    }
    let quic_drain_deadline = Duration::from_secs(2);
    for handle in quic_drain_handles {
        if tokio::time::timeout(quic_drain_deadline, handle)
            .await
            .is_err()
        {
            tracing::warn!("QUIC listener did not drain within {quic_drain_deadline:?}");
        }
    }

    // Allow a brief drain period for any remaining TCP connections.
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
                ListenerMode::H1 { proxy } => Arc::clone(proxy)
                    .serve_connection(client_stream, client_addr)
                    .await
                    .map_err(anyhow::Error::from),
                ListenerMode::H1s {
                    h1_proxy,
                    h2_proxy,
                    acceptor,
                    ..
                } => match acceptor.accept(client_stream).await {
                    Ok(tls_stream) => {
                        // ALPN-based dispatch: h2 → H2Proxy, anything else
                        // (http/1.1 or unknown) → H1Proxy. rustls returns
                        // the negotiated protocol via
                        // `ServerConnection::alpn_protocol()` on the inner
                        // `(io, conn)` tuple of `TlsStream`.
                        let alpn = tls_stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                        if alpn.as_deref() == Some(b"h2".as_ref()) {
                            Arc::clone(h2_proxy)
                                .serve_connection(tls_stream, client_addr)
                                .await
                                .map_err(anyhow::Error::from)
                        } else {
                            Arc::clone(h1_proxy)
                                .serve_connection(tls_stream, client_addr)
                                .await
                                .map_err(anyhow::Error::from)
                        }
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
