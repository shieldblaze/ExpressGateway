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
use lb_config::{
    AltSvcConfig, GrpcListenerConfig, H2SecurityConfig, HttpTimeoutsConfig, QuicListenerConfig,
    TlsConfig, WebsocketConfig,
};
use lb_io::Runtime;
use lb_io::dns::{DnsResolver, ResolverConfig};
use lb_io::http2_pool::{Http2Pool, Http2PoolConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::quic_pool::{QuicPoolConfig, QuicUpstreamPool};
use lb_io::sockopts::{BackendSockOpts, ListenerSockOpts};
use lb_l7::grpc_proxy::{GrpcConfig, GrpcProxy};
use lb_l7::h1_proxy::{AltSvcConfig as H1AltSvcConfig, H1Proxy, HttpTimeouts};
use lb_l7::h2_proxy::H2Proxy;
use lb_l7::h2_security::H2SecurityThresholds;
use lb_l7::upstream::{RoundRobinUpstreams, UpstreamBackend, UpstreamProto};
use lb_l7::ws_proxy::{WsConfig, WsProxy};
use lb_observability::{MetricsRegistry, admin_http, http_latency_buckets};
use lb_quic::{QuicListener, QuicListenerParams};
use lb_security::{TicketRotator, build_server_config};

mod xdp;

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

/// Parse a backend `protocol` string into [`UpstreamProto`]. Accepts
/// the same set [`lb_config::validate_config`] does (`"tcp"` and `"h1"`
/// both map to [`UpstreamProto::H1`]; raw TCP plays HTTP/1.1 on the
/// L7 wire today). Returns an error naming the offending value so a
/// misconfigured TOML lands a clear message at startup rather than a
/// silent fallback.
fn parse_upstream_proto(s: &str) -> anyhow::Result<UpstreamProto> {
    match s {
        "tcp" | "h1" => Ok(UpstreamProto::H1),
        "h2" => Ok(UpstreamProto::H2),
        "h3" => Ok(UpstreamProto::H3),
        other => Err(anyhow::anyhow!(
            "unknown backend protocol {other:?} (expected one of: tcp, h1, h2, h3)"
        )),
    }
}

/// Build the [`UpstreamBackend`] vector for a listener by zipping the
/// resolved `addresses` with each [`lb_config::BackendConfig::protocol`].
/// `addresses[i]` must correspond to `backends[i]` — `spawn_tcp` already
/// enforces that ordering.
fn build_upstream_backends(
    listener_cfg: &lb_config::ListenerConfig,
    addresses: &[SocketAddr],
) -> anyhow::Result<Vec<UpstreamBackend>> {
    if listener_cfg.backends.is_empty() {
        anyhow::bail!(
            "listener {} has no backends configured",
            listener_cfg.address
        );
    }
    if addresses.len() != listener_cfg.backends.len() {
        anyhow::bail!(
            "listener {}: resolved {} addresses for {} backends",
            listener_cfg.address,
            addresses.len(),
            listener_cfg.backends.len()
        );
    }
    let mut out = Vec::with_capacity(listener_cfg.backends.len());
    for (i, b) in listener_cfg.backends.iter().enumerate() {
        let proto = parse_upstream_proto(b.protocol.as_str()).with_context(|| {
            format!(
                "listener {} backend {i} (address {})",
                listener_cfg.address, b.address
            )
        })?;
        let Some(addr) = addresses.get(i).copied() else {
            anyhow::bail!(
                "listener {}: address slot {i} missing for backend {}",
                listener_cfg.address,
                b.address
            );
        };
        // SNI for H3 backends is required by the upstream pool's TLS
        // handshake. The config does not yet carry an explicit SNI knob;
        // fall back to the host portion of the configured address so a
        // hostname-shaped backend still works. IP-literal addresses
        // produce an empty SNI which the pool surfaces as a TLS error.
        let sni = if proto == UpstreamProto::H3 {
            split_host_port(&b.address)
                .ok()
                .map(|(host, _)| host.to_owned())
        } else {
            None
        };
        out.push(UpstreamBackend { addr, proto, sni });
    }
    Ok(out)
}

/// Build an [`Http2Pool`] sized to the listener's [`H2SecurityConfig`]
/// where supplied, falling back to [`Http2PoolConfig`]'s defaults
/// otherwise. Wires through [`H2SecurityConfig::max_concurrent_streams`]
/// so the per-upstream stream cap matches the frontend's policy.
fn build_h2_upstream_pool(
    tcp_pool: TcpPool,
    h2_security_cfg: Option<&H2SecurityConfig>,
) -> Arc<Http2Pool> {
    let mut cfg = Http2PoolConfig::default();
    if let Some(c) = h2_security_cfg {
        if let Some(v) = c.max_concurrent_streams {
            cfg.max_concurrent_streams = v;
        }
        if let Some(v) = c.initial_stream_window_size {
            cfg.initial_stream_window = v;
        }
        if let Some(ms) = c.keep_alive_interval_ms {
            cfg.keep_alive_interval = Duration::from_millis(ms);
        }
        if let Some(ms) = c.keep_alive_timeout_ms {
            cfg.keep_alive_timeout = Duration::from_millis(ms);
        }
    }
    Arc::new(Http2Pool::new(cfg, tcp_pool))
}

/// Build a [`QuicUpstreamPool`] with a config factory that produces
/// fresh [`quiche::Config`]s. The factory installs [`LB_QUIC_ALPN`]
/// (`"lb-quic"`) and inherits the listener's QUIC tunables when present,
/// or [`QuicPoolConfig`] defaults otherwise.
///
/// Note: this is a **plaintext / no-peer-verify** dial config. Production
/// deployments dialling untrusted H3 origins must extend this builder
/// with a CA bundle via
/// [`quiche::Config::load_verify_locations_from_file`] +
/// [`quiche::Config::verify_peer(true)`]. Tracked as a v1 limitation in
/// `docs/gap-analysis.md`.
fn build_h3_upstream_pool() -> Arc<QuicUpstreamPool> {
    let factory: Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> =
        Arc::new(|| {
            let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
            cfg.set_application_protos(&[b"lb-quic"])?;
            cfg.verify_peer(false);
            cfg.set_max_idle_timeout(30_000);
            cfg.set_max_recv_udp_payload_size(1_350);
            cfg.set_max_send_udp_payload_size(1_350);
            cfg.set_initial_max_data(1024 * 1024);
            cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
            cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
            cfg.set_initial_max_stream_data_uni(64 * 1024);
            cfg.set_initial_max_streams_bidi(64);
            cfg.set_initial_max_streams_uni(64);
            cfg.set_disable_active_migration(true);
            Ok(cfg)
        });
    Arc::new(QuicUpstreamPool::new(QuicPoolConfig::default(), factory))
}

/// Build a [`H1Proxy`] from the listener's resolved upstream backends
/// and optional H2/H3 upstream pools.
///
/// Wraps the picker into an [`Arc<RoundRobinUpstreams>`] and threads
/// through the multi-protocol surface ([`H1Proxy::with_multi_proto`])
/// so a single listener can fan out to mixed-protocol backends in one
/// round-robin cycle.
fn build_h1_proxy(
    pool: TcpPool,
    upstreams: Vec<UpstreamBackend>,
    h2_pool: Option<Arc<Http2Pool>>,
    h3_pool: Option<Arc<QuicUpstreamPool>>,
    alt_svc_cfg: Option<&AltSvcConfig>,
    http_cfg: Option<&HttpTimeoutsConfig>,
    ws_cfg: Option<&WebsocketConfig>,
    is_https: bool,
) -> anyhow::Result<Arc<H1Proxy>> {
    let picker = RoundRobinUpstreams::new(upstreams)
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
    let mut proxy = H1Proxy::with_multi_proto(pool, Arc::new(picker), alt_svc, timeouts, is_https);
    if let Some(h2) = h2_pool {
        proxy = proxy.with_h2_upstream(h2);
    }
    if let Some(h3) = h3_pool {
        proxy = proxy.with_h3_upstream(h3);
    }
    if let Some(ws) = ws_cfg {
        proxy = proxy.with_websocket(Arc::new(WsProxy::new(ws_config_to_runtime(ws))));
    }
    Ok(Arc::new(proxy))
}

/// Translate the TOML `[listeners.websocket]` block to the runtime
/// [`WsConfig`]. Centralised so H1 and H2 paths agree byte-for-byte.
fn ws_config_to_runtime(cfg: &WebsocketConfig) -> WsConfig {
    WsConfig {
        idle_timeout: Duration::from_secs(cfg.idle_timeout_seconds),
        max_message_size: cfg.max_message_size_bytes,
        enabled: cfg.enabled,
        ping_rate_limit_per_window: cfg.ping_rate_limit_per_window,
        ping_rate_limit_window: Duration::from_secs(cfg.ping_rate_limit_window_seconds),
        read_frame_timeout: Duration::from_secs(cfg.read_frame_timeout_seconds),
    }
}

/// Build a [`H2Proxy`] sharing the same picker/alt_svc/timeouts shape as
/// the matching [`H1Proxy`]. Used when the `h1s` listener negotiates
/// `h2` via ALPN.
fn build_h2_proxy(
    pool: TcpPool,
    upstreams: Vec<UpstreamBackend>,
    h2_pool: Option<Arc<Http2Pool>>,
    h3_pool: Option<Arc<QuicUpstreamPool>>,
    alt_svc_cfg: Option<&AltSvcConfig>,
    http_cfg: Option<&HttpTimeoutsConfig>,
    h2_security_cfg: Option<&H2SecurityConfig>,
    ws_cfg: Option<&WebsocketConfig>,
    grpc_cfg: Option<&GrpcListenerConfig>,
    is_https: bool,
) -> anyhow::Result<Arc<H2Proxy>> {
    let picker = RoundRobinUpstreams::new(upstreams)
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
    let security = merge_h2_security(h2_security_cfg);
    let mut proxy = H2Proxy::with_multi_proto(
        pool.clone(),
        Arc::new(picker),
        alt_svc,
        timeouts,
        is_https,
        security,
    );
    if let Some(h2) = h2_pool {
        proxy = proxy.with_h2_upstream(h2);
    }
    if let Some(h3) = h3_pool {
        proxy = proxy.with_h3_upstream(h3);
    }
    if let Some(ws) = ws_cfg {
        proxy = proxy.with_websocket(Arc::new(WsProxy::new(ws_config_to_runtime(ws))));
    }
    if let Some(grpc) = grpc_cfg {
        proxy = proxy.with_grpc(GrpcProxy::new(grpc_config_to_runtime(grpc), pool.clone()));
    }
    Ok(Arc::new(proxy))
}

/// Translate the TOML `[listeners.grpc]` block to the runtime
/// [`GrpcConfig`].
fn grpc_config_to_runtime(cfg: &GrpcListenerConfig) -> GrpcConfig {
    GrpcConfig {
        enabled: cfg.enabled,
        max_deadline: Duration::from_secs(cfg.max_deadline_seconds),
        health_synthesized: cfg.health_synthesized,
    }
}

/// Merge the optional TOML block into the default `H2SecurityThresholds`.
/// Every field in the TOML block is itself optional; unset fields inherit
/// the detector-derived default. This keeps threshold sources of truth
/// centralised in `lb_h2::security` while still letting an operator
/// override a single knob in the config file.
fn merge_h2_security(cfg: Option<&H2SecurityConfig>) -> H2SecurityThresholds {
    let mut t = H2SecurityThresholds::default();
    if let Some(c) = cfg {
        if let Some(v) = c.max_pending_accept_reset_streams {
            t.max_pending_accept_reset_streams = v;
        }
        if let Some(v) = c.max_local_error_reset_streams {
            t.max_local_error_reset_streams = v;
        }
        if let Some(v) = c.max_concurrent_streams {
            t.max_concurrent_streams = v;
        }
        if let Some(v) = c.max_header_list_size {
            t.max_header_list_size = v;
        }
        if let Some(v) = c.max_send_buf_size {
            t.max_send_buf_size = v;
        }
        if let Some(ms) = c.keep_alive_interval_ms {
            t.keep_alive_interval = if ms == 0 {
                None
            } else {
                Some(Duration::from_millis(ms))
            };
        }
        if let Some(ms) = c.keep_alive_timeout_ms {
            t.keep_alive_timeout = Duration::from_millis(ms);
        }
        if let Some(v) = c.initial_stream_window_size {
            t.initial_stream_window_size = v;
        }
        if let Some(v) = c.initial_connection_window_size {
            t.initial_connection_window_size = v;
        }
    }
    t
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
        let pre_cache = resolver.cache_size();
        let lookup = resolver
            .resolve(host, port)
            .await
            .with_context(|| format!("cannot resolve backend: {}", b.address))?;
        let grew = resolver.cache_size() > pre_cache;
        let name = if grew {
            ("dns_cache_misses_total", "DNS resolver cache misses")
        } else {
            ("dns_cache_hits_total", "DNS resolver cache hits")
        };
        if let Ok(c) = metrics.counter(name.0, name.1) {
            c.inc();
        }
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
            let upstreams = build_upstream_backends(listener_cfg, addresses)?;
            let needs_h2 = upstreams.iter().any(|b| b.proto == UpstreamProto::H2);
            let needs_h3 = upstreams.iter().any(|b| b.proto == UpstreamProto::H3);
            let h2_pool = needs_h2
                .then(|| build_h2_upstream_pool(pool.clone(), listener_cfg.h2_security.as_ref()));
            let h3_pool = needs_h3.then(build_h3_upstream_pool);
            let proxy = build_h1_proxy(
                pool.clone(),
                upstreams,
                h2_pool,
                h3_pool,
                listener_cfg.alt_svc.as_ref(),
                listener_cfg.http.as_ref(),
                listener_cfg.websocket.as_ref(),
                false,
            )
            .with_context(|| format!("H1 setup failed for {}", listener_cfg.address))?;
            tracing::info!(
                address = %listener_cfg.address,
                protocol = "h1",
                alt_svc = ?listener_cfg.alt_svc.as_ref().map(|a| format!("h3:{}", a.h3_port)),
                upstream_h2 = needs_h2,
                upstream_h3 = needs_h3,
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
            let upstreams_h1 = build_upstream_backends(listener_cfg, addresses)?;
            let upstreams_h2 = upstreams_h1.clone();
            let needs_h2 = upstreams_h1.iter().any(|b| b.proto == UpstreamProto::H2);
            let needs_h3 = upstreams_h1.iter().any(|b| b.proto == UpstreamProto::H3);
            // Share the H2/H3 upstream pools between the H1 + H2 proxies
            // wired to this listener — they dial the same backends, so a
            // single multiplex'd H2 connection or pooled QUIC conn serves
            // both ALPN paths.
            let h2_pool = needs_h2
                .then(|| build_h2_upstream_pool(pool.clone(), listener_cfg.h2_security.as_ref()));
            let h3_pool = needs_h3.then(build_h3_upstream_pool);
            let h1_proxy = build_h1_proxy(
                pool.clone(),
                upstreams_h1,
                h2_pool.clone(),
                h3_pool.clone(),
                listener_cfg.alt_svc.as_ref(),
                listener_cfg.http.as_ref(),
                listener_cfg.websocket.as_ref(),
                true,
            )
            .with_context(|| format!("H1s setup failed for {}", listener_cfg.address))?;
            let h2_proxy = build_h2_proxy(
                pool.clone(),
                upstreams_h2,
                h2_pool,
                h3_pool,
                listener_cfg.alt_svc.as_ref(),
                listener_cfg.http.as_ref(),
                listener_cfg.h2_security.as_ref(),
                listener_cfg.websocket.as_ref(),
                listener_cfg.grpc.as_ref(),
                true,
            )
            .with_context(|| format!("H2s setup failed for {}", listener_cfg.address))?;
            tracing::info!(
                address = %listener_cfg.address,
                protocol = "h1s",
                cert = %tls_cfg.cert_path,
                alpn = "h2,http/1.1",
                alt_svc = ?listener_cfg.alt_svc.as_ref().map(|a| format!("h3:{}", a.h3_port)),
                upstream_h2 = needs_h2,
                upstream_h3 = needs_h3,
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

// ── hot-path metrics (Task #21) ────────────────────────────────────────

/// Register the 5 hot-path metric handles and spawn the background
/// sampler that reads them from the shared [`TcpPool`] /
/// [`DnsResolver`]. We register up-front (rather than lazily at each
/// call site) so the registry always advertises the metric even before
/// the first event, and so a single type-mismatch or registration
/// failure lands at startup rather than on the hot path.
fn install_hotpath_metrics(metrics: &Arc<MetricsRegistry>, pool: &TcpPool, resolver: &DnsResolver) {
    // Pool + DNS counters (pre-register so /metrics shows them at 0).
    if let Err(e) = metrics.counter("pool_acquires_total", "TcpPool acquire attempts") {
        tracing::warn!(metric = "pool_acquires_total", error = %e, "counter register failed");
    }
    if let Err(e) = metrics.counter("pool_probe_failures_total", "TcpPool probe failures") {
        tracing::warn!(metric = "pool_probe_failures_total", error = %e, "counter register failed");
    }
    if let Err(e) = metrics.counter("dns_cache_hits_total", "DNS resolver cache hits") {
        tracing::warn!(metric = "dns_cache_hits_total", error = %e, "counter register failed");
    }
    if let Err(e) = metrics.counter("dns_cache_misses_total", "DNS resolver cache misses") {
        tracing::warn!(metric = "dns_cache_misses_total", error = %e, "counter register failed");
    }

    // http_requests_total{version,status_class}. Referenced by the H1
    // proxy wrapper installed in proxy_connection — pre-register so
    // scrape shape is stable from t0.
    if let Err(e) = metrics.counter_vec(
        "http_requests_total",
        "HTTP requests terminated by the L7 proxy",
        &["version", "status_class"],
    ) {
        tracing::warn!(metric = "http_requests_total", error = %e, "counter_vec register failed");
    }
    if let Err(e) = metrics.histogram_vec(
        "http_request_duration_seconds",
        "L7 request duration from accept to response body sent",
        &["version"],
        &http_latency_buckets(),
    ) {
        tracing::warn!(metric = "http_request_duration_seconds", error = %e, "histogram_vec register failed");
    }
    if let Err(e) = metrics.gauge("pool_idle_gauge", "TcpPool idle connection count") {
        tracing::warn!(metric = "pool_idle_gauge", error = %e, "gauge register failed");
    }

    // Background sampler: lift the pool's idle count + DNS cache size
    // into the registry every second. Neither crate publishes change
    // events today, so a periodic pull is the least invasive wiring.
    let pool_clone = pool.clone();
    let resolver_clone = resolver.clone();
    let metrics_clone = Arc::clone(metrics);
    tokio::spawn(async move {
        let idle_gauge =
            match metrics_clone.gauge("pool_idle_gauge", "TcpPool idle connection count") {
                Ok(g) => g,
                Err(_) => return,
            };
        let dns_entries_gauge =
            match metrics_clone.gauge("dns_cache_entries", "DNS resolver cache size") {
                Ok(g) => g,
                Err(_) => return,
            };
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            #[allow(clippy::cast_possible_wrap)]
            idle_gauge.set(pool_clone.idle_count() as i64);
            #[allow(clippy::cast_possible_wrap)]
            dns_entries_gauge.set(resolver_clone.cache_size() as i64);
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

    // ── optional XDP data-plane attach (Pillar 4b-1) ────────────────
    // Held for the process lifetime; dropping the `XdpLoader` on Linux
    // lets aya tear down the kernel-side attachment. Non-Linux / no-op
    // when disabled.
    let _xdp_loader = if let Some(rt) = config.runtime.as_ref() {
        xdp::try_attach_xdp(rt)
    } else {
        None
    };

    // ── metrics ─────────────────────────────────────────────────────
    let metrics = Arc::new(MetricsRegistry::new());
    install_hotpath_metrics(&metrics, &pool, &resolver);

    // ── optional admin HTTP listener (GET /metrics, GET /healthz) ──
    let admin_cancel = CancellationToken::new();
    if let Some(obs) = config.observability.as_ref() {
        if let Some(bind_str) = obs.metrics_bind.as_deref() {
            let bind_addr: SocketAddr = bind_str
                .trim()
                .parse()
                .with_context(|| format!("invalid observability.metrics_bind: {bind_str}"))?;
            match admin_http::serve(Arc::clone(&metrics), bind_addr, admin_cancel.clone()).await {
                Ok(local) => tracing::info!(
                    address = %local,
                    protocol = "admin-http",
                    "admin listener started (/metrics, /healthz)"
                ),
                Err(e) => {
                    tracing::error!(bind = %bind_addr, error = %e, "admin listener bind failed");
                }
            }
        }
    }

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
    // Stop admin HTTP listener (if any).
    admin_cancel.cancel();

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

            let http_start = Instant::now();
            let mut http_version: Option<&'static str> = None;
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
                ListenerMode::H1 { proxy } => {
                    http_version = Some("h1");
                    Arc::clone(proxy)
                        .serve_connection(client_stream, client_addr)
                        .await
                        .map_err(anyhow::Error::from)
                }
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
                            http_version = Some("h2");
                            Arc::clone(h2_proxy)
                                .serve_connection(tls_stream, client_addr)
                                .await
                                .map_err(anyhow::Error::from)
                        } else {
                            http_version = Some("h1");
                            Arc::clone(h1_proxy)
                                .serve_connection(tls_stream, client_addr)
                                .await
                                .map_err(anyhow::Error::from)
                        }
                    }
                    Err(e) => Err(e.into()),
                },
            };

            // Metric: http_requests_total{version, status_class} +
            // http_request_duration_seconds{version}. Connection-level
            // today — per-request instrumentation requires a hook
            // inside lb-l7 and is tracked as follow-up.
            if let Some(version) = http_version {
                let status_class = if result.is_ok() { "2xx" } else { "5xx" };
                if let Ok(v) = st.metrics.counter_vec(
                    "http_requests_total",
                    "HTTP requests terminated by the L7 proxy",
                    &["version", "status_class"],
                ) {
                    v.with_label_values(&[version, status_class]).inc();
                }
                if let Ok(h) = st.metrics.histogram_vec(
                    "http_request_duration_seconds",
                    "L7 request duration from accept to response body sent",
                    &["version"],
                    &http_latency_buckets(),
                ) {
                    h.with_label_values(&[version])
                        .observe(http_start.elapsed().as_secs_f64());
                }
            }

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
    if let Ok(c) = metrics.counter("pool_acquires_total", "TcpPool acquire attempts") {
        c.inc();
    }
    let pool_for_dial = pool.clone();
    let mut pooled = match tokio::task::spawn_blocking(move || pool_for_dial.acquire(backend_addr))
        .await
        .with_context(|| format!("backend acquire task joined {backend_addr}"))?
    {
        Ok(p) => p,
        Err(e) => {
            if let Ok(c) = metrics.counter("pool_probe_failures_total", "TcpPool probe failures") {
                c.inc();
            }
            return Err(e).with_context(|| format!("cannot connect to backend {backend_addr}"));
        }
    };

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
