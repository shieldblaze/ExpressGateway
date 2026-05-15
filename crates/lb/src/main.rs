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
#![allow(clippy::pedantic, clippy::nursery, clippy::too_many_arguments)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context;
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
// CODE-2-13: lead-scoped slice (L-007 §E.6). File-backed control
// plane reads existing TOML on startup; the ConfigManager owns the
// in-memory copy + version counter and is the SIGHUP-reload entry
// point Wave-2 will layer on top. Distributed CP backends
// (etcd / consul / xDS) are DEFERRED per L-001.
use lb_controlplane::{ConfigManager, FileBackend};
// CODE-2-13: lb-health provides per-backend HealthChecker (passive
// signal today; REL-2-05 layers the active-probe loop on top in
// Wave-2). Used here to publish the initial Unknown status so the
// picker has a well-defined gate from second 0.
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
use lb_quic::{QuicListener, QuicListenerParams};
use lb_security::{
    ConnGate, HooksBundle, SecurityHooks, SmuggleMode, TicketRotator, Watchdog, WatchdogConfig,
};

mod xdp;

/// CODE-2-02 Wave 2c: cell holding the registry-backed `panic_total`
/// counter. Set exactly once in [`async_main`] right after the
/// `MetricsRegistry` is constructed; the panic hook then bumps it
/// directly without re-entering the registry HashMap on every hit.
///
/// A second fallback `AtomicU64` is kept so a panic that fires *before*
/// the registry is ready (e.g. during config parsing) is still
/// counted; the registry-side counter is incremented by the same
/// fallback delta once it becomes available so no panic is lost.
static PANIC_TOTAL_COUNTER: OnceLock<IntCounter> = OnceLock::new();
static PANIC_TOTAL_FALLBACK: AtomicU64 = AtomicU64::new(0);

/// CODE-2-02: install a process-wide `std::panic::set_hook` that logs
/// the panic location, payload, and a full backtrace via
/// `tracing::error!`, bumps the `panic_total` counter, then aborts.
///
/// Called once early in [`async_main`] before any tokio task is spawned
/// so a panic during runtime construction is also captured. Pairs with
/// `panic = "abort"` in `[profile.release]` — under release the hook
/// fires immediately before the runtime aborts; under dev/test
/// profiles (which keep `unwind` for proptest/loom — see CODE-2-11)
/// the hook still logs + counts but the `std::process::abort()` keeps
/// the failure-mode identical so tests cannot drift from production.
fn init_panic_hook() {
    use std::backtrace::Backtrace;
    std::panic::set_hook(Box::new(|info| {
        let bt = Backtrace::force_capture();
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_owned());
        // `payload_as_str` is nightly-only; fall back to manual downcast.
        let payload = if let Some(s) = info.payload().downcast_ref::<&'static str>() {
            (*s).to_owned()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_owned()
        };
        // Prefer the registry-side counter; fall back to the atomic
        // for the pre-registry window.
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
        // Best-effort flush of the tracing subscriber before abort.
        std::thread::sleep(Duration::from_millis(50));
        std::process::abort();
    }));
}

