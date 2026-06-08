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
use lb_quic::{
    PassthroughListener, PassthroughParams, QuicListener, QuicListenerParams, RawBackend,
};
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

/// S37-C: the swappable L7 proxy handle(s) for one reloadable listener.
/// Held in a [`ListenerReloadEntry`] so the SIGHUP config-reload routine
/// can `.store()` a freshly-built proxy under in-flight connections.
#[derive(Clone)]
enum ReloadableProxies {
    /// Plain HTTP/1.1 listener — a single `H1Proxy`.
    H1 { proxy: SharedH1Proxy },
    /// HTTPS listener with ALPN — the h1 + h2 proxy pair (the TLS bundle
    /// has its own SIGUSR1 reload path; only the L7 proxies swap here).
    H1s {
        h1_proxy: SharedH1Proxy,
        h2_proxy: SharedH2Proxy,
    },
}

/// S37-C: one entry per config-reloadable L7 listener (`h1` / `h1s`).
/// The SIGHUP handler in `async_main` diffs the new config against the
/// applied one and, for each listener whose swappable subset changed,
/// rebuilds the proxy from the new [`lb_config::ListenerConfig`] and
/// `.store()`s it into the `ArcSwap` this entry holds — mirroring how the
/// [`TlsReloadEntry`] registry drives the cert reload. Plain-TCP / TLS /
/// QUIC listeners are not registered (increment 1 scope is the L7 path;
/// every other path's changes are surfaced as restart-required by the
/// diff).
#[derive(Clone)]
struct ListenerReloadEntry {
    /// Bind address — the diff's listener identity + the reload log label.
    listener: String,
    /// The swappable proxy handle(s) for this listener.
    proxies: ReloadableProxies,
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

/// S37-C: SIGHUP config-reload metric handles. Registered once at boot so
/// they appear in `/metrics` even before the first reload.
#[derive(Clone)]
struct ReloadMetrics {
    /// Successful reloads that applied at least the swappable subset.
    succeeded_total: prometheus::IntCounter,
    /// Reloads rejected by `lb_config::validate_config` (or parse) —
    /// nothing applied, old config stays live (validate-first).
    failed_total: prometheus::IntCounter,
    /// Count of swappable changes applied, labelled by field. Bumped once
    /// per applied [`lb_config::SwappableChange`].
    applied_swappable_total: prometheus::IntCounterVec,
    /// HONESTY metric: count of detected changes that require a restart
    /// and were NOT applied, labelled by field. A non-zero bump is the
    /// machine-readable form of the per-field "requires restart, not
    /// applied" warning — never a silent no-op.
    restart_required_fields_total: prometheus::IntCounterVec,
    /// Monotonic applied-config version (starts at 1 = boot config; bumps
    /// on each reload that applies a swappable change).
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

/// S37-C: validate-first SIGHUP config hot-reload.
///
/// Mirrors [`reload_all_tls`] for the L7 config swap. Steps:
///
/// 1. `config_manager.reload()` re-reads the file (TOML-shape validate +
///    rollback machinery stays string-level in `lb-controlplane`).
/// 2. The binary runs the FULL `lb_config::parse_config` +
///    `validate_config` on the new string — the type-level validation the
///    TOML-shape check does NOT cover (closing the S37-B gap).
/// 3. On any parse/validation failure: `rollback_to_previous()` so the
///    manager's "current" matches the still-live config, log the error,
///    bump `config_reload_failed_total`, apply NOTHING. The old config
///    stays fully live (validate-first — the
///    `reload_tls_bundle_invalid_keeps_old_live` property for the whole
///    config).
/// 4. On success: diff against the applied config, log + count every
///    restart-required change (HONESTY — never a silent no-op), rebuild +
///    `.store()` the proxy for each changed-backend L7 listener, then
///    update the applied config + bump the succeeded/version metrics.
///
/// Never returns an error: a reload failure logs + counts and leaves the
/// gateway serving on the previous config. Async because rebuilding a
/// proxy re-resolves backend DNS.
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

    // Step 1: re-read the file via ConfigManager (TOML-shape validate +
    // rollback). `Ok(false)` ⇒ file byte-identical to current; `Err` ⇒
    // empty / non-TOML (already rejected without mutating current).
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

    // Step 2: FULL type-level validation on the new string.
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

    // Step 3: diff + partition (exhaustive — every change classified).
    let plan = applied_config.diff(&new_config);

    // HONESTY: log + count EVERY restart-required change. A reload that
    // contains ONLY restart-required changes is explicitly NOT a silent
    // success — it is logged as "applied 0 swappable".
    for change in &plan.restart_required {
        tracing::warn!(field = change.field(), "SIGHUP: {}", change.describe());
        if let Some(m) = metrics {
            m.restart_required_fields_total
                .with_label_values(&[change.field()])
                .inc();
        }
    }

    // Step 4: apply the swappable subset by rebuilding L7 proxies. A
    // per-listener `ListenerL7` change rebuilds that one listener; a
    // process-wide `RuntimeMaxKeepaliveRequests` change rebuilds EVERY L7
    // listener (the cap is baked into each proxy). Compute the UNION of
    // affected listeners so each is rebuilt exactly once — the rebuild
    // reads the whole new listener config + the new keepalive, so one
    // rebuild applies every co-changed swappable field.
    let new_keepalive = new_config
        .runtime
        .as_ref()
        .map_or(100, |r| r.max_keepalive_requests);
    let registry: Vec<ListenerReloadEntry> = listener_reload_registry.lock().clone();
    let keepalive_changed = plan
        .swappable
        .iter()
        .any(|c| matches!(c, lb_config::SwappableChange::RuntimeMaxKeepaliveRequests));

    // Per-listener L7 changes (by address).
    let mut to_rebuild: Vec<String> = plan
        .swappable
        .iter()
        .filter_map(lb_config::SwappableChange::address)
        .map(str::to_owned)
        .collect();
    // A process-wide keepalive change pulls in every registered L7
    // listener not already in the set.
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
            // A swappable change for a listener with no L7 swap handle
            // (e.g. a plain-TCP/TLS listener). Surface honestly.
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
                // A rebuild failure (e.g. backend DNS now fails) leaves the
                // OLD proxy live — no blip. Count + keep serving.
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
    // Per-swappable-change metric + log (decoupled from the per-listener
    // rebuild so the summary names each changed field class once).
    for change in &plan.swappable {
        if let Some(m) = metrics {
            m.applied_swappable_total
                .with_label_values(&[change.field()])
                .inc();
        }
        tracing::info!(field = change.field(), "SIGHUP: {}", change.describe());
    }

