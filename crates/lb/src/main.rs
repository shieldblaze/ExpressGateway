//! `ExpressGateway` — L4/L7 load balancer entry point.
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
#![allow(clippy::pedantic, clippy::nursery, clippy::too_many_arguments)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use arc_swap::ArcSwap;
use parking_lot::Mutex as PlMutex;
use prometheus::IntCounter;
use tokio::io::{self, AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
#[cfg(not(unix))]
use tokio::signal;
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;

use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use lb_balancer::round_robin::RoundRobin;
use lb_balancer::{Backend, LoadBalancer};
use lb_config::{
    AltSvcConfig, GrpcListenerConfig, H2SecurityConfig, HttpTimeoutsConfig, QuicListenerConfig,
    TlsConfig, WebsocketConfig,
};
use lb_controlplane::{ConfigManager, FileBackend};
use lb_health::{HealthChecker, HealthStatus};
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
use lb_quic::{
    PassthroughListener, PassthroughParams, QuicListener, QuicListenerParams, RawBackend,
};
use lb_security::{
    ConnGate, HooksBundle, SecurityHooks, SmuggleMode, TicketRotator, Watchdog, WatchdogConfig,
};

mod xdp;

/// CODE-2-02: registry-backed `panic_total`; the fallback atomic counts panics fired before the
/// registry exists and is drained into it on bind.
static PANIC_TOTAL_COUNTER: OnceLock<IntCounter> = OnceLock::new();
static PANIC_TOTAL_FALLBACK: AtomicU64 = AtomicU64::new(0);

/// CODE-2-02: log + count + abort on panic. Dev/test builds unwind (proptest/loom), so the explicit
/// `abort()` keeps the release failure mode.
fn init_panic_hook() {
    use std::backtrace::Backtrace;
    std::panic::set_hook(Box::new(|info| {
        let bt = Backtrace::force_capture();
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_owned());
        let payload = if let Some(s) = info.payload().downcast_ref::<&'static str>() {
            (*s).to_owned()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_owned()
        };
        if let Some(c) = PANIC_TOTAL_COUNTER.get() {
            c.inc();
        } else {
            PANIC_TOTAL_FALLBACK.fetch_add(1, Ordering::Release);
        }
        tracing::error!(
            target: "panic",
            location = %location,
            payload = %payload,
            backtrace = %bt,
            "process panic — aborting"
        );
        std::thread::sleep(Duration::from_millis(50));
        std::process::abort();
    }));
}

fn bind_panic_counter(metrics: &MetricsRegistry) {
    match metrics.panic_total_counter() {
        Ok(c) => {
            let pre = PANIC_TOTAL_FALLBACK.swap(0, Ordering::AcqRel);
            if pre > 0 {
                c.inc_by(pre);
            }
            let _ = PANIC_TOTAL_COUNTER.set(c);
        }
        Err(e) => {
            tracing::warn!(error = %e, "panic_total counter registration failed");
        }
    }
}

#[allow(dead_code)]
fn panic_total() -> u64 {
    let from_registry = PANIC_TOTAL_COUNTER.get().map_or(0, IntCounter::get);
    let from_fallback = PANIC_TOTAL_FALLBACK.load(Ordering::Acquire);
    from_registry.saturating_add(from_fallback)
}

#[derive(Clone)]
struct TlsReloadEntry {
    listener: String,
    cert_path: PathBuf,
    key_path: PathBuf,
    alpn: Vec<Vec<u8>>,
    bundle: lb_security::SharedTlsBundle,
    /// Held so a reload re-installs the SAME ticketer — session-ticket resumption survives a cert
    /// swap.
    rotator: Arc<PlMutex<TicketRotator>>,
}

#[derive(Clone)]
enum ReloadableProxies {
    /// Plain HTTP/1.1 listener.
    H1 { proxy: SharedH1Proxy },
    H1s {
        h1_proxy: SharedH1Proxy,
        h2_proxy: SharedH2Proxy,
    },
}

/// One entry per config-reloadable L7 listener (`h1`/`h1s`). Plain-TCP/TLS/QUIC listeners are
/// deliberately absent — the diff reports their changes as restart-required.
#[derive(Clone)]
struct ListenerReloadEntry {
    listener: String,
    proxies: ReloadableProxies,
}

#[derive(Clone)]
struct CertMetrics {
    succeeded_total: prometheus::IntCounter,
    failed_total: prometheus::IntCounterVec,
    loaded_at_seconds: prometheus::IntGaugeVec,
}

impl CertMetrics {
    fn register(metrics: &MetricsRegistry) -> Option<Self> {
        let succeeded_total = metrics
            .counter(
                "cert_rotation_succeeded_total",
                "REL-2-03: number of successful TLS cert reloads (SIGUSR1 or inotify)",
            )
            .ok()?;
        let failed_total = metrics
            .counter_vec(
                "cert_rotation_failed_total",
                "REL-2-03: number of failed TLS cert reloads, labelled by reason",
                &["reason"],
            )
            .ok()?;
        let loaded_at_seconds = metrics
            .gauge_vec(
                "cert_loaded_at_seconds",
                "REL-2-03: wall-clock unix timestamp the listener's TLS bundle was last (re)loaded",
                &["listener"],
            )
            .ok()?;
        Some(Self {
            succeeded_total,
            failed_total,
            loaded_at_seconds,
        })
    }
}

#[derive(Clone)]
struct ReloadMetrics {
    succeeded_total: prometheus::IntCounter,
    failed_total: prometheus::IntCounter,
    applied_swappable_total: prometheus::IntCounterVec,
    /// HONESTY metric: detected changes that require a restart and were NOT applied, labelled by
    /// field.
    restart_required_fields_total: prometheus::IntCounterVec,
    applied_version: prometheus::IntGauge,
}

impl ReloadMetrics {
    fn register(metrics: &MetricsRegistry) -> Option<Self> {
        let succeeded_total = metrics
            .counter(
                "config_reload_succeeded_total",
                "S37-C: number of SIGHUP config reloads that applied the swappable subset",
            )
            .ok()?;
        let failed_total = metrics
            .counter(
                "config_reload_failed_total",
                "S37-C: number of SIGHUP config reloads rejected by validation (nothing applied)",
            )
            .ok()?;
        let applied_swappable_total = metrics
            .counter_vec(
                "config_reload_applied_swappable_total",
                "S37-C: swappable config changes applied live, labelled by field",
                &["field"],
            )
            .ok()?;
        let restart_required_fields_total = metrics
            .counter_vec(
                "config_reload_restart_required_fields_total",
                "S37-C: detected config changes that require a restart and were NOT applied \
                 (honesty metric), labelled by field",
                &["field"],
            )
            .ok()?;
        let applied_version = metrics
            .gauge(
                "config_reload_applied_version",
                "S37-C: monotonic version of the currently-applied config (1 = boot)",
            )
            .ok()?;
        applied_version.set(1);
        Some(Self {
            succeeded_total,
            failed_total,
            applied_swappable_total,
            restart_required_fields_total,
            applied_version,
        })
    }
}

fn reload_all_tls(registry: &[TlsReloadEntry], metrics: Option<&CertMetrics>) -> (usize, usize) {
    let mut ok = 0_usize;
    let mut fail = 0_usize;
    for entry in registry {
        let alpn_slices: Vec<&[u8]> = entry.alpn.iter().map(Vec::as_slice).collect();
        let ticketer = lb_security::RotatingTicketer::ticketer_from(Arc::clone(&entry.rotator));
        match lb_security::reload_tls_bundle(
            &entry.bundle,
            &entry.cert_path,
            &entry.key_path,
            &alpn_slices,
            Some(ticketer),
        ) {
            Ok(()) => {
                ok += 1;
                if let Some(m) = metrics {
                    m.succeeded_total.inc();
                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0_i64, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
                    m.loaded_at_seconds
                        .with_label_values(&[entry.listener.as_str()])
                        .set(now_secs);
                }
                tracing::info!(
                    listener = %entry.listener,
                    cert = %entry.cert_path.display(),
                    key = %entry.key_path.display(),
                    "REL-2-03 TLS cert reload succeeded"
                );
            }
            Err(e) => {
                fail += 1;
                let reason = e.reason();
                if let Some(m) = metrics {
                    m.failed_total.with_label_values(&[reason]).inc();
                }
                tracing::warn!(
                    listener = %entry.listener,
                    reason,
                    error = %e,
                    "REL-2-03 TLS cert reload failed — keeping previous bundle live"
                );
            }
        }
    }
    (ok, fail)
}

/// S37-C: validate-first SIGHUP config hot-reload. Re-runs the FULL `parse_config` +
/// `validate_config` (the control plane only checks TOML shape); any failure rolls back and applies
/// NOTHING.
#[allow(clippy::too_many_arguments)]
async fn reload_config(
    config_manager: Option<&mut ConfigManager>,
    applied_config: &mut lb_config::LbConfig,
    listener_reload_registry: &Arc<PlMutex<Vec<ListenerReloadEntry>>>,
    pool: &TcpPool,
    resolver: &DnsResolver,
    hooks: &Arc<HooksBundle>,
    watchdog: Option<&Watchdog>,
    metrics: Option<&ReloadMetrics>,
) {
    let Some(mgr) = config_manager else {
        tracing::warn!("SIGHUP received but control-plane manager is unavailable — cannot reload");
        if let Some(m) = metrics {
            m.failed_total.inc();
        }
        return;
    };

    // `Ok(false)` ⇒ byte-identical to current; `Err` ⇒ already rejected without mutating current.
    match mgr.reload() {
        Ok(false) => {
            tracing::info!("SIGHUP: config file unchanged — nothing to reload");
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, "SIGHUP: config reload rejected (TOML shape) — keeping live config");
            if let Some(m) = metrics {
                m.failed_total.inc();
            }
            return;
        }
        Ok(true) => {}
    }

    let new_config = match lb_config::parse_config(mgr.current_config()) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "SIGHUP: new config failed to parse into LbConfig — rolling back, keeping live config");
            let _ = mgr.rollback_to_previous();
            if let Some(m) = metrics {
                m.failed_total.inc();
            }
            return;
        }
    };
    if let Err(e) = lb_config::validate_config(&new_config) {
        tracing::warn!(error = %e, "SIGHUP: new config failed validation — rolling back, keeping live config");
        let _ = mgr.rollback_to_previous();
        if let Some(m) = metrics {
            m.failed_total.inc();
        }
        return;
    }

    let plan = applied_config.diff(&new_config);

    // HONESTY: a reload carrying ONLY restart-required changes is logged as such, never a silent
    // success.
    for change in &plan.restart_required {
        tracing::warn!(field = change.field(), "SIGHUP: {}", change.describe());
        if let Some(m) = metrics {
            m.restart_required_fields_total
                .with_label_values(&[change.field()])
                .inc();
        }
    }

    // Rebuild the UNION of affected listeners so each is rebuilt exactly once — one rebuild reads
    // the whole new listener config, so it applies every co-changed swappable field.
    let new_keepalive = new_config
        .runtime
        .as_ref()
        .map_or(100, |r| r.max_keepalive_requests);
    let registry: Vec<ListenerReloadEntry> = listener_reload_registry.lock().clone();
    let keepalive_changed = plan
        .swappable
        .iter()
        .any(|c| matches!(c, lb_config::SwappableChange::RuntimeMaxKeepaliveRequests));

    let mut to_rebuild: Vec<String> = plan
        .swappable
        .iter()
        .filter_map(lb_config::SwappableChange::address)
        .map(str::to_owned)
        .collect();
    if keepalive_changed {
        for e in &registry {
            if !to_rebuild.iter().any(|a| a == &e.listener) {
                to_rebuild.push(e.listener.clone());
            }
        }
    }

    let mut applied_count = 0_usize;
    for address in &to_rebuild {
        let Some(new_l) = new_config.listeners.iter().find(|l| &l.address == address) else {
            continue; // listener no longer present (removed) — handled elsewhere
        };
        let Some(entry) = registry.iter().find(|e| &e.listener == address) else {
            tracing::warn!(
                listener = %address,
                "SIGHUP: swappable change detected but no L7 swap handle — not applied (requires restart)"
            );
            if let Some(m) = metrics {
                m.restart_required_fields_total
                    .with_label_values(&["listener.l7.no_handle"])
                    .inc();
            }
            continue;
        };
        match rebuild_l7_proxies(
            new_l,
            &entry.proxies,
            pool,
            resolver,
            hooks,
            watchdog,
            new_keepalive,
        )
        .await
        {
            Ok(()) => {
                applied_count += 1;
                tracing::info!(listener = %address, "SIGHUP: L7 proxy rebuilt + swapped (new config applied)");
            }
            Err(e) => {
                tracing::warn!(
                    listener = %address,
                    error = %e,
                    "SIGHUP: L7 swap rebuild failed — keeping previous proxy live"
                );
                if let Some(m) = metrics {
                    m.failed_total.inc();
                }
            }
        }
    }
    for change in &plan.swappable {
        if let Some(m) = metrics {
            m.applied_swappable_total
                .with_label_values(&[change.field()])
                .inc();
        }
        tracing::info!(field = change.field(), "SIGHUP: {}", change.describe());
    }

    *applied_config = new_config;
    if let Some(m) = metrics {
        if applied_count > 0 {
            m.succeeded_total.inc();
            m.applied_version.inc();
        }
    }
    tracing::info!(
        applied_swappable = applied_count,
        restart_required = plan.restart_required.len(),
        "SIGHUP config reload pass complete"
    );
}

/// S37-C: rebuild one listener's L7 proxies and atomically `.store()` them; the OLD proxy stays
/// live until in-flight connections drop.
///
/// HONESTY INVARIANT: applies exactly the fields [`lb_config::LbConfig::diff`] calls swappable and
/// PRESERVES the process-wide `hooks` bundle (`strict_te` + per-IP gate) — which is why `diff`
/// reports those as restart-required. Changing the set here without reclassifying it in `diff`
/// breaks the invariant the verifier tests.
async fn rebuild_l7_proxies(
    new_l: &lb_config::ListenerConfig,
    handles: &ReloadableProxies,
    pool: &TcpPool,
    resolver: &DnsResolver,
    hooks: &Arc<HooksBundle>,
    watchdog: Option<&Watchdog>,
    max_keepalive_requests: u32,
) -> anyhow::Result<()> {
    let mut addresses = Vec::with_capacity(new_l.backends.len());
    for b in &new_l.backends {
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
    }

    let hooks_arc_dyn: Arc<dyn lb_l7::security_hooks::DynSecurityHooks> =
        Arc::clone(hooks) as Arc<_>;
    let upstreams_h1 = build_upstream_backends(new_l, &addresses)?;
    let needs_h2 = upstreams_h1.iter().any(|b| b.proto == UpstreamProto::H2);
    let needs_h3 = upstreams_h1.iter().any(|b| b.proto == UpstreamProto::H3);
    let h2_pool =
        needs_h2.then(|| build_h2_upstream_pool(pool.clone(), new_l.h2_security.as_ref()));
    let h3_pool = if needs_h3 {
        Some(build_h3_upstream_pool(&collect_h3_backends(new_l))?)
    } else {
        None
    };

    match handles {
        ReloadableProxies::H1 { proxy } => {
            let rebuilt = build_h1_proxy(
                pool.clone(),
                upstreams_h1,
                h2_pool,
                h3_pool,
                new_l.alt_svc.as_ref(),
                new_l.http.as_ref(),
                new_l.websocket.as_ref(),
                false,
                hooks_arc_dyn,
                watchdog.cloned(),
                max_keepalive_requests,
            )
            .with_context(|| format!("H1 rebuild failed for {}", new_l.address))?;
            proxy.store(rebuilt);
        }
        ReloadableProxies::H1s { h1_proxy, h2_proxy } => {
            let upstreams_h2 = upstreams_h1.clone();
            let rebuilt_h1 = build_h1_proxy(
                pool.clone(),
                upstreams_h1,
                h2_pool.clone(),
                h3_pool.clone(),
                new_l.alt_svc.as_ref(),
                new_l.http.as_ref(),
                new_l.websocket.as_ref(),
                true,
                Arc::clone(&hooks_arc_dyn),
                watchdog.cloned(),
                max_keepalive_requests,
            )
            .with_context(|| format!("H1s (h1 leg) rebuild failed for {}", new_l.address))?;
            let rebuilt_h2 = build_h2_proxy(
                pool.clone(),
                upstreams_h2,
                h2_pool,
                h3_pool,
                new_l.alt_svc.as_ref(),
                new_l.http.as_ref(),
                new_l.h2_security.as_ref(),
                new_l.websocket.as_ref(),
                new_l.grpc.as_ref(),
                true,
                hooks_arc_dyn,
                watchdog.cloned(),
            )
            .with_context(|| format!("H1s (h2 leg) rebuild failed for {}", new_l.address))?;
            // Two independent atomic RCU swaps. A connection snapshotting between them still gets a
            // consistent view (each proxy is internally consistent) and ALPN picks exactly one leg
            // per
            // connection, so there is no cross-leg tearing.
            h1_proxy.store(rebuilt_h1);
            h2_proxy.store(rebuilt_h2);
        }
    }
    Ok(())
}

enum ListenerMode {
    /// Plain TCP — forward the socket directly.
    PlainTcp,
    /// TLS over TCP; `_rotator` is held so the ticket-rotation ticker stays alive.
    Tls {
        bundle: lb_security::SharedTlsBundle,
        _rotator: Arc<PlMutex<TicketRotator>>,
    },
    /// Plain HTTP/1.1.
    H1 { proxy: SharedH1Proxy },
    /// HTTPS offering h2 + http/1.1 via ALPN.
    H1s {
        h1_proxy: SharedH1Proxy,
        h2_proxy: SharedH2Proxy,
        bundle: lb_security::SharedTlsBundle,
        _rotator: Arc<PlMutex<TicketRotator>>,
    },
}

type SharedH1Proxy = Arc<ArcSwap<H1Proxy>>;

type SharedH2Proxy = Arc<ArcSwap<H2Proxy>>;

struct ListenerState {
    backends: Vec<Backend>,
    balancer: parking_lot::Mutex<RoundRobin>,
    addresses: Vec<SocketAddr>,
    metrics: Arc<MetricsRegistry>,
    active_connections: AtomicU64,
    io_runtime: Runtime,
    /// Held only so its idle-count sampler keeps running; the plain-TCP path dials directly via
    /// `TcpStream::connect`.
    #[allow(dead_code)]
    pool: TcpPool,
    #[allow(dead_code)]
    resolver: DnsResolver,
    mode: ListenerMode,
    handshake_timeout: Duration,
    inflight: Arc<Semaphore>,
    connect_timeout: Duration,
    hooks: Arc<HooksBundle>,
    shutdown_token: CancellationToken,
    /// OPS-04+L4-12: child of `shutdown_token`. The accept loop selects on it so drain phase 4
    /// stops accepting WITHOUT cancelling in-flight connections — that is phase 5.
    listener_cancel_token: CancellationToken,
    tracker: TaskTracker,
    listener_label: Arc<String>,
    per_conn_drain_jitter_ms: u64,
}

struct AcceptInflightGuard {
    metrics: Arc<MetricsRegistry>,
    listener: Arc<String>,
}

impl AcceptInflightGuard {
    fn new(metrics: Arc<MetricsRegistry>, listener: Arc<String>) -> Self {
        metrics.accept_inflight_inc(listener.as_str());
        Self { metrics, listener }
    }
}