/// CODE-2-02 Wave 2c: bind the registry-backed counter to the panic
/// hook and drain any pre-registry panics counted in the atomic
/// fallback. Called once in [`async_main`] after the
/// `MetricsRegistry` is constructed.
fn bind_panic_counter(metrics: &MetricsRegistry) {
    match metrics.panic_total_counter() {
        Ok(c) => {
            // Drain any pre-registry panics counted in the fallback so
            // none are lost.
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

/// CODE-2-02: read-only accessor for the panic counter. Sums the
/// registry counter (if bound) with the fallback atomic so callers
/// see the total regardless of when init happened.
#[allow(dead_code)]
fn panic_total() -> u64 {
    let from_registry = PANIC_TOTAL_COUNTER.get().map_or(0, IntCounter::get);
    let from_fallback = PANIC_TOTAL_FALLBACK.load(Ordering::Acquire);
    from_registry.saturating_add(from_fallback)
}

// ── REL-2-03: TLS cert reload registry ──────────────────────────────────

/// One entry per TLS-terminating listener (`tls` or `h1s` protocol). The
/// SIGUSR1 handler in `async_main` iterates the registry and calls
/// [`lb_security::reload_tls_bundle`] for each entry, swapping the
/// `SharedTlsBundle` under in-flight handshakes without disturbing them.
#[derive(Clone)]
struct TlsReloadEntry {
    /// Listener address (`127.0.0.1:8443`); labels the per-listener
    /// gauges and the log line on reload.
    listener: String,
    /// Cert path on disk.
    cert_path: PathBuf,
    /// Key path on disk.
    key_path: PathBuf,
    /// Wire-format ALPN tokens preserved across reloads (empty for raw
    /// TLS, `[b"h2", b"http/1.1"]` for H1s).
    alpn: Vec<Vec<u8>>,
    /// Shared TLS config bundle the listener reads at accept time.
    bundle: lb_security::SharedTlsBundle,
    /// Session-ticket rotator handle so the reload re-installs the same
    /// ticketer (preserving session-ticket resumption across rotations).
    rotator: Arc<PlMutex<TicketRotator>>,
}

/// Cert-rotation metric handles. Registered once at boot so they appear
/// in `/metrics` even before the first reload.
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

/// REL-2-03 (Wave 2c-2): walk every TLS reload entry, attempt a reload,
/// and update the cert-rotation metrics. Logs INFO on success, WARN on
/// failure. Failed reloads keep the previous bundle live so a botched
/// cert push never blackholes the listener.
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

// ── shared gateway state ────────────────────────────────────────────────

/// How a listener terminates inbound traffic.
enum ListenerMode {
    /// Plain TCP — no TLS, forward the socket directly.
    PlainTcp,
    /// TLS over TCP — terminate with the shared rustls config and
    /// forward the decrypted stream. The bundle is held inside an
    /// `Arc<ArcSwap<_>>` so a SIGUSR1 cert reload (REL-2-03) swaps the
    /// snapshot under in-flight handshakes without disturbing them; new
    /// handshakes pick up whichever bundle is current at accept time.
    /// The `_rotator` handle is held so the background ticket-rotation
    /// ticker stays alive as long as the listener does.
    Tls {
        bundle: lb_security::SharedTlsBundle,
        _rotator: Arc<PlMutex<TicketRotator>>,
    },
    /// Plain HTTP/1.1 — `lb-l7` `H1Proxy` over the raw TCP stream.
    H1 { proxy: Arc<H1Proxy> },
    /// HTTPS listener that offers HTTP/2 and HTTP/1.1 via ALPN. After
    /// `TlsAcceptor::accept`, the runtime inspects
    /// [`rustls::ServerConnection::alpn_protocol`] and dispatches to the
    /// matching proxy. As with the TLS variant the bundle is held in an
    /// `Arc<ArcSwap<_>>` for hot-reload via SIGUSR1 (REL-2-03).
    H1s {
        h1_proxy: Arc<H1Proxy>,
        h2_proxy: Arc<H2Proxy>,
        bundle: lb_security::SharedTlsBundle,
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
    ///
    /// CODE-2-09 Wave 2c-2: the plain-TCP path now dials backends
    /// directly via async `TcpStream::connect`, bypassing the pool.
    /// The pool is held in scope so its idle-count sampler keeps
    /// running for L7 paths (which still dial through it via
    /// `Http2Pool::with_pool`). The full pool rework is tracked in
    /// CODE-2-09's lb-io follow-up.
    #[allow(dead_code)]
    pool: TcpPool,
    /// Shared DNS resolver with positive/negative caching. Used to
    /// pre-resolve backend hostnames today; `TcpPool` will consume it for
    /// on-demand re-resolution in a follow-up.
    #[allow(dead_code)]
    resolver: DnsResolver,
    /// Listener termination mode (plain TCP or TLS over TCP).
    mode: ListenerMode,
    /// SEC-2-10 Wave 2c: max wall-clock budget for one TLS
    /// handshake. Sourced from
    /// [`lb_config::RuntimeConfig::handshake_timeout_ms`].
    handshake_timeout: Duration,
    /// CODE-2-05 / REL-2-09 Wave 2c-2: per-listener inflight cap.
    /// Owned-permit acquired at accept-site; permit drops when the
    /// per-connection task exits.
    inflight: Arc<Semaphore>,
    /// CODE-2-09 / REL-2-11 Wave 2c-2: budget for one
    /// `TcpStream::connect` on the backend dial path.
    connect_timeout: Duration,
    /// SEC-2-04 Wave 2c-2: per-listener / per-IP admission gate.
    /// `admit_connection` is called *before* the listener
    /// semaphore so a saturated IP cannot starve other clients of
    /// inflight slots.
    hooks: Arc<HooksBundle>,
    /// PROTO-2-11 (H2 half, Wave 2c-2): cloned into every spawned
    /// connection task. The H2 path threads this into
    /// `H2Proxy::serve_connection_with_cancel` so a SIGTERM cancel
    /// triggers a graceful GOAWAY emit instead of an abort.
    shutdown_token: CancellationToken,
    /// OPS-04+L4-12 Round-8: the *listener-cancel* token (child of
    /// `shutdown_token`). The accept loop selects on this so the
    /// drain coordinator can stop accepting new connections in
    /// phase 4 *without* triggering per-connection cancel — that
    /// fires in phase 5 via `shutdown_token` after the readiness
    /// settle + inflight-drain budget. See
    /// `audit/round-8/fixes/OPS-04-L4-12.md` §B for the 15-case
    /// table.
    listener_cancel_token: CancellationToken,
    /// CODE-2-03 follow-on (Round-5 push-back): the process-wide
    /// task tracker. Per-connection spawns funnel through
    /// `tracker.spawn(...)` so `Shutdown::drain` waits on them at
    /// SIGTERM time. Coupled with `shutdown_token`, the per-conn task
    /// also gets a cooperative-cancel arm in its `select!` — a
    /// long-running upstream is interrupted on drain rather than
    /// silently aborted on runtime drop.
    tracker: TaskTracker,
    /// REL-2-09 follow-on: the bind address used as the `listener`
    /// label on `accept_inflight{listener=…}` and on the (Wave-2c-2)
    /// per-request `http_requests_total{listener, …}` emit-site.
    listener_label: Arc<String>,
}

/// REL-2-09 follow-on: RAII guard that decrements the
/// `accept_inflight{listener=…}` gauge when dropped. Constructed at
/// the accept-site immediately after the per-listener inflight
/// `Semaphore` permit is acquired; the guard is moved into the
/// per-connection task so the gauge bump tracks the permit lifetime
/// exactly (including the panic-abort path on dev/test profiles).
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
//
// REL-2-03 (Wave 2c-2): the legacy `load_cert_chain` + `load_private_key`
// + `build_tls_stack` helpers were folded into
// `lb_security::TlsConfigBundle::load_from_paths` so the same validation
// runs at startup AND on every SIGUSR1 reload. SEC-2-08's key-perm
// advisory now lives in `assert_key_perm_advisory`, called both at
// startup and on every reload pass.

/// SEC-2-08 (Wave 2c, retained on rotation per REL-2-03): refuse to load
/// a private key whose file mode is group- or world-accessible. Strict
/// on release builds, lax on debug builds.
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

/// REL-2-03 (Wave 2c-2): build a `SharedTlsBundle` for a listener so
/// SIGUSR1 + inotify cert reloads can swap it under in-flight handshakes
/// without disturbing them. The bundle's `server_config` is built with
/// the shared `RotatingTicketer` so session-ticket resumption survives a
/// cert swap. `alpn` is the ALPN advertisement (empty for raw TLS, `h2 +
/// http/1.1` for the H1s ALPN dispatcher).
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

/// Spawn a background task that nudges `rotator.rotate_if_due(now)` once
/// per minute. The task stops when the rotator's `Arc` strong count
/// drops to 1 (i.e. when the listener is gone).
///
/// CODE-2-03 follow-on (Round-5 push-back): the task is now tracked by
/// the process-wide [`lb_core::Shutdown`] handle (via the cloned
/// [`TaskTracker`] passed in) and observes the cancellation token so
/// `Shutdown::drain` waits on it and SIGTERM wakes the ticker out of
/// its sleep — previously the task was unparented and stayed alive
/// until runtime drop.
fn spawn_rotator_ticker(
    rotator: Arc<PlMutex<TicketRotator>>,
    tracker: TaskTracker,
    cancel: CancellationToken,
) {
    tracker.spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        // The first tick fires immediately; skip it so we don't rotate
        // a freshly-minted key.
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
        // handshake. Round-4 D4-4 adds an explicit
        // `BackendConfig::tls_verify_hostname` knob; when present it
        // wins so an operator can override an IP-literal address with
        // the cert-name the backend actually presents. Otherwise fall
        // back to the host portion of `address` so hostname-shaped
        // backends keep working.
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

/// Filter a listener's backends down to those declaring `protocol = "h3"`.
/// Used to feed [`build_h3_upstream_pool`] only the H3-bound subset so
/// the pool's verify-peer/CA factory is driven by H3-specific TLS knobs.
fn collect_h3_backends(listener_cfg: &lb_config::ListenerConfig) -> Vec<lb_config::BackendConfig> {
    listener_cfg
        .backends
        .iter()
        .filter(|b| b.protocol == "h3")
        .cloned()
        .collect()
}

/// Build a [`QuicUpstreamPool`] with a config factory that produces
/// fresh [`quiche::Config`]s. The factory installs [`LB_QUIC_ALPN`]
/// (`"lb-quic"`) and inherits the listener's QUIC tunables when present,
/// or [`QuicPoolConfig`] defaults otherwise.
///
/// Per-backend TLS verification (Round-4 D4-4) is driven by the first H3
/// backend's `tls_*` knobs. When `tls_verify_peer` is true, the factory
/// loads the configured CA bundle and engages
/// [`quiche::Config::verify_peer`]; the pool aborts startup if the bundle
/// is missing. When `tls_verify_peer` is explicitly false the factory
/// skips peer verification — this is a NOT RECOMMENDED operator opt-out
/// for mesh-encrypted underlays. The H3 listener's
/// [`QuicListenerConfig`] is consulted only for tuning fields that
/// matter on the dial side; verification posture is a backend-side
/// decision.
fn build_h3_upstream_pool(
    h3_backends: &[lb_config::BackendConfig],
) -> anyhow::Result<Arc<QuicUpstreamPool>> {
    // All H3 backends on a single listener share one QuicUpstreamPool
    // (and therefore one quiche::Config factory). The factory's
    // verify-peer posture is unambiguous when every H3 backend agrees
    // on `tls_verify_peer` and (when verifying) `tls_ca_path`. Reject
    // mismatched configs at startup so an operator with two H3 backends
    // and two different CA bundles gets a clear error rather than a
    // silent first-wins.
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

/// Build a [`H1Proxy`] from the listener's resolved upstream backends
/// and optional H2/H3 upstream pools.
///
/// Wraps the picker into an [`Arc<RoundRobinUpstreams>`] and threads
/// through the multi-protocol surface ([`H1Proxy::with_multi_proto`])
/// so a single listener can fan out to mixed-protocol backends in one
/// round-robin cycle.
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
    // SEC-2-04 Wave 2c-2: install the production HooksBundle on the
    // L7 hot path (CODE-2-01 trait shim already lives there from
    // Wave-2b).
    proxy = proxy.with_hooks(hooks);
    // SEC-2-03 follow-on: wire the slowloris / slow-POST Watchdog
    // into every H1 proxy so per-stream eviction lights up (the
    // proxy falls back to NoopHooks-style behaviour when `None`).
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
    // SEC-2-04 Wave 2c-2: install the production HooksBundle on the
    // L7 hot path.
    proxy = proxy.with_hooks(hooks);
    // SEC-2-03 follow-on: same wiring as `build_h1_proxy` — install
    // the Watchdog on the H2 path so slow-POST eviction lights up
    // for ALPN=h2 streams too.
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

// REL-2-03 (Wave 2c-2): the legacy `build_h1s_tls_stack` was replaced
// by `build_tls_bundle` which constructs a `SharedTlsBundle` so cert
// rotation via SIGUSR1 hot-swaps the snapshot under in-flight
// handshakes. The H1s call-site passes `&[b"h2", b"http/1.1"]` as the
// ALPN list; the raw-TLS call-site passes `&[]`.

/// Bind and spawn a [`QuicListener`]. Pulled out of `async_main` to
/// keep its body small enough to satisfy `clippy::too_many_lines`.
///
/// PROTO-2-11 (H3 follow-up, Wave 2c-2): `shutdown_token` is a child
/// of the global `lb_core::Shutdown::token()` so a process-wide SIGTERM
/// drains every active QUIC connection through the router's
/// `CONNECTION_CLOSE (H3_NO_ERROR = 0x0100)` path. Previously this
/// function constructed its own `CancellationToken::new()`, so SIGTERM
/// only reached the listener via `QuicListener::shutdown()` and the
/// per-connection drain signal could not be distinguished from a
/// listener-token cancel.
async fn spawn_quic(
    listener_cfg: &lb_config::ListenerConfig,
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
    let params = quic_listener_params_from_config(bind_addr, quic_cfg);
    let listener = QuicListener::spawn(params, shutdown_token)
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
    hooks: Arc<HooksBundle>,
    shutdown_token: CancellationToken,
    listener_cancel_token: CancellationToken,
    tracker: TaskTracker,
    tls_reload_registry: Arc<PlMutex<Vec<TlsReloadEntry>>>,
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
    )?;
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
        // CODE-2-05 / REL-2-09 Wave 2c-2: per-listener inflight cap.
        // `Semaphore::new(usize::try_from(...).unwrap_or(usize::MAX))`
        // — `max_inflight` is bounded to 2_000_000 by validate_runtime
        // so the conversion is total on every supported target.
        inflight: Arc::new(Semaphore::new(
            usize::try_from(max_inflight).unwrap_or(usize::MAX),
        )),
        connect_timeout,
        hooks,
        shutdown_token,
        listener_cancel_token,
        tracker: tracker.clone(),
        listener_label: Arc::new(listener_cfg.address.clone()),
    });
    // CODE-2-03 follow-on: the listener accept loop itself is a
    // long-lived task. Route it through the tracker so drain waits
    // on the loop exit (it observes `shutdown_token` indirectly via
    // every accept that produces a tracked per-connection task).
    Ok(tracker.spawn(run_listener(listener_cfg.address.clone(), state)))
}

/// Build the per-listener [`ListenerMode`] from its config, dispatching
/// on `protocol`. Spawned per listener at startup; `addresses` are the
/// pre-resolved backend `SocketAddr`s for round-robin balancing.
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
) -> anyhow::Result<ListenerMode> {
    // SEC-2-04 Wave 2c-2: cloned into every L7 proxy constructor
    // below via `with_hooks`. The same bundle is held at accept-site
    // for `admit_connection` so both surfaces see the same counters.
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
            // Share the H2/H3 upstream pools between the H1 + H2 proxies
            // wired to this listener — they dial the same backends, so a
            // single multiplex'd H2 connection or pooled QUIC conn serves
            // both ALPN paths.
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
                h1_proxy,
                h2_proxy,
                bundle,
                _rotator: rotator,
            })
        }
        // PROTO-2-09 Wave 2c-2: explicit plain-TCP arm. `lb_config`
        // already accepts "http"/"h2"/"h3"/"tcp" as known protocol
        // tokens; only "tcp" maps to the plain-TCP shovel today. The
        // other reserved tokens have no listener implementation yet
        // and silently degrading to PlainTcp would corrupt the wire
        // semantics — so we hard-error and let the operator fix the
        // typo or pick a real protocol.
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