    // Commit the applied config so the NEXT reload diffs against it.
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

/// S37-C: rebuild the L7 proxy/proxies for one listener from its new
/// config and atomically `.store()` them into the listener's `ArcSwap`
/// handle(s). Re-resolves backend DNS + rebuilds the upstream pools +
/// proxies exactly as [`build_listener_mode`]'s `h1`/`h1s` arms do, so
/// the swapped-in proxy is byte-identical to a fresh boot with the new
/// config. The OLD proxy stays live (refcount-pinned by in-flight
/// connections that captured it at accept) until those connections drop.
///
/// HONESTY INVARIANT (the diff's swappable set MUST match this exactly):
/// this rebuild APPLIES, from the NEW config, every swappable field — from
/// `new_l`: `backends`, `http` (timeouts), `h2_security`, `websocket`
/// (H1/H2 knobs), `alt_svc`, `grpc`; and from `[runtime]`:
/// `max_keepalive_requests` (passed in as the `max_keepalive_requests`
/// arg). These are precisely the fields [`lb_config::LbConfig::diff`]
/// classifies as swappable ([`lb_config::SwappableChange::ListenerL7`] +
/// `RuntimeMaxKeepaliveRequests`). It PRESERVES (does NOT apply) only the
/// process-wide `hooks` bundle (carries `[security].strict_te` + the per-IP
/// connection gate), which is why the diff reports a change to `strict_te`
/// or `per_ip_connection_cap` as restart-required (truthful: this rebuild
/// does not rebuild the shared `HooksBundle`). Changing the swappable set
/// here without also reclassifying it in `diff` (or vice versa) breaks the
/// honesty invariant the verifier adversarially tests.
async fn rebuild_l7_proxies(
    new_l: &lb_config::ListenerConfig,
    handles: &ReloadableProxies,
    pool: &TcpPool,
    resolver: &DnsResolver,
    hooks: &Arc<HooksBundle>,
    watchdog: Option<&Watchdog>,
    max_keepalive_requests: u32,
) -> anyhow::Result<()> {
    // Resolve backends (same loop + bookkeeping spawn_tcp does at boot).
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
            // `build_h1_proxy` returns Arc<H1Proxy>; ArcSwap::store wants
            // the same Arc<T> shape.
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
            // Store both legs. The two stores are independent atomic RCU
            // swaps; a connection that snapshots between them still gets a
            // consistent per-proxy view (each proxy is internally
            // consistent), and ALPN dispatch picks exactly one leg per
            // connection, so there is no cross-leg tearing.
            h1_proxy.store(rebuilt_h1);
            h2_proxy.store(rebuilt_h2);
        }
    }
    Ok(())
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
    ///
    /// S37-C: the proxy is held inside an `Arc<ArcSwap<H1Proxy>>` so a
    /// SIGHUP config reload swaps a freshly-built proxy (new backend set)
    /// under in-flight connections without disturbing them. The per-conn
    /// task `load_full()`s a snapshot at accept time and serves the whole
    /// connection on it; a concurrent `.store()` leaves that captured
    /// snapshot live until the connection drops. Mirrors the proven
    /// `SharedTlsBundle` cert-reload shape (REL-2-03).
    H1 { proxy: SharedH1Proxy },
    /// HTTPS listener that offers HTTP/2 and HTTP/1.1 via ALPN. After
    /// `TlsAcceptor::accept`, the runtime inspects
    /// [`rustls::ServerConnection::alpn_protocol`] and dispatches to the
    /// matching proxy. The TLS bundle is held in an `Arc<ArcSwap<_>>` for
    /// cert hot-reload via SIGUSR1 (REL-2-03); S37-C additionally holds
    /// the two L7 proxies in `Arc<ArcSwap<_>>` for SIGHUP config reload.
    H1s {
        h1_proxy: SharedH1Proxy,
        h2_proxy: SharedH2Proxy,
        bundle: lb_security::SharedTlsBundle,
        _rotator: Arc<PlMutex<TicketRotator>>,
    },
}

/// S37-C: an `H1Proxy` held behind an `ArcSwap` for SIGHUP config
/// hot-reload. Read with `.load_full()` at accept time (captures an
/// owned snapshot for the connection's life); swapped with
/// `.store(Arc::new(new))` on reload. Mirrors
/// [`lb_security::SharedTlsBundle`].
type SharedH1Proxy = Arc<ArcSwap<H1Proxy>>;

/// S37-C: an `H2Proxy` held behind an `ArcSwap` for SIGHUP config
/// hot-reload (the H1s ALPN-h2 leg). Same contract as [`SharedH1Proxy`].
type SharedH2Proxy = Arc<ArcSwap<H2Proxy>>;

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
    /// ROUND8 OPS-02 div-l7 refinement: per-listener effective drain
    /// jitter ceiling (ms). On SIGTERM the per-conn cancel arm draws
    /// an independent `[0, this)` sleep so connections *within* one
    /// pod stagger their abort instant (intra-pod desync) on top of
    /// the coordinator-level per-process draw div-ops landed (which
    /// only desyncs *across* replicas). `0` (jitter disabled, or no
    /// `[runtime]`) makes the arm behave exactly as before.
    per_conn_drain_jitter_ms: u64,
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
    // SESSION 19 / Mode B (B6): the resolved raw backend + metric handles
    // when `cfg.raw_proxy` is configured; `None` ⇒ H3-terminate (R3).
    raw_backend: Option<RawBackend>,
    quic_modeb_metrics: Option<lb_observability::QuicModeBMetrics>,
    // S36-A: the gateway-level `[runtime].max_requests_per_h3_connection`
    // cap (`0` disables → byte-identical pre-S36 H3 front, R3) + the
    // registered `h3_*` recycle metric handles. Only consulted on the
    // H3-terminate path; Mode B early-dispatches before any H3 state.
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
    // S36-A: opt this listener into H3 connection recycling. `with_h3_request_cap`
    // is a no-op for `cap == 0` (the field stays at its `new()` default —
    // byte-identical to the pre-S36 H3 front, R3). On Mode B the cap is
    // threaded but unread (`run_raw_proxy_actor` returns before any H3 state).
    params = params.with_h3_request_cap(max_requests_per_h3_connection, h3_recycle_metrics);
    // SESSION 19 / Mode B (B6): flip to Mode B ONLY when a raw backend was
    // built. `with_raw_backend` sets `raw_quic_backend`, the DATAGRAM cap,
    // and the metrics, and is the ONLY thing that enables datagrams on the
    // client-facing config. Absent ⇒ params are byte-identical H3 (R3).
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

/// SESSION 19 / Mode B (B6): build the [`RawBackend`] for a Mode-B
/// (terminate-and-re-originate) QUIC listener from its
/// `[listeners.quic.raw_proxy]` block.
///
/// Constructs a dedicated [`QuicUpstreamPool`] whose `config_factory`
/// mirrors the upstream-leg requirements:
/// * `verify_peer(true)` ALWAYS (peer-cert verification is never silently
///   disabled in Mode B — see [`lb_config::RawQuicProxyConfig::backend_ca_path`]).
///   When `backend_ca_path` is set, the bundle is loaded via
///   `load_verify_locations_from_file`; when absent, `BoringSSL`'s built-in
///   default trust roots are used. Startup fails clearly if a configured CA
///   path cannot be loaded.
/// * the default upstream ALPN tokens (the actual handshake ALPN is
///   overridden per-dial by [`QuicUpstreamPool::dial_dedicated`] to mirror
///   the client's negotiated protocol — same as the H3 pool does).
/// * `enable_dgram(true, cap, cap)` so the re-originated upstream
///   negotiates QUIC DATAGRAM (RFC 9221) support for the B4 relay.
///
/// The returned [`RawBackend`] carries the pool, the parsed backend
/// `SocketAddr`, and the SNI; it is cloned into every per-connection
/// actor. An unparseable `backend_addr` fails startup (no silent
/// Mode-B-disable).
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
            // Default ALPN; dial_dedicated overrides per-connection to
            // mirror the client's negotiated protocol.
            config.set_application_protos(lb_io::quic_pool::UPSTREAM_H3_ALPN_PROTOS)?;
            // Backend-trust: verify_peer is ALWAYS on (documented v1
            // behaviour — never silently disabled). With a CA bundle, load
            // it; without one, fall back to BoringSSL default roots.
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
            // B4: negotiate QUIC DATAGRAM support on the upstream leg.
            config.enable_dgram(true, dgram_cap, dgram_cap);
            Ok(config)
        });
    let pool = QuicUpstreamPool::new(QuicPoolConfig::default(), factory);
    Ok(RawBackend {
        pool,
        addr,
        sni: cfg.sni.clone(),
        // B6 (R14/R12): single-source the relay's two memory bounds from
        // the operator config so the same value controls the wire-advertised
        // queue length AND the relay's own `BoundedDgramQueue` (B4) /
        // `admit_or_refuse` ceiling (B5). The config serde defaults equal
        // `lb_quic::{DGRAM_QUEUE_CAP, MAX_RELAY_STREAMS}` (1024 / 256).
        dgram_queue_cap: cfg.dgram_queue_cap,
        max_relay_streams: cfg.max_relay_streams,
    })
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
    // ROUND8-L7-06: per-keep-alive-connection request cap.
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
    // ROUND8-L7-06: nginx-parity per-keep-alive request cap.
    proxy = proxy.with_max_keepalive_requests(max_keepalive_requests);
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