impl Drop for AcceptInflightGuard {
    fn drop(&mut self) {
        self.metrics.accept_inflight_dec(self.listener.as_str());
    }
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

/// Split a backend address of the form `host:port`, `[v6]:port`, or `1.2.3.4:port` into its
/// components.
fn split_host_port(s: &str) -> anyhow::Result<(&str, u16)> {
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

fn quic_listener_params_from_config(
    bind_addr: SocketAddr,
    cfg: &QuicListenerConfig,
    raw_backend: Option<RawBackend>,
    quic_modeb_metrics: Option<lb_observability::QuicModeBMetrics>,
    max_requests_per_h3_connection: u32,
    h3_recycle_metrics: Option<lb_observability::QuicH3RecycleMetrics>,
) -> QuicListenerParams {
    let mut params = QuicListenerParams::new(
        bind_addr,
        std::path::PathBuf::from(&cfg.cert_path),
        std::path::PathBuf::from(&cfg.key_path),
        std::path::PathBuf::from(&cfg.retry_secret_path),
    );
    params.max_idle_timeout = Duration::from_millis(cfg.max_idle_timeout_ms);
    params.max_recv_udp_payload_size = cfg.max_recv_udp_payload_size;
    // S36-A: `with_h3_request_cap` is a no-op for `cap == 0` (byte-identical pre-S36 front, R3).
    params = params.with_h3_request_cap(max_requests_per_h3_connection, h3_recycle_metrics);
    // `with_raw_backend` is the ONLY thing that enables datagrams on the client-facing config;
    // absent ⇒ byte-identical H3.
    if let Some(backend) = raw_backend {
        params = params.with_raw_backend(
            backend,
            cfg.raw_proxy
                .as_ref()
                .map_or(1_024, |rp| rp.dgram_queue_cap),
            quic_modeb_metrics,
        );
    }
    params
}

fn assert_key_perm_advisory(path: &Path) -> anyhow::Result<()> {
    let strict = !cfg!(debug_assertions);
    match lb_security::assert_owner_only(path, strict) {
        Ok(lb_security::KeyPermAdvice::Ok | lb_security::KeyPermAdvice::NotApplicable) => Ok(()),
        Ok(lb_security::KeyPermAdvice::TooPermissive { mode }) => {
            tracing::warn!(
                key = %path.display(),
                mode = format!("{mode:o}"),
                "TLS key file permissions wider than 0o600 — tighten with `chmod 600`"
            );
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!(
            "TLS key permission check failed for {}: {e}",
            path.display()
        )),
    }
}

fn build_tls_bundle(
    tls_cfg: &TlsConfig,
    alpn: &[&[u8]],
) -> anyhow::Result<(lb_security::SharedTlsBundle, Arc<PlMutex<TicketRotator>>)> {
    assert_key_perm_advisory(Path::new(&tls_cfg.key_path))?;
    let interval = Duration::from_secs(tls_cfg.ticket_rotation_interval_seconds);
    let overlap = Duration::from_secs(tls_cfg.ticket_rotation_overlap_seconds);
    let rotator = TicketRotator::new(interval, overlap)
        .map_err(|e| anyhow::anyhow!("ticket rotator init failed: {e}"))?;
    let rot_arc = Arc::new(PlMutex::new(rotator));
    let ticketer = lb_security::RotatingTicketer::ticketer_from(Arc::clone(&rot_arc));
    let bundle = lb_security::TlsConfigBundle::load_from_paths_with(
        Path::new(&tls_cfg.cert_path),
        Path::new(&tls_cfg.key_path),
        alpn,
        lb_security::DEFAULT_MAX_CHAIN_DEPTH,
        Some(ticketer),
    )
    .map_err(|e| {
        anyhow::anyhow!(
            "TLS bundle load failed for cert={:?} key={:?}: {e}",
            tls_cfg.cert_path,
            tls_cfg.key_path
        )
    })?;
    Ok((bundle.into_shared(), rot_arc))
}

fn spawn_rotator_ticker(
    rotator: Arc<PlMutex<TicketRotator>>,
    tracker: TaskTracker,
    cancel: CancellationToken,
) {
    tracker.spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        // The first tick fires immediately; skip it so we don't rotate a freshly-minted key.
        ticker.tick().await;
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    tracing::debug!("ticket rotator ticker shutting down");
                    return;
                }
                _ = ticker.tick() => {}
            }
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

/// `addresses[i]` MUST correspond to `backends[i]` — `spawn_tcp` enforces that ordering.
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
        // An explicit `tls_verify_hostname` wins, so an IP-literal address can be matched against
        // the cert-name the backend actually presents.
        let sni = if proto == UpstreamProto::H3 {
            b.tls_verify_hostname.clone().or_else(|| {
                split_host_port(&b.address)
                    .ok()
                    .map(|(host, _)| host.to_owned())
            })
        } else {
            None
        };
        out.push(UpstreamBackend { addr, proto, sni });
    }
    Ok(out)
}

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

fn collect_h3_backends(listener_cfg: &lb_config::ListenerConfig) -> Vec<lb_config::BackendConfig> {
    listener_cfg
        .backends
        .iter()
        .filter(|b| b.protocol == "h3")
        .cloned()
        .collect()
}

fn build_h3_upstream_pool(
    h3_backends: &[lb_config::BackendConfig],
) -> anyhow::Result<Arc<QuicUpstreamPool>> {
    let mut iter = h3_backends.iter();
    let Some(first) = iter.next() else {
        anyhow::bail!("build_h3_upstream_pool called with zero H3 backends");
    };
    for other in iter {
        if other.tls_verify_peer != first.tls_verify_peer || other.tls_ca_path != first.tls_ca_path
        {
            anyhow::bail!(
                "H3 backends on a single listener must share tls_verify_peer + \
                 tls_ca_path (mismatch between {} and {}); one QuicUpstreamPool \
                 cannot dial multiple distinct trust roots",
                first.address,
                other.address
            );
        }
    }
    let verify = first.tls_verify_peer;
    let ca_path = first.tls_ca_path.clone();
    if verify && ca_path.as_deref().is_none_or(str::is_empty) {
        anyhow::bail!(
            "H3 backend {} requires tls_ca_path for verification; \
             set it or explicitly opt out via tls_verify_peer = false (NOT RECOMMENDED)",
            first.address
        );
    }
    let factory: Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> =
        Arc::new(move || {
            let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
            cfg.set_application_protos(&[b"lb-quic"])?;
            if verify {
                if let Some(path) = ca_path.as_deref() {
                    cfg.load_verify_locations_from_file(path)?;
                }
                cfg.verify_peer(true);
            } else {
                cfg.verify_peer(false);
            }
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
    Ok(Arc::new(QuicUpstreamPool::new(
        QuicPoolConfig::default(),
        factory,
    )))
}

fn build_raw_quic_backend(cfg: &lb_config::RawQuicProxyConfig) -> anyhow::Result<RawBackend> {
    let addr: SocketAddr = cfg.backend_addr.parse().with_context(|| {
        format!(
            "invalid Mode B raw_proxy backend_addr: {}",
            cfg.backend_addr
        )
    })?;
    let ca_path = cfg.backend_ca_path.clone();
    let dgram_cap = cfg.dgram_queue_cap;
    let factory: Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> =
        Arc::new(move || {
            let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
            // Default ALPN; dial_dedicated overrides per-connection to mirror the client's
            // protocol.
            config.set_application_protos(lb_io::quic_pool::UPSTREAM_H3_ALPN_PROTOS)?;
            // Backend-trust: verify_peer is ALWAYS on, never silently disabled. Without a CA
            // bundle, fall back to BoringSSL default roots.
            if let Some(path) = ca_path.as_deref() {
                config.load_verify_locations_from_file(path)?;
            }
            config.verify_peer(true);
            config.set_max_idle_timeout(30_000);
            config.set_max_recv_udp_payload_size(1_350);
            config.set_max_send_udp_payload_size(1_350);
            config.set_initial_max_data(10 * 1024 * 1024);
            config.set_initial_max_stream_data_bidi_local(1024 * 1024);
            config.set_initial_max_stream_data_bidi_remote(1024 * 1024);
            config.set_initial_max_stream_data_uni(1024 * 1024);
            config.set_initial_max_streams_bidi(64);
            config.set_initial_max_streams_uni(64);
            config.set_disable_active_migration(true);
            config.enable_dgram(true, dgram_cap, dgram_cap);
            Ok(config)
        });
    let pool = QuicUpstreamPool::new(QuicPoolConfig::default(), factory);
    Ok(RawBackend {
        pool,
        addr,
        sni: cfg.sni.clone(),
        // B6 (R14/R12): one operator value drives both the wire-advertised queue length and the
        // relay's own queue/admit ceiling.
        dgram_queue_cap: cfg.dgram_queue_cap,
        max_relay_streams: cfg.max_relay_streams,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_h1_proxy(
    pool: TcpPool,
    upstreams: Vec<UpstreamBackend>,
    h2_pool: Option<Arc<Http2Pool>>,
    h3_pool: Option<Arc<QuicUpstreamPool>>,
    alt_svc_cfg: Option<&AltSvcConfig>,
    http_cfg: Option<&HttpTimeoutsConfig>,
    ws_cfg: Option<&WebsocketConfig>,
    is_https: bool,
    hooks: Arc<dyn lb_l7::security_hooks::DynSecurityHooks>,
    watchdog: Option<Watchdog>,
    max_keepalive_requests: u32,
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
        head: Duration::from_millis(h.head_timeout_ms),
    });
    let mut proxy = H1Proxy::with_multi_proto(pool, Arc::new(picker), alt_svc, timeouts, is_https);
    proxy = proxy.with_max_keepalive_requests(max_keepalive_requests);
    proxy = proxy.with_hooks(hooks);
    if let Some(wd) = watchdog {
        proxy = proxy.with_watchdog(wd);
    }
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

/// WS-over-H3 (RFC 9220) relay launcher — only the binary sees both `lb-quic`'s `H3WsTunnel` seam
/// and `lb-l7`'s `proxy_frames`, so the relay runs across a boundary `lb-quic` cannot.
fn build_ws_h3_launcher(
    backends: Vec<SocketAddr>,
    pool: TcpPool,
    ws_cfg: WsConfig,
    header_budget: Duration,
) -> lb_quic::ws_tunnel::WsRelayLauncher {
    use lb_quic::ws_tunnel::{H3WsTunnel, WsConnectRequest, WsRelayHandle, WsUpstreamOutcome};
    let ws_proxy = Arc::new(WsProxy::new(ws_cfg));
    Arc::new(
        move |tunnel: H3WsTunnel, req: WsConnectRequest| -> WsRelayHandle {
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<WsUpstreamOutcome>();
            let backend = backends.first().copied();
            let pool = pool.clone();
            let ws_proxy = Arc::clone(&ws_proxy);
            let task = tokio::spawn(async move {
                let Some(backend_addr) = backend else {
                    let _ = ready_tx.send(WsUpstreamOutcome::Failed { status: 502 });
                    return;
                };
                let ws_cfg = ws_proxy.config();
                // Dial + upstream RFC 6455 handshake INLINE, before readiness, so the client never
                // sees a 200 toward a backend that never agreed.
                let dial = async {
                    let pooled = pool
                        .acquire_async(backend_addr)
                        .await
                        .map_err(|e| format!("backend dial failed: {e}"))?;
                    let stream = pooled
                        .take_stream()
                        .ok_or_else(|| "pooled stream missing".to_string())?;
                    lb_l7::ws_proxy::dial_backend_ws(
                        stream,
                        backend_addr,
                        &req.path,
                        req.subprotocols.as_deref(),
                        &ws_cfg,
                    )
                    .await
                };
                match tokio::time::timeout(header_budget, dial).await {
                    Ok(Ok((backend_ws, negotiated))) => {
                        let mut headers = Vec::new();
                        if let Some(p) = negotiated {
                            headers.push(("sec-websocket-protocol".to_owned(), p));
                        }
                        if ready_tx.send(WsUpstreamOutcome::Ready { headers }).is_err() {
                            return;
                        }
                        let client_ws = lb_l7::ws_proxy::server_ws(tunnel, &ws_cfg).await;
                        if let Err(e) = ws_proxy.proxy_frames(client_ws, backend_ws).await {
                            tracing::debug!(error = %e, "WS-H3: frame proxy ended with error");
                        }
                    }
                    Ok(Err(msg)) => {
                        tracing::debug!(backend = %backend_addr, error = %msg, "WS-H3: upstream handshake refused — 502");
                        let _ = ready_tx.send(WsUpstreamOutcome::Failed { status: 502 });
                    }
                    Err(_elapsed) => {
                        tracing::debug!(backend = %backend_addr, "WS-H3: upstream dial/handshake budget elapsed — 504");
                        let _ = ready_tx.send(WsUpstreamOutcome::Failed { status: 504 });
                    }
                }
            });
            WsRelayHandle {
                ready: ready_rx,
                task,
            }
        },
    )
}

#[allow(clippy::too_many_arguments)]
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
    hooks: Arc<dyn lb_l7::security_hooks::DynSecurityHooks>,
    watchdog: Option<Watchdog>,
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
        head: Duration::from_millis(h.head_timeout_ms),
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
    proxy = proxy.with_hooks(hooks);
    if let Some(wd) = watchdog {
        proxy = proxy.with_watchdog(wd);
    }
    if let Some(h2) = h2_pool {
        proxy = proxy.with_h2_upstream(h2);
    }
    if let Some(h3) = h3_pool {
        proxy = proxy.with_h3_upstream(h3);
    }
    if let Some(ws) = ws_cfg {
        proxy = proxy
            .with_websocket(Arc::new(WsProxy::new(ws_config_to_runtime(ws))))
            // CF-S27-2: WS-over-H2 extended CONNECT is OFF by default — advertise
            // `SETTINGS_ENABLE_CONNECT_PROTOCOL` and intercept only on opt-in.
            .with_h2_extended_connect(ws.h2_extended_connect);
    }
    if let Some(grpc) = grpc_cfg {
        proxy = proxy.with_grpc(GrpcProxy::new(grpc_config_to_runtime(grpc), pool.clone()));
    }
    Ok(Arc::new(proxy))
}

fn grpc_config_to_runtime(cfg: &GrpcListenerConfig) -> GrpcConfig {
    GrpcConfig {
        enabled: cfg.enabled,
        max_deadline: Duration::from_secs(cfg.max_deadline_seconds),
        health_synthesized: cfg.health_synthesized,
    }
}

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

/// Bind and spawn a [`QuicListener`]. PROTO-2-11: `shutdown_token` MUST be a CHILD of the global
/// [`lb_core::Shutdown`] token, or SIGTERM cannot be distinguished from a listener-token cancel.
async fn spawn_quic(
    listener_cfg: &lb_config::ListenerConfig,
    pool: &TcpPool,
    resolver: &DnsResolver,
    metrics: &Arc<MetricsRegistry>,
    max_requests_per_h3_connection: u32,
    shutdown_token: CancellationToken,
) -> anyhow::Result<QuicListener> {
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

    let (raw_backend, modeb_metrics) = match quic_cfg.raw_proxy.as_ref() {
        Some(rp) => {
            let backend = build_raw_quic_backend(rp)
                .with_context(|| format!("building Mode B raw_proxy backend for {bind_addr}"))?;
            let m = lb_observability::QuicModeBMetrics::register(metrics)
                .context("registering quic_modeb_* metrics")?;
            (Some(backend), Some(m))
        }
        None => (None, None),
    };
    let mode_b = raw_backend.is_some();

    // S36-A: register the `h3_*` recycle rows at spawn so they exist in `/metrics` from the start.
    // Never on Mode B and never when the cap is disabled (R3).
    let h3_recycle_metrics = if !mode_b && max_requests_per_h3_connection != 0 {
        Some(
            lb_observability::QuicH3RecycleMetrics::register(metrics)
                .context("registering h3_* recycle metrics")?,
        )
    } else {
        None
    };

    let mut params = quic_listener_params_from_config(
        bind_addr,
        quic_cfg,
        raw_backend,
        modeb_metrics,
        max_requests_per_h3_connection,
        h3_recycle_metrics,
    );

    // WS-over-H3 needs websocket.enabled AND `h3_extended_connect` — OFF by default (mirrors
    // CF-S27-2), so the H3 SETTINGS frame and `:protocol` rejection stay byte-identical.
    let ws_enabled = !mode_b
        && listener_cfg
            .websocket
            .as_ref()
            .is_some_and(|w| w.enabled && w.h3_extended_connect);
    if ws_enabled {
        params = params.with_websocket(true);
    }

    // F-S26-1: dispatch on the single validator-enforced backend family — h1/tcp → `with_backends`
    // (the WS-over-H3 backend leg), h2 → `with_h2_backend`, h3 → `with_h3_backend`.
    if !mode_b && !listener_cfg.backends.is_empty() {
        params = wire_h3_terminate_backends(params, listener_cfg, pool, resolver, metrics).await?;
    }

    let listener = QuicListener::spawn(params, shutdown_token)
        .await
        .with_context(|| format!("QUIC listener bind failed for {bind_addr}"))?;
    if mode_b {
        if let Some(rp) = quic_cfg.raw_proxy.as_ref() {
            tracing::info!(
                address = %listener.local_addr(),
                protocol = "quic",
                mode = "B",
                backend = %rp.backend_addr,
                sni = %rp.sni,
                dgram_queue_cap = rp.dgram_queue_cap,
                max_relay_streams = rp.max_relay_streams,
                backend_verify = "verify_peer(true)",
                backend_ca = rp.backend_ca_path.as_deref().unwrap_or("system-default-roots"),
                "Mode B raw-QUIC proxy listener started"
            );
        }
    } else {
        tracing::info!(
            address = %listener.local_addr(),
            protocol = "quic",
            mode = "H3-terminate",
            cert = %quic_cfg.cert_path,
            retry_secret = %quic_cfg.retry_secret_path,
            backends = listener_cfg.backends.len(),
            "QUIC listener started"
        );
    }
    Ok(listener)
}

/// F-S26-1: wire the H3-terminate → backend forwarding leg. Caller guarantees a non-empty backend
/// list and a non-Mode-B listener.
async fn wire_h3_terminate_backends(
    mut params: QuicListenerParams,
    listener_cfg: &lb_config::ListenerConfig,
    pool: &TcpPool,
    resolver: &DnsResolver,
    metrics: &Arc<MetricsRegistry>,
) -> anyhow::Result<QuicListenerParams> {
    let mut addresses: Vec<SocketAddr> = Vec::with_capacity(listener_cfg.backends.len());
    for b in &listener_cfg.backends {
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
    }

    // The validator enforces a single protocol family, so the first backend picks the leg for the
    // whole listener.
    let Some(first) = listener_cfg.backends.first() else {
        anyhow::bail!(
            "listener {}: wire_h3_terminate_backends called with no backends",
            listener_cfg.address
        );
    };
    let proto = parse_upstream_proto(first.protocol.as_str())
        .with_context(|| format!("listener {} backend 0", listener_cfg.address))?;
    match proto {
        UpstreamProto::H1 => {
            // WS-over-H3 Stage C: the relay launcher dials THESE same H1 backends as the H3→H1 leg
            // below.
            if params.ws_enabled {
                if let Some(ws) = listener_cfg.websocket.as_ref() {
                    let header_budget = listener_cfg.http.as_ref().map_or_else(
                        || Duration::from_secs(30),
                        |h| Duration::from_millis(h.header_timeout_ms),
                    );
                    let launcher = build_ws_h3_launcher(
                        addresses.clone(),
                        pool.clone(),
                        ws_config_to_runtime(ws),
                        header_budget,
                    );
                    params = params.with_ws_relay_launcher(launcher);
                }
            }
            params = params.with_backends(addresses, pool.clone());
        }
        UpstreamProto::H2 => {
            let h2pool = build_h2_upstream_pool(pool.clone(), listener_cfg.h2_security.as_ref());
            let Some(addr) = addresses.first().copied() else {
                anyhow::bail!("listener {}: no resolved H2 backend", listener_cfg.address);
            };
            params = params.with_h2_backend((*h2pool).clone(), addr);
        }
        UpstreamProto::H3 => {
            let h3_backends = collect_h3_backends(listener_cfg);
            let h3pool = build_h3_upstream_pool(&h3_backends)?;
            let Some(addr) = addresses.first().copied() else {
                anyhow::bail!("listener {}: no resolved H3 backend", listener_cfg.address);
            };
            let sni = first.tls_verify_hostname.clone().unwrap_or_else(|| {
                split_host_port(&first.address)
                    .ok()
                    .map_or_else(|| first.address.clone(), |(host, _)| host.to_owned())
            });
            params = params.with_h3_backend((*h3pool).clone(), addr, sni);
        }
    }
    Ok(params)
}

/// S15 A2-8: Mode A QUIC passthrough listener — its own UDP port and retry-secret, and it NEVER
/// decrypts client packets (`scripts/never_decrypted_proof.sh`).
async fn spawn_passthrough(
    cfg: &lb_config::PassthroughConfig,
    metrics: &Arc<MetricsRegistry>,
    shutdown_token: CancellationToken,
) -> anyhow::Result<PassthroughListener> {
    let mut params = PassthroughParams::new(
        cfg.bind_addr,
        cfg.backends.clone(),
        cfg.retry_secret_path.clone(),
    );
    params.max_quic_connections = cfg.max_quic_connections;
    params.min_client_dcid_len = cfg.min_client_dcid_len;
    params.per_flow_backlog = cfg.per_flow_backlog;
    params.strict_source_binding = cfg.strict_source_binding;
    params.audit_throttle_window = Duration::from_secs(cfg.audit_throttle_window_secs);
    params.max_dcid_len_routed = cfg.max_dcid_len_routed;
    params.mint_retry = cfg.mint_retry;
    params.flow_idle_timeout = Duration::from_millis(cfg.flow_idle_timeout_ms);
    params.metrics = Some(
        lb_observability::PassthroughMetrics::register(metrics)
            .context("registering quic_passthrough_* metrics")?,
    );

    let listener = PassthroughListener::spawn(params, shutdown_token)
        .await
        .with_context(|| format!("passthrough listener bind failed for {}", cfg.bind_addr))?;
    tracing::info!(
        address = %listener.local_addr(),
        protocol = "quic-passthrough",
        backends = cfg.backends.len(),
        strict_source_binding = cfg.strict_source_binding,
        "QUIC passthrough listener started"
    );
    Ok(listener)
}

#[allow(clippy::too_many_arguments)]
async fn spawn_tcp(
    listener_cfg: &lb_config::ListenerConfig,
    pool: &TcpPool,
    resolver: &DnsResolver,
    io_runtime: Runtime,
    metrics: &Arc<MetricsRegistry>,
    handshake_timeout: Duration,
    max_inflight: u32,
    connect_timeout: Duration,
    max_keepalive_requests: u32,
    per_conn_drain_jitter_ms: u64,
    hooks: Arc<HooksBundle>,
    shutdown_token: CancellationToken,
    listener_cancel_token: CancellationToken,
    tracker: TaskTracker,
    tls_reload_registry: Arc<PlMutex<Vec<TlsReloadEntry>>>,
    listener_reload_registry: Arc<PlMutex<Vec<ListenerReloadEntry>>>,
    watchdog: Option<Watchdog>,
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
    let mode = build_listener_mode(
        listener_cfg,
        pool,
        &addresses,
        &hooks,
        &tls_reload_registry,
        &tracker,
        &shutdown_token,
        watchdog.as_ref(),
        max_keepalive_requests,
    )?;
    match &mode {
        ListenerMode::H1 { proxy } => {
            listener_reload_registry.lock().push(ListenerReloadEntry {
                listener: listener_cfg.address.clone(),
                proxies: ReloadableProxies::H1 {
                    proxy: Arc::clone(proxy),
                },
            });
        }
        ListenerMode::H1s {
            h1_proxy, h2_proxy, ..
        } => {
            listener_reload_registry.lock().push(ListenerReloadEntry {
                listener: listener_cfg.address.clone(),
                proxies: ReloadableProxies::H1s {
                    h1_proxy: Arc::clone(h1_proxy),
                    h2_proxy: Arc::clone(h2_proxy),
                },
            });
        }
        ListenerMode::PlainTcp | ListenerMode::Tls { .. } => {}
    }
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
        handshake_timeout,
        inflight: Arc::new(Semaphore::new(
            usize::try_from(max_inflight).unwrap_or(usize::MAX),
        )),
        connect_timeout,
        hooks,
        shutdown_token,
        listener_cancel_token,
        tracker: tracker.clone(),
        listener_label: Arc::new(listener_cfg.address.clone()),
        per_conn_drain_jitter_ms,
    });
    Ok(tracker.spawn(run_listener(listener_cfg.address.clone(), state)))
}

#[allow(clippy::too_many_arguments)]
fn build_listener_mode(
    listener_cfg: &lb_config::ListenerConfig,
    pool: &TcpPool,
    addresses: &[SocketAddr],
    hooks: &Arc<HooksBundle>,
    tls_reload_registry: &Arc<PlMutex<Vec<TlsReloadEntry>>>,
    tracker: &TaskTracker,
    shutdown_token: &CancellationToken,
    watchdog: Option<&Watchdog>,
    max_keepalive_requests: u32,
) -> anyhow::Result<ListenerMode> {
    let hooks_arc_dyn: Arc<dyn lb_l7::security_hooks::DynSecurityHooks> =
        Arc::clone(hooks) as Arc<_>;
    match listener_cfg.protocol.as_str() {
        "tls" => {
            let Some(tls_cfg) = listener_cfg.tls.as_ref() else {
                anyhow::bail!(
                    "listener {} has protocol=tls but no [listeners.tls] block",
                    listener_cfg.address
                );
            };
            let (bundle, rotator) = build_tls_bundle(tls_cfg, &[])
                .with_context(|| format!("TLS setup failed for {}", listener_cfg.address))?;
            spawn_rotator_ticker(
                Arc::clone(&rotator),
                tracker.clone(),
                shutdown_token.clone(),
            );
            tls_reload_registry.lock().push(TlsReloadEntry {
                listener: listener_cfg.address.clone(),
                cert_path: PathBuf::from(&tls_cfg.cert_path),
                key_path: PathBuf::from(&tls_cfg.key_path),
                alpn: Vec::new(),
                bundle: Arc::clone(&bundle),
                rotator: Arc::clone(&rotator),
            });
            tracing::info!(
                address = %listener_cfg.address,
                protocol = "tls",
                cert = %tls_cfg.cert_path,
                "listener configured with TLS termination (REL-2-03 hot-reload bundle)"
            );
            Ok(ListenerMode::Tls {
                bundle,
                _rotator: rotator,
            })
        }
        "h1" => {
            let upstreams = build_upstream_backends(listener_cfg, addresses)?;
            let needs_h2 = upstreams.iter().any(|b| b.proto == UpstreamProto::H2);
            let needs_h3 = upstreams.iter().any(|b| b.proto == UpstreamProto::H3);
            let h2_pool = needs_h2
                .then(|| build_h2_upstream_pool(pool.clone(), listener_cfg.h2_security.as_ref()));
            let h3_pool = if needs_h3 {
                Some(build_h3_upstream_pool(&collect_h3_backends(listener_cfg))?)
            } else {
                None
            };
            let proxy = build_h1_proxy(
                pool.clone(),
                upstreams,
                h2_pool,
                h3_pool,
                listener_cfg.alt_svc.as_ref(),
                listener_cfg.http.as_ref(),
                listener_cfg.websocket.as_ref(),
                false,
                Arc::clone(&hooks_arc_dyn),
                watchdog.cloned(),
                max_keepalive_requests,
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
            Ok(ListenerMode::H1 {
                proxy: Arc::new(ArcSwap::new(proxy)),
            })
        }
        "h1s" => {
            let Some(tls_cfg) = listener_cfg.tls.as_ref() else {
                anyhow::bail!(
                    "listener {} has protocol=h1s but no [listeners.tls] block",
                    listener_cfg.address
                );
            };
            let h1s_alpn: &[&[u8]] = &[b"h2", b"http/1.1"];
            let (bundle, rotator) = build_tls_bundle(tls_cfg, h1s_alpn)
                .with_context(|| format!("H1s TLS setup failed for {}", listener_cfg.address))?;
            spawn_rotator_ticker(
                Arc::clone(&rotator),
                tracker.clone(),
                shutdown_token.clone(),
            );
            tls_reload_registry.lock().push(TlsReloadEntry {
                listener: listener_cfg.address.clone(),
                cert_path: PathBuf::from(&tls_cfg.cert_path),
                key_path: PathBuf::from(&tls_cfg.key_path),
                alpn: h1s_alpn.iter().map(|p| p.to_vec()).collect(),
                bundle: Arc::clone(&bundle),
                rotator: Arc::clone(&rotator),
            });
            let upstreams_h1 = build_upstream_backends(listener_cfg, addresses)?;
            let upstreams_h2 = upstreams_h1.clone();
            let needs_h2 = upstreams_h1.iter().any(|b| b.proto == UpstreamProto::H2);
            let needs_h3 = upstreams_h1.iter().any(|b| b.proto == UpstreamProto::H3);
            let h2_pool = needs_h2
                .then(|| build_h2_upstream_pool(pool.clone(), listener_cfg.h2_security.as_ref()));
            let h3_pool = if needs_h3 {
                Some(build_h3_upstream_pool(&collect_h3_backends(listener_cfg))?)
            } else {
                None
            };
            let h1_proxy = build_h1_proxy(
                pool.clone(),
                upstreams_h1,
                h2_pool.clone(),
                h3_pool.clone(),
                listener_cfg.alt_svc.as_ref(),
                listener_cfg.http.as_ref(),
                listener_cfg.websocket.as_ref(),
                true,
                Arc::clone(&hooks_arc_dyn),
                watchdog.cloned(),
                max_keepalive_requests,
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
                Arc::clone(&hooks_arc_dyn),
                watchdog.cloned(),
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
                h1_proxy: Arc::new(ArcSwap::new(h1_proxy)),
                h2_proxy: Arc::new(ArcSwap::new(h2_proxy)),
                bundle,
                _rotator: rotator,
            })
        }
        // PROTO-2-09: `lb_config` accepts more protocol spellings than this binary wires, so an
        // unhandled one must fail loudly at startup rather than degrade to raw TCP.
        "tcp" => {
            tracing::info!(
                address = %listener_cfg.address,
                protocol = "tcp",
                "listener configured for plain TCP forwarding"
            );
            Ok(ListenerMode::PlainTcp)
        }
        other => Err(anyhow::anyhow!(
            "listener {} has protocol={other:?} which has no runtime implementation; \
             supported values are: tcp, tls, h1, h1s, quic",
            listener_cfg.address
        )),
    }
}

fn install_hotpath_metrics(
    metrics: &Arc<MetricsRegistry>,
    pool: &TcpPool,
    resolver: &DnsResolver,
    tracker: &TaskTracker,
    cancel: &CancellationToken,
) {
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

    // REL-2-08: the label set is bounded on purpose — `route` is capped by MAX_ROUTES_BUDGET so a
    // hostile path cannot explode the series count.
    if let Err(e) = metrics.counter_vec(
        "http_requests_total",
        "HTTP requests terminated by the L7 proxy",
        &["listener", "route", "version", "status_class"],
    ) {
        tracing::warn!(metric = "http_requests_total", error = %e, "counter_vec register failed");
    }
    if let Err(e) = metrics.histogram_vec(
        "http_request_duration_seconds",
        "L7 request duration from accept to response body sent",
        &["listener", "route", "version"],
        &http_latency_buckets(),
    ) {
        tracing::warn!(metric = "http_request_duration_seconds", error = %e, "histogram_vec register failed");
    }
    if let Err(e) = metrics.gauge("pool_idle_gauge", "TcpPool idle connection count") {
        tracing::warn!(metric = "pool_idle_gauge", error = %e, "gauge register failed");
    }

    if let Err(e) = metrics.counter(
        "accept_shed_total",
        "Accepts shed because the per-listener inflight cap was hit",
    ) {
        tracing::warn!(metric = "accept_shed_total", error = %e, "counter register failed");
    }
    if let Err(e) = metrics.accept_inflight_gauge() {
        tracing::warn!(metric = "accept_inflight", error = %e, "gauge register failed");
    }
    if let Err(e) = metrics.counter_vec(
        "accept_errors_total",
        "accept(2) errors classified by kind (transient backoff vs. fatal)",
        &["kind"],
    ) {
        tracing::warn!(metric = "accept_errors_total", error = %e, "counter_vec register failed");
    }
    if let Err(e) = metrics.counter(
        "backend_connect_timeout_total",
        "Backend TcpStream::connect timeouts",
    ) {
        tracing::warn!(metric = "backend_connect_timeout_total", error = %e, "counter register failed");
    }

    let pool_clone = pool.clone();
    let resolver_clone = resolver.clone();
    let metrics_clone = Arc::clone(metrics);
    let cancel = cancel.clone();
    tracker.spawn(async move {
        let Ok(idle_gauge) =
            metrics_clone.gauge("pool_idle_gauge", "TcpPool idle connection count")
        else {
            return;
        };
        let Ok(dns_entries_gauge) =
            metrics_clone.gauge("dns_cache_entries", "DNS resolver cache size")
        else {
            return;
        };
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    tracing::debug!("pool/dns sampler shutting down");
                    return;
                }
                _ = ticker.tick() => {}
            }
            #[allow(clippy::cast_possible_wrap)]
            idle_gauge.set(pool_clone.idle_count() as i64);
            #[allow(clippy::cast_possible_wrap)]
            dns_entries_gauge.set(resolver_clone.cache_size() as i64);
        }
    });
}

fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;

    rt.block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    match lb_observability::init_tracing(&lb_observability::TracingConfig::default()) {
        Ok(()) | Err(lb_observability::TracingError::AlreadyInitialised) => {}
    }

    // CODE-2-02: install the panic hook IMMEDIATELY after the subscriber, so anything panicking
    // during the rest of boot is logged and counted.
    init_panic_hook();

    tracing::info!("ExpressGateway v{}", env!("CARGO_PKG_VERSION"));

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config/default.toml".to_owned());

    let config_str = std::fs::read_to_string(&config_path)
        .with_context(|| format!("cannot read config file: {config_path}"))?;

    let config = lb_config::parse_config(&config_str).context("config parse error")?;
    lb_config::validate_config(&config).context("config validation error")?;

    // REL-2-08: refuse to boot if the live config shape would blow the per-family series ceiling.
    {
        let listeners = config.listeners.len();
        let backends_per = config
            .listeners
            .iter()
            .map(|l| l.backends.len())
            .max()
            .unwrap_or(0);
        // ROUND8-OPS-05: route fan-out is bounded by MAX_ROUTES_BUDGET, not by the literal
        // placeholder.
        let budget = lb_observability::LabelBudget::from_config_shape(
            listeners,
            backends_per,
            lb_observability::MAX_ROUTES_BUDGET,
            lb_observability::DEFAULT_MAX_LABEL_CARDINALITY,
        );
        budget
            .check()
            .map_err(|e| anyhow::anyhow!("label cardinality budget exceeded: {e}"))?;
        tracing::info!(
            listeners,
            backends_per,
            ceiling = lb_observability::DEFAULT_MAX_LABEL_CARDINALITY,
            "label cardinality budget OK"
        );
    }

    tracing::info!(
        listeners = config.listeners.len(),
        "configuration loaded from {config_path}"
    );

    let cp_backend = FileBackend::new(std::path::PathBuf::from(&config_path));
    let mut config_manager = match ConfigManager::new(Box::new(cp_backend)) {
        Ok(mgr) => {
            tracing::info!(
                path = %config_path,
                version = mgr.version(),
                "control plane (file-backed) ready — config reloads are SIGHUP-driven (S37-C)"
            );
            Some(mgr)
        }
        Err(e) => {
            // Fail-soft: `parse_config` already succeeded above, so an InvalidConfig error here
            // cannot mean the file is bad.
            tracing::warn!(error = %e, "control plane manager init skipped");
            None
        }
    };

    let mut applied_config = config.clone();

    let shutdown: lb_core::Shutdown = lb_core::Shutdown::new();

    let probes = lb_observability::ProbeRegistry::shared();

    let io_runtime = Runtime::new();
    tracing::info!(
        backend = %io_runtime.backend(),
        high_water = Runtime::high_water_mark(),
        low_water = Runtime::low_water_mark(),
        "lb-io runtime ready"
    );

    let pool = TcpPool::new(PoolConfig::default(), backend_opts(), io_runtime);
    tracing::info!("TCP backend pool ready (defaults from PROMPT.md §21)");

    let mut health_seed: Vec<(String, HealthChecker)> = Vec::new();
    for listener in &config.listeners {
        for backend in &listener.backends {
            health_seed.push((backend.address.clone(), HealthChecker::new(3, 2)));
        }
    }
    let initial_unknown = health_seed
        .iter()
        .filter(|(_, c)| c.status() == HealthStatus::Unknown)
        .count();
    tracing::info!(
        backends = health_seed.len(),
        unknown = initial_unknown,
        "passive health checkers seeded — active probe loop is Wave-2 (REL-2-05)"
    );
    // Hold the seed in scope so its existence is observable to the borrow checker.
    let _health_seed = health_seed;

    let resolver = DnsResolver::new(ResolverConfig::default());
    tracing::info!("DNS resolver ready (positive cap 300s, negative TTL 5s)");

    // Optional XDP data-plane attach. Held for the process lifetime — dropping it detaches.
    let _xdp_loader = if let Some(rt) = config.runtime.as_ref() {
        xdp::try_attach_xdp(rt)
    } else {
        None
    };

    let metrics = Arc::new(MetricsRegistry::new());
    install_hotpath_metrics(
        &metrics,
        &pool,
        &resolver,
        shutdown.tracker(),
        shutdown.token(),
    );

    bind_panic_counter(&metrics);

    let xdp_metrics = lb_observability::xdp_metrics::XdpMetrics::register(&metrics)
        .map_err(|e| anyhow::anyhow!("XDP metric registration failed: {e}"))?;

    let admin_cancel = CancellationToken::new();
    if let Some(obs) = config.observability.as_ref() {
        if let Some(bind_str) = obs.metrics_bind.as_deref() {
            let bind_addr: SocketAddr = bind_str
                .trim()
                .parse()
                .with_context(|| format!("invalid observability.metrics_bind: {bind_str}"))?;
            let admin_cfg = config.admin.as_ref();
            let token_hash = admin_cfg
                .and_then(|a| a.api_token_hash.as_deref())
                .map(|hex| {
                    lb_security::AdminTokenHash::from_hex(hex).map_err(|_| {
                        anyhow::anyhow!(
                            "[admin].api_token_hash must be exactly 64 hex chars (SHA-256)"
                        )
                    })
                })
                .transpose()?;
            let allow_non_loopback = admin_cfg.is_some_and(|a| a.allow_non_loopback);
            // SEC-2-06: refuse to start on a non-loopback bind without an explicit override
            // (foot-gun).
            lb_security::AdminAuthGate::validate_bind(
                bind_addr,
                allow_non_loopback,
                token_hash.is_some(),
            )
            .map_err(|e| anyhow::anyhow!("admin bind refused: {e}"))?;
            let gate = Arc::new(lb_security::AdminAuthGate::new(token_hash));
            match admin_http::serve_with_auth(
                Arc::clone(&metrics),
                Arc::clone(&probes),
                Some(Arc::clone(&gate)),
                bind_addr,
                admin_cancel.clone(),
            )
            .await
            {
                Ok(local) => tracing::info!(
                    address = %local,
                    protocol = "admin-http",
                    bearer_auth = gate.enforced(),
                    "admin listener started (/metrics, /livez, /readyz, /startupz, /healthz)"
                ),
                Err(e) => {
                    tracing::error!(bind = %bind_addr, error = %e, "admin listener bind failed");
                }
            }
        }
    }

    let mut listener_handles = Vec::new();
    let mut quic_listeners: Vec<QuicListener> = Vec::new();
    let mut passthrough_listeners: Vec<PassthroughListener> = Vec::new();

    let handshake_timeout = Duration::from_millis(
        config
            .runtime
            .as_ref()
            .map_or(5_000, |r| r.handshake_timeout_ms),
    );
    let max_inflight = config
        .runtime
        .as_ref()
        .map_or(65_536, |r| r.max_inflight_connections);
    let connect_timeout = Duration::from_millis(
        config
            .runtime
            .as_ref()
            .map_or(5_000, |r| r.connect_timeout_ms),
    );
    let max_keepalive_requests = config
        .runtime
        .as_ref()
        .map_or(100, |r| r.max_keepalive_requests);
    let max_requests_per_h3_connection = config
        .runtime
        .as_ref()
        .map_or(1000, |r| r.max_requests_per_h3_connection);
    // SEC-2-04: the SAME `Arc<HooksBundle>` is shared across listeners, so the per-IP cap is
    // process-wide, not per-listener.
    let per_ip_cap = config
        .runtime
        .as_ref()
        .map_or(1_024, |r| r.per_ip_connection_cap);
    let conn_gate = ConnGate::new(max_inflight, per_ip_cap, Vec::new());
    let smuggle_mode = if config.security.as_ref().is_some_and(|s| s.strict_te) {
        SmuggleMode::H1Strict
    } else {
        SmuggleMode::H1
    };
    let hooks: Arc<HooksBundle> = Arc::new(HooksBundle::new(conn_gate, smuggle_mode));
    tracing::info!(
        strict_te = matches!(smuggle_mode, SmuggleMode::H1Strict),
        "PROTO-2-17: HooksBundle SmuggleMode selected from [security].strict_te"
    );
    tracing::info!(
        max_inflight,
        per_ip_cap,
        connect_timeout_ms = connect_timeout.as_millis() as u64,
        "accept-loop guards configured (CODE-2-05/06/09 + SEC-2-04 — Wave 2c-2)"
    );

    let watchdog_cfg = config
        .runtime
        .as_ref()
        .and_then(|r| r.watchdog)
        .unwrap_or_default();
    let watchdog = Watchdog::new(WatchdogConfig {
        min_rate_bps: watchdog_cfg.body_progress_min_bps,
        rate_window: Duration::from_secs(1),
        max_registered: 100_000,
    });
    {
        let wd = watchdog.clone();
        let cancel = shutdown.token().clone();
        let sweep_interval = Duration::from_millis(watchdog_cfg.sweep_interval_ms);
        shutdown.tracker().spawn(async move {
            let mut ticker = tokio::time::interval(sweep_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => {
                        tracing::debug!("Watchdog sweeper shutting down");
                        return;
                    }
                    _ = ticker.tick() => {}
                }
                // F-RES-5 (S38): the sweeper is OBSERVABILITY-ONLY — closing the socket here would
                // race the drain coordinator. Enforcement belongs to the timeout stack.
                let detected = wd.sweep_expired();
                if !detected.is_empty() {
                    tracing::warn!(
                        detected = detected.len(),
                        "Watchdog detected stalled connections (slow-loris/slow-POST); \
                         enforcement is the timeout stack, this is an alerting signal",
                    );
                }
            }
        });
    }
    tracing::info!(
        header_deadline_ms = watchdog_cfg.header_deadline_ms,
        body_progress_min_bps = watchdog_cfg.body_progress_min_bps,
        sweep_interval_ms = watchdog_cfg.sweep_interval_ms,
        "SEC-2-03 Watchdog wired into accept-site + L7 proxies"
    );

    let tls_reload_registry: Arc<PlMutex<Vec<TlsReloadEntry>>> = Arc::new(PlMutex::new(Vec::new()));

    let listener_reload_registry: Arc<PlMutex<Vec<ListenerReloadEntry>>> =
        Arc::new(PlMutex::new(Vec::new()));

    let cert_metrics = CertMetrics::register(&metrics);

    let reload_metrics = ReloadMetrics::register(&metrics);

    for listener_cfg in &config.listeners {
        if listener_cfg.protocol == "quic" {
            quic_listeners.push(
                spawn_quic(
                    listener_cfg,
                    &pool,
                    &resolver,
                    &metrics,
                    max_requests_per_h3_connection,
                    shutdown.token().child_token(),
                )
                .await?,
            );
            continue;
        }
        if listener_cfg.backends.is_empty() {
            tracing::warn!(
                address = %listener_cfg.address,
                "listener has no backends configured — skipping"
            );
            continue;
        }
        // ROUND8 OPS-02: the coordinator jitter desyncs ACROSS replicas, this per-listener one
        // desyncs connections WITHIN a pod. Both are needed.
        let per_conn_drain_jitter_ms =
            listener_cfg.effective_drain_jitter_ms(config.runtime.as_ref());
        let handle = spawn_tcp(
            listener_cfg,
            &pool,
            &resolver,
            io_runtime,
            &metrics,
            handshake_timeout,
            max_inflight,
            connect_timeout,
            max_keepalive_requests,
            per_conn_drain_jitter_ms,
            Arc::clone(&hooks),
            shutdown.token().clone(),
            shutdown.listener_token().clone(),
            shutdown.tracker().clone(),
            Arc::clone(&tls_reload_registry),
            Arc::clone(&listener_reload_registry),
            Some(watchdog.clone()),
        )
        .await?;
        listener_handles.push(handle);
    }

    if let Some(pt_cfg) = config.passthrough.as_ref() {
        passthrough_listeners
            .push(spawn_passthrough(pt_cfg, &metrics, shutdown.token().child_token()).await?);
    }

    if listener_handles.is_empty() && quic_listeners.is_empty() && passthrough_listeners.is_empty()
    {
        anyhow::bail!("no listeners started — check your configuration");
    }

    probes.set_ready();
    tracing::info!("probes flipped to Ready — service open for traffic");

    {
        let xdp_metrics = xdp_metrics.clone();
        let cancel = shutdown.token().clone();
        shutdown.tracker().spawn(async move {
            let mut baseline = lb_observability::SamplerBaseline::default();
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => {
                        tracing::debug!("XDP stats sampler shutting down");
                        return;
                    }
                    _ = ticker.tick() => {}
                }
                match lb_l4_xdp::stats_export::read_stats() {
                    Ok(snap) => {
                        let deltas = baseline.delta(&snap.summed);
                        lb_observability::xdp_metrics::apply_packet_deltas(&xdp_metrics, &deltas);
                    }
                    Err(e) => {
                        xdp_metrics.sampler_errors_total.inc();
                        tracing::debug!(error = %e, "XDP stats read failed");
                    }
                }
            }
        });
    }

    // S37-C/R6: installed ONCE outside the loop. Re-installing per iteration LOSES a
    // SIGTERM/SIGINT that lands while a non-terminal SIGHUP/SIGUSR1 is being serviced —
    // a persistent stream latches it. Non-terminal signals are handled and `continue`d,
    // so an operator can re-signal after a rejected reload.
    #[cfg(unix)]
    let mut lifecycle_signals = LifecycleSignals::install()?;
    let signal_kind = loop {
        #[cfg(unix)]
        let s = lifecycle_signals.recv().await;
        #[cfg(not(unix))]
        let s = {
            let _ = signal::ctrl_c().await;
            LifecycleSignal::SigInt
        };
        tracing::info!(signal = %s, "lifecycle signal received");
        match s {
            LifecycleSignal::SigUsr1 => {
                let entries: Vec<TlsReloadEntry> = tls_reload_registry.lock().clone();
                if entries.is_empty() {
                    tracing::info!(
                        "SIGUSR1 received but no TLS listeners configured — nothing to reload"
                    );
                    continue;
                }
                let (ok, fail) = reload_all_tls(&entries, cert_metrics.as_ref());
                tracing::info!(
                    ok,
                    fail,
                    entries = entries.len(),
                    "REL-2-03 SIGUSR1 cert reload pass complete"
                );
            }
            LifecycleSignal::SigHup => {
                reload_config(
                    config_manager.as_mut(),
                    &mut applied_config,
                    &listener_reload_registry,
                    &pool,
                    &resolver,
                    &hooks,
                    Some(&watchdog),
                    reload_metrics.as_ref(),
                )
                .await;
            }
            LifecycleSignal::SigTerm | LifecycleSignal::SigInt => break s,
        }
    };
    tracing::info!(signal = %signal_kind, "terminal signal — entering drain");

    let runtime_cfg = config.runtime.as_ref();
    let probes_for_mark = Arc::clone(&probes);
    let metrics_for_obs = Arc::clone(&metrics);
    let observer: std::sync::Arc<dyn lb_core::DrainObserver> =
        std::sync::Arc::new(MetricsDrainObserver {
            metrics: Arc::clone(&metrics_for_obs),
        });
    let mut max_listener_drain_ms = runtime_cfg.map_or(10_000, |r| r.drain_timeout_ms);
    let mut max_listener_jitter_ms = runtime_cfg.map_or(
        10_000 / 4,
        lb_config::RuntimeConfig::effective_drain_jitter_ms,
    );
    {
        let drain_budget_gauge = metrics
            .gauge_vec(
                "lb_drain_timeout_ms_listener",
                "ROUND-8 OPS-10: effective per-listener drain budget (ms), \
                 build-info style — used by the LbShutdownSlow alert",
                &["listener"],
            )
            .ok();
        for lc in &config.listeners {
            let eff_t = lc.effective_drain_timeout_ms(runtime_cfg);
            let eff_j = lc.effective_drain_jitter_ms(runtime_cfg);
            max_listener_drain_ms = max_listener_drain_ms.max(eff_t);
            max_listener_jitter_ms = max_listener_jitter_ms.max(eff_j);
            if let Some(g) = drain_budget_gauge.as_ref() {
                g.with_label_values(&[lc.address.as_str()])
                    .set(eff_t as i64);
            }
        }
    }
    let spec = lb_core::DrainSpec {
        readiness_settle: Duration::from_millis(
            // ROUND-8 OPS-11: this fallback must match `lb_config::default_readiness_settle_ms()`
            // (11 s — one kube probe interval plus slack).
            runtime_cfg.map_or(11_000, |r| r.readiness_settle_ms),
        ),
        listener_cancel_deadline: Duration::from_millis(500),
        inflight_drain_deadline: Duration::from_millis(max_listener_drain_ms),
        xdp_detach_deadline: None,
        jitter_max: Duration::from_millis(max_listener_jitter_ms),
        mark_draining: Some(Box::new(move || {
            tracing::info!("entering drain — flipping /readyz to 503");
            probes_for_mark.set_draining();
        })),
        xdp_detach: None,
        observer: Some(observer),
    };

    // Cancel the admin listener BEFORE the coordinator so it does not serve `/readyz` Ready during
    // the settle window.
    admin_cancel.cancel();

    let report = shutdown.run_drain(spec).await;
    tracing::info!(
        mark_draining_ms = report.mark_draining.duration.as_millis() as u64,
        readiness_settle_ms = report.readiness_settle.duration.as_millis() as u64,
        listener_cancel_ms = report.listener_cancel.timing.duration.as_millis() as u64,
        in_flight_drain_ms = report.in_flight_drain.timing.duration.as_millis() as u64,
        xdp_detach_ms = report.xdp_detach.timing.duration.as_millis() as u64,
        total_ms = report.total.duration.as_millis() as u64,
        in_flight_remaining = report.in_flight_remaining,
        listener_outcome = report.listener_cancel.outcome.as_label(),
        drain_outcome = report.in_flight_drain.outcome.as_label(),
        xdp_outcome = report.xdp_detach.outcome.as_label(),
        "OPS-04+L4-12 drain coordinator complete"
    );

    for h in &listener_handles {
        if !h.is_finished() {
            if let Ok(c) = metrics.counter(
                "shutdown_listener_cancel_timeout_total",
                "Listener accept loops that did not exit within the cancel deadline",
            ) {
                c.inc();
            }
            h.abort();
        }
    }

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

    let mut passthrough_drain_handles = Vec::with_capacity(passthrough_listeners.len());
    for listener in passthrough_listeners {
        passthrough_drain_handles.push(listener.shutdown());
    }
    for handle in passthrough_drain_handles {
        if tokio::time::timeout(quic_drain_deadline, handle)
            .await
            .is_err()
        {
            tracing::warn!(
                "QUIC passthrough listener did not drain within {quic_drain_deadline:?}"
            );
        }
    }

    if matches!(
        report.in_flight_drain.outcome,
        lb_core::ListenerOutcome::TimedOut
    ) {
        if let Ok(c) = metrics.counter(
            "shutdown_aborted_connections_total",
            "Tasks still live when the drain deadline elapsed",
        ) {
            c.inc_by(report.in_flight_remaining as u64);
        }
        if let Ok(c) = metrics.counter(
            "shutdown_inflight_drain_timeout_total",
            "Drain coordinator: inflight-drain phase hit its deadline",
        ) {
            c.inc();
        }
        tracing::warn!(
            remaining = report.in_flight_remaining,
            "drain deadline elapsed — survivors will be aborted on runtime drop"
        );
    } else {
        tracing::info!("drain completed cleanly");
    }

    let total = metrics.get("connections_total").unwrap_or(0);
    let bytes_in = metrics.get("bytes_client_to_backend").unwrap_or(0);
    let bytes_out = metrics.get("bytes_backend_to_client").unwrap_or(0);
    tracing::info!(
        total_connections = total,
        bytes_in,
        bytes_out,
        "ExpressGateway stopped"
    );

    // _xdp_loader drops HERE, after the drain has settled, so the userspace inserter sees a stable
    // map until the last connection is gone.
    drop(_xdp_loader);

    Ok(())
}