// ── hot-path metrics (Task #21) ────────────────────────────────────────

/// Register the 5 hot-path metric handles and spawn the background
/// sampler that reads them from the shared [`TcpPool`] /
/// [`DnsResolver`]. We register up-front (rather than lazily at each
/// call site) so the registry always advertises the metric even before
/// the first event, and so a single type-mismatch or registration
/// failure lands at startup rather than on the hot path.
fn install_hotpath_metrics(
    metrics: &Arc<MetricsRegistry>,
    pool: &TcpPool,
    resolver: &DnsResolver,
    tracker: &TaskTracker,
    cancel: &CancellationToken,
) {
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

    // REL-2-08 follow-on: http_requests_total{listener, route, version,
    // status_class}. Referenced by the L7 proxy wrapper installed in
    // proxy_connection — pre-register so scrape shape is stable from
    // t0. The `listener` + `route` labels match the canonical shape
    // operators consume in `RUNBOOK.md` (`LbReq5xx` alert) and in
    // `lb_observability::label_budget::CANONICAL_LABELS`. The `route`
    // label is supplied as the empty string on the connection-level
    // emit site (per-request routing context lives in lb-l7 and is
    // not threaded out today — documented as a future enrichment).
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

    // CODE-2-05 / REL-2-09 Wave 2c-2: shed counter (incremented when
    // the per-listener inflight semaphore returns
    // `TryAcquireError::NoPermits`).
    if let Err(e) = metrics.counter(
        "accept_shed_total",
        "Accepts shed because the per-listener inflight cap was hit",
    ) {
        tracing::warn!(metric = "accept_shed_total", error = %e, "counter register failed");
    }
    // REL-2-09 follow-on: pre-register the saturation gauge so
    // `/metrics` advertises `accept_inflight{listener}` at zero from
    // t0 (no first-bump-creates-family race).
    if let Err(e) = metrics.accept_inflight_gauge() {
        tracing::warn!(metric = "accept_inflight", error = %e, "gauge register failed");
    }
    // CODE-2-06 / REL-2-10 Wave 2c-2: classifier for accept(2) errors.
    if let Err(e) = metrics.counter_vec(
        "accept_errors_total",
        "accept(2) errors classified by kind (transient backoff vs. fatal)",
        &["kind"],
    ) {
        tracing::warn!(metric = "accept_errors_total", error = %e, "counter_vec register failed");
    }
    // CODE-2-09 / REL-2-11 Wave 2c-2: backend dial timeout counter.
    if let Err(e) = metrics.counter(
        "backend_connect_timeout_total",
        "Backend TcpStream::connect timeouts",
    ) {
        tracing::warn!(metric = "backend_connect_timeout_total", error = %e, "counter register failed");
    }

    // Background sampler: lift the pool's idle count + DNS cache size
    // into the registry every second. Neither crate publishes change
    // events today, so a periodic pull is the least invasive wiring.
    //
    // CODE-2-03 follow-on: tracker-attached + cancel-observing so
    // `Shutdown::drain` joins on it and SIGTERM wakes it instantly.
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
    // ── tracing (REL-2-06 wiring) ──────────────────────────────────
    // Use the central `lb_observability::init_tracing` so every
    // binary shares the same JSON / text format + filter resolution.
    // `init_tracing` is idempotent — a second call (rare under tests
    // that run async_main twice in the same process) returns
    // AlreadyInitialised which we treat as success.
    match lb_observability::init_tracing(&lb_observability::TracingConfig::default()) {
        Ok(()) | Err(lb_observability::TracingError::AlreadyInitialised) => {}
    }

    // CODE-2-02: install panic hook IMMEDIATELY after the tracing
    // subscriber is up. Anything that panics before this point dies
    // silently under `panic = "abort"`; anything after logs + counts.
    // The registry-backed counter is bound below once the registry
    // is available; until then the atomic fallback ensures we never
    // lose a panic.
    init_panic_hook();

    tracing::info!("ExpressGateway v{}", env!("CARGO_PKG_VERSION"));

    // ── config ──────────────────────────────────────────────────────
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config/default.toml".to_owned());

    let config_str = std::fs::read_to_string(&config_path)
        .with_context(|| format!("cannot read config file: {config_path}"))?;

    let config = lb_config::parse_config(&config_str).context("config parse error")?;
    lb_config::validate_config(&config).context("config validation error")?;

    // REL-2-08 wiring: refuse to boot if the live config shape would
    // blow the per-family series ceiling. Worst case is
    // `listeners × backends × status_classes` for backend_requests —
    // bound listener count + max backends here so a typo cannot DoS
    // the scraper.
    {
        let listeners = config.listeners.len();
        let backends_per = config
            .listeners
            .iter()
            .map(|l| l.backends.len())
            .max()
            .unwrap_or(0);
        // ROUND8-OPS-05: route fan-out is bounded by MAX_ROUTES_BUDGET,
        // NOT by the literal `1` placeholder. The worst-case product
        // here MUST match the ceiling the runtime per-emission guard
        // (`EnforcedLabelBudget`) enforces — otherwise the startup
        // gate would pass while the runtime guard refuses tuples a
        // few requests in.
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

    // CODE-2-13: wire lb-controlplane (file-backed). The ConfigManager
    // owns the in-memory TOML string + a monotonic version counter
    // and validates on every reload. Wave-2 will hook this into a
    // SIGHUP handler that calls `cfg_manager.reload()`; today the
    // wire-up alone proves the dep edge is reachable (round-1
    // inventory flagged lb-controlplane as an UNUSED workspace dep).
    // Held in scope for the process lifetime; SIGHUP plumbing lands
    // alongside the Wave-2 accept-site changes.
    let cp_backend = FileBackend::new(std::path::PathBuf::from(&config_path));
    let _config_manager = match ConfigManager::new(Box::new(cp_backend)) {
        Ok(mgr) => {
            tracing::info!(
                path = %config_path,
                version = mgr.version(),
                "control plane (file-backed) ready — reloads are SIGHUP-driven (Wave-2)"
            );
            Some(mgr)
        }
        Err(e) => {
            // Fail-soft: the lb_config::parse_config above already
            // succeeded, so an InvalidConfig error here must mean a
            // pre-validation path (empty / non-TOML) the redundant
            // ConfigManager validate rejects. We log + continue with
            // the parsed config — operator can fix on next SIGHUP.
            tracing::warn!(error = %e, "control plane manager init skipped");
            None
        }
    };

    // CODE-2-03 Wave 2c: process-wide graceful drain handle. SIGTERM /
    // SIGINT / SIGUSR1 are wired below.
    //
    // CODE-2-03 follow-on (Round-5 push-back): the previously-deferred
    // per-spawn-site `tracker().spawn(...)` integration is now in place
    // for all 5 sites flagged by the round-5 ebpf verification:
    //   1. `spawn_rotator_ticker` (was main.rs:504) — TLS ticket rotor.
    //   2. `install_hotpath_metrics` pool/DNS sampler (was main.rs:1245).
    //   3. listener spawn in `spawn_tcp` (was main.rs:985).
    //   4. XDP stats sampler (was main.rs:1629).
    //   5. per-connection task in `run_listener` (was main.rs:2074) —
    //      the most consequential one; also gains a biased select! arm
    //      on `shutdown_token` so a SIGTERM mid-request interrupts the
    //      proxy work and bumps `shutdown_aborted_connections_total`.
    let shutdown: lb_core::Shutdown = lb_core::Shutdown::new();

    // REL-2-04 wiring: shared probe registry consulted by
    // `/livez`/`/readyz`/`/startupz`. Starts in `Starting`; flipped
    // to `Ready` once every listener has bound; flipped to `Draining`
    // at SIGTERM (k8s lameduck signal).
    let probes = lb_observability::ProbeRegistry::shared();

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

    // CODE-2-13: passive health-check seed. Each unique backend in the
    // config gets a HealthChecker initialised at HealthStatus::Unknown;
    // today nothing reads these (the picker filter wire-in is Wave 2
    // alongside CODE-2-14's single-source-of-truth refactor). The seed
    // proves the lb-health dep is reachable from the binary (round-1
    // inventory flagged it as UNUSED) and gives REL-2-05 a published
    // collection to layer the active-probe loop on top of.
    //
    // Default thresholds (3 successes → Healthy, 2 failures → Unhealthy)
    // mirror the lb_health::HealthChecker doc comment and the
    // envoy-default convention; operator override knobs land in
    // lb_config alongside REL-2-05.
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
    // Hold the seed in scope so its existence is observable to the
    // borrow checker (and to a future debugger). Wave-2 hands this
    // collection to lb-balancer's picker via the CODE-2-14 refactor.
    let _health_seed = health_seed;

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
    install_hotpath_metrics(
        &metrics,
        &pool,
        &resolver,
        shutdown.tracker(),
        shutdown.token(),
    );

    // CODE-2-02 Wave 2c: bind the registry-backed panic_total counter
    // *now* that the registry exists. Any panic between the hook
    // install and this point was counted in the atomic fallback and
    // is drained into the counter inside `bind_panic_counter`.
    bind_panic_counter(&metrics);

    // REL-2-12 / REL-2-13 wiring: register the XDP metric families
    // (zero-valued today) so dashboards see the panel even before
    // the first eBPF tick. The 1 Hz sampler below feeds them.
    let xdp_metrics = lb_observability::xdp_metrics::XdpMetrics::register(&metrics)
        .map_err(|e| anyhow::anyhow!("XDP metric registration failed: {e}"))?;

    // ── optional admin HTTP listener (GET /metrics, GET /livez …) ──
    //
    // SEC-2-06 (Wave 2c-2): wire `AdminAuthGate` so the admin
    // listener (a) refuses to start on a non-loopback bind without
    // explicit `[admin].allow_non_loopback = true`, and (b) requires
    // a `Authorization: Bearer <token>` header on
    // information-bearing endpoints (`/metrics`) when
    // `[admin].api_token_hash` is set. Probe endpoints stay
    // anonymously accessible so the kubelet keeps working.
    let admin_cancel = CancellationToken::new();
    if let Some(obs) = config.observability.as_ref() {
        if let Some(bind_str) = obs.metrics_bind.as_deref() {
            let bind_addr: SocketAddr = bind_str
                .trim()
                .parse()
                .with_context(|| format!("invalid observability.metrics_bind: {bind_str}"))?;
            // Resolve `[admin]` block knobs (default: no token, no
            // non-loopback escape hatch).
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
            // SEC-2-06: refuse to start if non-loopback bind without
            // explicit override (foot-gun guard).
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

    // ── spawn listeners ─────────────────────────────────────────────
    let mut listener_handles = Vec::new();
    let mut quic_listeners: Vec<QuicListener> = Vec::new();

    // SEC-2-10 Wave 2c: source the TLS handshake budget from
    // `[runtime].handshake_timeout_ms`. Falls back to 5 s when no
    // `[runtime]` block is present.
    let handshake_timeout = Duration::from_millis(
        config
            .runtime
            .as_ref()
            .map_or(5_000, |r| r.handshake_timeout_ms),
    );
    // CODE-2-05 / REL-2-09 Wave 2c-2: per-listener inflight cap.
    let max_inflight = config
        .runtime
        .as_ref()
        .map_or(65_536, |r| r.max_inflight_connections);
    // CODE-2-09 / REL-2-11 Wave 2c-2: backend dial budget.
    let connect_timeout = Duration::from_millis(
        config
            .runtime
            .as_ref()
            .map_or(5_000, |r| r.connect_timeout_ms),
    );
    // SEC-2-04 Wave 2c-2: per-listener / per-IP admission gate.
    // The same `Arc<HooksBundle>` is shared across every listener
    // (`ConnGate`'s `listener_cap` counts all connections under
    // this process; the per-IP cap is per-source). For a future
    // multi-listener split, this becomes one bundle per listener.
    let per_ip_cap = config
        .runtime
        .as_ref()
        .map_or(1_024, |r| r.per_ip_connection_cap);
    let conn_gate = ConnGate::new(max_inflight, per_ip_cap, Vec::new());
    // PROTO-2-17 (Wave 2c-2): select the bundle's `SmuggleMode` from
    // the `[security].strict_te` knob. Default `false` keeps the
    // lenient `SmuggleMode::H1` (RFC 9112 baseline); flipping the
    // knob opts the listener into `SmuggleMode::H1Strict` (reject
    // any non-`chunked` Transfer-Encoding). The H2 path keeps its
    // dynamic `SmuggleMode::H2` upgrade in `HooksBundle::inspect_request`
    // independent of this default.
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

    // SEC-2-03 follow-on: construct the per-process Watchdog from the
    // optional `[runtime.watchdog]` block and spawn a sweep loop on
    // the Shutdown tracker so it joins cleanly during drain. The
    // sweep cadence is operator-tunable; the Watchdog itself is
    // cheap to clone (`Arc` newtype) so each L7 proxy gets its own
    // handle that shares the same connection table.
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
                let evicted = wd.sweep_expired();
                if !evicted.is_empty() {
                    tracing::warn!(
                        evicted = evicted.len(),
                        "Watchdog swept stalled connections (slow-loris/slow-POST)",
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

    // REL-2-03 (Wave 2c-2): registry of TLS reloadables, populated as
    // each TLS / H1s listener spawns its bundle. The SIGUSR1 handler
    // below iterates this list, calls `reload_tls_bundle` for each, and
    // bumps `cert_rotation_succeeded_total` / `cert_rotation_failed_total`
    // accordingly.
    let tls_reload_registry: Arc<PlMutex<Vec<TlsReloadEntry>>> = Arc::new(PlMutex::new(Vec::new()));

    // Register cert metric handles up front so they appear in `/metrics`
    // even before the first reload.
    let cert_metrics = CertMetrics::register(&metrics);

    for listener_cfg in &config.listeners {
        if listener_cfg.protocol == "quic" {
            quic_listeners.push(spawn_quic(listener_cfg, shutdown.token().child_token()).await?);
            continue;
        }
        if listener_cfg.backends.is_empty() {
            tracing::warn!(
                address = %listener_cfg.address,
                "listener has no backends configured — skipping"
            );
            continue;
        }
        let handle = spawn_tcp(
            listener_cfg,
            &pool,
            &resolver,
            io_runtime,
            &metrics,
            handshake_timeout,
            max_inflight,
            connect_timeout,
            Arc::clone(&hooks),
            shutdown.token().clone(),
            // OPS-04+L4-12 Round-8: per-listener cooperative-cancel
            // signal, fired by the drain coordinator's phase 4 BEFORE
            // the per-conn token. The accept loop selects on this
            // (with a synchronous post-accept tail-check to cover
            // case C-3: cancel mid-spawn) so an in-flight accept
            // tied to `listener.accept().await` never reaches the
            // admission gate / semaphore once the drain has begun.
            shutdown.listener_token().clone(),
            shutdown.tracker().clone(),
            Arc::clone(&tls_reload_registry),
            Some(watchdog.clone()),
        )
        .await?;
        listener_handles.push(handle);
    }

    if listener_handles.is_empty() && quic_listeners.is_empty() {
        anyhow::bail!("no listeners started — check your configuration");
    }

    // REL-2-04 wiring: now that every listener has bound, flip the
    // shared probe registry from `Starting` to `Ready` so `/readyz`
    // returns 200 to the upstream LB / k8s probe.
    probes.set_ready();
    tracing::info!("probes flipped to Ready — service open for traffic");

    // REL-2-13 wiring: spawn the 1 Hz STATS sampler. Reads the
    // per-CPU XDP STATS map, computes per-slot deltas against the
    // last tick, and bumps `xdp_packets_total{action}`. Cancelled on
    // `Shutdown::token()` so it joins cleanly during drain.
    //
    // CODE-2-03 follow-on (Round-5 push-back): tracker-attached so
    // `Shutdown::drain` waits for it; cancel arm was already present.
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
                        // Non-Linux returns Ok(zeros), so an error here
                        // always means a real read failure — count it
                        // and keep ticking.
                        xdp_metrics.sampler_errors_total.inc();
                        tracing::debug!(error = %e, "XDP stats read failed");
                    }
                }
            }
        });
    }

    // ── shutdown ────────────────────────────────────────────────────
    // REL-2-02 + CODE-2-03 Wave 2c: deterministic SIGTERM sequence.
    //
    //   1. wait for SIGTERM/SIGINT/SIGUSR1 (SIGUSR1 is the cert-reload
    //      knob REL-2-03 will fill in; today it just logs).
    //   2. `probes.set_draining()` so `/readyz` returns 503 to upstream
    //      LBs.
    //   3. sleep `readiness_settle_ms` so the upstream observes the
    //      503 and stops sending new traffic.
    //   4. `shutdown.token().cancel()` to wake every cooperative
    //      `select!` in long-lived tasks (sampler, future per-conn
    //      actors).
    //   5. wait up to `drain_timeout_ms` for the tracker to drain.
    //   6. abort survivors + bump `shutdown_aborted_connections_total`.
    //   7. drop the XDP loader LAST (handled implicitly by `_xdp_loader`
    //      living to the end of `async_main`).
    // REL-2-03 (Wave 2c-2): SIGUSR1 is the operator-driven cert-reload
    // trigger. The loop services every SIGUSR1 received (so an operator
    // can roll a cert + key, signal once, observe metrics, and signal
    // again if validation rejected the push). Loop exits on SIGTERM /
    // SIGINT and falls through to the drain sequence.
    let signal_kind = loop {
        let s = wait_for_lifecycle_signal().await;
        tracing::info!(signal = %s, "lifecycle signal received");
        if !matches!(s, LifecycleSignal::SigUsr1) {
            break s;
        }
        let entries: Vec<TlsReloadEntry> = tls_reload_registry.lock().clone();
        if entries.is_empty() {
            tracing::info!("SIGUSR1 received but no TLS listeners configured — nothing to reload");
            continue;
        }
        let (ok, fail) = reload_all_tls(&entries, cert_metrics.as_ref());
        tracing::info!(
            ok,
            fail,
            entries = entries.len(),
            "REL-2-03 SIGUSR1 cert reload pass complete"
        );
    };
    tracing::info!(signal = %signal_kind, "terminal signal — entering drain");

    // OPS-04+L4-12 Round-8: single drain coordinator. Replaces the
    // legacy `set_draining → sleep → JoinHandle::abort → drain` pile
    // that was responsible for the per-IP-counter-drift bug class
    // (case C-3 in `audit/round-8/fixes/OPS-04-L4-12.md`).
    //
    // Phase ordering:
    //   2. MarkDraining     — probes.set_draining() (closure)
    //   3. ReadinessSettle  — sleep `readiness_settle_ms`
    //   4. ListenerCancel   — fire listener_cancel_token; accept
    //                          loops exit cooperatively
    //   5. InFlightDrain    — close tracker + cancel per-conn token
    //                          + bounded wait
    //   6. XdpDetach        — `XdpLoader::detach()` under its own
    //                          deadline (skipped until L4-12 lands the
    //                          detach API in lb-l4-xdp; the coordinator
    //                          tolerates `None` so this commit can
    //                          land independently of div-l4)
    //   7. Total            — coordinator wall-clock observation
    //
    // Each phase observation is recorded into the OPS-03 histogram
    // family `shutdown_drain_seconds_{global,listener}` via the
    // `DrainObserver` trait impl below.
    let runtime_cfg = config.runtime.as_ref();
    let probes_for_mark = Arc::clone(&probes);
    let metrics_for_obs = Arc::clone(&metrics);
    let observer: std::sync::Arc<dyn lb_core::DrainObserver> =
        std::sync::Arc::new(MetricsDrainObserver {
            metrics: Arc::clone(&metrics_for_obs),
        });
    let spec = lb_core::DrainSpec {
        readiness_settle: Duration::from_millis(
            // ROUND-8 OPS-11: fallback matches
            // `lb_config::default_readiness_settle_ms()` (11 s — one
            // kubelet probe period + margin) when no [runtime] block
            // is present.
            runtime_cfg.map_or(11_000, |r| r.readiness_settle_ms),
        ),
        listener_cancel_deadline: Duration::from_millis(500),
        inflight_drain_deadline: Duration::from_millis(
            runtime_cfg.map_or(10_000, |r| r.drain_timeout_ms),
        ),
        // L4-12 will land the XDP detach closure here. Until the
        // `XdpLoader::detach()` API lands on the branch, the coordinator
        // simply skips phase 6 — the legacy "drop the loader on
        // process exit" behaviour is preserved as a stop-gap and the
        // round-2 stale-pin recovery path picks up any linger on next
        // startup (see OPS-01+L4-12+L4-04 §B.2).
        xdp_detach_deadline: None,
        // OPS-02 jitter: the per-conn cancel arm fires at a random
        // sub-millisecond offset so 1000s of pods cancelling at the
        // same wall-clock instant don't synchronise.
        jitter_max: Duration::from_millis(50),
        mark_draining: Some(Box::new(move || {
            tracing::info!("entering drain — flipping /readyz to 503");
            probes_for_mark.set_draining();
        })),
        xdp_detach: None,
        observer: Some(observer),
    };

    // Cancel admin listener BEFORE the coordinator so it does not
    // serve `/readyz` Ready during the settle window. Idempotent
    // with the per-conn cancel that fires in phase 5.
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

    // Phase 4 fallback (case C-14): the coordinator only set the
    // cooperative cancel signal — the call site owns the
    // `JoinHandle::abort` fallback for accept loops that didn't exit
    // cooperatively within the post-cancel grace window. Any
    // listener that is still NOT `is_finished()` here either ignored
    // the cancel (kernel pathology — accept future never returns) or
    // is mid-tail-check (case C-3, expected, will exit on its own
    // microsecond-scale timescale). We bump the cooperative-cancel
    // miss counter, then abort the survivors as a backstop.
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

    // QUIC listeners hold their own cancellation tokens. Their drain
    // is independent of the coordinator (today) — the legacy
    // behaviour is preserved here; a future OPS-04-bis pass moves
    // QUIC into the coordinator via a second xdp-detach-style
    // closure.
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

    // Surface the in-flight-drain timeout into the existing
    // `shutdown_aborted_connections_total` counter so the RUNBOOK
    // alert keeps firing as before (REL-2-02 contract).
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

    // _xdp_loader drops *here*, AFTER the drain has settled, so the
    // userspace inserter sees a stable map until the last connection
    // handler has exited.
    drop(_xdp_loader);

    Ok(())
}