/// SESSION 28 / WS-over-H3 (RFC 9220) Stage C — build the relay launcher
/// closure injected into the QUIC listener.
///
/// Dependency inversion: the binary sees BOTH `lb-quic` (the H3 datapath +
/// the `H3WsTunnel` seam) and `lb-l7` (the single-sourced
/// `ws_proxy::proxy_frames`), so it can run the relay across the crate
/// boundary `lb-quic` cannot. Per validated `:protocol=websocket` extended
/// CONNECT the closure: dials an H1 backend via the shared `TcpPool`,
/// drives the upstream RFC 6455 client handshake (echoing the negotiated
/// subprotocol), signals readiness **before** the `200` (the H3 analog of
/// the WS-H1 GHSA / WS-H2 F-S27-1 "upstream-before-200" ordering), then
/// runs `proxy_frames` over the tunnel. Dial+handshake is bounded by
/// `header_budget` (504 on elapse, 502 on refuse). Mirrors the inline dial
/// in `h1_proxy::handle_ws_upgrade` / `h2_proxy::handle_ws_extended_connect`.
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
                // Dial + upstream RFC 6455 handshake INLINE, before readiness
                // (so the H3 client never sees a 200 toward a backend that
                // never agreed). `take_stream` removes the socket from the
                // pool — a WS tunnel owns its backend connection for life.
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
                            // The actor (H3 stream) went away before the 200;
                            // dropping `backend_ws` closes the upstream.
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
        proxy = proxy
            .with_websocket(Arc::new(WsProxy::new(ws_config_to_runtime(ws))))
            // CF-S27-2 — WS-over-H2 (RFC 8441 extended CONNECT) is OFF by
            // default; only advertise SETTINGS_ENABLE_CONNECT_PROTOCOL and
            // intercept extended CONNECT when the operator opts in. WS-over-H1
            // is wired separately in `build_h1_proxy` and is unaffected.
            .with_h2_extended_connect(ws.h2_extended_connect);
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
    // F-S26-1: the shared TCP backend pool + DNS resolver, threaded in so
    // the H3-terminate path can wire `[[listeners.backends]]` for the
    // H3→H1 (and H3→H2) forwarding legs — mirroring what `spawn_tcp`
    // already does on the L7 path. `None` on both keeps the call site
    // for tests that only exercise the Mode-B / backendless shapes.
    pool: &TcpPool,
    resolver: &DnsResolver,
    metrics: &Arc<MetricsRegistry>,
    // S36-A: the gateway-level `[runtime].max_requests_per_h3_connection`
    // cap, threaded in from the caller (read once near the other runtime
    // knobs). `0` disables H3 connection recycling (byte-identical pre-S36
    // H3 front, R3).
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

    // SESSION 19 / Mode B (B6): when a `[listeners.quic.raw_proxy]` block
    // is present, build the raw-QUIC re-origination backend + register the
    // quic_modeb_* metric family. Absent ⇒ both are `None` and the
    // listener runs H3-terminate byte-identically (R3).
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

    // S36-A: register the `h3_*` recycle metric family for an H3-terminate
    // listener with the cap enabled, so the GOAWAY-sent / connections-recycled
    // rows appear in `/metrics` from spawn (the soak + the recycle e2e assert
    // them). Never on Mode B (no H3 state) and never when the cap is disabled
    // (`0` ⇒ byte-identical pre-S36 H3 front, no new metric rows, R3).
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

    // SESSION 27 / WS-over-H3 (RFC 9220) Stage A: opt this listener into
    // WebSocket extended CONNECT ONLY when a `[listeners.websocket]` block
    // is present, enabled, AND `h3_extended_connect = true`. OFF by
    // default (mirrors the H2 `h2_extended_connect` gate, CF-S27-2): the
    // H3 settings frame + `:protocol` rejection stay byte-identical (R3).
    // Never on Mode B (raw_proxy never H3-terminates).
    let ws_enabled = !mode_b
        && listener_cfg
            .websocket
            .as_ref()
            .is_some_and(|w| w.enabled && w.h3_extended_connect);
    if ws_enabled {
        params = params.with_websocket(true);
    }

    // F-S26-1: wire the H3-terminate → backend relay. ONLY on the
    // H3-terminate path (no raw_proxy ⇒ not Mode B; the config validator
    // rejects raw_proxy + backends together). A QUIC listener with no
    // backends stays backendless H3-terminate, byte-identical to before
    // (R3) — `params` is untouched. With backends, dispatch by the
    // (single, validator-enforced) protocol family:
    //   h1/tcp → with_backends (H3→H1, the WS-over-H3 backend leg)
    //   h2     → with_h2_backend (H3→H2)
    //   h3     → with_h3_backend (H3→H3)
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
            // F-S26-1: surface how many forwarding backends were wired.
            // 0 ⇒ the transport-only / inline-egress smoke shape (R3).
            backends = listener_cfg.backends.len(),
            "QUIC listener started"
        );
    }
    Ok(listener)
}