#[derive(Copy, Clone, Debug)]
enum LifecycleSignal {
    SigTerm,
    SigInt,
    SigUsr1,
    SigHup,
}

impl std::fmt::Display for LifecycleSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SigTerm => "SIGTERM",
            Self::SigInt => "SIGINT",
            Self::SigUsr1 => "SIGUSR1",
            Self::SigHup => "SIGHUP",
        })
    }
}

#[cfg(unix)]
struct LifecycleSignals {
    sigterm: tokio::signal::unix::Signal,
    sigint: tokio::signal::unix::Signal,
    sigusr1: Option<tokio::signal::unix::Signal>,
    sighup: Option<tokio::signal::unix::Signal>,
}

#[cfg(unix)]
impl LifecycleSignals {
    fn install() -> anyhow::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal as unix_signal};
        let sigterm = unix_signal(SignalKind::terminate()).context("install SIGTERM handler")?;
        let sigint = unix_signal(SignalKind::interrupt()).context("install SIGINT handler")?;
        let sigusr1 = match unix_signal(SignalKind::user_defined1()) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(error = %e, "SIGUSR1 handler install failed — cert reload disabled");
                None
            }
        };
        let sighup = match unix_signal(SignalKind::hangup()) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(error = %e, "SIGHUP handler install failed — config reload disabled");
                None
            }
        };
        Ok(Self {
            sigterm,
            sigint,
            sigusr1,
            sighup,
        })
    }

    async fn recv(&mut self) -> LifecycleSignal {
        // Borrow the disjoint fields up front so the four `select!` arms don't each borrow `self`.
        let LifecycleSignals {
            sigterm,
            sigint,
            sigusr1,
            sighup,
        } = self;
        let usr1 = async {
            match sigusr1.as_mut() {
                Some(s) => {
                    s.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        let hup = async {
            match sighup.as_mut() {
                Some(s) => {
                    s.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = sigterm.recv() => LifecycleSignal::SigTerm,
            _ = sigint.recv() => LifecycleSignal::SigInt,
            () = usr1 => LifecycleSignal::SigUsr1,
            () = hup => LifecycleSignal::SigHup,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum AcceptErrorKind {
    EmfileOrEnfile,
    ConnReset,
    Fatal,
}

impl AcceptErrorKind {
    const fn as_label(self) -> &'static str {
        match self {
            Self::EmfileOrEnfile => "fd_exhausted",
            Self::ConnReset => "conn_reset",
            Self::Fatal => "fatal",
        }
    }
}

fn classify_accept_error(err: &std::io::Error) -> AcceptErrorKind {
    use std::io::ErrorKind;
    if let Some(raw) = err.raw_os_error() {
        // ENFILE = 23, EMFILE = 24 on Linux/glibc + musl + macOS.
        if raw == 23 || raw == 24 {
            return AcceptErrorKind::EmfileOrEnfile;
        }
    }
    match err.kind() {
        ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => AcceptErrorKind::ConnReset,
        _ => AcceptErrorKind::Fatal,
    }
}

fn next_accept_backoff(prev: Duration) -> Duration {
    use rand::RngExt;
    let base = if prev.is_zero() {
        Duration::from_millis(10)
    } else {
        prev.saturating_mul(2)
    };
    let capped = base.min(Duration::from_secs(1));
    let mut rng = rand::rng();
    let jitter_ms = capped.as_millis() as i64 / 4;
    let delta = rng.random_range(-jitter_ms..=jitter_ms);
    let final_ms = (capped.as_millis() as i64 + delta).max(1) as u64;
    Duration::from_millis(final_ms)
}

async fn write_h1_shed_response<W: AsyncWrite + Unpin>(io: &mut W) -> std::io::Result<()> {
    const BODY: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\n\
        content-type: text/plain; charset=utf-8\r\n\
        content-length: 23\r\n\
        connection: close\r\n\
        \r\n\
        listener over capacity\n";
    io.write_all(BODY).await?;
    io.shutdown().await
}

struct MetricsDrainObserver {
    metrics: Arc<MetricsRegistry>,
}

impl MetricsDrainObserver {
    const BUCKETS: &'static [f64] = &[0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0];

    fn is_listener_scoped(phase: lb_core::DrainPhase) -> bool {
        matches!(
            phase,
            lb_core::DrainPhase::ListenerCancel | lb_core::DrainPhase::InFlightDrain
        )
    }
}

impl lb_core::DrainObserver for MetricsDrainObserver {
    fn observe(&self, timing: &lb_core::PhaseTiming, listener: Option<&str>) {
        let secs = timing.duration.as_secs_f64();
        if Self::is_listener_scoped(timing.phase) {
            // The observer plumbing calls us once per PHASE, not once per listener.
            if let Ok(hv) = self.metrics.histogram_vec(
                "shutdown_drain_seconds_listener",
                "Per-phase wall-clock for the drain coordinator (listener-scoped phases)",
                &["phase", "outcome", "listener"],
                Self::BUCKETS,
            ) {
                let lbl = listener.unwrap_or("<aggregate>");
                hv.with_label_values(&[timing.phase.as_label(), timing.outcome.as_label(), lbl])
                    .observe(secs);
            }
        } else if let Ok(hv) = self.metrics.histogram_vec(
            "shutdown_drain_seconds_global",
            "Per-phase wall-clock for the drain coordinator (global phases)",
            &["phase", "outcome"],
            Self::BUCKETS,
        ) {
            hv.with_label_values(&[timing.phase.as_label(), timing.outcome.as_label()])
                .observe(secs);
        }
    }
}

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

    let mut backoff = Duration::ZERO;

    loop {
        // OPS-04+L4-12 (C-2/C-3/C-15): the cancel arm is `biased` so a pending cancel wins over a
        // ready accept — otherwise a saturated listener accepts forever.
        let accept_outcome = tokio::select! {
            biased;
            () = state.listener_cancel_token.cancelled() => {
                tracing::info!(
                    address = %bind_addr,
                    "listener cancelled by drain coordinator (phase 4)"
                );
                return Ok(());
            }
            res = listener.accept() => res,
        };
        let (mut client_stream, client_addr) = match accept_outcome {
            Ok(conn) => {
                backoff = Duration::ZERO;
                conn
            }
            Err(e) => {
                let kind = classify_accept_error(&e);
                if let Ok(v) = state.metrics.counter_vec(
                    "accept_errors_total",
                    "accept(2) errors classified by kind (transient backoff vs. fatal)",
                    &["kind"],
                ) {
                    v.with_label_values(&[kind.as_label()]).inc();
                }
                match kind {
                    AcceptErrorKind::Fatal => {
                        return Err(anyhow::Error::new(e))
                            .with_context(|| format!("fatal accept error on {bind_addr}"));
                    }
                    AcceptErrorKind::EmfileOrEnfile | AcceptErrorKind::ConnReset => {
                        backoff = next_accept_backoff(backoff);
                        tracing::warn!(
                            kind = %kind.as_label(),
                            sleep_ms = backoff.as_millis() as u64,
                            "transient accept error — backing off"
                        );
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                }
            }
        };

        // OPS-04+L4-12 case C-3 — SYNCHRONOUS post-accept tail check: `select!` only covers the
        // FUTURE, so an accept completing in the same poll as the cancel
        // would otherwise leak an accepted fd and drift the per-IP counter.
        if state.listener_cancel_token.is_cancelled() {
            tracing::debug!(
                client = %client_addr,
                address = %bind_addr,
                "accepted socket dropped post-cancel (OPS-04 case C-3)"
            );
            let _ = client_stream.shutdown().await;
            return Ok(());
        }

        // SEC-2-04: admission gate runs BEFORE the inflight semaphore so a saturated IP cannot
        // starve other clients of slots.
        let conn_permit = match state.hooks.admit_connection(client_addr.ip()) {
            Ok(p) => p,
            Err(reject) => {
                if let Ok(v) = state.metrics.counter_vec(
                    "accept_reject_total",
                    "Accepts refused by per-IP / per-listener admission gate",
                    &["reason"],
                ) {
                    let reason = match reject {
                        lb_security::SecurityReject::OverCap(_) => "over_cap",
                        lb_security::SecurityReject::Smuggle(_) => "smuggle",
                        lb_security::SecurityReject::RateLimited => "rate_limited",
                        lb_security::SecurityReject::SlowHandshake => "slow_handshake",
                    };
                    v.with_label_values(&[reason]).inc();
                }
                tracing::debug!(
                    client = %client_addr,
                    reject = ?reject,
                    "admission gate refused connection"
                );
                // RST-style close: no body, no amplification surface.
                let _ = client_stream.shutdown().await;
                continue;
            }
        };

        // CODE-2-05: `try_acquire_owned` returns immediately, so a saturated listener sheds rather
        // than queueing.
        let permit = match Arc::clone(&state.inflight).try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                if let Ok(c) = state.metrics.counter(
                    "accept_shed_total",
                    "Accepts shed because the per-listener inflight cap was hit",
                ) {
                    c.inc();
                }
                tracing::warn!(
                    client = %client_addr,
                    cap = state.inflight.available_permits(),
                    "shed accept — per-listener inflight cap reached"
                );
                if matches!(state.mode, ListenerMode::H1 { .. }) {
                    let _ = write_h1_shed_response(&mut client_stream).await;
                } else {
                    let _ = client_stream.shutdown().await;
                }
                continue;
            }
        };

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
        let _inflight_permit = permit;
        let _admission_permit = conn_permit;
        let conn_cancel = st.shutdown_token.clone();
        let inflight_gauge_guard = AcceptInflightGuard::new(
            Arc::clone(&state.metrics),
            Arc::clone(&state.listener_label),
        );
        st.tracker.clone().spawn(async move {
            let _permit = _inflight_permit;
            let _conn_permit = _admission_permit;
            let _gauge_guard = inflight_gauge_guard;
            st.active_connections.fetch_add(1, Ordering::Relaxed);
            st.metrics.increment("connections_total", 1);

            let http_start = Instant::now();
            let work = async {
                let mut http_version: Option<&'static str> = None;
                let res: anyhow::Result<()> = match &st.mode {
                    ListenerMode::PlainTcp => {
                        proxy_connection(
                            client_stream,
                            backend_addr,
                            &st.metrics,
                            st.connect_timeout,
                        )
                        .await
                    }
                    ListenerMode::Tls { bundle, .. } => {
                        // REL-2-03: snapshot the bundle at accept — a concurrent SIGUSR1 reload
                        // must not disturb this handshake.
                        let snapshot = bundle.load_full();
                        let acceptor = TlsAcceptor::from(Arc::clone(&snapshot.server_config));
                        match lb_security::timeout_accept(
                            &acceptor,
                            client_stream,
                            st.handshake_timeout,
                        )
                        .await
                        {
                            Ok(tls_stream) => {
                                if let Some(sni) = tls_stream.get_ref().1.server_name() {
                                    tracing::trace!(
                                        client = %client_addr,
                                        sni = sni,
                                        "TLS SNI captured (PROTO-2-15 observability)"
                                    );
                                }
                                proxy_connection(
                                    tls_stream,
                                    backend_addr,
                                    &st.metrics,
                                    st.connect_timeout,
                                )
                                .await
                            }
                            Err(e) => Err(anyhow::Error::new(e)),
                        }
                    }
                    ListenerMode::H1 { proxy } => {
                        http_version = Some("h1");
                        proxy
                            .load_full()
                            .serve_connection_with_cancel(
                                client_stream,
                                client_addr,
                                st.shutdown_token.clone(),
                            )
                            .await
                            .map_err(anyhow::Error::from)
                    }
                    ListenerMode::H1s {
                        h1_proxy,
                        h2_proxy,
                        bundle,
                        ..
                    } => {
                        let snapshot = bundle.load_full();
                        let acceptor = TlsAcceptor::from(Arc::clone(&snapshot.server_config));
                        match lb_security::timeout_accept(
                            &acceptor,
                            client_stream,
                            st.handshake_timeout,
                        )
                        .await
                        {
                            Ok(tls_stream) => {
                                let sni = tls_stream.get_ref().1.server_name().map(str::to_owned);
                                if let Some(s) = sni.as_deref() {
                                    tracing::trace!(
                                        client = %client_addr,
                                        sni = s,
                                        "TLS SNI captured on H1s (PROTO-2-18)"
                                    );
                                }
                                let alpn =
                                    tls_stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                                if alpn.as_deref() == Some(b"h2".as_ref()) {
                                    http_version = Some("h2");
                                    h2_proxy
                                        .load_full()
                                        .serve_connection_with_cancel_sni(
                                            tls_stream,
                                            client_addr,
                                            st.shutdown_token.clone(),
                                            sni,
                                        )
                                        .await
                                        .map_err(anyhow::Error::from)
                                } else {
                                    http_version = Some("h1");
                                    h1_proxy
                                        .load_full()
                                        .serve_connection_with_cancel_sni(
                                            tls_stream,
                                            client_addr,
                                            st.shutdown_token.clone(),
                                            sni,
                                        )
                                        .await
                                        .map_err(anyhow::Error::from)
                                }
                            }
                            Err(e) => Err(anyhow::Error::new(e)),
                        }
                    }
                };
                (http_version, res)
            };

            // `biased` polls the cancel arm FIRST so a pending shutdown is not starved by a
            // continuously-ready work future.
            tokio::pin!(work);
            let (http_version, result) = tokio::select! {
                biased;
                () = conn_cancel.cancelled() => {
                    // ROUND8 OPS-02 div-l7: each connection draws its own `[0, jitter)` sleep on
                    // cancel so aborts spread WITHIN the pod, on top of the coordinator's
                    // per-process draw.
                    let jitter = {
                        let ceil = st.per_conn_drain_jitter_ms;
                        if ceil == 0 {
                            Duration::ZERO
                        } else {
                            use rand::RngExt;
                            Duration::from_millis(
                                rand::rng().random_range(0..ceil),
                            )
                        }
                    };
                    tokio::select! {
                        biased;
                        r = &mut work => r,
                        () = tokio::time::sleep(jitter) => {
                            if let Ok(c) = st.metrics.counter(
                                "shutdown_aborted_connections_total",
                                "Per-connection tasks cancelled mid-flight by SIGTERM drain",
                            ) {
                                c.inc();
                            }
                            tracing::debug!(
                                client = %client_addr,
                                backend = %backend_addr,
                                jitter_ms = jitter.as_millis() as u64,
                                "per-conn task cancelled by SIGTERM drain (post per-conn jitter)"
                            );
                            (None, Err(anyhow::anyhow!("connection cancelled by shutdown")))
                        }
                    }
                }
                r = &mut work => r,
            };

            if let Some(version) = http_version {
                let status_class = if result.is_ok() { "2xx" } else { "5xx" };
                let listener_label = st.listener_label.as_str();
                let route_label = "";
                if let Ok(v) = st.metrics.counter_vec(
                    "http_requests_total",
                    "HTTP requests terminated by the L7 proxy",
                    &["listener", "route", "version", "status_class"],
                ) {
                    v.with_label_values(&[listener_label, route_label, version, status_class])
                        .inc();
                }
                if let Ok(h) = st.metrics.histogram_vec(
                    "http_request_duration_seconds",
                    "L7 request duration from accept to response body sent",
                    &["listener", "route", "version"],
                    &http_latency_buckets(),
                ) {
                    h.with_label_values(&[listener_label, route_label, version])
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

async fn proxy_connection<C>(
    mut client: C,
    backend_addr: SocketAddr,
    metrics: &MetricsRegistry,
    connect_timeout: Duration,
) -> anyhow::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
{
    if let Ok(c) = metrics.counter("pool_acquires_total", "TcpPool acquire attempts") {
        c.inc();
    }
    let dial = tokio::time::timeout(connect_timeout, TcpStream::connect(backend_addr)).await;
    let mut backend = match dial {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            if let Ok(c) = metrics.counter("pool_probe_failures_total", "TcpPool probe failures") {
                c.inc();
            }
            return Err(anyhow::Error::new(e))
                .with_context(|| format!("cannot connect to backend {backend_addr}"));
        }
        Err(_elapsed) => {
            if let Ok(c) = metrics.counter(
                "backend_connect_timeout_total",
                "Backend TcpStream::connect timeouts",
            ) {
                c.inc();
            }
            anyhow::bail!(
                "backend connect timeout ({}ms) for {backend_addr}",
                connect_timeout.as_millis()
            );
        }
    };

    let copy_result = io::copy_bidirectional(&mut client, &mut backend).await;

    match copy_result {
        Ok((client_to_backend, backend_to_client)) => {
            metrics.increment("bytes_client_to_backend", client_to_backend);
            metrics.increment("bytes_backend_to_client", backend_to_client);
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_config::BackendConfig;
    use std::net::Ipv4Addr;

    fn h3_backend(address: &str, ca: Option<&str>, verify: bool) -> BackendConfig {
        BackendConfig {
            address: address.to_string(),
            protocol: "h3".to_string(),
            weight: 1,
            tls_ca_path: ca.map(String::from),
            tls_verify_hostname: None,
            tls_verify_peer: verify,
        }
    }

    #[test]
    fn build_h3_upstream_pool_rejects_mismatched_verify_peer() {
        let a = h3_backend("127.0.0.1:4001", Some("/etc/ssl/ca.pem"), true);
        let b = h3_backend("127.0.0.1:4002", Some("/etc/ssl/ca.pem"), false);
        let err = build_h3_upstream_pool(&[a, b]).unwrap_err();
        assert!(
            err.to_string().contains("must share tls_verify_peer"),
            "expected mismatch error, got: {err}"
        );
    }

    #[test]
    fn build_h3_upstream_pool_rejects_mismatched_ca_path() {
        let a = h3_backend("127.0.0.1:4001", Some("/etc/ssl/ca-a.pem"), true);
        let b = h3_backend("127.0.0.1:4002", Some("/etc/ssl/ca-b.pem"), true);
        let err = build_h3_upstream_pool(&[a, b]).unwrap_err();
        assert!(
            err.to_string().contains("must share"),
            "expected mismatch error, got: {err}"
        );
    }

    #[test]
    fn build_h3_upstream_pool_rejects_empty_backend_list() {
        let err = build_h3_upstream_pool(&[]).unwrap_err();
        assert!(
            err.to_string().contains("zero H3 backends"),
            "expected zero-backends error, got: {err}"
        );
    }

    #[test]
    fn build_h3_upstream_pool_rejects_verify_without_ca() {
        let a = h3_backend("127.0.0.1:4001", None, true);
        let err = build_h3_upstream_pool(&[a]).unwrap_err();
        assert!(
            err.to_string().contains("requires tls_ca_path"),
            "expected ca-required error, got: {err}"
        );
    }

    #[test]
    fn build_h3_upstream_pool_accepts_uniform_verify_off_without_ca() {
        let a = h3_backend("127.0.0.1:4001", None, false);
        let b = h3_backend("127.0.0.1:4002", None, false);
        build_h3_upstream_pool(&[a, b]).unwrap();
    }

    // Proves the binary's config→params path actually reaches Mode B, not just that the library
    // can.

    fn quic_cfg_with_raw_proxy(raw: Option<lb_config::RawQuicProxyConfig>) -> QuicListenerConfig {
        QuicListenerConfig {
            cert_path: "/tmp/eg-test-cert.pem".into(),
            key_path: "/tmp/eg-test-key.pem".into(),
            retry_secret_path: "/tmp/eg-test-retry.secret".into(),
            max_idle_timeout_ms: 30_000,
            max_recv_udp_payload_size: 1_350,
            raw_proxy: raw,
        }
    }

    fn raw_proxy_block() -> lb_config::RawQuicProxyConfig {
        lb_config::RawQuicProxyConfig {
            backend_addr: "127.0.0.1:4443".into(),
            sni: "backend.test".into(),
            backend_ca_path: None,
            dgram_queue_cap: 512,
            max_relay_streams: 128,
        }
    }

    #[test]
    fn raw_proxy_present_builds_mode_b_params() {
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let rp = raw_proxy_block();
        let cfg = quic_cfg_with_raw_proxy(Some(rp.clone()));
        let backend = build_raw_quic_backend(&rp).expect("build raw backend");
        let params = quic_listener_params_from_config(bind, &cfg, Some(backend), None, 0, None);
        assert!(
            params.raw_quic_backend.is_some(),
            "a raw_proxy block must produce a Mode-B listener (raw_quic_backend = Some)"
        );
        assert_eq!(
            params.dgram_queue_cap, 512,
            "the DATAGRAM cap must come from the raw_proxy block"
        );
    }

    #[test]
    fn no_raw_proxy_keeps_h3_termination_params() {
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let cfg = quic_cfg_with_raw_proxy(None);
        // R3: with no raw_proxy block no backend is built, so the listener is byte-identical to the
        // pre-Mode-B H3 front.
        let params = quic_listener_params_from_config(bind, &cfg, None, None, 0, None);
        assert!(
            params.raw_quic_backend.is_none(),
            "R3: a config without raw_proxy must stay on the H3-terminate path (raw_quic_backend = None)"
        );
        assert!(
            params.quic_modeb_metrics.is_none(),
            "R3: no Mode-B metrics on the H3 path"
        );
    }

    fn h3_terminate_cfg_with_backend(
        backend: lb_config::BackendConfig,
    ) -> lb_config::ListenerConfig {
        lb_config::ListenerConfig {
            address: "127.0.0.1:0".to_string(),
            protocol: "quic".to_string(),
            tls: None,
            quic: Some(quic_cfg_with_raw_proxy(None)),
            alt_svc: None,
            http: None,
            h2_security: None,
            websocket: None,
            grpc: None,
            drain_timeout_ms: None,
            drain_jitter_ms: None,
            backends: vec![backend],
        }
    }

    #[tokio::test]
    async fn wire_h3_terminate_backends_dispatches_h2_arm() {
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let cfg = h3_terminate_cfg_with_backend(lb_config::BackendConfig {
            address: "127.0.0.1:3001".to_string(),
            protocol: "h2".to_string(),
            weight: 1,
            tls_ca_path: None,
            tls_verify_hostname: None,
            tls_verify_peer: true,
        });
        let params =
            quic_listener_params_from_config(bind, cfg.quic.as_ref().unwrap(), None, None, 0, None);
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let metrics = Arc::new(MetricsRegistry::new());
        let params = wire_h3_terminate_backends(params, &cfg, &pool, &resolver, &metrics)
            .await
            .expect("h2 arm must wire");
        assert!(
            params.h2_backend.is_some(),
            "an h2 backend must wire with_h2_backend (the H3→H2 arm)"
        );
        assert!(
            params.h3_backend.is_none(),
            "h2 backend must NOT set the h3_backend slot"
        );
        assert!(
            params.backends.is_empty(),
            "h2 backend must NOT populate the H1 backend list"
        );
    }

    #[tokio::test]
    async fn wire_h3_terminate_backends_dispatches_h3_arm() {
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let cfg = h3_terminate_cfg_with_backend(lb_config::BackendConfig {
            address: "127.0.0.1:3002".to_string(),
            protocol: "h3".to_string(),
            weight: 1,
            tls_ca_path: None,
            tls_verify_hostname: Some("h3.backend.test".to_string()),
            tls_verify_peer: false,
        });
        let params =
            quic_listener_params_from_config(bind, cfg.quic.as_ref().unwrap(), None, None, 0, None);
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let metrics = Arc::new(MetricsRegistry::new());
        let params = wire_h3_terminate_backends(params, &cfg, &pool, &resolver, &metrics)
            .await
            .expect("h3 arm must wire");
        let (_, addr, sni) = params
            .h3_backend
            .as_ref()
            .expect("an h3 backend must wire with_h3_backend (the H3→H3 arm)");
        assert_eq!(
            addr.to_string(),
            "127.0.0.1:3002",
            "the resolved H3 backend address must be threaded through"
        );
        assert_eq!(
            sni, "h3.backend.test",
            "the tls_verify_hostname override must become the upstream SNI"
        );
        assert!(
            params.h2_backend.is_none(),
            "h3 backend must NOT set the h2_backend slot"
        );
        assert!(
            params.backends.is_empty(),
            "h3 backend must NOT populate the H1 backend list"
        );
    }

    #[tokio::test]
    async fn wire_h3_terminate_backends_dispatches_h1_arm() {
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let cfg = h3_terminate_cfg_with_backend(lb_config::BackendConfig {
            address: "127.0.0.1:3003".to_string(),
            protocol: "h1".to_string(),
            weight: 1,
            tls_ca_path: None,
            tls_verify_hostname: None,
            tls_verify_peer: true,
        });
        let params =
            quic_listener_params_from_config(bind, cfg.quic.as_ref().unwrap(), None, None, 0, None);
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let metrics = Arc::new(MetricsRegistry::new());
        let params = wire_h3_terminate_backends(params, &cfg, &pool, &resolver, &metrics)
            .await
            .expect("h1 arm must wire");
        assert_eq!(
            params.backends,
            vec!["127.0.0.1:3003".parse::<SocketAddr>().unwrap()],
            "an h1 backend must populate the H1 backend list (with_backends)"
        );
        assert!(params.h2_backend.is_none() && params.h3_backend.is_none());
    }

    #[test]
    fn build_raw_quic_backend_rejects_unparseable_addr() {
        let mut rp = raw_proxy_block();
        rp.backend_addr = "not-an-addr".into();
        let err = build_raw_quic_backend(&rp).unwrap_err();
        assert!(
            err.to_string()
                .contains("invalid Mode B raw_proxy backend_addr"),
            "expected a clear parse error (no silent Mode-B disable), got: {err}"
        );
    }

    const MODEB_E2E_MAX_UDP: usize = 65_535;
    const MODEB_E2E_LB_SNI: &str = "lb.modeb.test";
    const MODEB_E2E_BACKEND_SNI: &str = "backend.modeb.test";
    const MODEB_E2E_ALPN: &[u8] = b"h3";
    const MODEB_E2E_BUDGET: Duration = Duration::from_secs(10);

    struct ModeBE2eCerts {
        dir: std::path::PathBuf,
        cert: std::path::PathBuf,
        key: std::path::PathBuf,
        ca: std::path::PathBuf,
    }

    impl Drop for ModeBE2eCerts {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn modeb_e2e_gen_certs(sni: &str, tag: &str) -> ModeBE2eCerts {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "lb-s19-b6-e2e-{tag}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut params = rcgen::CertificateParams::new(vec![sni.to_string()]).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params
            .extended_key_usages
            .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        let ca_path = dir.join("ca.pem");
        std::fs::write(&cert_path, cert.pem().as_bytes()).unwrap();
        std::fs::write(&key_path, key_pair.serialize_pem().as_bytes()).unwrap();
        std::fs::write(&ca_path, cert.pem().as_bytes()).unwrap();
        ModeBE2eCerts {
            dir,
            cert: cert_path,
            key: key_path,
            ca: ca_path,
        }
    }

    fn modeb_e2e_random_scid() -> [u8; quiche::MAX_CONN_ID_LEN] {
        use ring::rand::SecureRandom;
        let mut scid = [0u8; quiche::MAX_CONN_ID_LEN];
        ring::rand::SystemRandom::new().fill(&mut scid).unwrap();
        scid
    }

    fn modeb_e2e_payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 31 + 7) % 256) as u8).collect()
    }

    fn modeb_e2e_client_config(lb_ca: &std::path::Path) -> quiche::Config {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        cfg.set_application_protos(&[MODEB_E2E_ALPN]).unwrap();
        cfg.load_verify_locations_from_file(lb_ca.to_str().unwrap())
            .unwrap();
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(10_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(256 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(8);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        cfg.enable_dgram(true, 1024, 1024);
        cfg
    }

    fn modeb_e2e_spawn_echo_backend(certs: &ModeBE2eCerts) -> SocketAddr {
        let std_sock = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        std_sock.set_nonblocking(true).unwrap();
        let addr = std_sock.local_addr().unwrap();

        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        cfg.set_application_protos(&[MODEB_E2E_ALPN]).unwrap();
        cfg.load_cert_chain_from_pem_file(certs.cert.to_str().unwrap())
            .unwrap();
        cfg.load_priv_key_from_pem_file(certs.key.to_str().unwrap())
            .unwrap();
        cfg.set_max_idle_timeout(10_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(256 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(8);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        cfg.enable_dgram(true, 1024, 1024);

        tokio::spawn(async move {
            let socket = tokio::net::UdpSocket::from_std(std_sock).unwrap();
            let mut in_buf = vec![0u8; MODEB_E2E_MAX_UDP];
            let mut out_buf = vec![0u8; MODEB_E2E_MAX_UDP];
            let mut rd = vec![0u8; MODEB_E2E_MAX_UDP];
            let mut conn: Option<quiche::Connection> = None;
            let mut echo: std::collections::HashMap<u64, (Vec<u8>, bool, bool)> =
                std::collections::HashMap::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return;
                }
                if let Some(c) = conn.as_mut() {
                    let readable: Vec<u64> = c.readable().collect();
                    for sid in readable {
                        while let Ok((n, fin)) = c.stream_recv(sid, &mut rd) {
                            let e = echo.entry(sid).or_insert((Vec::new(), false, false));
                            e.0.extend_from_slice(rd.get(..n).unwrap_or(&[]));
                            if fin {
                                e.1 = true;
                            }
                            if fin || n == 0 {
                                break;
                            }
                        }
                    }
                    let sids: Vec<u64> = echo.keys().copied().collect();
                    for sid in sids {
                        if let Some(e) = echo.get_mut(&sid) {
                            let mut acc = 0usize;
                            while acc < e.0.len() {
                                let chunk = e.0.get(acc..).unwrap_or(&[]);
                                match c.stream_send(sid, chunk, false) {
                                    Ok(0) | Err(quiche::Error::Done) => break,
                                    Ok(n) => {
                                        acc += n;
                                        if n < chunk.len() {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                            if acc > 0 {
                                e.0.drain(..acc.min(e.0.len()));
                            }
                            if e.1
                                && e.0.is_empty()
                                && !e.2
                                && c.stream_send(sid, &[], true).is_ok()
                            {
                                e.2 = true;
                            }
                        }
                    }
                    while let Ok((n, info)) = c.send(&mut out_buf) {
                        let _ = socket
                            .send_to(out_buf.get(..n).unwrap_or(&[]), info.to)
                            .await;
                    }
                }
                let timeout = conn
                    .as_ref()
                    .and_then(quiche::Connection::timeout)
                    .unwrap_or(Duration::from_millis(5));
                match tokio::time::timeout(timeout, socket.recv_from(&mut in_buf)).await {
                    Ok(Ok((n, from))) => {
                        if conn.is_none() {
                            let scid = modeb_e2e_random_scid();
                            let scid_ref = quiche::ConnectionId::from_ref(&scid);
                            match quiche::accept(&scid_ref, None, addr, from, &mut cfg) {
                                Ok(c) => conn = Some(c),
                                Err(_) => continue,
                            }
                        }
                        if let Some(c) = conn.as_mut() {
                            let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                            let info = quiche::RecvInfo { from, to: addr };
                            let _ = c.recv(slice, info);
                        }
                    }
                    Ok(Err(_)) | Err(_) => {
                        if let Some(c) = conn.as_mut() {
                            c.on_timeout();
                        }
                    }
                }
            }
        });

        addr
    }

    /// **S19 B6 — the REAL `spawn_quic` Mode-B e2e.** A real quiche client.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_quic_mode_b_e2e_round_trips_through_real_listener() {
        // Distinct CAs: a single shared CA would not prove the two legs are separately verified.
        let lb_certs = modeb_e2e_gen_certs(MODEB_E2E_LB_SNI, "lb");
        let backend_certs = modeb_e2e_gen_certs(MODEB_E2E_BACKEND_SNI, "be");

        let backend_addr = modeb_e2e_spawn_echo_backend(&backend_certs);

        let retry_secret_path = lb_certs.dir.join("retry.secret");

        let listener_cfg = lb_config::ListenerConfig {
            address: "127.0.0.1:0".to_string(),
            protocol: "quic".to_string(),
            tls: None,
            quic: Some(QuicListenerConfig {
                cert_path: lb_certs.cert.to_string_lossy().into_owned(),
                key_path: lb_certs.key.to_string_lossy().into_owned(),
                retry_secret_path: retry_secret_path.to_string_lossy().into_owned(),
                max_idle_timeout_ms: 10_000,
                max_recv_udp_payload_size: 1_350,
                raw_proxy: Some(lb_config::RawQuicProxyConfig {
                    backend_addr: backend_addr.to_string(),
                    sni: MODEB_E2E_BACKEND_SNI.to_string(),
                    backend_ca_path: Some(backend_certs.ca.to_string_lossy().into_owned()),
                    dgram_queue_cap: 1024,
                    max_relay_streams: 256,
                }),
            }),
            alt_svc: None,
            http: None,
            h2_security: None,
            websocket: None,
            grpc: None,
            drain_timeout_ms: None,
            drain_jitter_ms: None,
            backends: vec![],
        };

        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic Mode-B must start");
        let lb_addr = listener.local_addr();

        let client_socket = Arc::new(
            tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap(),
        );
        let client_local = client_socket.local_addr().unwrap();
        let mut client_cfg = modeb_e2e_client_config(&lb_certs.ca);
        let c_scid = modeb_e2e_random_scid();
        let mut client = quiche::connect(
            Some(MODEB_E2E_LB_SNI),
            &quiche::ConnectionId::from_ref(&c_scid),
            client_local,
            lb_addr,
            &mut client_cfg,
        )
        .unwrap();

        let payload = modeb_e2e_payload(4096);
        let mut out = vec![0u8; MODEB_E2E_MAX_UDP];
        let mut in_buf = vec![0u8; MODEB_E2E_MAX_UDP];
        let mut sent = false;
        let mut echoed: Vec<u8> = Vec::new();
        let mut fin_seen = false;
        let deadline = tokio::time::Instant::now() + MODEB_E2E_BUDGET;

        loop {
            assert!(
                tokio::time::Instant::now() < deadline,
                "Mode-B e2e budget exhausted: established={}, echoed={}, fin={fin_seen}",
                client.is_established(),
                echoed.len()
            );

            loop {
                match client.send(&mut out) {
                    Ok((n, info)) => {
                        let _ = client_socket
                            .send_to(out.get(..n).unwrap_or(&[]), info.to)
                            .await;
                    }
                    Err(quiche::Error::Done) => break,
                    Err(e) => panic!("client send: {e:?}"),
                }
            }

            if client.is_established() && !sent {
                let n = client
                    .stream_send(0, &payload, true)
                    .expect("client stream_send");
                assert_eq!(n, payload.len(), "fixture: whole payload fits the window");
                sent = true;
                loop {
                    match client.send(&mut out) {
                        Ok((m, info)) => {
                            let _ = client_socket
                                .send_to(out.get(..m).unwrap_or(&[]), info.to)
                                .await;
                        }
                        Err(quiche::Error::Done) => break,
                        Err(e) => panic!("client send (post stream): {e:?}"),
                    }
                }
            }

            if client.is_established() {
                let readable: Vec<u64> = client.readable().collect();
                for sid in readable {
                    if sid != 0 {
                        continue;
                    }
                    loop {
                        match client.stream_recv(sid, &mut in_buf) {
                            Ok((n, fin)) => {
                                echoed.extend_from_slice(in_buf.get(..n).unwrap_or(&[]));
                                if fin {
                                    fin_seen = true;
                                    break;
                                }
                            }
                            Err(quiche::Error::Done) => break,
                            Err(quiche::Error::InvalidStreamState(_)) => break,
                            Err(e) => panic!("client stream_recv: {e:?}"),
                        }
                    }
                }
                if fin_seen && echoed.len() >= payload.len() {
                    break;
                }
            }

            let timeout = client.timeout().unwrap_or(Duration::from_millis(20));
            let wait = timeout.min(Duration::from_millis(20));
            if let Ok(Ok((n, from))) =
                tokio::time::timeout(wait, client_socket.recv_from(&mut in_buf)).await
            {
                let info = quiche::RecvInfo {
                    from,
                    to: client_local,
                };
                let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                let _ = client.recv(slice, info);
            } else {
                client.on_timeout();
            }
        }

        // 7) THE PROOF: the bytes round-tripped THROUGH the spawned listener, not around it.
        assert!(fin_seen, "client must observe the relayed FIN");
        assert_eq!(
            echoed, payload,
            "the payload must round-trip byte-identically through the real Mode-B listener"
        );
        assert_eq!(client.application_proto(), MODEB_E2E_ALPN);

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    const H3H1_E2E_ALPN: &[u8] = b"h3";
    const H3H1_E2E_SNI: &str = "lb.h3h1.test";
    const H3H1_E2E_MAX_UDP: usize = 65_535;
    const H3H1_E2E_BACKEND_STATUS: u16 = 200;
    const H3H1_E2E_BACKEND_BODY: &[u8] = b"f-s26-1-backend-ok";
    const H3H1_E2E_BUDGET: Duration = Duration::from_secs(20);

    fn h3h1_e2e_spawn_h1_backend() -> (SocketAddr, tokio::sync::oneshot::Receiver<String>) {
        let std_listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        tokio::spawn(async move {
            let listener = TcpListener::from_std(std_listener).unwrap();
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = Vec::with_capacity(2048);
            let mut tmp = [0u8; 2048];
            loop {
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                match tokio::io::AsyncReadExt::read(&mut sock, &mut tmp).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(tmp.get(..n).unwrap_or(&[])),
                }
            }
            let head = String::from_utf8_lossy(&buf).into_owned();
            let request_line = head.lines().next().unwrap_or("").to_string();
            let _ = tx.send(request_line);
            let resp = format!(
                "HTTP/1.1 {H3H1_E2E_BACKEND_STATUS} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                H3H1_E2E_BACKEND_BODY.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.write_all(H3H1_E2E_BACKEND_BODY).await;
            let _ = sock.shutdown().await;
        });
        (addr, rx)
    }

    fn h3h1_e2e_client_config(lb_ca: &std::path::Path) -> quiche::Config {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        cfg.set_application_protos(&[H3H1_E2E_ALPN]).unwrap();
        cfg.load_verify_locations_from_file(lb_ca.to_str().unwrap())
            .unwrap();
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(10_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        cfg.set_initial_max_data(1024 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(256 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(256 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(8);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_disable_active_migration(true);
        cfg
    }

    /// **F-S26-1 — the REAL `spawn_quic` H3→H1 e2e.**
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_quic_h3_terminate_forwards_to_h1_backend_through_real_listener() {
        use quiche::h3::NameValue;

        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "h3h1-lb");
        let retry_secret_path = lb_certs.dir.join("retry.secret");

        let (backend_addr, request_line_rx) = h3h1_e2e_spawn_h1_backend();

        let listener_cfg = lb_config::ListenerConfig {
            address: "127.0.0.1:0".to_string(),
            protocol: "quic".to_string(),
            tls: None,
            quic: Some(QuicListenerConfig {
                cert_path: lb_certs.cert.to_string_lossy().into_owned(),
                key_path: lb_certs.key.to_string_lossy().into_owned(),
                retry_secret_path: retry_secret_path.to_string_lossy().into_owned(),
                max_idle_timeout_ms: 10_000,
                max_recv_udp_payload_size: 1_350,
                raw_proxy: None,
            }),
            alt_svc: None,
            http: None,
            h2_security: None,
            websocket: None,
            grpc: None,
            drain_timeout_ms: None,
            drain_jitter_ms: None,
            backends: vec![lb_config::BackendConfig {
                address: backend_addr.to_string(),
                protocol: "h1".to_string(),
                weight: 1,
                tls_ca_path: None,
                tls_verify_hostname: None,
                tls_verify_peer: true,
            }],
        };

        // Sanity: the validator must ACCEPT a quic listener with an h1 backend, or the rest of this
        // test proves nothing.
        lb_config::validate_config(&lb_config::LbConfig {
            listeners: vec![listener_cfg.clone()],
            ..Default::default()
        })
        .expect("a quic H3-terminate listener with an h1 backend must validate");

        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic H3-terminate must start");
        let lb_addr = listener.local_addr();

        let client_socket = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let client_local = client_socket.local_addr().unwrap();
        let mut client_cfg = h3h1_e2e_client_config(&lb_certs.ca);
        let c_scid = modeb_e2e_random_scid();
        let mut conn = quiche::connect(
            Some(H3H1_E2E_SNI),
            &quiche::ConnectionId::from_ref(&c_scid),
            client_local,
            lb_addr,
            &mut client_cfg,
        )
        .unwrap();

        let h3_config = quiche::h3::Config::new().unwrap();
        let mut h3: Option<quiche::h3::Connection> = None;
        let mut req_sent = false;
        let mut status: Option<u16> = None;
        let mut body: Vec<u8> = Vec::new();
        let mut finished = false;
        let mut out = vec![0u8; H3H1_E2E_MAX_UDP];
        let mut in_buf = vec![0u8; H3H1_E2E_MAX_UDP];
        let deadline = tokio::time::Instant::now() + H3H1_E2E_BUDGET;

        loop {
            assert!(
                tokio::time::Instant::now() < deadline,
                "H3→H1 e2e budget exhausted: established={}, req_sent={req_sent}, status={status:?}",
                conn.is_established()
            );
            if conn.is_closed() {
                panic!(
                    "client conn closed before completion: peer={:?} local={:?} status={status:?}",
                    conn.peer_error(),
                    conn.local_error()
                );
            }

            if conn.is_established() && h3.is_none() {
                h3 = Some(
                    quiche::h3::Connection::with_transport(&mut conn, &h3_config)
                        .expect("h3 with_transport"),
                );
            }

            if let Some(h3c) = h3.as_mut() {
                if !req_sent {
                    let req = vec![
                        quiche::h3::Header::new(b":method", b"GET"),
                        quiche::h3::Header::new(b":scheme", b"https"),
                        quiche::h3::Header::new(b":authority", H3H1_E2E_SNI.as_bytes()),
                        quiche::h3::Header::new(b":path", b"/f-s26-1/probe"),
                    ];
                    match h3c.send_request(&mut conn, &req, true) {
                        Ok(_) => req_sent = true,
                        Err(quiche::h3::Error::StreamBlocked) => {}
                        Err(e) => panic!("send_request: {e:?}"),
                    }
                }
            }

            if let Some(h3c) = h3.as_mut() {
                loop {
                    match h3c.poll(&mut conn) {
                        Ok((_sid, quiche::h3::Event::Headers { list, .. })) => {
                            for h in &list {
                                if h.name() == b":status" {
                                    status = std::str::from_utf8(h.value())
                                        .ok()
                                        .and_then(|s| s.parse().ok());
                                }
                            }
                        }
                        Ok((sid, quiche::h3::Event::Data)) => {
                            let mut chunk = [0u8; 4096];
                            while let Ok(n) = h3c.recv_body(&mut conn, sid, &mut chunk) {
                                if n == 0 {
                                    break;
                                }
                                body.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                            }
                        }
                        Ok((_sid, quiche::h3::Event::Finished)) => {
                            finished = true;
                        }
                        Ok((_sid, quiche::h3::Event::Reset(e))) => {
                            panic!("H3 stream reset by LB: {e}");
                        }
                        Ok(_) => {}
                        Err(quiche::h3::Error::Done) => break,
                        Err(e) => panic!("h3 poll: {e:?}"),
                    }
                }
            }

            if finished && status.is_some() {
                break;
            }

            loop {
                match conn.send(&mut out) {
                    Ok((n, info)) => {
                        let _ = client_socket
                            .send_to(out.get(..n).unwrap_or(&[]), info.to)
                            .await;
                    }
                    Err(quiche::Error::Done) => break,
                    Err(e) => panic!("conn.send: {e:?}"),
                }
            }

            let qto = conn.timeout().unwrap_or(Duration::from_millis(20));
            let wait = qto.clamp(Duration::from_millis(2), Duration::from_millis(20));
            match tokio::time::timeout(wait, client_socket.recv_from(&mut in_buf)).await {
                Ok(Ok((n, from))) => {
                    let info = quiche::RecvInfo {
                        from,
                        to: client_local,
                    };
                    let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                    let _ = conn.recv(slice, info);
                }
                Ok(Err(_)) | Err(_) => conn.on_timeout(),
            }
        }

        // 7) THE PROOF: the request reached the H1 backend AND its 200 (NOT a 502) came back.
        assert_eq!(
            status,
            Some(H3H1_E2E_BACKEND_STATUS),
            "the H1 backend's 200 must come back (a 502 ⇒ backends NOT wired — the F-S26-1 gap)"
        );
        assert_eq!(
            body, H3H1_E2E_BACKEND_BODY,
            "the H1 backend body must round-trip byte-identically"
        );
        let request_line = tokio::time::timeout(Duration::from_secs(2), request_line_rx)
            .await
            .ok()
            .and_then(Result::ok)
            .expect("the H1 backend must have received the forwarded request");
        assert!(
            request_line.starts_with("GET /f-s26-1/probe"),
            "the backend must see the forwarded GET (request-line: {request_line:?})"
        );

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    fn ws_h3_e2e_spawn_ws_echo_backend() -> SocketAddr {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let std_listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        tokio::spawn(async move {
            let listener = TcpListener::from_std(std_listener).unwrap();
            while let Ok((sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(sock).await else {
                        return;
                    };
                    while let Some(Ok(msg)) = ws.next().await {
                        match msg {
                            Message::Text(_) | Message::Binary(_) => {
                                if ws.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Message::Ping(p) => {
                                let _ = ws.send(Message::Pong(p)).await;
                            }
                            Message::Close(c) => {
                                let _ = ws.send(Message::Close(c)).await;
                                break;
                            }
                            _ => {}
                        }
                    }
                });
            }
        });
        addr
    }

    #[allow(clippy::indexing_slicing)]
    fn ws_h3_e2e_encode_masked(opcode: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 126, "e2e WS frames stay small (7-bit len)");
        let mut f = Vec::with_capacity(payload.len() + 6);
        f.push(0x80 | opcode); // FIN=1 + opcode
        f.push(0x80 | (payload.len() as u8)); // MASK=1 + 7-bit length
        let mask = [0xA1u8, 0xB2, 0xC3, 0xD4];
        f.extend_from_slice(&mask);
        for (i, b) in payload.iter().enumerate() {
            f.push(b ^ mask[i % 4]);
        }
        f
    }

    fn ws_h3_e2e_close_frame() -> Vec<u8> {
        ws_h3_e2e_encode_masked(0x8, &[0x03, 0xE8])
    }

    #[allow(clippy::indexing_slicing)]
    fn ws_h3_e2e_parse_frame(buf: &[u8]) -> Option<(u8, Vec<u8>, usize)> {
        if buf.len() < 2 {
            return None;
        }
        let opcode = buf[0] & 0x0F;
        let masked = buf[1] & 0x80 != 0;
        let len7 = (buf[1] & 0x7F) as usize;
        let mut idx = 2usize;
        let plen = match len7.cmp(&126) {
            std::cmp::Ordering::Less => len7,
            std::cmp::Ordering::Equal => {
                if buf.len() < 4 {
                    return None;
                }
                let l = ((buf[2] as usize) << 8) | (buf[3] as usize);
                idx = 4;
                l
            }
            std::cmp::Ordering::Greater => return None,
        };
        let mask = if masked {
            if buf.len() < idx + 4 {
                return None;
            }
            let m = [buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]];
            idx += 4;
            Some(m)
        } else {
            None
        };
        if buf.len() < idx + plen {
            return None;
        }
        let mut payload = buf[idx..idx + plen].to_vec();
        if let Some(m) = mask {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= m[i % 4];
            }
        }
        Some((opcode, payload, idx + plen))
    }

    fn ws_h3_e2e_listener_cfg(
        lb_certs: &ModeBE2eCerts,
        retry_secret_path: &std::path::Path,
        backend_addr: SocketAddr,
    ) -> lb_config::ListenerConfig {
        lb_config::ListenerConfig {
            address: "127.0.0.1:0".to_string(),
            protocol: "quic".to_string(),
            tls: None,
            quic: Some(QuicListenerConfig {
                cert_path: lb_certs.cert.to_string_lossy().into_owned(),
                key_path: lb_certs.key.to_string_lossy().into_owned(),
                retry_secret_path: retry_secret_path.to_string_lossy().into_owned(),
                max_idle_timeout_ms: 15_000,
                max_recv_udp_payload_size: 1_350,
                raw_proxy: None,
            }),
            alt_svc: None,
            http: None,
            h2_security: None,
            websocket: Some(lb_config::WebsocketConfig {
                enabled: true,
                h3_extended_connect: true,
                ..Default::default()
            }),
            grpc: None,
            drain_timeout_ms: None,
            drain_jitter_ms: None,
            backends: vec![lb_config::BackendConfig {
                address: backend_addr.to_string(),
                protocol: "h1".to_string(),
                weight: 1,
                tls_ca_path: None,
                tls_verify_hostname: None,
                tls_verify_peer: true,
            }],
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum WsH3CloseMode {
        Clean,
        Reset,
        Fin,
    }

    async fn ws_h3_e2e_drive_client(
        lb_addr: SocketAddr,
        ca: &std::path::Path,
        mode: WsH3CloseMode,
        protocol: &[u8],
    ) -> (Option<u16>, bool) {
        use quiche::h3::NameValue;
        const PAYLOAD: &[u8] = b"hello over h3 ws";

        let client_socket = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let client_local = client_socket.local_addr().unwrap();
        let mut client_cfg = h3h1_e2e_client_config(ca);
        let c_scid = modeb_e2e_random_scid();
        let mut conn = quiche::connect(
            Some(H3H1_E2E_SNI),
            &quiche::ConnectionId::from_ref(&c_scid),
            client_local,
            lb_addr,
            &mut client_cfg,
        )
        .unwrap();

        let h3_config = quiche::h3::Config::new().unwrap();
        let mut h3: Option<quiche::h3::Connection> = None;
        let mut connect_sid: Option<u64> = None;
        let mut status: Option<u16> = None;
        let mut sent_frame = false;
        let mut closed = false;
        let mut close_drain = 0u32;
        let mut rx_buf: Vec<u8> = Vec::new();
        let mut echo_ok = false;
        let mut out = vec![0u8; H3H1_E2E_MAX_UDP];
        let mut in_buf = vec![0u8; H3H1_E2E_MAX_UDP];
        let deadline = tokio::time::Instant::now() + H3H1_E2E_BUDGET;

        loop {
            if tokio::time::Instant::now() >= deadline || conn.is_closed() {
                break;
            }
            if conn.is_established() && h3.is_none() {
                h3 = Some(
                    quiche::h3::Connection::with_transport(&mut conn, &h3_config)
                        .expect("h3 with_transport"),
                );
            }
            if let Some(h3c) = h3.as_mut() {
                if connect_sid.is_none() {
                    let req = [
                        quiche::h3::Header::new(b":method", b"CONNECT"),
                        quiche::h3::Header::new(b":protocol", protocol),
                        quiche::h3::Header::new(b":scheme", b"https"),
                        quiche::h3::Header::new(b":authority", H3H1_E2E_SNI.as_bytes()),
                        quiche::h3::Header::new(b":path", b"/chat"),
                    ];
                    match h3c.send_request(&mut conn, &req, false) {
                        Ok(sid) => connect_sid = Some(sid),
                        Err(quiche::h3::Error::StreamBlocked) => {}
                        Err(e) => panic!("send_request (extended CONNECT): {e:?}"),
                    }
                }
            }
            if let (Some(h3c), Some(sid)) = (h3.as_mut(), connect_sid) {
                if status == Some(200) && !sent_frame {
                    let frame = ws_h3_e2e_encode_masked(0x1, PAYLOAD);
                    if let Ok(n) = h3c.send_body(&mut conn, sid, &frame, false) {
                        if n == frame.len() {
                            sent_frame = true;
                        }
                    }
                }
            }
            if let Some(h3c) = h3.as_mut() {
                loop {
                    match h3c.poll(&mut conn) {
                        Ok((_sid, quiche::h3::Event::Headers { list, .. })) => {
                            for h in &list {
                                if h.name() == b":status" {
                                    status = std::str::from_utf8(h.value())
                                        .ok()
                                        .and_then(|s| s.parse().ok());
                                }
                            }
                        }
                        Ok((sid, quiche::h3::Event::Data)) => {
                            let mut chunk = [0u8; 4096];
                            while let Ok(n) = h3c.recv_body(&mut conn, sid, &mut chunk) {
                                if n == 0 {
                                    break;
                                }
                                rx_buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                            }
                        }
                        Ok(_) => {}
                        Err(quiche::h3::Error::Done) => break,
                        Err(_) => break,
                    }
                }
            }
            while let Some((opcode, payload, consumed)) = ws_h3_e2e_parse_frame(&rx_buf) {
                rx_buf.drain(..consumed);
                if opcode == 0x1 && payload == PAYLOAD {
                    echo_ok = true;
                }
            }
            if matches!(status, Some(s) if s != 200) {
                break;
            }
            if echo_ok && !closed {
                if let Some(sid) = connect_sid {
                    match mode {
                        WsH3CloseMode::Clean => {
                            if let Some(h3c) = h3.as_mut() {
                                let close = ws_h3_e2e_close_frame();
                                let _ = h3c.send_body(&mut conn, sid, &close, false);
                            }
                        }
                        WsH3CloseMode::Reset => {
                            // Abnormal drop: RESET_STREAM + STOP_SENDING (H3_REQUEST_CANCELLED) —
                            // the reset-vs-EOF control.
                            let _ = conn.stream_shutdown(sid, quiche::Shutdown::Write, 0x010c);
                            let _ = conn.stream_shutdown(sid, quiche::Shutdown::Read, 0x010c);
                        }
                        WsH3CloseMode::Fin => {
                            // Clean stream FIN with no WS Close frame: the client closes its send
                            // half.
                            if let Some(h3c) = h3.as_mut() {
                                let _ = h3c.send_body(&mut conn, sid, &[], true);
                            }
                        }
                    }
                    closed = true;
                }
            }
            if closed {
                close_drain += 1;
            }
            loop {
                match conn.send(&mut out) {
                    Ok((n, info)) => {
                        let _ = client_socket
                            .send_to(out.get(..n).unwrap_or(&[]), info.to)
                            .await;
                    }
                    Err(quiche::Error::Done) => break,
                    Err(_) => break,
                }
            }
            if closed && close_drain > 8 {
                break;
            }
            let qto = conn.timeout().unwrap_or(Duration::from_millis(20));
            let wait = qto.clamp(Duration::from_millis(2), Duration::from_millis(20));
            match tokio::time::timeout(wait, client_socket.recv_from(&mut in_buf)).await {
                Ok(Ok((n, from))) => {
                    let info = quiche::RecvInfo {
                        from,
                        to: client_local,
                    };
                    let slice = in_buf.get_mut(..n).unwrap_or(&mut []);
                    let _ = conn.recv(slice, info);
                }
                Ok(Err(_)) | Err(_) => conn.on_timeout(),
            }
        }
        (status, echo_ok)
    }

    /// **WS-over-H3 Stage C — the REAL-BINARY e2e.** Extended CONNECT → 200 → bidirectional WS
    /// frame relay (echo) → clean close, all through `spawn_quic`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ws_over_h3_extended_connect_echo_roundtrip_through_real_listener() {
        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "wsh3-lb");
        let retry_secret_path = lb_certs.dir.join("retry.secret");
        let backend_addr = ws_h3_e2e_spawn_ws_echo_backend();
        let listener_cfg = ws_h3_e2e_listener_cfg(&lb_certs, &retry_secret_path, backend_addr);

        lb_config::validate_config(&lb_config::LbConfig {
            listeners: vec![listener_cfg.clone()],
            ..Default::default()
        })
        .expect("a quic WS-over-H3 listener with an h1 backend must validate");

        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic WS-over-H3 must start");
        let lb_addr = listener.local_addr();

        let (status, echo_ok) =
            ws_h3_e2e_drive_client(lb_addr, &lb_certs.ca, WsH3CloseMode::Clean, b"websocket").await;

        // THE PROOF: a 200 (extended CONNECT success, NOT a 502) came back AND a WS Text frame
        // round-tripped through the relay.
        assert_eq!(
            status,
            Some(200),
            "extended CONNECT must yield 200 (a 502 ⇒ launcher/upstream-before-200 failed)"
        );
        assert!(
            echo_ok,
            "the WS Text frame must echo back through the wired tunnel (bidirectional relay)"
        );

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    #[derive(Debug, PartialEq, Eq)]
    enum WsBackendOutcome {
        CleanClose,
        Abrupt,
    }

    fn ws_h3_e2e_spawn_reporting_backend() -> (
        SocketAddr,
        tokio::sync::mpsc::UnboundedReceiver<WsBackendOutcome>,
    ) {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let std_listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let listener = TcpListener::from_std(std_listener).unwrap();
            while let Ok((sock, _)) = listener.accept().await {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(sock).await else {
                        return;
                    };
                    let mut saw_close = false;
                    loop {
                        match ws.next().await {
                            Some(Ok(msg @ (Message::Text(_) | Message::Binary(_)))) => {
                                if ws.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Some(Ok(Message::Close(_))) => {
                                saw_close = true;
                                break;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => break,
                        }
                    }
                    let _ = tx.send(if saw_close {
                        WsBackendOutcome::CleanClose
                    } else {
                        WsBackendOutcome::Abrupt
                    });
                });
            }
        });
        (addr, rx)
    }

    /// **R13 reset-vs-EOF NEGATIVE CONTROL (wired).** A clean WS Close reaches the backend AS a
    /// Close; a client `RESET_STREAM` of the tunnel stream reaches the backend as an ABRUPT end
    /// (NOT a clean Close).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ws_over_h3_reset_maps_to_abnormal_drop_not_clean_close() {
        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "wsh3-reset");
        let retry_secret_path = lb_certs.dir.join("retry.secret");
        let (backend_addr, mut outcomes) = ws_h3_e2e_spawn_reporting_backend();
        let listener_cfg = ws_h3_e2e_listener_cfg(&lb_certs, &retry_secret_path, backend_addr);

        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic WS-over-H3 must start");
        let lb_addr = listener.local_addr();

        // POSITIVE CONTROL: a clean WS Close → the backend sees CleanClose.
        let (status_c, echo_c) =
            ws_h3_e2e_drive_client(lb_addr, &lb_certs.ca, WsH3CloseMode::Clean, b"websocket").await;
        assert_eq!(status_c, Some(200), "clean: extended CONNECT must 200");
        assert!(echo_c, "clean: the frame must echo");
        let clean = tokio::time::timeout(Duration::from_secs(5), outcomes.recv())
            .await
            .expect("clean: backend must report an outcome")
            .expect("clean: outcomes channel open");
        assert_eq!(
            clean,
            WsBackendOutcome::CleanClose,
            "a WS Close must reach the backend AS a clean Close"
        );

        // NEGATIVE CONTROL: a RESET_STREAM → the backend sees an Abrupt end.
        let (status_r, echo_r) =
            ws_h3_e2e_drive_client(lb_addr, &lb_certs.ca, WsH3CloseMode::Reset, b"websocket").await;
        assert_eq!(status_r, Some(200), "reset: extended CONNECT must 200");
        assert!(echo_r, "reset: the frame must echo before the reset");
        let reset = tokio::time::timeout(Duration::from_secs(5), outcomes.recv())
            .await
            .expect("reset: backend must report an outcome")
            .expect("reset: outcomes channel open");
        assert_eq!(
            reset,
            WsBackendOutcome::Abrupt,
            "a client RESET_STREAM must reach the backend as an ABNORMAL drop, NOT a clean Close \
             (reset-vs-EOF mapping on the wired tunnel)"
        );

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    /// **R13 BURST.** ≥50 sequential extended-CONNECT → echo → close cycles against ONE listener +
    /// backend.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ws_over_h3_burst_50_upgrade_relay_close_cycles() {
        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "wsh3-burst");
        let retry_secret_path = lb_certs.dir.join("retry.secret");
        let backend_addr = ws_h3_e2e_spawn_ws_echo_backend();
        let listener_cfg = ws_h3_e2e_listener_cfg(&lb_certs, &retry_secret_path, backend_addr);

        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic WS-over-H3 must start");
        let lb_addr = listener.local_addr();

        const ITERS: u32 = 50;
        let mut ok = 0u32;
        for i in 0..ITERS {
            let (status, echo) =
                ws_h3_e2e_drive_client(lb_addr, &lb_certs.ca, WsH3CloseMode::Clean, b"websocket")
                    .await;
            assert_eq!(
                status,
                Some(200),
                "burst iter {i}: extended CONNECT must 200"
            );
            assert!(echo, "burst iter {i}: the frame must echo");
            ok += 1;
        }
        assert_eq!(
            ok, ITERS,
            "all {ITERS} upgrade+relay+close cycles must succeed"
        );

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    fn ws_h3_e2e_spawn_flood_backend(
        frame_len: usize,
        count: u64,
    ) -> (SocketAddr, std::sync::Arc<std::sync::atomic::AtomicU64>) {
        use futures_util::{SinkExt, StreamExt};
        use std::sync::atomic::{AtomicU64, Ordering};
        use tokio_tungstenite::tungstenite::Message;
        let std_listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        let sent = std::sync::Arc::new(AtomicU64::new(0));
        let sent_bg = std::sync::Arc::clone(&sent);
        tokio::spawn(async move {
            let listener = TcpListener::from_std(std_listener).unwrap();
            if let Ok((sock, _)) = listener.accept().await {
                // Shrink the backend's send buffer so a gateway that stops reading backpressures
                // quickly, instead of the flood hiding in kernel memory.
                let _ = socket2::SockRef::from(&sock).set_send_buffer_size(16 * 1024);
                let Ok(mut ws) = tokio_tungstenite::accept_async(sock).await else {
                    return;
                };
                let _ = ws.next().await;
                let payload = vec![0xCDu8; frame_len];
                for _ in 0..count {
                    if ws
                        .feed(Message::Binary(payload.clone().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    if ws.flush().await.is_err() {
                        break;
                    }
                    sent_bg.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        (addr, sent)
    }

    fn ws_h3_e2e_small_window_client_config(lb_ca: &std::path::Path) -> quiche::Config {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        cfg.set_application_protos(&[H3H1_E2E_ALPN]).unwrap();
        cfg.load_verify_locations_from_file(lb_ca.to_str().unwrap())
            .unwrap();
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(15_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        // CRITICAL: quiche AUTO-TUNES receive windows, so the CLIENT's are capped explicitly — left
        // to auto-tune to 16/24 MiB it would absorb the flood,
        // mask the gateway's backpressure and make this test vacuous.
        cfg.set_initial_max_data(64 * 1024);
        cfg.set_initial_max_stream_data_bidi_local(64 * 1024);
        cfg.set_initial_max_stream_data_bidi_remote(64 * 1024);
        cfg.set_initial_max_stream_data_uni(64 * 1024);
        cfg.set_initial_max_streams_bidi(8);
        cfg.set_initial_max_streams_uni(8);
        cfg.set_max_stream_window(64 * 1024);
        cfg.set_max_connection_window(64 * 1024);
        cfg.set_disable_active_migration(true);
        cfg
    }

    /// **R8 WIRED-TUNNEL backpressure (outbound, the CF-S27-2-relevant direction).** A backend
    /// floods `COUNT` frames at a client that WITHHOLDS reads.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ws_over_h3_outbound_backpressure_plateaus_then_drains() {
        use quiche::h3::NameValue;

        const FRAME_LEN: usize = 2048;
        const COUNT: u64 = 512; // 1 MiB of flood
        const CEILING: u64 = 256; // decisive vs 512; well above the true plateau

        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "wsh3-r8");
        let retry_secret_path = lb_certs.dir.join("retry.secret");
        let (backend_addr, sent) = ws_h3_e2e_spawn_flood_backend(FRAME_LEN, COUNT);
        let listener_cfg = ws_h3_e2e_listener_cfg(&lb_certs, &retry_secret_path, backend_addr);

        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        // Tiny backend SO_RCVBUF so the kernel TCP buffer between backend and gateway cannot hide
        // the plateau this test measures.
        let tiny_opts = BackendSockOpts {
            rcvbuf: Some(16 * 1024),
            sndbuf: Some(16 * 1024),
            ..backend_opts()
        };
        let pool = TcpPool::new(PoolConfig::default(), tiny_opts, Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic WS-over-H3 must start");
        let lb_addr = listener.local_addr();

        let client_socket = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let client_local = client_socket.local_addr().unwrap();
        let mut client_cfg = ws_h3_e2e_small_window_client_config(&lb_certs.ca);
        let c_scid = modeb_e2e_random_scid();
        let mut conn = quiche::connect(
            Some(H3H1_E2E_SNI),
            &quiche::ConnectionId::from_ref(&c_scid),
            client_local,
            lb_addr,
            &mut client_cfg,
        )
        .unwrap();

        let h3_config = quiche::h3::Config::new().unwrap();
        let mut h3: Option<quiche::h3::Connection> = None;
        let mut sid: Option<u64> = None;
        let mut status: Option<u16> = None;
        let mut triggered = false;
        let mut out = vec![0u8; H3H1_E2E_MAX_UDP];
        let mut in_buf = vec![0u8; H3H1_E2E_MAX_UDP];

        macro_rules! flush_out {
            () => {
                while let Ok((n, info)) = conn.send(&mut out) {
                    let _ = client_socket
                        .send_to(out.get(..n).unwrap_or(&[]), info.to)
                        .await;
                }
            };
        }

        let setup_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while status != Some(200) || !triggered {
            assert!(
                tokio::time::Instant::now() < setup_deadline && !conn.is_closed(),
                "R8 setup failed: status={status:?} triggered={triggered}"
            );
            if conn.is_established() && h3.is_none() {
                h3 = Some(
                    quiche::h3::Connection::with_transport(&mut conn, &h3_config)
                        .expect("h3 with_transport"),
                );
            }
            if let Some(h3c) = h3.as_mut() {
                if sid.is_none() {
                    let req = [
                        quiche::h3::Header::new(b":method", b"CONNECT"),
                        quiche::h3::Header::new(b":protocol", b"websocket"),
                        quiche::h3::Header::new(b":scheme", b"https"),
                        quiche::h3::Header::new(b":authority", H3H1_E2E_SNI.as_bytes()),
                        quiche::h3::Header::new(b":path", b"/flood"),
                    ];
                    if let Ok(s) = h3c.send_request(&mut conn, &req, false) {
                        sid = Some(s);
                    }
                }
                loop {
                    match h3c.poll(&mut conn) {
                        Ok((_s, quiche::h3::Event::Headers { list, .. })) => {
                            for h in &list {
                                if h.name() == b":status" {
                                    status = std::str::from_utf8(h.value())
                                        .ok()
                                        .and_then(|s| s.parse().ok());
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(quiche::h3::Error::Done) => break,
                        Err(_) => break,
                    }
                }
                if status == Some(200) && !triggered {
                    if let Some(s) = sid {
                        let frame = ws_h3_e2e_encode_masked(0x1, b"go");
                        if let Ok(n) = h3c.send_body(&mut conn, s, &frame, false) {
                            if n == frame.len() {
                                triggered = true;
                            }
                        }
                    }
                }
            }
            flush_out!();
            let qto = conn.timeout().unwrap_or(Duration::from_millis(20));
            let wait = qto.clamp(Duration::from_millis(2), Duration::from_millis(20));
            if let Ok(Ok((n, from))) =
                tokio::time::timeout(wait, client_socket.recv_from(&mut in_buf)).await
            {
                let info = quiche::RecvInfo {
                    from,
                    to: client_local,
                };
                let _ = conn.recv(in_buf.get_mut(..n).unwrap_or(&mut []), info);
            } else {
                conn.on_timeout();
            }
        }

        // Phase A: WITHHOLD reads long enough that an unbounded relay would drain the backend and
        // grow without limit.
        let withhold_until = tokio::time::Instant::now() + Duration::from_millis(1200);
        while tokio::time::Instant::now() < withhold_until {
            flush_out!();
            let qto = conn.timeout().unwrap_or(Duration::from_millis(20));
            let wait = qto.clamp(Duration::from_millis(2), Duration::from_millis(20));
            if let Ok(Ok((n, from))) =
                tokio::time::timeout(wait, client_socket.recv_from(&mut in_buf)).await
            {
                let info = quiche::RecvInfo {
                    from,
                    to: client_local,
                };
                let _ = conn.recv(in_buf.get_mut(..n).unwrap_or(&mut []), info);
            } else {
                conn.on_timeout();
            }
        }
        let plateau = sent.load(std::sync::atomic::Ordering::Relaxed);
        let cstats = conn.stats();
        eprintln!(
            "R8 WS-H3 outbound plateau: backend sent {plateau} / {COUNT} (ceiling {CEILING}); \
             client recv_bytes={} lost={}",
            cstats.recv, cstats.lost
        );
        assert!(
            plateau > 0,
            "non-vacuous: the backend must have pushed at least one frame, got {plateau}"
        );
        assert!(
            plateau < CEILING,
            "R8 VIOLATION: with the client stalled the backend pushed {plateau} of {COUNT} frames \
             — the wired tunnel is NOT backpressuring (expected a plateau < {CEILING})"
        );

        // Phase B: RESUME reading → every frame drains (liveness, no loss).
        let mut payload_bytes: u64 = 0;
        let mut rx_buf: Vec<u8> = Vec::new();
        let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while payload_bytes < COUNT * (FRAME_LEN as u64) {
            assert!(
                tokio::time::Instant::now() < drain_deadline && !conn.is_closed(),
                "R8 drain incomplete: got {payload_bytes} / {} bytes (sent={})",
                COUNT * (FRAME_LEN as u64),
                sent.load(std::sync::atomic::Ordering::Relaxed)
            );
            if let Some(h3c) = h3.as_mut() {
                loop {
                    match h3c.poll(&mut conn) {
                        Ok((s, quiche::h3::Event::Data)) => {
                            let mut chunk = [0u8; 8192];
                            while let Ok(n) = h3c.recv_body(&mut conn, s, &mut chunk) {
                                if n == 0 {
                                    break;
                                }
                                rx_buf.extend_from_slice(chunk.get(..n).unwrap_or(&[]));
                            }
                        }
                        Ok(_) => {}
                        Err(quiche::h3::Error::Done) => break,
                        Err(_) => break,
                    }
                }
            }
            while let Some((opcode, payload, consumed)) = ws_h3_e2e_parse_frame(&rx_buf) {
                rx_buf.drain(..consumed);
                if opcode == 0x2 {
                    payload_bytes += payload.len() as u64;
                }
            }
            flush_out!();
            let qto = conn.timeout().unwrap_or(Duration::from_millis(20));
            let wait = qto.clamp(Duration::from_millis(2), Duration::from_millis(20));
            if let Ok(Ok((n, from))) =
                tokio::time::timeout(wait, client_socket.recv_from(&mut in_buf)).await
            {
                let info = quiche::RecvInfo {
                    from,
                    to: client_local,
                };
                let _ = conn.recv(in_buf.get_mut(..n).unwrap_or(&mut []), info);
            } else {
                conn.on_timeout();
            }
        }
        assert_eq!(
            payload_bytes,
            COUNT * (FRAME_LEN as u64),
            "liveness: every flooded byte must arrive once the client resumes reading"
        );
        assert_eq!(
            sent.load(std::sync::atomic::Ordering::Relaxed),
            COUNT,
            "the backend must have flushed all {COUNT} frames once backpressure released"
        );

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    fn ws_h3_e2e_spawn_dead_backend() -> SocketAddr {
        let std_listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        std_listener.set_nonblocking(true).unwrap();
        let addr = std_listener.local_addr().unwrap();
        tokio::spawn(async move {
            let listener = TcpListener::from_std(std_listener).unwrap();
            while let Ok((sock, _)) = listener.accept().await {
                drop(sock); // close immediately — no WS handshake
            }
        });
        addr
    }

    /// **RFC 9220 §4 — unknown `:protocol` → 501.** An extended CONNECT with `:protocol=mqtt`
    /// (registered-but-unsupported) is rejected with 501 BEFORE any backend is dialed; no tunnel is
    /// built.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ws_over_h3_unknown_protocol_yields_501() {
        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "wsh3-501");
        let retry_secret_path = lb_certs.dir.join("retry.secret");
        let backend_addr = ws_h3_e2e_spawn_ws_echo_backend();
        let listener_cfg = ws_h3_e2e_listener_cfg(&lb_certs, &retry_secret_path, backend_addr);

        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic WS-over-H3 must start");
        let lb_addr = listener.local_addr();

        let (status, echo) =
            ws_h3_e2e_drive_client(lb_addr, &lb_certs.ca, WsH3CloseMode::Clean, b"mqtt").await;
        assert_eq!(
            status,
            Some(501),
            "an unsupported :protocol must yield 501 (RFC 9220 §4)"
        );
        assert!(!echo, "no tunnel ⇒ no echo");

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    /// **RFC 9220 §5 — upstream unreachable ⇒ 502, never a premature 200.**
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ws_over_h3_upstream_unreachable_yields_502() {
        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "wsh3-502");
        let retry_secret_path = lb_certs.dir.join("retry.secret");
        let dead = ws_h3_e2e_spawn_dead_backend();
        let listener_cfg = ws_h3_e2e_listener_cfg(&lb_certs, &retry_secret_path, dead);

        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic WS-over-H3 must start");
        let lb_addr = listener.local_addr();

        let (status, echo) =
            ws_h3_e2e_drive_client(lb_addr, &lb_certs.ca, WsH3CloseMode::Clean, b"websocket").await;
        assert_eq!(
            status,
            Some(502),
            "a failed upstream WS handshake must yield 502 (NOT 200 — upstream-before-200)"
        );
        assert!(!echo, "no tunnel established ⇒ no echo");

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    /// **Client stream-FIN (no WS Close frame) → abnormal close.** The client closes its WS
    /// send half by FINning the H3 tunnel stream without a WS Close frame.
    /// `conn_actor::ws_handle_client_fin` must map that FIN to a clean EOF, NOT a Reset:
    /// per RFC 6455 §7.1.5 the only clean close is the Close-frame handshake, so the
    /// gateway must not fabricate one from a bare FIN.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ws_over_h3_client_stream_fin_without_close_is_abnormal() {
        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "wsh3-fin");
        let retry_secret_path = lb_certs.dir.join("retry.secret");
        let (backend_addr, mut outcomes) = ws_h3_e2e_spawn_reporting_backend();
        let listener_cfg = ws_h3_e2e_listener_cfg(&lb_certs, &retry_secret_path, backend_addr);

        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic WS-over-H3 must start");
        let lb_addr = listener.local_addr();

        let (status, echo) =
            ws_h3_e2e_drive_client(lb_addr, &lb_certs.ca, WsH3CloseMode::Fin, b"websocket").await;
        assert_eq!(status, Some(200), "fin: extended CONNECT must 200");
        assert!(echo, "fin: the frame must echo before the stream FIN");
        let outcome = tokio::time::timeout(Duration::from_secs(5), outcomes.recv())
            .await
            .expect("backend must report an outcome")
            .expect("outcomes channel open");
        assert_eq!(
            outcome,
            WsBackendOutcome::Abrupt,
            "a bare stream-FIN (no WS Close frame) is an RFC 6455 abnormal closure — the backend \
             must NOT see a fabricated clean Close"
        );

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    /// **Fail-closed negative control: `ws_enabled` but NO relay launcher ⇒ 502.**
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ws_over_h3_enabled_without_launcher_fails_closed_502() {
        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "wsh3-nolauncher");
        let retry_secret_path = lb_certs.dir.join("retry.secret");
        let backend_addr = ws_h3_e2e_spawn_ws_echo_backend(); // present but never dialed

        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        // The negative control: ws_enabled + backends, but no `with_ws_relay_launcher`.
        let params = QuicListenerParams::new(
            "127.0.0.1:0".parse().unwrap(),
            lb_certs.cert.clone(),
            lb_certs.key.clone(),
            retry_secret_path,
        )
        .with_backends(vec![backend_addr], pool)
        .with_websocket(true);

        let token = CancellationToken::new();
        let listener = QuicListener::spawn(params, token.clone())
            .await
            .expect("listener must bind");
        let lb_addr = listener.local_addr();

        let (status, echo) =
            ws_h3_e2e_drive_client(lb_addr, &lb_certs.ca, WsH3CloseMode::Clean, b"websocket").await;
        assert_eq!(
            status,
            Some(502),
            "ws_enabled without a launcher must fail closed with 502 (never tunnel)"
        );
        assert!(!echo, "no relay ⇒ no tunnel ⇒ no echo");

        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    #[test]
    fn lifecycle_signal_display_matches_canonical_names() {
        assert_eq!(LifecycleSignal::SigTerm.to_string(), "SIGTERM");
        assert_eq!(LifecycleSignal::SigInt.to_string(), "SIGINT");
        assert_eq!(LifecycleSignal::SigUsr1.to_string(), "SIGUSR1");
        assert_eq!(LifecycleSignal::SigHup.to_string(), "SIGHUP");
    }

    // S37-C: an in-flight connection keeps the ArcSwap snapshot it captured at accept, while a new
    // connection sees the swapped-in value.
    #[test]
    fn arcswap_captured_snapshot_survives_store() {
        use arc_swap::ArcSwap;
        use std::sync::Arc;

        let cell: Arc<ArcSwap<u32>> = Arc::new(ArcSwap::new(Arc::new(1_u32)));

        let in_flight = cell.load_full();
        assert_eq!(*in_flight, 1);

        cell.store(Arc::new(2_u32));

        assert_eq!(
            *in_flight, 1,
            "captured snapshot must be unaffected by store"
        );
        let new_conn = cell.load_full();
        assert_eq!(
            *new_conn, 2,
            "new connection must observe the swapped value"
        );

        // Pointer identity: the old snapshot is the SAME allocation the in-flight conn holds,
        // proving it was not reset under it.
        assert!(Arc::ptr_eq(&in_flight, &in_flight.clone()));
        assert!(
            !Arc::ptr_eq(&in_flight, &new_conn),
            "new connection must hold a distinct snapshot from the in-flight one"
        );
    }

    #[test]
    fn classify_accept_error_recognises_emfile() {
        let e = std::io::Error::from_raw_os_error(24);
        assert_eq!(classify_accept_error(&e), AcceptErrorKind::EmfileOrEnfile);
    }

    #[test]
    fn classify_accept_error_recognises_enfile() {
        let e = std::io::Error::from_raw_os_error(23);
        assert_eq!(classify_accept_error(&e), AcceptErrorKind::EmfileOrEnfile);
    }

    #[test]
    fn classify_accept_error_recognises_conn_reset() {
        let e = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "peer rst");
        assert_eq!(classify_accept_error(&e), AcceptErrorKind::ConnReset);
    }

    #[test]
    fn classify_accept_error_unknown_is_fatal() {
        let e = std::io::Error::other("permission denied");
        assert_eq!(classify_accept_error(&e), AcceptErrorKind::Fatal);
    }

    #[test]
    fn test_emfile_no_busy_loop() {
        let mut d = Duration::ZERO;
        for _ in 0..20 {
            d = next_accept_backoff(d);
            assert!(d >= Duration::from_millis(1), "backoff must not be zero");
            // The cap is 1 s ± 25 % jitter → never exceed 1250 ms.
            assert!(
                d <= Duration::from_millis(1_250),
                "backoff capped at 1 s + jitter, got {d:?}"
            );
        }
        for _ in 0..20 {
            d = next_accept_backoff(d);
            assert!(d >= Duration::from_millis(750));
            assert!(d <= Duration::from_millis(1_250));
        }
    }

    #[tokio::test]
    async fn test_503_when_over_inflight_h1() {
        let (mut a, mut b) = tokio::io::duplex(8 * 1024);
        write_h1_shed_response(&mut a).await.unwrap();
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut b, &mut buf)
            .await
            .unwrap();
        let body = std::str::from_utf8(&buf).unwrap();
        assert!(
            body.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
            "unexpected status line: {body}"
        );
        assert!(
            body.contains("connection: close"),
            "must signal close: {body}"
        );
        assert!(
            body.contains("listener over capacity"),
            "body must explain the shed: {body}"
        );
    }

    #[tokio::test]
    async fn test_connect_uses_async_path() {
        // Reserved TEST-NET-1 — guaranteed to black-hole SYN.
        let dead: SocketAddr = "192.0.2.1:1".parse().unwrap();
        let metrics = MetricsRegistry::new();
        let (a, _b) = tokio::io::duplex(1024);
        let start = Instant::now();
        let err = proxy_connection(a, dead, &metrics, Duration::from_millis(120))
            .await
            .unwrap_err();
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(800),
            "async timeout did not fire (elapsed {elapsed:?}); likely still on spawn_blocking"
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("timeout") || msg.contains("connect"),
            "expected timeout/connect error, got: {msg}"
        );
    }

    #[test]
    fn test_typo_protocol_errors() {
        let listener_cfg = lb_config::ListenerConfig {
            address: "127.0.0.1:0".into(),
            protocol: "h1z".into(), // typo for "h1s"
            tls: None,
            quic: None,
            alt_svc: None,
            http: None,
            h2_security: None,
            websocket: None,
            grpc: None,
            drain_timeout_ms: None,
            drain_jitter_ms: None,
            backends: vec![],
        };
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let hooks = Arc::new(HooksBundle::new(
            ConnGate::new(64, 16, Vec::new()),
            SmuggleMode::H1,
        ));
        let tls_reload_registry: Arc<PlMutex<Vec<TlsReloadEntry>>> =
            Arc::new(PlMutex::new(Vec::new()));
        let tracker = TaskTracker::new();
        let cancel = CancellationToken::new();
        let outcome = build_listener_mode(
            &listener_cfg,
            &pool,
            &[],
            &hooks,
            &tls_reload_registry,
            &tracker,
            &cancel,
            None,
            100,
        );
        assert!(outcome.is_err(), "typo protocol should have errored");
        let msg = match outcome {
            Err(e) => e.to_string(),
            Ok(_) => String::new(),
        };
        assert!(
            msg.contains("no runtime implementation"),
            "expected explicit reject, got: {msg}"
        );
        assert!(
            msg.contains("h1z"),
            "error must name the offending value: {msg}"
        );
    }

    #[test]
    fn test_non_loopback_refused() {
        use lb_security::{AdminAuthGate, AdminBindError};
        let bind: SocketAddr = "0.0.0.0:9090".parse().unwrap();
        let err = AdminAuthGate::validate_bind(bind, false, false).unwrap_err();
        assert!(
            matches!(err, AdminBindError::NonLoopbackWithoutOverride { .. }),
            "expected non-loopback refusal, got: {err:?}"
        );
        AdminAuthGate::validate_bind("127.0.0.1:9090".parse().unwrap(), false, false).unwrap();
        let err2 = AdminAuthGate::validate_bind(bind, true, false).unwrap_err();
        assert!(matches!(
            err2,
            AdminBindError::PublicBindWithoutToken { .. }
        ));
        AdminAuthGate::validate_bind(bind, true, true).unwrap();
    }

    #[test]
    fn test_per_ip_cap_enforced_at_accept() {
        use std::net::{IpAddr, Ipv4Addr};
        let bundle = HooksBundle::new(ConnGate::new(64, 2, Vec::new()), SmuggleMode::H1);
        let peer: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let p1 = bundle.admit_connection(peer).unwrap();
        let p2 = bundle.admit_connection(peer).unwrap();
        let err = bundle.admit_connection(peer).unwrap_err();
        assert!(
            matches!(err, lb_security::SecurityReject::OverCap(_)),
            "third admission must be over_cap: {err:?}"
        );
        drop(p1);
        let _p3 = bundle.admit_connection(peer).unwrap();
        drop(p2);
    }

    #[test]
    fn panic_total_drains_fallback_into_registry_counter() {
        let baseline = panic_total();

        PANIC_TOTAL_FALLBACK.fetch_add(3, Ordering::Release);
        assert_eq!(panic_total(), baseline + 3, "fallback must be visible");

        let registry = MetricsRegistry::new();
        bind_panic_counter(&registry);

        assert_eq!(
            PANIC_TOTAL_FALLBACK.load(Ordering::Acquire),
            0,
            "bind_panic_counter must drain the fallback"
        );
        assert!(
            panic_total() >= baseline + 3,
            "drained fallback must show up in panic_total"
        );
    }
}