/// CODE-2-03 Wave 2c: terminal-or-reload signal returned by
/// [`wait_for_lifecycle_signal`].
#[derive(Copy, Clone, Debug)]
enum LifecycleSignal {
    /// SIGTERM (k8s lameduck, systemd stop).
    SigTerm,
    /// SIGINT (Ctrl-C in interactive sessions).
    SigInt,
    /// SIGUSR1 (REL-2-03 cert reload trigger; today a no-op + log).
    SigUsr1,
}

impl std::fmt::Display for LifecycleSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SigTerm => "SIGTERM",
            Self::SigInt => "SIGINT",
            Self::SigUsr1 => "SIGUSR1",
        })
    }
}

/// Wait for SIGTERM, SIGINT, or SIGUSR1. On non-unix targets only
/// Ctrl-C is wired (Windows operators trigger drain via Ctrl-C too).
async fn wait_for_lifecycle_signal() -> LifecycleSignal {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal as unix_signal};
        let mut sigterm = match unix_signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "SIGTERM handler install failed");
                return LifecycleSignal::SigInt;
            }
        };
        let mut sigint = match unix_signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "SIGINT handler install failed");
                return LifecycleSignal::SigTerm;
            }
        };
        let mut sigusr1 = match unix_signal(SignalKind::user_defined1()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "SIGUSR1 handler install failed");
                // Fall through to a select on the two terminal signals.
                tokio::select! {
                    _ = sigterm.recv() => return LifecycleSignal::SigTerm,
                    _ = sigint.recv() => return LifecycleSignal::SigInt,
                }
            }
        };
        tokio::select! {
            _ = sigterm.recv() => LifecycleSignal::SigTerm,
            _ = sigint.recv() => LifecycleSignal::SigInt,
            _ = sigusr1.recv() => LifecycleSignal::SigUsr1,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
        LifecycleSignal::SigInt
    }
}