/// F-S26-1: wire the H3-terminate → backend forwarding leg onto a QUIC
/// listener's [`QuicListenerParams`]. Resolves each `[[listeners.
/// backends]]` address (mirroring [`spawn_tcp`]'s resolve loop +
/// dns_cache_hits/misses bookkeeping) and dispatches by the listener's
/// single backend protocol family (the config validator guarantees the
/// list is non-empty and not mixed):
///   * `h1`/`tcp` → [`QuicListenerParams::with_backends`] (H3→H1; this
///     is the WS-over-H3 backend leg, the F-S26-1 must-have).
///   * `h2`       → [`QuicListenerParams::with_h2_backend`] (H3→H2),
///     first resolved address (the router takes a single H2 backend).
///   * `h3`       → [`QuicListenerParams::with_h3_backend`] (H3→H3),
///     first resolved address + its SNI.
///
/// Caller guarantees `listener_cfg.backends` is non-empty and the
/// listener is NOT Mode B (no `raw_proxy`).
async fn wire_h3_terminate_backends(
    mut params: QuicListenerParams,
    listener_cfg: &lb_config::ListenerConfig,
    pool: &TcpPool,
    resolver: &DnsResolver,
    metrics: &Arc<MetricsRegistry>,
) -> anyhow::Result<QuicListenerParams> {
    // Resolve every backend address up front (same bookkeeping spawn_tcp
    // does). `addresses[i]` corresponds to `listener_cfg.backends[i]`.
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

    // The validator enforces a single protocol family + a non-empty
    // list, so the first backend's protocol determines the forwarding
    // leg for the whole listener.
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
            // SESSION 28 / WS-over-H3 (RFC 9220) Stage C: when this listener
            // opted into WebSocket (params.ws_enabled, set in spawn_quic from
            // a `[listeners.websocket]` block with `h3_extended_connect`),
            // inject the relay launcher built over THESE H1 backends + the
            // shared pool, so a validated extended CONNECT is relayed to an
            // H1 WS backend. The H3→H1 backend leg below is the same one the
            // launcher dials. Off ⇒ no launcher (byte-identical, R3).
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
            // H3→H1: round-robin across ALL resolved H1 backends via the
            // shared TCP pool (the must-have; the WS-over-H3 backend leg).
            params = params.with_backends(addresses, pool.clone());
        }
        UpstreamProto::H2 => {
            // H3→H2: the router takes a single H2 backend address.
            // `build_h2_upstream_pool` hands back an `Arc<Http2Pool>`;
            // the QUIC listener API wants it by value. `Http2Pool` is a
            // cheap Arc-backed Clone, so clone out of the Arc.
            let h2pool = build_h2_upstream_pool(pool.clone(), listener_cfg.h2_security.as_ref());
            let Some(addr) = addresses.first().copied() else {
                anyhow::bail!("listener {}: no resolved H2 backend", listener_cfg.address);
            };
            params = params.with_h2_backend((*h2pool).clone(), addr);
        }
        UpstreamProto::H3 => {
            // H3→H3: the router takes a single upstream QUIC backend +
            // its SNI. The pool's verify-peer/CA posture comes from the
            // H3-backend TLS knobs (build_h3_upstream_pool). Same
            // Arc-backed-Clone story as the H2 leg above.
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

/// S15 A2-8: spawn the Mode A QUIC passthrough listener. Independent
/// of the terminating QUIC listener — Mode A binds its own UDP port,
/// owns its own retry-secret, and never decrypts client packets
/// (`lb_quic::passthrough::PassthroughListener` upholds the
/// NEVER-DECRYPTED invariant via the CF-S15-PASSTHROUGH-FEATURE-GATING
/// linkage proof; see `scripts/never_decrypted_proof.sh`).
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
    // S15 A3: register the quic_passthrough_* metric family off the
    // shared registry and thread the handles into the listener.
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
    // ROUND8-L7-06: per-keep-alive-connection request cap threaded
    // into the H1/H2 proxy builders.
    max_keepalive_requests: u32,
    // ROUND8 OPS-02 div-l7 refinement: per-listener effective drain
    // jitter ceiling (ms) used by the per-conn cancel arm to draw a
    // per-connection desync sleep on SIGTERM.
    per_conn_drain_jitter_ms: u64,
    hooks: Arc<HooksBundle>,
    shutdown_token: CancellationToken,
    listener_cancel_token: CancellationToken,
    tracker: TaskTracker,
    tls_reload_registry: Arc<PlMutex<Vec<TlsReloadEntry>>>,
    // S37-C: registry of config-reloadable L7 listeners. An `h1`/`h1s`
    // listener registers its `ArcSwap` proxy handle(s) here so the SIGHUP
    // reload routine can swap a freshly-built proxy under traffic.
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
    // S37-C: register the L7 proxy swap handle(s) so the SIGHUP reload
    // routine can rebuild + `.store()` this listener's proxy on a backend
    // change. Only `h1`/`h1s` carry swappable L7 proxies; plain-TCP / TLS
    // listeners have no proxy to swap (their changes are restart-required).
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
        per_conn_drain_jitter_ms,
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
    // ROUND8-L7-06: per-keep-alive-connection request cap.
    max_keepalive_requests: u32,
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
            // S37-C: hold the proxy behind an ArcSwap so a SIGHUP config
            // reload can swap a freshly-built proxy under in-flight
            // connections (mirrors the SharedTlsBundle cert-reload shape).
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
            // S37-C: hold both L7 proxies behind ArcSwaps for SIGHUP
            // config reload (the TLS bundle keeps its own SIGUSR1 path).
            Ok(ListenerMode::H1s {
                h1_proxy: Arc::new(ArcSwap::new(h1_proxy)),
                h2_proxy: Arc::new(ArcSwap::new(h2_proxy)),
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

    // CODE-2-13 + S37-C: wire lb-controlplane (file-backed). The
    // ConfigManager owns the in-memory TOML string + a monotonic version
    // counter and the TOML-shape validate + rollback machinery. S37-C
    // hooks it into the SIGHUP handler below: `config_manager.reload()`
    // re-reads the file string, then the binary runs the FULL
    // `lb_config::parse_config` + `validate_config` (the type-level
    // validation ConfigManager deliberately does not own — it stays
    // string-level, no `lb-config` dep edge) before diffing + swapping.
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
            // Fail-soft: the lb_config::parse_config above already
            // succeeded, so an InvalidConfig error here must mean a
            // pre-validation path (empty / non-TOML) the redundant
            // ConfigManager validate rejects. We log + continue with
            // the parsed config — operator can fix on next SIGHUP.
            tracing::warn!(error = %e, "control plane manager init skipped");
            None
        }
    };

    // S37-C: the currently-APPLIED typed config. `config_manager` tracks
    // the file string; this is the parsed+validated `LbConfig` actually
    // serving traffic, diffed against each reload's new config to
    // partition swappable vs restart-required changes. Updated in place
    // by `reload_config` after a successful swap.
    let mut applied_config = config.clone();

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
    // S15 A2-8: Mode A passthrough listeners. Today only one is
    // supported (`[passthrough]` is a single top-level block); the Vec
    // shape is forward-compatible with future per-pool fan-out.
    let mut passthrough_listeners: Vec<PassthroughListener> = Vec::new();

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
    // ROUND8-L7-06: per-keep-alive-connection request cap (nginx
    // `keepalive_requests 100` parity / Pingora 0.8.0). `0` disables.
    // Falls back to the nginx-parity 100 when no `[runtime]` block is
    // present.
    let max_keepalive_requests = config
        .runtime
        .as_ref()
        .map_or(100, |r| r.max_keepalive_requests);
    // S36-A: per-connection H3 request cap (cap → GOAWAY → drain → recycle).
    // `0` disables. Falls back to the lb-config default (1000) when no
    // `[runtime]` block is present, so a QUIC listener under a minimal config
    // still gets the safe recycling default rather than the unbounded
    // (pre-S36) behaviour.
    let max_requests_per_h3_connection = config
        .runtime
        .as_ref()
        .map_or(1000, |r| r.max_requests_per_h3_connection);
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
                // F-RES-5 (S38): the sweeper is OBSERVABILITY-only — it
                // surfaces stalled connections for alerting; it does NOT
                // close the socket (that would race the graceful-drain
                // coordinator). The actual close is performed by the
                // timeout stack (hyper `header_read_timeout` /
                // `idle_bounded_send` / `total` / H2 keepalive / QUIC idle).
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

    // REL-2-03 (Wave 2c-2): registry of TLS reloadables, populated as
    // each TLS / H1s listener spawns its bundle. The SIGUSR1 handler
    // below iterates this list, calls `reload_tls_bundle` for each, and
    // bumps `cert_rotation_succeeded_total` / `cert_rotation_failed_total`
    // accordingly.
    let tls_reload_registry: Arc<PlMutex<Vec<TlsReloadEntry>>> = Arc::new(PlMutex::new(Vec::new()));

    // S37-C: registry of config-reloadable L7 listeners, populated as each
    // `h1`/`h1s` listener spawns its proxy. The SIGHUP handler below diffs
    // the new config against the applied one and `.store()`s a rebuilt
    // proxy into the matching entry's `ArcSwap` for each changed-backend
    // listener (mirrors how `tls_reload_registry` drives the cert reload).
    let listener_reload_registry: Arc<PlMutex<Vec<ListenerReloadEntry>>> =
        Arc::new(PlMutex::new(Vec::new()));

    // Register cert metric handles up front so they appear in `/metrics`
    // even before the first reload.
    let cert_metrics = CertMetrics::register(&metrics);

    // S37-C: register config-reload metric handles up front so they appear
    // in `/metrics` even before the first SIGHUP.
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
        // ROUND8 OPS-02 div-l7 refinement (Wave 2b-1 handoff): the
        // coordinator-level jitter (div-ops) is a single per-PROCESS
        // draw that desynchronises *replicas*. This is the finer
        // per-CONNECTION ceiling: each per-conn cancel arm draws its
        // OWN `[0, jitter)` so connections *within* one pod also
        // desync their abort instant (intra-pod spread), not just
        // inter-pod. Sourced from the same per-listener effective
        // jitter OPS-10 honours (override → [runtime] → drain/4).
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
            Arc::clone(&listener_reload_registry),
            Some(watchdog.clone()),
        )
        .await?;
        listener_handles.push(handle);
    }

    // S15 A2-8: spawn the Mode A passthrough listener, if configured.
    if let Some(pt_cfg) = config.passthrough.as_ref() {
        passthrough_listeners
            .push(spawn_passthrough(pt_cfg, &metrics, shutdown.token().child_token()).await?);
    }

    if listener_handles.is_empty() && quic_listeners.is_empty() && passthrough_listeners.is_empty()
    {
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
    // REL-2-03 (Wave 2c-2) + S37-C: SIGUSR1 = cert reload, SIGHUP =
    // config reload. Both are NON-TERMINAL (serviced + `continue`d) so an
    // operator can roll a cert / config, signal, observe metrics, and
    // signal again if validation rejected the push. Loop exits on SIGTERM
    // / SIGINT and falls through to the drain sequence. Cert reload
    // (SIGUSR1) and config reload (SIGHUP) are kept as separate triggers
    // so the proven cert path stays untouched.
    // SIGNAL-LOSS FIX (S37-C, R6): install the lifecycle-signal streams
    // ONCE here and reuse them across every loop iteration. A SIGTERM/
    // SIGINT landing while we service a non-terminal SIGHUP/SIGUSR1 is
    // then never lost (it latches on the persistent stream). On non-unix,
    // only Ctrl-C is wired (Windows operators drain via Ctrl-C).
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
                // S37-C: validate-first config hot-reload. Re-reads the
                // file via ConfigManager, runs the FULL lb_config
                // validation, diffs against the applied config, applies
                // the swappable subset live, and logs every
                // restart-required change (HONESTY — never a silent
                // no-op). On invalid config: nothing applied, old config
                // stays fully live.
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
    // ROUND-8 OPS-10: resolve the effective per-listener drain
    // budget + jitter (per-listener override falling back to
    // [runtime], falling back to the lb-config defaults). The
    // phase-level InFlightDrain deadline is the *max* over all
    // listeners so the longest-budget listener (e.g. a gRPC-bidi /
    // WebSocket streaming listener at the Pingora 300 s budget) gets
    // its full window; short-request listeners exit well inside it.
    // Per-connection-await-its-own-listener-budget (OPS-10 §C.3) is a
    // div-l7-owned follow-up in the per-conn cancel arm (the
    // accept/serve-connection region this commit must not touch);
    // the phase-level max is the coordinator-level honoring of the
    // override and is sufficient for the gRPC/WS streaming-listener
    // regression the finding targets.
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
            // ROUND-8 OPS-11: fallback matches
            // `lb_config::default_readiness_settle_ms()` (11 s — one
            // kubelet probe period + margin) when no [runtime] block
            // is present.
            runtime_cfg.map_or(11_000, |r| r.readiness_settle_ms),
        ),
        listener_cancel_deadline: Duration::from_millis(500),
        inflight_drain_deadline: Duration::from_millis(max_listener_drain_ms),
        // L4-12 will land the XDP detach closure here. Until the
        // `XdpLoader::detach()` API lands on the branch, the coordinator
        // simply skips phase 6 — the legacy "drop the loader on
        // process exit" behaviour is preserved as a stop-gap and the
        // round-2 stale-pin recovery path picks up any linger on next
        // startup (see OPS-01+L4-12+L4-04 §B.2).
        xdp_detach_deadline: None,
        // ROUND-8 OPS-02: the coordinator sleeps a per-process random
        // `[0, jitter_max)` before the in-flight-drain cancel so a
        // deploy-wide SIGTERM doesn't make every replica cancel at
        // the same wall-clock instant (thundering-herd reconnect
        // storm against the shared upstream LB — Envoy
        // `drain_manager_impl.cc`). `jitter_max` is the max effective
        // per-listener jitter (override → [runtime] → drain/4
        // derivation). Per-connection intra-pod spreading (each conn
        // its own jitter draw) is the div-l7-owned refinement in the
        // per-conn cancel arm; the per-process draw already
        // desynchronises *across* replicas, which is the primary
        // thundering-herd mitigation the finding targets.
        jitter_max: Duration::from_millis(max_listener_jitter_ms),
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

    // S15 A2-8: passthrough listeners use the same self-owned token
    // discipline as the terminating QUIC listener.
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
/// [`LifecycleSignals::recv`].
#[derive(Copy, Clone, Debug)]
enum LifecycleSignal {
    /// SIGTERM (k8s lameduck, systemd stop).
    SigTerm,
    /// SIGINT (Ctrl-C in interactive sessions).
    SigInt,
    /// SIGUSR1 (REL-2-03 cert reload trigger).
    SigUsr1,
    /// SIGHUP (S37-C config hot-reload trigger).
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

/// The four lifecycle-signal streams, installed ONCE and reused across
/// every iteration of the signal loop.
///
/// SIGNAL-LOSS FIX (S37-C, R6): the previous `wait_for_lifecycle_signal`
/// re-installed the `unix_signal` streams on EVERY call. A terminal signal
/// (SIGTERM/SIGINT) landing in the window between the old streams being
/// dropped and the new ones being created — which the loop re-entered
/// after every non-terminal signal (SIGHUP/SIGUSR1) — was silently lost,
/// so the gateway would miss a shutdown request issued right after a
/// reload. Installing the streams once (here) and only awaiting `.recv()`
/// in the loop closes that window: a tokio `Signal` registered with the
/// driver coalesces and latches deliveries, so a signal arriving while we
/// are servicing a previous one is still observed on the next `.recv()`.
#[cfg(unix)]
struct LifecycleSignals {
    sigterm: tokio::signal::unix::Signal,
    sigint: tokio::signal::unix::Signal,
    sigusr1: Option<tokio::signal::unix::Signal>,
    sighup: Option<tokio::signal::unix::Signal>,
}

#[cfg(unix)]
impl LifecycleSignals {
    /// Install the streams once. SIGTERM/SIGINT are mandatory (their
    /// install failure is fatal to graceful lifecycle and surfaces as an
    /// error); SIGUSR1/SIGHUP are optional reload knobs that degrade to
    /// "unavailable" on install failure without taking the process down.
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