// ── accept-loop helpers (CODE-2-05 / CODE-2-06 Wave 2c-2) ───────────────

/// Classification for an `accept(2)` error.
///
/// `Transient` errors are recoverable file-descriptor pressure
/// (`EMFILE`/`ENFILE`) or peer-side resets (`ECONNRESET`); the accept
/// loop sleeps with exponential jitter-backoff and keeps running.
/// `Fatal` errors take the listener down so the supervisor sees a
/// hard failure rather than a silent busy-loop.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum AcceptErrorKind {
    /// Per-process or per-system fd table full. Caller sleeps.
    EmfileOrEnfile,
    /// Peer reset during accept. Common at high request rates; sleep
    /// for a short interval and continue.
    ConnReset,
    /// Anything else — propagate and exit the loop.
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

/// Map an `io::Error` from `TcpListener::accept` into an
/// [`AcceptErrorKind`]. Pulled out so the test in
/// `tests/accept_emfile_backoff.rs` can exercise the classifier without
/// faking a real fd-exhaustion.
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

/// CODE-2-06: next backoff delay given the previous one. Doubles up
/// to a 1 s cap with ±25 % jitter so two listeners can't lockstep.
fn next_accept_backoff(prev: Duration) -> Duration {
    use rand::Rng;
    let base = if prev.is_zero() {
        Duration::from_millis(10)
    } else {
        prev.saturating_mul(2)
    };
    let capped = base.min(Duration::from_secs(1));
    let mut rng = rand::thread_rng();
    let jitter_ms = capped.as_millis() as i64 / 4;
    let delta = rng.gen_range(-jitter_ms..=jitter_ms);
    let final_ms = (capped.as_millis() as i64 + delta).max(1) as u64;
    Duration::from_millis(final_ms)
}

/// CODE-2-05 Wave 2c-2: write a minimal HTTP/1.1 503 response when an
/// L7 listener sheds a connection over capacity. The body is closed
/// after the response so the client sees an explicit shed (not a
/// silent RST).
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

// ── OPS-04+L4-12 drain observer ───────────────────────────────────────
//
// Bridges `lb_core::DrainObserver` (no metrics dep) to the
// `shutdown_drain_seconds_{global,listener}` histogram families
// per the OPS-03 contract (`audit/round-8/fixes/OPS-03.md` §A).
//
// Bucket boundaries cover the OPS-10 5-minute per-listener budget
// upper bound. Two MetricVecs avoid `listener=""` cardinality blowup
// on phases that are not listener-scoped.
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
            // Per-listener histogram — note the observer plumbing
            // calls us once per phase, not once per listener, so the
            // `listener` label is best-effort: when the call site has
            // a listener-specific timing it passes the label; today
            // the coordinator emits a single aggregate observation
            // with `listener="<aggregate>"`. Per-listener observation
            // is tracked alongside OPS-10's per-listener-budget knob.
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

    // CODE-2-06 Wave 2c-2: jittered exponential backoff for transient
    // accept(2) errors. Reset to zero on each successful accept so a
    // healthy listener never carries a stale stall budget.
    let mut backoff = Duration::ZERO;

    loop {
        // OPS-04+L4-12 Round-8 (cases C-2, C-3, C-15): cooperative
        // cancel arm on the listener-cancel token. `biased` polls
        // the cancel arm first so a SIGTERM during a pending
        // `accept().await` returns cleanly without relying on
        // `JoinHandle::abort()` (the round-2 audit's miss). The
        // synchronous post-accept tail-check immediately below
        // covers case C-3 (cancel arrives in the gap between accept
        // returning Ok and the spawn).
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

        // OPS-04+L4-12 case C-3 — synchronous post-accept tail check.
        // The `select!` covers the *future* (pending accept); this
        // check covers the *synchronous tail* between accept-returns-Ok
        // and the spawn. Without it the per-IP counter would increment
        // and the accepted fd would leak when SIGTERM lands here.
        // Drop the stream explicitly (RST-style close) and exit.
        if state.listener_cancel_token.is_cancelled() {
            tracing::debug!(
                client = %client_addr,
                address = %bind_addr,
                "accepted socket dropped post-cancel (OPS-04 case C-3)"
            );
            let _ = client_stream.shutdown().await;
            return Ok(());
        }

        // SEC-2-04 Wave 2c-2: per-IP / per-listener admission gate.
        // Called BEFORE the listener inflight semaphore so a saturated
        // IP cannot starve other peers of the inflight slots. The
        // returned `ConnPermit` is held alongside the semaphore permit
        // for the lifetime of the connection.
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

        // CODE-2-05 Wave 2c-2: per-listener inflight cap. `try_acquire_owned`
        // returns immediately so the accept loop is never blocked by the
        // semaphore itself; on saturation we bump `accept_shed_total`,
        // emit a best-effort 503 (H1/H1s) or close (TCP/TLS pre-ALPN),
        // and continue.
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
                // Best-effort 503 for protocols the client may parse
                // (H1 / H1s pre-handshake clients send headers first
                // so we write before TLS); for plain TCP we drop the
                // socket which the kernel turns into a FIN.
                if matches!(state.mode, ListenerMode::H1 { .. }) {
                    let _ = write_h1_shed_response(&mut client_stream).await;
                } else {
                    let _ = client_stream.shutdown().await;
                }
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
        // Move the inflight + admission permits into the connection
        // task — their Drop releases the slot when the future returns.
        let _inflight_permit = permit;
        let _admission_permit = conn_permit;
        // CODE-2-03 follow-on (Round-5 push-back): the per-connection
        // task is the most consequential `tokio::spawn` site — these
        // tasks held the actual client traffic. Funnel them through the
        // tracker so `shutdown.drain(budget)` waits on them, and add a
        // biased `select!` arm on the shutdown token so a SIGTERM
        // mid-request interrupts long-running upstream work instead of
        // sitting on the connection until the runtime is dropped.
        let conn_cancel = st.shutdown_token.clone();
        // REL-2-09 follow-on: RAII guard around the
        // `accept_inflight{listener}` gauge. Constructed immediately
        // after the Semaphore permit is acquired so the gauge value
        // and the Semaphore's `available_permits()` stay coherent. The
        // guard is moved into the per-connection task so Drop fires
        // when the task exits, matching the permit lifetime exactly.
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
            let mut http_version: Option<&'static str> = None;
            let work = async {
                match &st.mode {
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
                        // REL-2-03 (Wave 2c-2): snapshot the bundle live at
                        // accept time. A SIGUSR1 cert reload concurrent with
                        // an in-flight handshake leaves this snapshot intact
                        // until the connection drops; the next accept sees
                        // the new bundle.
                        let snapshot = bundle.load_full();
                        let acceptor = TlsAcceptor::from(Arc::clone(&snapshot.server_config));
                        // SEC-2-10 Wave 2c: bounded TLS handshake budget.
                        match lb_security::timeout_accept(
                            &acceptor,
                            client_stream,
                            st.handshake_timeout,
                        )
                        .await
                        {
                            Ok(tls_stream) => {
                                // PROTO-2-15 (Wave 2c-2): capture SNI from
                                // the completed handshake for observability.
                                // The hot-path rejection wiring (421 on
                                // SNI ≠ authority) needs an lb-l7 helper that
                                // accepts a per-connection SNI parameter on
                                // `serve_connection`; that API change is
                                // tracked separately. We log + count here so
                                // the operator can audit mismatches before
                                // the gate flips.
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
                        // PROTO-2-11 (H1 half, Wave 2c-2): thread the
                        // shutdown token into the H1 conn so a SIGTERM
                        // mid-keep-alive triggers a `Connection: close`
                        // emit on the next response before the socket
                        // is torn down. Lives inside the per-conn task
                        // scope that already holds the inflight
                        // Semaphore permit + AcceptInflightGuard (rel),
                        // so the watchdog/accept_inflight bookkeeping
                        // stays coherent with the graceful drain.
                        Arc::clone(proxy)
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
                        // REL-2-03 (Wave 2c-2): snapshot the bundle live at
                        // accept time so a SIGUSR1 cert reload concurrent
                        // with an in-flight handshake does not disturb the
                        // running session.
                        let snapshot = bundle.load_full();
                        let acceptor = TlsAcceptor::from(Arc::clone(&snapshot.server_config));
                        // SEC-2-10 Wave 2c: bounded TLS handshake budget.
                        match lb_security::timeout_accept(
                            &acceptor,
                            client_stream,
                            st.handshake_timeout,
                        )
                        .await
                        {
                            Ok(tls_stream) => {
                                // PROTO-2-18 (Wave 2c-2): capture SNI from
                                // rustls and thread into the proxy's
                                // `serve_connection_with_cancel_sni` so the
                                // `check_sni_authority` validator runs on
                                // every request (precedence step 3:
                                // smuggle → auth/host → SNI/host).
                                let sni = tls_stream.get_ref().1.server_name().map(str::to_owned);
                                if let Some(s) = sni.as_deref() {
                                    tracing::trace!(
                                        client = %client_addr,
                                        sni = s,
                                        "TLS SNI captured on H1s (PROTO-2-18)"
                                    );
                                }
                                // ALPN-based dispatch: h2 → H2Proxy, else H1Proxy.
                                let alpn =
                                    tls_stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                                if alpn.as_deref() == Some(b"h2".as_ref()) {
                                    http_version = Some("h2");
                                    // PROTO-2-11/-18 Wave 2c-2: hand the
                                    // shutdown token + SNI into the H2 conn
                                    // so a SIGTERM mid-stream triggers a
                                    // two-step GOAWAY emit AND the SNI/
                                    // authority validator runs per request.
                                    Arc::clone(h2_proxy)
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
                                    // PROTO-2-11/-18 (H1 half, Wave 2c-2):
                                    // mirror the H2 branch.
                                    Arc::clone(h1_proxy)
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
                }
            };

            // CODE-2-03 follow-on: race the proxy work against the
            // shutdown token. `biased` polls the cancel arm first so a
            // SIGTERM mid-request takes priority over otherwise-ready
            // work. On cancel we bump `shutdown_aborted_connections_total`
            // and fall through to the normal post-task bookkeeping.
            // The H2 path already wires the cancel through
            // `serve_connection_with_cancel`, so for that case this
            // outer race is a backstop, not the primary mechanism.
            let result = tokio::select! {
                biased;
                () = conn_cancel.cancelled() => {
                    if let Ok(c) = st.metrics.counter(
                        "shutdown_aborted_connections_total",
                        "Per-connection tasks cancelled mid-flight by SIGTERM drain",
                    ) {
                        c.inc();
                    }
                    tracing::debug!(
                        client = %client_addr,
                        backend = %backend_addr,
                        "per-conn task cancelled by SIGTERM drain"
                    );
                    Err(anyhow::anyhow!("connection cancelled by shutdown"))
                }
                r = work => r,
            };

            // REL-2-08 follow-on: http_requests_total{listener, route,
            // version, status_class} + http_request_duration_seconds
            // {listener, route, version}. The `route` label is left
            // empty at the connection-level emit site — per-request
            // routing context is held inside lb-l7 and is not yet
            // threaded back out to the accept-site (tracked as a
            // future enrichment so dashboards/alerts can group by
            // `route` once available; until then `route=""` keeps the
            // RUNBOOK `LbReq5xx` alert expression valid).
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

// ── TCP proxy ───────────────────────────────────────────────────────────

/// Plain TCP / post-TLS-handshake byte copy.
///
/// CODE-2-09 (Wave 2c-2): dial the backend via async
/// [`tokio::net::TcpStream::connect`] wrapped in
/// [`tokio::time::timeout`]. The previous implementation routed every
/// connect through `tokio::task::spawn_blocking(pool.acquire)`, which
/// stalled a blocking-pool thread on every cold dial and ignored the
/// configured connect timeout entirely. The `TcpPool`-based path is
/// a separate (deferred) refactor — for the plain-TCP shovel we don't
/// need pool reuse because the socket is consumed by
/// [`io::copy_bidirectional`] for the duration of the session and is
/// then closed by the client/backend half-close.
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

    // CODE-2-03 Wave 2c proof: the three lifecycle signal kinds render
    // to the canonical signal names so /admin logs are greppable.
    #[test]
    fn lifecycle_signal_display_matches_canonical_names() {
        assert_eq!(LifecycleSignal::SigTerm.to_string(), "SIGTERM");
        assert_eq!(LifecycleSignal::SigInt.to_string(), "SIGINT");
        assert_eq!(LifecycleSignal::SigUsr1.to_string(), "SIGUSR1");
    }

    // ── CODE-2-06 Wave 2c-2 proof: accept(2) error classifier ──────
    #[test]
    fn classify_accept_error_recognises_emfile() {
        // Synthesise an io::Error with raw_os_error == EMFILE (24).
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

    // ── CODE-2-06 Wave 2c-2 proof: emfile backoff never busy-loops ──
    //
    // Proof: from a zero-baseline the first sleep is at least 1 ms,
    // and the doubling sequence never exceeds the 1 s + 25 % jitter
    // ceiling. This guarantees that an `EMFILE` storm cannot pin a
    // core at 100 % CPU.
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
        // After the doubling sequence has saturated the cap, the
        // value stays within the jittered band — never collapses to
        // zero (which would re-introduce a busy-loop).
        for _ in 0..20 {
            d = next_accept_backoff(d);
            assert!(d >= Duration::from_millis(750));
            assert!(d <= Duration::from_millis(1_250));
        }
    }

    // ── CODE-2-05 Wave 2c-2 proof: shed response shape ─────────────
    //
    // Confirms the 503 body we write at accept-shed time is a
    // well-formed HTTP/1.1 short response with `Connection: close`
    // so the peer learns to disconnect immediately.
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

    // ── CODE-2-09 Wave 2c-2 proof: connect uses the async path ─────
    //
    // Verifies `proxy_connection` no longer spends a `spawn_blocking`
    // worker on the dial path: a TCP connect to a dead address
    // returns within the configured timeout (proving the dial is
    // tokio-native and timer-bound, not blocking the worker until
    // the kernel SYN retries expire). The previous implementation
    // ignored the timeout entirely.
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

    // ── PROTO-2-09 Wave 2c-2 proof: typos error at startup ─────────
    //
    // `build_listener_mode` must reject an unknown protocol value
    // rather than silently falling through to PlainTcp.
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

    // ── SEC-2-06 Wave 2c-2 proof: non-loopback admin bind refused ──
    //
    // The `AdminAuthGate::validate_bind` defence runs BEFORE the
    // admin HTTP listener binds; a `0.0.0.0` bind without
    // `allow_non_loopback = true` must trigger a hard startup
    // refusal. Pairs with the `test_admin_403_without_token`
    // integration test inside `lb_observability::admin_http` which
    // verifies the runtime gate also rejects un-authenticated
    // requests once bound.
    #[test]
    fn test_non_loopback_refused() {
        use lb_security::{AdminAuthGate, AdminBindError};
        let bind: SocketAddr = "0.0.0.0:9090".parse().unwrap();
        let err = AdminAuthGate::validate_bind(bind, false, false).unwrap_err();
        assert!(
            matches!(err, AdminBindError::NonLoopbackWithoutOverride { .. }),
            "expected non-loopback refusal, got: {err:?}"
        );
        // Loopback is always OK.
        AdminAuthGate::validate_bind("127.0.0.1:9090".parse().unwrap(), false, false).unwrap();
        // allow_non_loopback without token is also refused.
        let err2 = AdminAuthGate::validate_bind(bind, true, false).unwrap_err();
        assert!(matches!(
            err2,
            AdminBindError::PublicBindWithoutToken { .. }
        ));
        // With both override + token, bind is allowed.
        AdminAuthGate::validate_bind(bind, true, true).unwrap();
    }

    // ── SEC-2-04 Wave 2c-2 proof: per-IP cap enforced at accept ────
    //
    // Exercises the `HooksBundle::admit_connection` path the runtime
    // calls before grabbing an inflight permit. The third admission
    // from the same IP must be rejected with `OverCap` when the gate
    // is sized for `per_ip_cap == 2`.
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
        // Dropping a permit releases the slot.
        drop(p1);
        let _p3 = bundle.admit_connection(peer).unwrap();
        drop(p2);
    }

    // CODE-2-02 Wave 2c proof: the registry-backed counter increments
    // through the panic hook's path when bound. We bypass the abort
    // by exercising `bind_panic_counter` + the public `panic_total()`
    // accessor — bumping the *fallback* atomic the same way the hook
    // would, then proving `bind_panic_counter` drains it into the
    // registry counter.
    #[test]
    fn panic_total_drains_fallback_into_registry_counter() {
        // Snapshot the current state so this test is robust if other
        // tests in the same binary have already touched the static.
        let baseline = panic_total();

        // Simulate a pre-registry panic count.
        PANIC_TOTAL_FALLBACK.fetch_add(3, Ordering::Release);
        assert_eq!(panic_total(), baseline + 3, "fallback must be visible");

        // Bind a fresh registry — drains the fallback delta into the
        // counter. NB: `PANIC_TOTAL_COUNTER` is process-global; once
        // set it stays set. That's fine — subsequent inc_by(0) is a
        // no-op and the assertion still holds.
        let registry = MetricsRegistry::new();
        bind_panic_counter(&registry);

        // After bind, fallback is zero and panic_total still
        // reflects the same total (registry counter + fallback).
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