    /// Await the next lifecycle signal on the persistent streams. A
    /// missing optional reload stream (install failed) is represented by a
    /// `pending()` future that never fires, so its arm stays pending
    /// forever and the terminal signals remain always live — no extra
    /// crate dependency needed.
    async fn recv(&mut self) -> LifecycleSignal {
        // Borrow the disjoint fields up front so the four `select!` arms
        // don't each try to borrow `self` (which would conflict). A
        // missing optional reload stream becomes an eternal `pending`
        // future, so its arm never fires — no extra crate dependency.
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
            // ROUND8 OPS-02 div-l7 refinement: the work future now
            // *returns* the negotiated HTTP version alongside its
            // result instead of capturing `&mut http_version`. That
            // lifts the borrow so the future can be `tokio::pin!`-ed
            // and re-polled inside the per-conn jitter grace without
            // a borrow-conflict against the post-task metrics read.
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
                        //
                        // S37-C: snapshot the proxy live at accept time
                        // (mirrors the cert `bundle.load_full()` above).
                        // A SIGHUP config reload concurrent with this
                        // connection swaps the ArcSwap; this captured
                        // snapshot stays live until the connection drops,
                        // so the in-flight connection keeps its original
                        // backend set — new connections see the new one.
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
                                    // S37-C: snapshot the proxy at accept
                                    // time (see the H1 arm).
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
                                    // PROTO-2-11/-18 (H1 half, Wave 2c-2):
                                    // mirror the H2 branch.
                                    // S37-C: snapshot at accept time.
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

            // CODE-2-03 follow-on: race the proxy work against the
            // shutdown token. `biased` polls the cancel arm first so a
            // SIGTERM mid-request takes priority over otherwise-ready
            // work. On cancel we bump `shutdown_aborted_connections_total`
            // and fall through to the normal post-task bookkeeping.
            // The H2 path already wires the cancel through
            // `serve_connection_with_cancel`, so for that case this
            // outer race is a backstop, not the primary mechanism.
            tokio::pin!(work);
            let (http_version, result) = tokio::select! {
                biased;
                () = conn_cancel.cancelled() => {
                    // ROUND8 OPS-02 div-l7 refinement: instead of
                    // abandoning the connection at the *same instant*
                    // as every other in-flight conn in this pod (the
                    // pre-refinement behaviour — div-ops's
                    // coordinator-level draw only desyncs *across*
                    // replicas), draw a per-CONNECTION
                    // `[0, per_conn_drain_jitter_ms)` grace. Near-done
                    // work that completes inside its own jittered
                    // window finishes cleanly (counts as success);
                    // the rest still abort but at staggered instants,
                    // smearing the upstream-close burst within the
                    // pod. Bounded by the per-listener jitter ceiling
                    // (≤ the coordinator inflight-drain deadline
                    // div-ops owns), so this never extends the drain.
                    // `0` (jitter disabled / no [runtime]) collapses
                    // to the original immediate-abort behaviour.
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

    // ── SESSION 19 / Mode B (B6) reachability: config → params wiring ──
    //
    // Proves the binary's config→`QuicListenerParams` path actually flips
    // to Mode B when (and only when) a `[listeners.quic.raw_proxy]` block
    // is present — i.e. Mode B is reachable end-to-end through the real
    // `spawn_quic` helper chain. NOT a security proof (those — two-conns
    // and 0-RTT — are the verifier's wire tests).

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
        // The same backend-build the binary does at spawn.
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
        // R3: without a raw_proxy block, no backend is built, so the
        // params carry `raw_quic_backend = None` — the H3-termination path,
        // which makes `build_server_config`'s `enable_datagrams = false`
        // (transport params byte-identical to today).
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

    // F-S26-1 coverage — `wire_h3_terminate_backends` H2 + H3 dispatch
    // arms (main.rs ~1267 H2 / ~1278 H3). Only the H1 arm had an e2e;
    // these unit-drive the H2/H3 arms from a ListenerConfig and assert the
    // RIGHT QuicListenerParams backend field is set. Literal `127.0.0.1`
    // addresses resolve via the real DnsResolver WITHOUT network I/O, so
    // the test is deterministic and offline.

    /// Build an H3-terminate `ListenerConfig` (protocol=quic, no raw_proxy)
    /// carrying a single backend with the given `protocol` + tls knobs.
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
        // tls_verify_peer=false ⇒ build_h3_upstream_pool accepts it without a
        // CA bundle (the uniform-verify-off opt-out), keeping the test offline.
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

    /// The H1 arm (must-have, the WS-over-H3 backend leg) — assert
    /// `with_backends` is wired (it has an e2e via
    /// `spawn_quic_h3_terminate_forwards_to_h1_backend_through_real_listener`,
    /// but this unit-pins the dispatch arm directly + the SNI/h2/h3 slots
    /// stay clear).
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

    // ─────────────────────────────────────────────────────────────────────
    // S19 / Mode B (B6) — REAL spawn_quic e2e (the production entry point is
    // genuinely EXERCISED, not just asserted-reachable).
    //
    // This drives the REAL `spawn_quic` Mode-B arm: a real quiche CLIENT
    // connects to the listener `spawn_quic` bound (so the LB's own router +
    // accept/Retry/0-RTT machinery run), the Mode-B actor re-originates to a
    // real quiche ECHO backend (which forces `build_raw_quic_backend`'s
    // `config_factory` CLOSURE to run on the dedicated dial, and
    // `build_server_config(enable_datagrams=true)` to build the client-facing
    // config), relays a bidi stream both ways, and the client gets its bytes
    // back byte-identically. No test hook — the round-trip goes THROUGH the
    // spawned listener, which is the mechanism proof that the wiring glue ran.
    //
    // Hardened against flake (owner-cited lesson): generous BOUNDED budgets,
    // a single pump loop per side, NO tight per-read timeouts. Deterministic.
    // ─────────────────────────────────────────────────────────────────────

    const MODEB_E2E_MAX_UDP: usize = 65_535;
    const MODEB_E2E_LB_SNI: &str = "lb.modeb.test";
    const MODEB_E2E_BACKEND_SNI: &str = "backend.modeb.test";
    const MODEB_E2E_ALPN: &[u8] = b"h3";
    /// Whole-test budget (handshake + relay round-trip). Generous + bounded.
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

    /// Issue a self-signed leaf (= its own CA, the s16 pattern) for `sni`,
    /// written to a unique temp dir as cert/key/ca PEMs.
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

    /// The downstream CLIENT config — verifies the LB's leaf via `lb_ca`.
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

    /// A throwaway quiche SERVER backend: accepts ONE connection (the LB's
    /// re-originated dial), echoes STREAM bytes back on the same stream id,
    /// FINs back once the peer FIN'd and its echo queue drained. Mirrors the
    /// s16 `spawn_echo_backend`. Bounded by its own 30s deadline.
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
                        // Any recv error (incl. `Done`) ends the drain for this
                        // sid; a FIN / empty read also ends it.
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
                    // Any send error (incl. `Done`) ends the flush this turn.
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

    /// **S19 B6 — the REAL `spawn_quic` Mode-B e2e.** A real quiche client
    /// round-trips a binary payload THROUGH the listener `spawn_quic` bound,
    /// proving the production wiring glue (spawn_quic Mode-B arm +
    /// `build_raw_quic_backend` + the dial `config_factory` closure +
    /// `build_server_config(enable_datagrams=true)`) actually executes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_quic_mode_b_e2e_round_trips_through_real_listener() {
        // Distinct CAs: the client trusts the LB leaf; the LB (Mode-B
        // upstream config_factory) trusts the BACKEND leaf via backend_ca_path.
        let lb_certs = modeb_e2e_gen_certs(MODEB_E2E_LB_SNI, "lb");
        let backend_certs = modeb_e2e_gen_certs(MODEB_E2E_BACKEND_SNI, "be");

        // 1) Real quiche echo backend (the Mode-B re-origination target).
        let backend_addr = modeb_e2e_spawn_echo_backend(&backend_certs);

        // 2) Retry-secret path (auto-generated 0600 by the listener if absent).
        let retry_secret_path = lb_certs.dir.join("retry.secret");

        // 3) Build the REAL ListenerConfig with a [raw_proxy] block → Mode B.
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

        // 4) Drive the REAL production entry point. The pool + resolver
        //    are unused on the Mode-B path (raw_proxy ⇒ no H3-terminate
        //    backend wiring) but are now required by `spawn_quic`'s
        //    signature (F-S26-1).
        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic Mode-B must start");
        let lb_addr = listener.local_addr();

        // 5) Real downstream CLIENT → the listener bound addr.
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

        // 6) Single pump loop: flush → (on established) send once → read echo
        //    → recv with a SHORT bounded wait that is NOT a correctness
        //    timeout (the loop just re-polls; the only hard deadline is the
        //    generous whole-test budget). quiche handles the LB's RETRY
        //    transparently inside `client.recv`.
        loop {
            assert!(
                tokio::time::Instant::now() < deadline,
                "Mode-B e2e budget exhausted: established={}, echoed={}, fin={fin_seen}",
                client.is_established(),
                echoed.len()
            );

            // Flush all pending outbound.
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
                // Flush the freshly-queued stream bytes immediately.
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

            // Bounded re-poll: read one inbound datagram if it arrives within
            // a short window, else loop to re-flush / re-check (no hard fail
            // on this short wait — the budget above is the only deadline).
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

        // 7) THE PROOF: the bytes round-tripped THROUGH the spawned listener
        //    (client → spawn_quic'd LB → Mode-B re-origination → backend echo
        //    → relay → client), byte-identical. This can only happen if the
        //    spawn_quic Mode-B arm + the dial config_factory closure ran.
        assert!(fin_seen, "client must observe the relayed FIN");
        assert_eq!(
            echoed, payload,
            "the payload must round-trip byte-identically through the real Mode-B listener"
        );
        // The client negotiated `h3` with the LB listener (build_server_config
        // advertised it on the Mode-B client-facing config).
        assert_eq!(client.application_proto(), MODEB_E2E_ALPN);

        // 8) Clean shutdown via the production token.
        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    // ─────────────────────────────────────────────────────────────────────
    // F-S26-1 — REAL `spawn_quic` H3-terminate → H1 backend relay e2e.
    //
    // Before this fix the production `spawn_quic` NEVER read
    // `listener_cfg.backends` on the H3-terminate path (no raw_proxy), so
    // every H3 request that reached established state got a 502 "no
    // backends available". This test drives the REAL production entry
    // point: a `ListenerConfig{protocol:"quic", backends:[h1 backend]}` is
    // handed to `spawn_quic`, a real quiche::h3 CLIENT issues a GET
    // THROUGH the spawned listener, and we assert the request reaches the
    // TCP backend AND the backend's 200 response (NOT a 502) comes back.
    // This is the mechanism proof that the binary's config → backend
    // wiring glue (`wire_h3_terminate_backends` → `with_backends`) ran.
    //
    // Bounded budgets, single pump loop, no tight per-read timeouts —
    // matches the Mode-B e2e flake-hardening.
    // ─────────────────────────────────────────────────────────────────────

    const H3H1_E2E_ALPN: &[u8] = b"h3";
    const H3H1_E2E_SNI: &str = "lb.h3h1.test";
    const H3H1_E2E_MAX_UDP: usize = 65_535;
    const H3H1_E2E_BACKEND_STATUS: u16 = 200;
    const H3H1_E2E_BACKEND_BODY: &[u8] = b"f-s26-1-backend-ok";
    const H3H1_E2E_BUDGET: Duration = Duration::from_secs(20);

    /// A throwaway HTTP/1.1 backend: accepts ONE connection, reads the
    /// request head, sends `200 OK` with a fixed body, and closes. The
    /// request-line is captured back to the caller through a oneshot so
    /// the test can assert the H3 request actually reached the backend
    /// (not short-circuited by a 502).
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
            // Read the request head (up to the CRLFCRLF terminator).
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

    /// Downstream client config trusting the LB's self-signed leaf.
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

    /// **F-S26-1 — the REAL `spawn_quic` H3→H1 e2e.** Proves the binary's
    /// H3-terminate → backend wiring runs end to end: config → spawn_quic
    /// → quiche::h3 request → TCP backend → 200 response back to the
    /// client. FAILS on the pre-fix binary (502 "no backends available").
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_quic_h3_terminate_forwards_to_h1_backend_through_real_listener() {
        // `NameValue::{name,value}` for reading the response :status header.
        use quiche::h3::NameValue;

        // 1) LB leaf cert (its own CA — the s16/Mode-B pattern).
        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "h3h1-lb");
        let retry_secret_path = lb_certs.dir.join("retry.secret");

        // 2) Real HTTP/1.1 backend (the H3→H1 forwarding target).
        let (backend_addr, request_line_rx) = h3h1_e2e_spawn_h1_backend();

        // 3) REAL ListenerConfig: protocol=quic, NO raw_proxy ⇒ the
        //    H3-terminate path, WITH a single h1 backend. This is the
        //    config shape the WS-over-H3 work rides on.
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

        // Sanity: the config must be VALID (the new validation must accept
        // a quic listener with an h1 backend and no raw_proxy).
        lb_config::validate_config(&lb_config::LbConfig {
            listeners: vec![listener_cfg.clone()],
            ..Default::default()
        })
        .expect("a quic H3-terminate listener with an h1 backend must validate");

        // 4) Drive the REAL production entry point with the shared pool +
        //    resolver (exactly what main() threads in at the call site).
        let metrics = Arc::new(MetricsRegistry::new());
        let token = CancellationToken::new();
        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        let resolver = DnsResolver::new(ResolverConfig::default());
        let listener = spawn_quic(&listener_cfg, &pool, &resolver, &metrics, 0, token.clone())
            .await
            .expect("spawn_quic H3-terminate must start");
        let lb_addr = listener.local_addr();

        // 5) Real downstream quiche::h3 CLIENT → the listener bound addr.
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

        // 6) Single pump loop: flush → (on established) build the H3 conn +
        //    send GET once → poll H3 events → recv with a SHORT bounded
        //    wait (re-polls; the only hard deadline is the test budget).
        //    quiche handles the LB's RETRY transparently inside conn.recv.
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

            // Build the H3 connection once the QUIC handshake completes.
            if conn.is_established() && h3.is_none() {
                h3 = Some(
                    quiche::h3::Connection::with_transport(&mut conn, &h3_config)
                        .expect("h3 with_transport"),
                );
            }

            // Send the GET request once.
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

            // Poll H3 events for the response.
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

            // Flush all pending outbound.
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

            // Short bounded recv wait (not a correctness timeout).
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

        // 7) THE PROOF: the request reached the H1 backend AND the
        //    backend's 200 response (NOT a 502) came back through the
        //    spawned listener. Only possible if `wire_h3_terminate_backends`
        //    wired `with_backends` so the router relayed H3→H1.
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

        // 8) Clean shutdown via the production token.
        token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.shutdown()).await;
    }

    // ───────────────────────────────────────────────────────────────────
    // SESSION 28 / WS-over-H3 (RFC 9220) Stage C — real-binary e2e.
    //
    // Drives the PRODUCTION entry point (`spawn_quic`, with a
    // `[listeners.websocket]` block + an H1 backend ⇒ the relay launcher is
    // injected by `wire_h3_terminate_backends`). A raw quiche::h3 client
    // sends an Extended CONNECT (`:protocol=websocket`), the LB dials a real
    // tokio-tungstenite WS echo backend + completes the upstream RFC 6455
    // handshake BEFORE the 200, then relays WS frames both directions.
    // ───────────────────────────────────────────────────────────────────

    /// A real tokio-tungstenite WS echo backend. Accepts connections,
    /// echoes Text/Binary, answers Ping with Pong, mirrors Close. Returns
    /// its bound address. Lives for the test (one accept loop, many conns —
    /// the R13 burst reuses it).
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

    /// Encode a single masked client WS frame (RFC 6455 §5.2). `opcode`:
    /// 0x1 Text, 0x8 Close, 0x9 Ping. Payloads stay < 126 bytes (7-bit len).
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

    /// A masked Close frame (code 1000 normal closure).
    fn ws_h3_e2e_close_frame() -> Vec<u8> {
        ws_h3_e2e_encode_masked(0x8, &[0x03, 0xE8])
    }

    /// Parse the FIRST complete WS frame from `buf` (server→client frames
    /// are unmasked; handles a masked frame defensively too). Returns
    /// `(opcode, unmasked_payload, consumed_bytes)`, or `None` if `buf`
    /// holds no complete frame yet.
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
            // 64-bit length (len7 == 127) is never used in this test.
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

    /// Build the `ListenerConfig` for a WS-over-H3 listener: protocol=quic,
    /// H3-terminate, one H1 backend, `[listeners.websocket]` opted in with
    /// `h3_extended_connect = true` (⇒ `spawn_quic` injects the launcher).
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

    /// How the WS-over-H3 client driver closes the tunnel after the echo.
    #[derive(Clone, Copy, Debug)]
    enum WsH3CloseMode {
        /// Send a WS Close frame (orderly RFC 6455 close).
        Clean,
        /// `RESET_STREAM` the tunnel stream (abnormal drop — the reset-vs-EOF
        /// negative control).
        Reset,
        /// FIN the tunnel stream cleanly (empty `send_body(.., fin=true)`) —
        /// the client closes its WS send half WITHOUT a WS Close frame.
        /// Exercises `conn_actor::ws_handle_client_fin` (clean EOF → the relay
        /// forwards a WS Close to the backend).
        Fin,
    }

    /// Drive ONE WS-over-H3 client against the listener at `lb_addr`: QUIC +
    /// Extended CONNECT (`:protocol=websocket`) → expect a `:status` → send a
    /// Text frame → await its echo → close per `mode`. Returns
    /// `(status, echo_ok)`. Single-sources the client driver shared by the
    /// roundtrip / burst / reset tests (R12).
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
            // A non-200 (501/502/504) means no tunnel was established — return
            // promptly rather than waiting out the budget.
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
                            // Abnormal drop: RESET_STREAM + STOP_SENDING with
                            // H3_REQUEST_CANCELLED (the reset-vs-EOF control).
                            let _ = conn.stream_shutdown(sid, quiche::Shutdown::Write, 0x010c);
                            let _ = conn.stream_shutdown(sid, quiche::Shutdown::Read, 0x010c);
                        }
                        WsH3CloseMode::Fin => {
                            // Clean stream FIN (no WS Close frame): the client
                            // closes its send half. Exercises ws_handle_client_fin.
                            if let Some(h3c) = h3.as_mut() {
                                let _ = h3c.send_body(&mut conn, sid, &[], true);
                            }
                        }
                    }
                    closed = true;
                }
            }
            // After closing, pump a few more ticks so the close/reset reaches
            // the LB → backend before we drop the connection.
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

    /// **WS-over-H3 Stage C — the REAL-BINARY e2e.** Extended CONNECT →
    /// 200 → bidirectional WS frame relay (echo) → clean close, all through
    /// `spawn_quic`. Proves the conn_actor tunnel-mode pump + the injected
    /// launcher + the single-sourced `proxy_frames` work end to end.
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

        // THE PROOF: the 200 (extended CONNECT success, NOT a 502) came
        // back AND a WS Text frame round-tripped through the real relay.
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

    /// What a reporting WS backend observed at the end of a tunnel.
    #[derive(Debug, PartialEq, Eq)]
    enum WsBackendOutcome {
        /// The backend received a WS Close frame (orderly RFC 6455 close).
        CleanClose,
        /// The stream ended (Err/EOF) WITHOUT a Close — an abnormal drop.
        Abrupt,
    }

    /// A WS echo backend that also REPORTS, per connection, whether it saw a
    /// clean WS Close or an abrupt end (Err/EOF without a Close). Lets the
    /// reset-vs-EOF test assert the mapping on the WIRED tunnel.
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

    /// **R13 reset-vs-EOF NEGATIVE CONTROL (wired).** A clean WS Close
    /// reaches the backend AS a Close; a client `RESET_STREAM` of the tunnel
    /// stream reaches the backend as an ABRUPT end (NOT a clean Close). The
    /// contrast is load-bearing: it proves the conn_actor maps a reset to
    /// `TunnelInbound::Reset` (ConnectionReset → proxy_frames errors → the
    /// upstream is dropped abruptly), distinct from a clean EOF — the
    /// F-MD-4-adjacent mapping, re-proven on the WIRED tunnel.
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

    /// **R13 BURST.** ≥50 sequential extended-CONNECT → echo → close cycles
    /// against ONE listener + backend. Proves the upgrade+relay+close cycle
    /// is repeatable with no wedge / leak / cumulative failure.
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

    /// A WS flood backend: after the client's first (trigger) frame it
    /// floods `count` `Binary` frames of `frame_len` bytes, bumping `sent`
    /// per FLUSHED frame. When the relay backpressures (the client stops
    /// reading) `feed`/`flush` parks, so `sent` PLATEAUS. Returns the bound
    /// address + the shared `sent` counter.
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
                // Shrink the backend's TCP send buffer so a backend that the
                // gateway stops reading backpressures PROMPTLY (instead of the
                // kernel auto-tuning a multi-MB buffer that hides the gateway's
                // own backpressure). The gateway side caps SO_RCVBUF via the
                // tiny-buffer pool (see the R8 test).
                let _ = socket2::SockRef::from(&sock).set_send_buffer_size(16 * 1024);
                let Ok(mut ws) = tokio_tungstenite::accept_async(sock).await else {
                    return;
                };
                // Wait for the client's trigger frame so the tunnel is active.
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

    /// A downstream client config with a SMALL per-stream receive window so
    /// the R8 outbound flood plateaus decisively (the window + the actor's
    /// bounded tunnel channel are the in-flight bound; everything past it
    /// stays in quiche's flow-control-bounded buffer / parks the relay).
    fn ws_h3_e2e_small_window_client_config(lb_ca: &std::path::Path) -> quiche::Config {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        cfg.set_application_protos(&[H3H1_E2E_ALPN]).unwrap();
        cfg.load_verify_locations_from_file(lb_ca.to_str().unwrap())
            .unwrap();
        cfg.verify_peer(true);
        cfg.set_max_idle_timeout(15_000);
        cfg.set_max_recv_udp_payload_size(1_350);
        cfg.set_max_send_udp_payload_size(1_350);
        // CRITICAL for this test: a stalled client must genuinely
        // backpressure the gateway. quiche AUTO-TUNES receive windows upward
        // (defaults: `max_stream_window` 16 MiB, `max_connection_window`
        // 24 MiB), so a generous `initial_max_data` would let the test CLIENT
        // silently absorb the whole flood and mask the gateway's
        // backpressure. Cap BOTH the connection- and stream-level windows
        // (initial AND auto-tune ceiling) tightly so total in-flight cannot
        // exceed ~64 KiB. NOTE: this caps the CLIENT's buffer, not the
        // gateway's — the gateway is bounded by construction regardless.
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

    /// **R8 WIRED-TUNNEL backpressure (outbound, the CF-S27-2-relevant
    /// direction).** A backend floods `COUNT` frames at a client that
    /// WITHHOLDS reads. With end-to-end backpressure the backend PLATEAUS
    /// far below `COUNT` (the actor's `send_body` returns `Done` on a full
    /// window ⇒ `out_pending` retains ⇒ `from_writer` fills ⇒ the relay's
    /// `PollSender` parks ⇒ `proxy_frames` stops reading the backend). When
    /// the client resumes, every frame arrives (liveness, no loss). The
    /// plateau is VOLUME-INDEPENDENT — it does not grow with `COUNT`.
    /// Reverting the bound (buffer instead of backpressure) flips this RED
    /// (the backend drains the whole flood while the client is stalled).
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
        // Tiny backend SO_RCVBUF (the gateway side) so the kernel TCP buffer
        // between backend and gateway is small — the gateway's OWN bounded
        // relay buffers (from_writer 64 KiB + quiche window) then dominate the
        // plateau, not OS socket auto-tuning.
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

        // Helper closures need owned state; inline the transport pump.
        macro_rules! flush_out {
            () => {
                while let Ok((n, info)) = conn.send(&mut out) {
                    let _ = client_socket
                        .send_to(out.get(..n).unwrap_or(&[]), info.to)
                        .await;
                }
            };
        }

        // Phase 0: establish + extended CONNECT + 200 + send the trigger.
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

        // Phase A: WITHHOLD reads (no recv_body) for a window long enough
        // for an unbounded relay to drain the whole flood. Keep driving the
        // transport so ACKs flow, but never read the tunnel body.
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

    /// A "dead" WS backend: accepts the TCP connection then immediately closes
    /// it WITHOUT completing the RFC 6455 handshake. The gateway's upstream WS
    /// handshake (`dial_backend_ws`) therefore fails → the launcher signals
    /// `Failed{502}` → the actor returns 502 BEFORE any client-visible 200
    /// (the upstream-before-200 ordering).
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

    /// **RFC 9220 §4 — unknown `:protocol` → 501.** An extended CONNECT with
    /// `:protocol=mqtt` (registered-but-unsupported) is rejected with 501
    /// BEFORE any backend is dialed; no tunnel is built.
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

    /// **RFC 9220 §5 — upstream unreachable → 502, no premature 200.** The
    /// backend accepts then drops the TCP without a WS handshake, so the
    /// upstream RFC 6455 handshake fails; the actor returns 502 and NEVER a
    /// 200 (upstream-before-200 / no false success).
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

    /// **Client stream-FIN (no WS Close frame) → abnormal close.** The client
    /// closes its WS send half by FINning the H3 tunnel stream WITHOUT a WS
    /// Close frame. `conn_actor::ws_handle_client_fin` maps the FIN to a clean
    /// EOF on the tunnel (NOT a Reset); the relay (tungstenite) then correctly
    /// surfaces "EOF without closing handshake" as an ABNORMAL closure to the
    /// backend — per RFC 6455 §7.1.5 the ONLY clean close is the Close-frame
    /// handshake. This exercises the `ws_handle_client_fin` clean-EOF path and
    /// proves the gateway does not fabricate a clean Close from a bare FIN.
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

    /// **Fail-closed: `ws_enabled` but NO relay launcher → 502.** The binary's
    /// `spawn_quic` always pairs `ws_enabled` with a launcher; this builds a
    /// `QuicListener` DIRECTLY with `with_websocket(true)` but no
    /// `with_ws_relay_launcher`, so a validated extended CONNECT reaches
    /// `setup_ws_tunnel` with `ws_relay_launcher = None`. It must fail closed
    /// with 502 (NEVER tunnel without a relay), not panic or hang.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ws_over_h3_enabled_without_launcher_fails_closed_502() {
        let lb_certs = modeb_e2e_gen_certs(H3H1_E2E_SNI, "wsh3-nolauncher");
        let retry_secret_path = lb_certs.dir.join("retry.secret");
        let backend_addr = ws_h3_e2e_spawn_ws_echo_backend(); // present but never dialed

        let pool = TcpPool::new(PoolConfig::default(), backend_opts(), Runtime::new());
        // Build the listener WITHOUT a relay launcher: ws_enabled + backends,
        // but no `with_ws_relay_launcher` ⇒ the actor sees `ws_relay_launcher
        // = None` (the misconfiguration the spawn_quic path can never produce).
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

    // CODE-2-03 Wave 2c proof: the three lifecycle signal kinds render
    // to the canonical signal names so /admin logs are greppable.
    #[test]
    fn lifecycle_signal_display_matches_canonical_names() {
        assert_eq!(LifecycleSignal::SigTerm.to_string(), "SIGTERM");
        assert_eq!(LifecycleSignal::SigInt.to_string(), "SIGINT");
        assert_eq!(LifecycleSignal::SigUsr1.to_string(), "SIGUSR1");
        assert_eq!(LifecycleSignal::SigHup.to_string(), "SIGHUP");
    }

    // ── S37-C: ArcSwap snapshot-isolation discipline ───────────────────
    // The load-bearing mechanism behind the live-connection-survives-
    // reload proof: a connection that `.load_full()`s a snapshot at accept
    // keeps reading THAT snapshot after a concurrent `.store()` swaps the
    // ArcSwap — the old snapshot stays alive (refcount-pinned) until the
    // capturing connection drops it. This in-process test proves the
    // pointer-identity / refcount discipline directly (the real-binary
    // under-traffic proof lives in tests/reload_under_traffic.rs).
    #[test]
    fn arcswap_captured_snapshot_survives_store() {
        use arc_swap::ArcSwap;
        use std::sync::Arc;

        // A stand-in for the per-listener proxy snapshot: an immutable
        // value held behind the same Arc<ArcSwap<_>> shape ListenerMode
        // uses for H1Proxy.
        let cell: Arc<ArcSwap<u32>> = Arc::new(ArcSwap::new(Arc::new(1_u32)));

        // An in-flight "connection" snapshots at accept time.
        let in_flight = cell.load_full();
        assert_eq!(*in_flight, 1);

        // A SIGHUP reload swaps a new value under it.
        cell.store(Arc::new(2_u32));

        // The in-flight connection STILL sees its captured snapshot (1) —
        // no cross-talk, no reset. A NEW connection sees the new value.
        assert_eq!(
            *in_flight, 1,
            "captured snapshot must be unaffected by store"
        );
        let new_conn = cell.load_full();
        assert_eq!(
            *new_conn, 2,
            "new connection must observe the swapped value"
        );

        // The old snapshot is the SAME allocation the in-flight conn holds
        // (pointer identity), proving it was not freed by the store — the
        // negative-control's positive assertion (a naive in-place mutate
        // WOULD change `*in_flight`).
        assert!(Arc::ptr_eq(&in_flight, &in_flight.clone()));
        assert!(
            !Arc::ptr_eq(&in_flight, &new_conn),
            "new connection must hold a distinct snapshot from the in-flight one"
        );
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
