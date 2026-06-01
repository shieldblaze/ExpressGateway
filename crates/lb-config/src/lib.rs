//! Configuration loading and management for the load balancer.
//!
//! Provides typed configuration structures and TOML parsing with validation.
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
#![allow(clippy::pedantic, clippy::nursery)]
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

/// Errors from configuration parsing and validation.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// TOML deserialization failed.
    #[error("toml parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    /// Validation error.
    #[error("validation error: {0}")]
    Validation(String),
}

/// Top-level load balancer configuration.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LbConfig {
    /// Configured listeners.
    #[serde(default)]
    pub listeners: Vec<ListenerConfig>,
    /// Global runtime knobs (optional). When absent all defaults apply.
    #[serde(default)]
    pub runtime: Option<RuntimeConfig>,
    /// Observability/admin listener settings. When absent, no admin
    /// HTTP listener is bound and the registry is in-process only.
    #[serde(default)]
    pub observability: Option<ObservabilityConfig>,
    /// SEC-2-06 (Wave 2c-2): optional `[admin]` block carrying the
    /// bearer-token hash + bind override flag for the admin HTTP
    /// listener. When absent, the listener (bound via
    /// `[observability].metrics_bind`) refuses to start on
    /// non-loopback addresses and serves every request without
    /// authentication.
    #[serde(default)]
    pub admin: Option<AdminConfig>,
    /// PROTO-2-17 (Wave 2c-2): optional `[security]` block exposing
    /// process-wide HTTP-security toggles. Currently carries a single
    /// field (`strict_te`) that opts into
    /// `lb_security::SmuggleMode::H1Strict` for the shared
    /// `HooksBundle`. When absent, all defaults apply (lenient RFC
    /// 9112 baseline, i.e. `SmuggleMode::H1`).
    #[serde(default)]
    pub security: Option<SecurityConfig>,
    /// S15 A2-8: optional `[passthrough]` block enabling the Mode A
    /// QUIC passthrough datapath (`lb_quic::PassthroughListener`).
    /// When `Some`, the binary binds a UDP socket and routes QUIC
    /// packets by Connection ID without decrypting (no TLS state,
    /// no `quiche::Connection`, no BoringSSL handshake). When `None`,
    /// no passthrough listener is started. Independent of
    /// `[[listeners]]` — Mode A coexists with terminating listeners
    /// on different ports.
    #[serde(default)]
    pub passthrough: Option<PassthroughConfig>,
}

/// PROTO-2-17 (Wave 2c-2): process-wide HTTP security toggles.
///
/// Lives under `[security]` to keep deployment-decision policy
/// (e.g. "this gateway rejects any non-`chunked` Transfer-Encoding")
/// separate from per-listener `[listeners.*]` blocks. The shared
/// `lb_security::HooksBundle` consumes these knobs at construction
/// time in `crates/lb/src/main.rs`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SecurityConfig {
    /// When `true`, the shared `HooksBundle` is constructed with
    /// `SmuggleMode::H1Strict` instead of the default `SmuggleMode::H1`.
    /// Strict mode rejects any `Transfer-Encoding` codec other than
    /// `chunked` (RFC 9112 §7.1) — the lenient default accepts any
    /// codec hyper can parse, which has historically been a smuggle
    /// vector against permissive backends.
    ///
    /// Default: `false`. Operators flip this on for environments that
    /// can guarantee chunked-only ingress (e.g. behind a known CDN).
    #[serde(default)]
    pub strict_te: bool,
}

/// SEC-2-06 (Wave 2c-2): bearer-token + bind policy for the admin
/// HTTP listener.
///
/// The token is stored as a 64-char hex SHA-256 digest — never the
/// plaintext. `lb_security::AdminTokenHash::from_hex` validates the
/// shape at startup. `allow_non_loopback` is a foot-gun guard: even
/// with a configured token, the listener defaults to loopback-only
/// unless this flag is `true`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AdminConfig {
    /// 64-character hex SHA-256 of the admin bearer token. When
    /// `Some`, every request to the admin HTTP listener must carry
    /// `Authorization: Bearer <plaintext>` whose SHA-256 matches.
    /// When `None`, no auth is enforced and the listener must bind
    /// loopback-only (enforced by `AdminAuthGate::validate_bind`).
    #[serde(default)]
    pub api_token_hash: Option<String>,
    /// SEC-2-06: opt-in escape hatch to allow the admin listener to
    /// bind a non-loopback address. Defaults to `false`. When `true`,
    /// `api_token_hash` MUST also be set or the gateway refuses to
    /// start.
    #[serde(default)]
    pub allow_non_loopback: bool,
}

/// Observability configuration (Task #21, Pillar 3b).
///
/// Currently covers the optional admin HTTP listener that exposes
/// `GET /metrics` (Prometheus text exposition) and `GET /healthz`.
/// Loopback-only is the expected deployment posture; there is no
/// built-in mTLS today.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ObservabilityConfig {
    /// Bind address for the admin HTTP listener. When `None` the
    /// listener is not started. Recommended value for single-host
    /// deployments: `"127.0.0.1:9090"`.
    #[serde(default)]
    pub metrics_bind: Option<String>,
}

/// Process-wide runtime configuration (Pillar 4b-1).
///
/// Currently covers the optional XDP data-plane attach. All fields are
/// opt-in and default to "disabled" — existing deployments that never set
/// `[runtime]` keep their current pure-userspace behaviour.
///
/// **Cross-column note (synthesis §D)**: this struct is `code`'s
/// territory; the EBPF-2-04 change widens it with an additive
/// `xdp_mode` field. The serde default keeps every existing config
/// file accepted unchanged.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RuntimeConfig {
    /// When true, the binary tries to load and attach the compiled BPF
    /// program on startup. Requires `CAP_BPF` + `CAP_NET_ADMIN` (or root)
    /// and a compiled ELF (`scripts/build-xdp.sh` must have produced
    /// `crates/lb-l4-xdp/src/lb_xdp.bin` at build time). If either is
    /// missing the process logs a warning and continues without XDP.
    #[serde(default)]
    pub xdp_enabled: bool,
    /// Network interface name to attach the XDP program to. Required when
    /// `xdp_enabled = true`. Ignored otherwise.
    #[serde(default)]
    pub xdp_interface: Option<String>,
    /// EBPF-2-04: XDP attach mode selector. Defaults to
    /// [`XdpModeChoice::Auto`] which probes Drv-then-Skb. Set
    /// `xdp_mode = "native"` on production NICs to refuse-to-start
    /// rather than silently degrade to 1-3 Mpps SKB mode.
    #[serde(default)]
    pub xdp_mode: XdpModeChoice,
    /// CODE-2-03: graceful-drain budget on SIGTERM, in milliseconds.
    /// The Wave-2 SIGTERM orchestrator calls
    /// `lb_core::Shutdown::drain(Duration::from_millis(drain_timeout_ms))`
    /// at shutdown time; outstanding tasks above this budget are
    /// aborted with a warn-level log. Lead decision §C: default
    /// **10000 ms (10 s)** matches the documented "30-second graceful
    /// drain" RUNBOOK claim once protocol-level GOAWAY (PROTO-2-11) +
    /// inflight gauge (REL-2-09) layer on top. Operators can lower
    /// the budget for fast cluster rotations or raise it for
    /// long-poll workloads.
    ///
    /// Validation: `validate_runtime` accepts 100..=300_000 ms;
    /// outside that range it bails with a clear error.
    #[serde(default = "default_drain_timeout_ms")]
    pub drain_timeout_ms: u64,
    /// CODE-2-03 (Wave 2c): kubelet-style settle window between
    /// flipping `/readyz` to `503 Draining` and starting the
    /// cooperative cancel. Gives upstream LBs / service-mesh
    /// sidecars one health-check interval to stop sending traffic
    /// before connections are torn down. Default: `11000 ms`
    /// (ROUND-8 OPS-11 — sized for the kubelet default
    /// `periodSeconds: 10` so at least one `/readyz` 503 falls
    /// inside the window; was `1000 ms` which was below the kubelet
    /// removal latency). Validation range 0..=30_000 ms.
    #[serde(default = "default_readiness_settle_ms")]
    pub readiness_settle_ms: u64,
    /// ROUND-8 OPS-02: gateway-level drain-cancel jitter ceiling,
    /// in milliseconds. On a deploy-wide SIGTERM, every replica's
    /// drain coordinator would otherwise fire `token.cancel()` at
    /// the same wall-clock instant, producing a thundering-herd
    /// reconnect storm against the shared upstream LB (Envoy hit
    /// this in production with stateful upstream LBs at >2-3
    /// replicas — `drain_manager_impl.cc`). The coordinator sleeps a
    /// per-process random `[0, jitter)` before the in-flight-drain
    /// cancel so close events spread across the fleet instead of
    /// synchronising.
    ///
    /// `None` (the default) means *derive* `drain_timeout_ms / 4`
    /// (Envoy's "first quarter" recipe). `Some(0)` disables jitter
    /// (single-instance / deterministic testing). Per-listener
    /// override: `[[listeners]].drain_jitter_ms`. Validation range
    /// for an explicit value: `0..=drain_timeout_ms`.
    #[serde(default)]
    pub drain_jitter_ms: Option<u64>,
    /// SEC-2-10 (Wave 2c): max wall-clock for `acceptor.accept()`
    /// to complete a TLS handshake. Caps slow-loris-style
    /// handshake-stall attacks at this many ms regardless of
    /// downstream backpressure. Default: `5_000 ms` per the audit
    /// recommendation; validation range 100..=60_000 ms.
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
    /// CODE-2-05 / REL-2-09 (Wave 2c-2): cap on per-listener inflight
    /// connections enforced via a `tokio::sync::Semaphore`. When the
    /// listener is saturated the accept loop returns a 503 (H1/H2) or
    /// closes the socket without a write (plain TCP / TLS pre-ALPN)
    /// and bumps `accept_shed_total`. Default `65_536` matches the
    /// PROMPT.md §21 backlog floor; validation range `100..=2_000_000`.
    #[serde(default = "default_max_inflight_connections")]
    pub max_inflight_connections: u32,
    /// CODE-2-09 / REL-2-11 (Wave 2c-2): wall-clock budget for a single
    /// upstream `TcpStream::connect`. Wraps the dial in
    /// `tokio::time::timeout` so an unresponsive backend (SYN black
    /// hole) returns quickly instead of monopolising a worker. Default
    /// `5_000 ms`; validation range `100..=60_000`.
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    /// SEC-2-04 (Wave 2c-2): per-source-IP concurrent-connection cap
    /// enforced at accept-site via `lb_security::ConnGate`. When the
    /// counter saturates, the accept loop closes the socket without
    /// a response (no amplification surface). Default `1024`;
    /// validation range `1..=2_000_000`. Operators reduce this for
    /// public-facing listeners where the per-IP fairness budget
    /// should be tight.
    #[serde(default = "default_per_ip_cap")]
    pub per_ip_connection_cap: u32,
    /// PROTO-2-14: optional `[runtime.tls]` block for process-wide
    /// TLS-policy knobs. Currently carries a single field
    /// (`tls13_only`); future knobs (preferred-cipher list, ALPN
    /// allow-list) live here too. When absent, all defaults apply
    /// (rustls 0.23 default `&[&TLS12, &TLS13]`).
    #[serde(default)]
    pub tls: Option<RuntimeTlsConfig>,
    /// SEC-2-03 follow-on: optional `[runtime.watchdog]` block. When
    /// present (or when defaulted via `RuntimeConfig::default`), the
    /// binary instantiates an `lb_security::Watchdog`, spawns a
    /// shutdown-tracked sweep loop, and threads it into every
    /// `H1Proxy` / `H2Proxy` for per-stream slowloris / slow-POST
    /// eviction. When absent, the proxies keep the legacy NoopHooks
    /// behaviour (hyper's header-timeout still bites, but the
    /// finer-grained rate floor is dormant).
    #[serde(default)]
    pub watchdog: Option<RuntimeWatchdogConfig>,
    /// ROUND8-L7-05: how to handle `_` in HTTP header names. Envoy
    /// edge best-practice mandates `REJECT_REQUEST`; nginx default
    /// silently drops (`underscores_in_headers off`). Both converge:
    /// the underscore is an auth-bypass primitive against backends
    /// that normalise `_` <-> `-` (Java middleware, some Python
    /// frameworks, SAP gateways). Default: [`HeaderUnderscorePolicy::Reject`].
    ///
    /// See `docs/edge-defaults.md` and `config/default.toml` for the
    /// documented operator surface. Wiring this knob from
    /// [`RuntimeConfig`] into the per-listener `H1Proxy` / `H2Proxy`
    /// builder is the responsibility of the main wiring crate; today
    /// the proxy builders expose
    /// `with_header_underscore_policy(...)` so the integration is a
    /// one-call boundary on the `lb` crate side.
    #[serde(default)]
    pub header_underscore_policy: HeaderUnderscorePolicy,
    /// ROUND8-L7-06: hard cap on the number of requests (H1) /
    /// lifetime streams (H2) served on a single keep-alive
    /// connection before the gateway proactively closes it. Mirrors
    /// nginx's `keepalive_requests 100` default and the Pingora
    /// 0.8.0 `keepalive_requests` cap (Cloudflare added it after
    /// hitting per-connection accounting growth + TLS-session-age +
    /// FD-pinning pain at the edge).
    ///
    /// `0` disables the cap (transparent-pass mode — only the
    /// wall-clock / idle timeouts apply). Default `100`
    /// (`default_max_keepalive_requests`). Any configured value
    /// above `u32::MAX` is clamped at parse time by the serde `u64`
    /// → `u32` conversion in the wiring crate; `validate_runtime`
    /// accepts the full `0..=u32::MAX` range so the only failure
    /// mode is a type error, not a runtime surprise.
    #[serde(default = "default_max_keepalive_requests")]
    pub max_keepalive_requests: u32,
    /// ROUND8-L4-03: per-CPU new-flow-rate cap for the XDP SYN-flood
    /// mitigation (Katran `balancer_kern.c` `is_under_flood()`,
    /// `MAX_CONN_RATE`). When the data plane sees more than this many
    /// conntrack-MISS (new) flows per second on a single CPU, the
    /// excess new flows are short-circuited to `XDP_PASS` WITHOUT the
    /// "populate conntrack" signal — established (CT-hit) flows are
    /// untouched, so an attacker spraying millions of unique
    /// 5-tuples/sec can no longer thrash the 1M-entry LRU and evict
    /// legitimate established connections. The same value gates the
    /// userspace control-plane `CtInsertGate` (leaky-bucket on
    /// `lb-balancer`'s conntrack inserts).
    ///
    /// `0` disables the rate limiter (data plane + control plane).
    /// Default `125_000` mirrors Katran's per-core
    /// `MAX_CONN_RATE`. Validation range: `0` (disabled) or
    /// `1_000..=10_000_000` — a cap below 1k/s/CPU would skip CT
    /// insertion for normal traffic (repeated lookup misses → packets
    /// fall to the kernel stack instead of `XDP_TX`); above 10M/s/CPU
    /// is past line rate on any current NIC and effectively unbounded.
    /// Multi-replica deployments must size this per-replica (the
    /// `CtInsertGate` is per-process) — see RUNBOOK.
    #[serde(default = "default_xdp_new_flow_cap_per_sec_per_cpu")]
    pub xdp_new_flow_cap_per_sec_per_cpu: u32,
}

/// ROUND8-L4-03: Katran `MAX_CONN_RATE` per-core parity. Mirrors
/// `lb_l4_xdp::DEFAULT_NEW_FLOW_CAP_PER_SEC_PER_CPU` and the eBPF
/// `DEFAULT_NEW_FLOW_CAP_PER_CPU`.
const fn default_xdp_new_flow_cap_per_sec_per_cpu() -> u32 {
    125_000
}

/// ROUND8-L7-06: nginx-parity default of 100 requests per keep-alive
/// connection. `0` would disable; we ship the safe industry floor.
const fn default_max_keepalive_requests() -> u32 {
    100
}

impl RuntimeConfig {
    /// ROUND-8 OPS-02: the effective gateway-level drain-cancel
    /// jitter ceiling in milliseconds. `drain_jitter_ms` when set,
    /// otherwise the Envoy "first quarter" derivation
    /// `drain_timeout_ms / 4`.
    #[must_use]
    pub const fn effective_drain_jitter_ms(&self) -> u64 {
        match self.drain_jitter_ms {
            Some(j) => j,
            None => self.drain_timeout_ms / 4,
        }
    }
}

impl ListenerConfig {
    /// ROUND-8 OPS-10: the effective drain budget for this listener
    /// in milliseconds. The per-listener `drain_timeout_ms` override
    /// when present, else the gateway-level `[runtime].drain_timeout_ms`
    /// (or the `default_drain_timeout_ms()` fallback when there is no
    /// `[runtime]` block).
    #[must_use]
    pub fn effective_drain_timeout_ms(&self, runtime: Option<&RuntimeConfig>) -> u64 {
        self.drain_timeout_ms.unwrap_or_else(|| {
            runtime.map_or_else(default_drain_timeout_ms, |r| r.drain_timeout_ms)
        })
    }

    /// ROUND-8 OPS-02/OPS-10: the effective drain-cancel jitter
    /// ceiling for this listener in milliseconds. The per-listener
    /// `drain_jitter_ms` override when present, else the gateway-level
    /// derived jitter (`RuntimeConfig::effective_drain_jitter_ms`, or
    /// `default_drain_timeout_ms() / 4` when there is no `[runtime]`
    /// block).
    #[must_use]
    pub fn effective_drain_jitter_ms(&self, runtime: Option<&RuntimeConfig>) -> u64 {
        self.drain_jitter_ms.unwrap_or_else(|| {
            runtime.map_or_else(
                || default_drain_timeout_ms() / 4,
                RuntimeConfig::effective_drain_jitter_ms,
            )
        })
    }
}

/// ROUND8-L7-05: per-runtime policy for handling `_` in HTTP header
/// names. Mirrors Envoy `headers_with_underscores_action` and nginx
/// `underscores_in_headers`. Both references default to a rejecting
/// stance at the edge; ExpressGateway adopts the same default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeaderUnderscorePolicy {
    /// Reject the request with `400 Bad Request` if any inbound
    /// header name contains `_`. Matches Envoy edge best-practice
    /// (`REJECT_REQUEST`). This is the default.
    #[default]
    Reject,
    /// Silently drop underscore-bearing headers before forwarding;
    /// matches nginx default (`underscores_in_headers off`).
    Drop,
    /// Pass underscore-bearing headers through verbatim. Matches
    /// Envoy `ALLOW` (the non-edge default). Set only if the
    /// downstream environment is known to be safe.
    Allow,
}

/// SEC-2-03 follow-on: per-process slowloris / slow-POST watchdog
/// knobs. Mirrors `lb_security::WatchdogConfig` plus the sweep-loop
/// cadence and the per-request header deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeWatchdogConfig {
    /// Per-request header-phase deadline, in milliseconds. Used as
    /// the `deadline` argument to `Watchdog::register` for inbound
    /// HTTP requests. Default `5_000 ms` (the SEC-2-03 plan's
    /// header-phase cap); validation range `100..=60_000`.
    #[serde(default = "default_watchdog_header_deadline_ms")]
    pub header_deadline_ms: u64,
    /// Slow-POST body-phase rate floor in bytes per second. Connections
    /// whose observed throughput drops below this over the configured
    /// `rate_window` are evicted with `WatchdogError::SlowRate`. `0`
    /// disables the body-phase rate check (deadline-only mode).
    /// Default `64 B/s` per the SEC-2-03 plan; validation range
    /// `0..=10_000_000`.
    #[serde(default = "default_watchdog_body_progress_min_bps")]
    pub body_progress_min_bps: u64,
    /// Cadence of the sweep-loop that evicts connections completely
    /// stalled (no `progress` calls). Default `1_000 ms`; validation
    /// range `100..=60_000`.
    #[serde(default = "default_watchdog_sweep_interval_ms")]
    pub sweep_interval_ms: u64,
}

impl Default for RuntimeWatchdogConfig {
    fn default() -> Self {
        Self {
            header_deadline_ms: default_watchdog_header_deadline_ms(),
            body_progress_min_bps: default_watchdog_body_progress_min_bps(),
            sweep_interval_ms: default_watchdog_sweep_interval_ms(),
        }
    }
}

/// SEC-2-03 follow-on: serde default for
/// `RuntimeWatchdogConfig::header_deadline_ms`.
const fn default_watchdog_header_deadline_ms() -> u64 {
    5_000
}

/// SEC-2-03 follow-on: serde default for
/// `RuntimeWatchdogConfig::body_progress_min_bps`.
const fn default_watchdog_body_progress_min_bps() -> u64 {
    64
}

/// SEC-2-03 follow-on: serde default for
/// `RuntimeWatchdogConfig::sweep_interval_ms`.
const fn default_watchdog_sweep_interval_ms() -> u64 {
    1_000
}

/// PROTO-2-14: process-wide TLS-policy block.
///
/// Lives under `[runtime.tls]` to keep listener-level
/// `[listeners.tls]` (cert / key / kid paths) separate from
/// gateway-wide *policy* knobs that apply to every TLS-bearing
/// listener uniformly.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeTlsConfig {
    /// When `true`, every TLS listener (`protocol = "tls" | "h1s"`)
    /// negotiates **only** TLS 1.3 — rustls is configured with
    /// `versions(&[&TLS13])` instead of the default
    /// `&[&TLS12, &TLS13]`. Default: `false` (rustls default).
    ///
    /// Operators turn this on to comply with policies that forbid
    /// TLS 1.2 (e.g. PCI-DSS 4.0 §4.2.1.1, NIST SP 800-52 Rev. 2
    /// post-2023 transition). It is **not** a security gain in
    /// general — rustls's TLS 1.2 cipher suites are post-quantum
    /// downgrade-safe — but the conformance audit may require it.
    #[serde(default)]
    pub tls13_only: bool,
}

/// CODE-2-03: serde default for `RuntimeConfig::drain_timeout_ms`.
/// 10 000 ms = 10 s per lead §C.
const fn default_drain_timeout_ms() -> u64 {
    10_000
}

/// CODE-2-03 (Wave 2c): serde default for
/// `RuntimeConfig::readiness_settle_ms`.
///
/// ROUND-8 OPS-11: raised from 1 000 ms to 11 000 ms. The old 1 s
/// default was below the kubelet default `periodSeconds: 10`
/// readiness-probe interval: a pod could transition to `Terminating`
/// and start cancelling connections while still listed `Ready` in
/// the Endpoints object, so the next ~10 s of new connections landed
/// on the draining pod. 11 s = one full kubelet probe period (10 s)
/// plus a 1 s margin, so at least one `/readyz` 503 falls inside
/// the settle window even in the worst case (set_draining firing
/// immediately after a probe). Validation cap stays 30 000 ms;
/// operators with aggressively-tuned kubelets can lower it (see
/// `RUNBOOK.md` "Tuning `readiness_settle_ms`"). Aligns with
/// Envoy/Kubernetes lameduck guidance (K8s "Termination of Pods"
/// docs; Envoy `drain_strategy` + endpoint-removal lag).
const fn default_readiness_settle_ms() -> u64 {
    11_000
}

/// SEC-2-10 (Wave 2c): serde default for
/// `RuntimeConfig::handshake_timeout_ms`. 5 000 ms = 5 s per the
/// audit recommendation — a normal TLS 1.3 1-RTT handshake on a
/// healthy network completes in <100 ms, so 5 s is a generous
/// upper bound that still bites on stalled clients.
const fn default_handshake_timeout_ms() -> u64 {
    5_000
}

/// CODE-2-05 / REL-2-09 (Wave 2c-2): serde default for
/// `RuntimeConfig::max_inflight_connections`. 65 536 matches
/// PROMPT.md §21's backlog floor.
const fn default_max_inflight_connections() -> u32 {
    65_536
}

/// CODE-2-09 / REL-2-11 (Wave 2c-2): serde default for
/// `RuntimeConfig::connect_timeout_ms`. 5 000 ms = 5 s — generous
/// upper bound for a healthy intra-DC dial while still cutting the
/// SYN-black-hole tail.
const fn default_connect_timeout_ms() -> u64 {
    5_000
}

/// SEC-2-04 (Wave 2c-2): serde default for
/// `RuntimeConfig::per_ip_connection_cap`. 1 024 matches the
/// pre-2025 industry "per-IP fair share" baseline for load
/// balancers in front of a typical web app.
const fn default_per_ip_cap() -> u32 {
    1_024
}

/// EBPF-2-04: operator-facing XDP attach-mode selector. Reuses the
/// kernel's mode vocabulary one-for-one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum XdpModeChoice {
    /// Ladder: try Drv first, fall back to Skb on `EOPNOTSUPP`/`EINVAL`.
    /// Skips Hw (operators explicitly opt in to hardware offload).
    /// **This is the default** — preserves least-surprise on CI/dev
    /// boxes with veth devices while delivering Drv on real NICs.
    #[default]
    Auto,
    /// Drv-mode only. Aborts startup if the NIC driver does not
    /// support it. The right setting for a 100 G production host
    /// where SKB mode would silently cost 10-50x throughput.
    Native,
    /// Generic SKB mode only. Today's behaviour pre-EBPF-2-04;
    /// keeps existing CI runners working unchanged.
    Skb,
    /// Hardware offload (mlx5 / nfp). Loud-fail if the NIC does not
    /// support it.
    Hw,
}

/// Configuration for a single listener.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ListenerConfig {
    /// Bind address (e.g. `"0.0.0.0:8080"`).
    pub address: String,
    /// Protocol selector. Valid values:
    ///
    /// * `"tcp"` — plain TCP proxy (default), forwarded unchanged to the
    ///   backend.
    /// * `"tls"` — TLS 1.2/1.3 over TCP with rustls. Requires
    ///   [`[listeners.tls]`](TlsConfig).
    /// * `"quic"` — QUIC over UDP with quiche. Requires
    ///   [`[listeners.quic]`](QuicListenerConfig). HTTP/3 bridging to
    ///   backends is Pillar 3b.3c-2; 3b.3c-1 validates the listener
    ///   seam + UDP binding + TLS handshake only.
    /// * `"h1"` — plain HTTP/1.1 on TCP, terminated by hyper. Optional
    ///   [`[listeners.alt_svc]`](AltSvcConfig) and
    ///   [`[listeners.http]`](HttpTimeoutsConfig) blocks.
    /// * `"h1s"` — HTTP/1.1 over TLS. Requires
    ///   [`[listeners.tls]`](TlsConfig). Same optional blocks as `"h1"`.
    /// * `"http"`, `"h2"`, `"h3"` — reserved for upcoming pillars.
    pub protocol: String,
    /// TLS settings. Required when `protocol == "tls"`; must be absent
    /// otherwise.
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    /// QUIC settings. Required when `protocol == "quic"`; must be absent
    /// otherwise.
    #[serde(default)]
    pub quic: Option<QuicListenerConfig>,
    /// Optional `Alt-Svc` advertisement applied to every H1 response.
    /// Only meaningful for `protocol = "h1"` or `"h1s"`.
    #[serde(default)]
    pub alt_svc: Option<AltSvcConfig>,
    /// Optional H1/H2 server timeouts. Only meaningful for `protocol =
    /// "h1"` or `"h1s"`.
    #[serde(default)]
    pub http: Option<HttpTimeoutsConfig>,
    /// Optional HTTP/2 security thresholds surfaced to hyper's H2
    /// builder. Only meaningful for `protocol = "h1s"` (the H2 path
    /// is negotiated via ALPN on that listener). When absent, the
    /// runtime uses `H2SecurityThresholds::default()`.
    #[serde(default)]
    pub h2_security: Option<H2SecurityConfig>,
    /// Optional WebSocket capability block (Item 2, PROMPT.md §14).
    /// Meaningful for `protocol = "h1"` and `"h1s"`. When absent, the
    /// listener silently rejects WebSocket upgrades (they fall through
    /// to the regular HTTP request path, which treats them as plain
    /// GET + unknown headers).
    #[serde(default)]
    pub websocket: Option<WebsocketConfig>,
    /// Optional gRPC capability block (Item 3, PROMPT.md §13). Only
    /// meaningful for `protocol = "h1s"` — gRPC requires HTTP/2, which
    /// is negotiated via ALPN on the h1s listener. When absent, gRPC
    /// requests arriving over H2 fall through to the regular H2→H1
    /// forward path (which will typically emit a 502 to a tonic client
    /// because the upstream protocol mismatches).
    #[serde(default)]
    pub grpc: Option<GrpcListenerConfig>,
    /// ROUND-8 OPS-10: per-listener graceful-drain budget override,
    /// in milliseconds. `None` (the default) inherits
    /// `[runtime].drain_timeout_ms`. The gateway-level default
    /// (10 s) is correct for short-request HTTP but materially
    /// insufficient for long-poll H1 / gRPC bidi / SSE / WebSocket
    /// listeners — Pingora ships `EXIT_TIMEOUT=300s` for exactly
    /// this reason. Set this per streaming listener instead of
    /// raising the gateway default (which would slow every
    /// short-request listener's restart). Matches the
    /// HAProxy-`hard-stop-after`-per-frontend granularity. When
    /// `Some`, must satisfy the same `100..=300_000` ms range as the
    /// gateway-level key. See `RUNBOOK.md` "Tuning the drain budget".
    #[serde(default)]
    pub drain_timeout_ms: Option<u64>,
    /// ROUND-8 OPS-02 / OPS-10: per-listener drain-cancel jitter
    /// ceiling override, in milliseconds. `None` inherits the
    /// gateway-level derived jitter (`drain_timeout_ms / 4`).
    /// `Some(0)` disables jitter for this listener (operator
    /// preference / single-instance). When `Some`, must satisfy
    /// `0..=` the *effective* per-listener `drain_timeout_ms`.
    #[serde(default)]
    pub drain_jitter_ms: Option<u64>,
    /// Upstream backends to load-balance across.
    #[serde(default)]
    pub backends: Vec<BackendConfig>,
}

/// gRPC capability config (Item 3, PROMPT.md §13).
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct GrpcListenerConfig {
    /// Master switch. Defaults to true when the block is present.
    #[serde(default = "default_grpc_enabled")]
    pub enabled: bool,
    /// Upper bound on an accepted `grpc-timeout`. Clients that send a
    /// larger value have it clamped before forwarding. Defaults to 300
    /// seconds (the gRPC spec guidance).
    #[serde(default = "default_grpc_max_deadline")]
    pub max_deadline_seconds: u64,
    /// When true, `/grpc.health.v1.Health/Check` is served locally
    /// (gateway liveness) without forwarding to a backend. Defaults to
    /// true.
    #[serde(default = "default_grpc_health_synthesized")]
    pub health_synthesized: bool,
}

impl Default for GrpcListenerConfig {
    fn default() -> Self {
        Self {
            enabled: default_grpc_enabled(),
            max_deadline_seconds: default_grpc_max_deadline(),
            health_synthesized: default_grpc_health_synthesized(),
        }
    }
}

const fn default_grpc_enabled() -> bool {
    true
}

const fn default_grpc_max_deadline() -> u64 {
    300
}

const fn default_grpc_health_synthesized() -> bool {
    true
}

/// WebSocket capability config (Item 2, PROMPT.md §14).
///
/// Every field is optional; omitted fields default to the canonical
/// value. When the block is absent from the TOML entirely, the listener
/// does NOT accept WebSocket upgrades.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct WebsocketConfig {
    /// Master switch. Defaults to true when the block is present so
    /// operators can enable the capability by declaring the empty table.
    /// Set to `false` to keep the listener's other knobs while disabling
    /// WebSocket handshakes.
    #[serde(default = "default_ws_enabled")]
    pub enabled: bool,
    /// Maximum time a connection may sit idle (no frames in either
    /// direction) before the proxy closes with code `1001 Going Away`.
    /// Defaults to 60 seconds.
    #[serde(default = "default_ws_idle_timeout")]
    pub idle_timeout_seconds: u64,
    /// Upper bound on a single incoming WebSocket message (bytes).
    /// Defaults to 16 MiB.
    #[serde(default = "default_ws_max_message_size")]
    pub max_message_size_bytes: usize,
    /// Maximum number of client-originated `Ping` frames per
    /// `ping_rate_limit_window_seconds` before the proxy emits
    /// `Close 1008` (Policy Violation) to the abusive client and
    /// shuts the upstream half. Mirrors the H/2 `PingFloodDetector`
    /// knob (auditor-delta finding WS-001). Defaults to 50.
    #[serde(default = "default_ws_ping_rate_limit_per_window")]
    pub ping_rate_limit_per_window: u32,
    /// Rolling-window duration (seconds) for the WebSocket client-Ping
    /// rate limit. Defaults to 10 seconds.
    #[serde(default = "default_ws_ping_rate_limit_window_seconds")]
    pub ping_rate_limit_window_seconds: u64,
    /// Per-direction read-frame watchdog. If neither direction produces
    /// a frame for this many seconds the proxy emits `Close 1008
    /// (Policy Violation)` with reason `"ws read frame timeout"` to
    /// bound per-peer pinned-buffer dwell (auditor-delta finding
    /// WS-002). Distinct from `idle_timeout_seconds`, which fires only
    /// when *both* directions are silent. Defaults to 30 seconds.
    #[serde(default = "default_ws_read_frame_timeout_seconds")]
    pub read_frame_timeout_seconds: u64,
    /// RFC 8441 WebSocket-over-HTTP/2 (extended CONNECT). OFF by default:
    /// the H2 upgraded-stream write path lacks true end-to-end backpressure
    /// (a non-reading client can force unbounded gateway memory; see
    /// CF-S27-2). Enable only for trusted client populations until the
    /// window-aware fix lands. When `false` (the default) the H2 listener
    /// neither advertises `SETTINGS_ENABLE_CONNECT_PROTOCOL` nor intercepts
    /// an inbound extended CONNECT. WS-over-HTTP/1.1 (RFC 6455) and the
    /// future WS-over-HTTP/3 are UNAFFECTED by this gate.
    #[serde(default)]
    pub h2_extended_connect: bool,
    /// SESSION 27 — RFC 9220 WebSocket-over-HTTP/3 (extended CONNECT).
    /// OFF by default, mirroring [`Self::h2_extended_connect`]: when
    /// `false` the QUIC/H3 listener neither advertises
    /// `SETTINGS_ENABLE_CONNECT_PROTOCOL` nor accepts a `:protocol`
    /// extended CONNECT (the `:protocol` pseudo-header is rejected exactly
    /// as today — R3). Distinct from the H2 gate because the H3 datapath
    /// is a separate listener type with its own backpressure story;
    /// opting one in does not opt the other in. WS-over-HTTP/1.1 (RFC
    /// 6455) is unaffected.
    #[serde(default)]
    pub h3_extended_connect: bool,
}

impl Default for WebsocketConfig {
    fn default() -> Self {
        Self {
            enabled: default_ws_enabled(),
            idle_timeout_seconds: default_ws_idle_timeout(),
            max_message_size_bytes: default_ws_max_message_size(),
            ping_rate_limit_per_window: default_ws_ping_rate_limit_per_window(),
            ping_rate_limit_window_seconds: default_ws_ping_rate_limit_window_seconds(),
            read_frame_timeout_seconds: default_ws_read_frame_timeout_seconds(),
            // OFF by default — H2-only DoS gate (CF-S27-2).
            h2_extended_connect: false,
            // OFF by default — H3 WS opt-in (S27, separate from H2's gate).
            h3_extended_connect: false,
        }
    }
}

const fn default_ws_enabled() -> bool {
    true
}

const fn default_ws_idle_timeout() -> u64 {
    60
}

const fn default_ws_max_message_size() -> usize {
    16 * 1024 * 1024
}

const fn default_ws_ping_rate_limit_per_window() -> u32 {
    50
}

const fn default_ws_ping_rate_limit_window_seconds() -> u64 {
    10
}

const fn default_ws_read_frame_timeout_seconds() -> u64 {
    30
}

/// HTTP/2 security thresholds (Item 1, auditor finding #3).
///
/// Every field is optional; omitted fields default to the canonical
/// value drawn from `lb_h2::security`. Mirrors the shape of
/// `lb_l7::h2_security::H2SecurityThresholds` without importing it
/// (keeping lb-config free of a hyper dependency).
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct H2SecurityConfig {
    /// Maximum queued pending-accept `RST_STREAM` frames before GOAWAY.
    #[serde(default)]
    pub max_pending_accept_reset_streams: Option<usize>,
    /// Maximum `RST_STREAM` frames triggered by local errors before GOAWAY.
    #[serde(default)]
    pub max_local_error_reset_streams: Option<usize>,
    /// Cap on concurrent streams the server will accept.
    #[serde(default)]
    pub max_concurrent_streams: Option<u32>,
    /// Absolute cap on decoded HPACK header list size (bytes).
    #[serde(default)]
    pub max_header_list_size: Option<u32>,
    /// Per-stream send buffer cap (bytes).
    #[serde(default)]
    pub max_send_buf_size: Option<usize>,
    /// Keep-alive PING interval in milliseconds. When absent, the
    /// keep-alive mechanism runs with the detector-derived default.
    /// Set to 0 to disable keep-alive.
    #[serde(default)]
    pub keep_alive_interval_ms: Option<u64>,
    /// Keep-alive timeout in milliseconds.
    #[serde(default)]
    pub keep_alive_timeout_ms: Option<u64>,
    /// Initial per-stream receive window.
    #[serde(default)]
    pub initial_stream_window_size: Option<u32>,
    /// Initial connection-level receive window.
    #[serde(default)]
    pub initial_connection_window_size: Option<u32>,
}

/// `Alt-Svc` injection config (Pillar 3b.3b-1).
///
/// When set, every H1 response gets `Alt-Svc: h3=":<h3_port>"; ma=<max_age>`.
/// This is how a TLS-terminated H1 listener advertises an HTTP/3 endpoint
/// for clients that support QUIC upgrade.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AltSvcConfig {
    /// UDP port hosting the H3 listener that should be advertised.
    pub h3_port: u16,
    /// `max-age` value in seconds. Defaults to one hour.
    #[serde(default = "default_alt_svc_max_age")]
    pub max_age: u32,
}

const fn default_alt_svc_max_age() -> u32 {
    3_600
}

/// HTTP server timeouts (Pillar 3b.3b-1).
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct HttpTimeoutsConfig {
    /// Maximum time the listener will spend reading the *request line +
    /// headers* before giving up. Defaults to 10 seconds.
    #[serde(default = "default_header_timeout_ms")]
    pub header_timeout_ms: u64,
    /// Maximum time the listener will spend draining the request *body*
    /// or waiting for response *body* bytes from the upstream. Defaults
    /// to 30 seconds.
    #[serde(default = "default_body_timeout_ms")]
    pub body_timeout_ms: u64,
    /// Hard upper bound on total request lifetime. Defaults to 60 seconds.
    #[serde(default = "default_total_timeout_ms")]
    pub total_timeout_ms: u64,
    /// **S14 / CF-BODY-WALLCLOCK (R-CFBW-2)** — Phase-B fixed cap on the
    /// post-upload head wait (separate from the Phase-A idle deadline
    /// derived from `body_timeout_ms`). Defaults to 60 seconds.
    #[serde(default = "default_head_timeout_ms")]
    pub head_timeout_ms: u64,
}

impl Default for HttpTimeoutsConfig {
    fn default() -> Self {
        Self {
            header_timeout_ms: default_header_timeout_ms(),
            body_timeout_ms: default_body_timeout_ms(),
            total_timeout_ms: default_total_timeout_ms(),
            head_timeout_ms: default_head_timeout_ms(),
        }
    }
}

const fn default_header_timeout_ms() -> u64 {
    10_000
}

const fn default_body_timeout_ms() -> u64 {
    30_000
}

const fn default_total_timeout_ms() -> u64 {
    60_000
}

const fn default_head_timeout_ms() -> u64 {
    60_000
}

/// TLS listener configuration (Pillar 3b.2).
///
/// Backed by rustls 0.23 + the `ring` crypto provider. The
/// [`TicketRotator`](lb-security) mints session-resumption tickets using
/// the configured rotation window.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TlsConfig {
    /// Filesystem path to the PEM-encoded certificate chain.
    pub cert_path: String,
    /// Filesystem path to the PEM-encoded private key (PKCS#8 or SEC1).
    pub key_path: String,
    /// How often to rotate the session-ticket key (seconds). Defaults
    /// to 24 hours, matching the Step 5b default.
    #[serde(default = "default_ticket_interval")]
    pub ticket_rotation_interval_seconds: u64,
    /// Grace period during which tickets encrypted with the previous
    /// key still decrypt (seconds). Defaults to 24 hours — together
    /// with the default interval this gives a 48-hour total ticket
    /// lifetime at the rustls layer.
    #[serde(default = "default_ticket_overlap")]
    pub ticket_rotation_overlap_seconds: u64,
}

const fn default_ticket_interval() -> u64 {
    86_400
}

const fn default_ticket_overlap() -> u64 {
    86_400
}

/// QUIC listener configuration (Pillar 3b.3c-1).
///
/// Backed by quiche 0.28 + `BoringSSL`. The `retry_secret_path` stores a
/// 32-byte HMAC-SHA256 key used by
/// [`lb_security::RetryTokenSigner`](../../lb-security) for
/// stateless-retry address validation (RFC 9000 §8.1.3). The file is
/// auto-generated with mode 0600 on first boot if missing. Pillar
/// 3b.3c-2 wires the signer + replay guard to the inbound packet
/// router; 3b.3c-1 only validates the seam and the UDP bind.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QuicListenerConfig {
    /// Filesystem path to the PEM-encoded certificate chain.
    pub cert_path: String,
    /// Filesystem path to the PEM-encoded private key (PKCS#8 or SEC1).
    pub key_path: String,
    /// Filesystem path to a 32-byte retry-token signing key. Auto-
    /// generated on first boot if the file does not exist.
    pub retry_secret_path: String,
    /// Connection idle timeout in milliseconds. Defaults to 30 seconds.
    #[serde(default = "default_quic_idle_timeout_ms")]
    pub max_idle_timeout_ms: u64,
    /// Maximum UDP payload the endpoint will accept. Defaults to 1350
    /// bytes (safe for a 1500-byte Ethernet MTU minus IPv4+UDP headers
    /// and QUIC overhead). Must be at least 1200 per RFC 9000 §14.
    #[serde(default = "default_quic_recv_udp_payload")]
    pub max_recv_udp_payload_size: u64,
    /// SESSION 16 / Mode B (terminate-and-re-originate) raw-QUIC proxy.
    ///
    /// **Absent (the default) ⇒ H3-terminate exactly as today (R3).** When
    /// this block is present the QUIC listener instead runs Mode B: it
    /// terminates the client QUIC connection (reusing the full
    /// accept/Retry/0-RTT machinery) and re-originates a fresh, dedicated
    /// upstream QUIC connection, relaying raw streams + datagrams between
    /// the two. Two distinct `quiche::Connection`s, never a CID bridge.
    ///
    /// Because the field is `#[serde(default)]` (deserialises to `None`
    /// when omitted) every existing config parses byte-identically and the
    /// listener's advertised transport parameters are unchanged: Mode-B
    /// only listeners enable QUIC DATAGRAM support and set the raw backend
    /// (see `lb_quic::QuicListenerParams`).
    #[serde(default)]
    pub raw_proxy: Option<RawQuicProxyConfig>,
}

const fn default_quic_idle_timeout_ms() -> u64 {
    30_000
}

const fn default_quic_recv_udp_payload() -> u64 {
    1_350
}

/// SESSION 16 / Mode B raw-QUIC proxy backend configuration (B6).
///
/// Present under a `[listeners.quic.raw_proxy]` block, this switches the
/// QUIC listener from H3-termination to terminate-and-re-originate Mode B
/// and names the single upstream QUIC backend to re-originate to. All caps
/// are R7 pre-auth (apply before the client is authenticated), so the
/// defaults are conservative, industry-safe bounds — documented per helper
/// below.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RawQuicProxyConfig {
    /// Resolved upstream QUIC backend address (`host:port`) to
    /// re-originate every terminated client connection to. Parsed to a
    /// `SocketAddr` by the binary at spawn; an unparseable value fails
    /// startup with a clear error rather than silently disabling Mode B.
    pub backend_addr: String,
    /// SNI presented to the upstream on the re-originated TLS handshake.
    pub sni: String,
    /// Optional PEM CA bundle used to verify the upstream backend's TLS
    /// certificate on the re-originated handshake.
    ///
    /// ## Backend-trust v1 behaviour (documented; never silently disabled)
    ///
    /// * **Set** ⇒ the upstream-leg `quiche::Config` loads this bundle via
    ///   `load_verify_locations_from_file` and engages `verify_peer(true)`.
    ///   The re-originated handshake is rejected unless the backend cert
    ///   chains to this CA.
    /// * **Absent** ⇒ `verify_peer(true)` is STILL engaged, relying on
    ///   `BoringSSL`'s built-in/system default trust roots. Verification is
    ///   NOT disabled. (To proxy to a backend with a private CA, set this
    ///   path; there is no config knob to turn verification off in Mode B —
    ///   that would be an undocumented downgrade and is intentionally
    ///   absent.)
    #[serde(default)]
    pub backend_ca_path: Option<String>,
    /// B4 — per-direction bounded DATAGRAM relay queue capacity (count of
    /// queued datagrams), and the DATAGRAM recv/send queue length
    /// advertised to both peers via `enable_dgram(true, cap, cap)`.
    /// Default `1024` (matches quiche's own recv/send-queue default and
    /// the `lb_quic::raw_proxy` `DGRAM_QUEUE_CAP`). R7 pre-auth bound:
    /// large enough to absorb a normal burst, small enough that a flooding
    /// peer cannot grow relay memory without bound (over-cap is
    /// drop-newest).
    #[serde(default = "default_raw_proxy_dgram_queue_cap")]
    pub dgram_queue_cap: usize,
    /// B5 — explicit, defense-in-depth ceiling on the per-connection relay
    /// stream table size. Default `256` (matches the `lb_quic::raw_proxy`
    /// `MAX_RELAY_STREAMS`): comfortably above the negotiated concurrent
    /// stream grant (~32) yet keeps the worst-case per-connection relay
    /// memory a hard constant independent of a mis-set `max_streams`. R7
    /// pre-auth bound.
    #[serde(default = "default_raw_proxy_max_relay_streams")]
    pub max_relay_streams: usize,
}

/// Mode B B4 default DATAGRAM relay queue capacity. `1024` mirrors
/// `lb_quic::raw_proxy::DGRAM_QUEUE_CAP` + quiche's recv/send-queue default.
const fn default_raw_proxy_dgram_queue_cap() -> usize {
    1_024
}

/// Mode B B5 default relay stream-table ceiling. `256` mirrors
/// `lb_quic::raw_proxy::MAX_RELAY_STREAMS` (defense-in-depth bound).
const fn default_raw_proxy_max_relay_streams() -> usize {
    256
}

/// S15 A2-8: Mode A QUIC passthrough listener configuration.
///
/// Lives at the top level (`[passthrough]`) rather than under
/// `[[listeners]]` because Mode A is a parallel datapath, not a
/// listener-protocol variant: it cannot share a UDP port with the
/// `protocol = "quic"` terminating listener and the field shape
/// (no cert/key, no per-listener drain knobs) differs structurally.
/// The shape mirrors `lb_quic::PassthroughParams` 1:1.
///
/// Field defaults match the owner rulings from
/// `audit/quic/s15-design.md` §9 — see each `default_*_passthrough`
/// helper below for citations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PassthroughConfig {
    /// Bind address for the listener UDP socket.
    pub bind_addr: std::net::SocketAddr,
    /// Resolved backend addresses; consumed by Maglev consistent
    /// hashing on every Initial. Must be non-empty.
    pub backends: Vec<std::net::SocketAddr>,
    /// Path to the 32-byte retry-secret file. Generated with mode
    /// 0600 if missing — same discipline as the terminating QUIC
    /// listener.
    pub retry_secret_path: std::path::PathBuf,
    /// Maximum concurrent QUIC flows. Default 100_000 per owner
    /// ruling §9.4; routing-table-entry cap is `2 * max`.
    #[serde(default = "default_passthrough_max_quic_connections")]
    pub max_quic_connections: usize,
    /// Minimum client-chosen DCID length accepted. Default 8 per
    /// owner ruling §9.3 (CVE-2022-30592-style cross-flow prefix
    /// collision defence).
    #[serde(default = "default_passthrough_min_client_dcid_len")]
    pub min_client_dcid_len: usize,
    /// Per-flow datagram backlog. Default 32; drop-newest on Full
    /// per design §5.1.
    #[serde(default = "default_passthrough_per_flow_backlog")]
    pub per_flow_backlog: usize,
    /// Strict source-IP binding: when true, short-header packets
    /// whose 4-tuple differs from the flow's recorded peer are
    /// dropped at the LB. Breaks NAT-rebind path-migration but
    /// catches off-path spoofed-CID injection. Default **false**
    /// per owner ruling §9.1' (mobile availability is load-bearing).
    #[serde(default)]
    pub strict_source_binding: bool,
    /// Audit-log throttle window, in seconds. 60s default per §6.2.
    #[serde(default = "default_passthrough_audit_throttle_window_secs")]
    pub audit_throttle_window_secs: u64,
    /// Short-header DCID length to try first when no per-flow length
    /// is known. Default 20 (RFC 9000 §17.3 max) per §3.3 fallback.
    #[serde(default = "default_passthrough_max_dcid_len_routed")]
    pub max_dcid_len_routed: usize,
    /// Whether the LB mints stateless Retry on no-token Initials
    /// (§6.5 Initial-flood defence per owner ruling §9.2). Default
    /// **true** for production deployments. When `false`, no-token
    /// Initials are forwarded to the backend verbatim — the backend's
    /// own `quiche::accept` then handles Initial-flood defence
    /// (either accepts directly or initiates its own Retry, which
    /// the LB just forwards). Documented test/trusted-network escape
    /// for **CF-S15-PASSTHROUGH-RETRY-ODCID**: with `mint_retry =
    /// true`, real-quiche backends reject the post-Retry
    /// `original_destination_connection_id` transport param because
    /// the LB-chosen new_scid hides the client's ODCID. RFC 9000
    /// §17.2.5 anticipates a "Retry Service" pattern (token-embedded
    /// ODCID + backend extracts on verify); deferred to S15.x / S16.
    #[serde(default = "default_passthrough_mint_retry")]
    pub mint_retry: bool,
    /// Idle-flow reaper threshold, in milliseconds (F-S20-2). A flow with
    /// no inbound packet for longer than this is reclaimed by the periodic
    /// idle sweep — its backend UDP socket fd + both pump tasks are freed —
    /// bounding the table by the LIVE connection count rather than waiting
    /// for the LRU cap at `2 * max_quic_connections`. Passthrough cannot
    /// observe the encrypted CONNECTION_CLOSE, so without this sweep a flow
    /// for a closed connection persists indefinitely (the S20 soak measured
    /// flows 0→56k, fds→28k, RSS→331MB, evicted=0). Default 60_000 (60 s),
    /// the standard stateless-passthrough reclamation window (Katran/Pingora
    /// style); `0` disables the sweep (LRU-only, the pre-S21 behaviour).
    #[serde(default = "default_passthrough_flow_idle_timeout_ms")]
    pub flow_idle_timeout_ms: u64,
}

/// S15 A2-8: owner ruling §9.4 — 100k flows is the documented
/// default flow-cap. Routing table entries are bounded at `2 * cap`.
const fn default_passthrough_max_quic_connections() -> usize {
    100_000
}

/// S15 A2-8: owner ruling §9.3 — 8-byte minimum client DCID is the
/// CVE-2022-30592-style cross-flow prefix-collision defence floor.
const fn default_passthrough_min_client_dcid_len() -> usize {
    8
}

/// S15 A2-8: design §5.1 — per-flow datagram backlog. 32 datagrams
/// is the drop-newest queue depth between the recv loop and the
/// per-flow forward task.
const fn default_passthrough_per_flow_backlog() -> usize {
    32
}

/// S15 A2-8: design §6.2 — audit-log throttle window. 60 s damps
/// per-flow drop logs so a misbehaving peer cannot flood the audit
/// stream.
const fn default_passthrough_audit_throttle_window_secs() -> u64 {
    60
}

/// S15 A2-8: design §3.3 — short-header DCID single-length fast
/// path. 20 bytes is RFC 9000 §17.3's maximum routable DCID length.
const fn default_passthrough_max_dcid_len_routed() -> usize {
    20
}

/// S15 A2-3 / CF-S15-PASSTHROUGH-RETRY-ODCID: §6.5 Initial-flood
/// defence is ON by default; production deployments leave this
/// `true`. The escape (`false`) delegates flood defence to the
/// backend; see `PassthroughConfig::mint_retry` doc.
const fn default_passthrough_mint_retry() -> bool {
    true
}

/// F-S20-2: idle-flow reaper default. 60 s is the standard stateless-
/// passthrough reclamation window (nginx-style idle reclaim / Katran /
/// Pingora). Configurable; `0` disables the sweep (LRU-only).
const fn default_passthrough_flow_idle_timeout_ms() -> u64 {
    60_000
}

/// Configuration for a single upstream backend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackendConfig {
    /// Backend address (e.g. `"127.0.0.1:3000"`).
    pub address: String,
    /// Wire protocol spoken to this backend. Defaults to `"tcp"`.
    /// Values accepted: `"tcp"` (raw stream, used by the plain-TCP and
    /// TLS-over-TCP listeners), `"h1"` (HTTP/1.1 over TCP — the QUIC
    /// listener's default bridge target in Pillar 3b.3c-2), `"h2"`
    /// (HTTP/2 over TCP+TLS via ALPN; consumed by `lb_io::Http2Pool` from
    /// every L7 listener — PROTO-001), `"h3"` (HTTP/3 over QUIC —
    /// consumed by the Pillar 3b.3c-3 upstream pool).
    #[serde(default = "default_backend_protocol")]
    pub protocol: String,
    /// Weight for weighted load-balancing algorithms (default 1).
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Path to a PEM CA bundle used to verify the H3 backend's TLS
    /// certificate during the upstream QUIC handshake. Required when
    /// `protocol = "h3"` unless `tls_verify_peer = false`. Ignored for
    /// non-H3 backends. (Round-4 D4-4: closes the binary's prior
    /// `verify_peer(false)` posture on the H3 upstream pool.)
    #[serde(default)]
    pub tls_ca_path: Option<String>,
    /// SNI override for backend TLS verification. Defaults to the host
    /// portion of `address` when absent. Useful when the backend cert
    /// presents a name that does not match the dial address (e.g. a
    /// virtual-host-style internal hostname behind a load-balanced VIP).
    /// Only meaningful for `protocol = "h3"`.
    #[serde(default)]
    pub tls_verify_hostname: Option<String>,
    /// If `true` (the default), the H3 upstream pool validates the
    /// backend's TLS certificate against `tls_ca_path` and the SNI
    /// resolved from `tls_verify_hostname` / `address`. Set to `false`
    /// to disable peer-cert verification entirely — **NOT RECOMMENDED**;
    /// only acceptable for operators using a separate mesh-encryption
    /// layer (e.g. `WireGuard`, an Istio-style ambient sidecar) that
    /// authenticates the underlay independently. Ignored for non-H3
    /// backends.
    #[serde(default = "default_verify_peer_true")]
    pub tls_verify_peer: bool,
}

const fn default_verify_peer_true() -> bool {
    true
}

fn default_backend_protocol() -> String {
    "tcp".to_string()
}

const fn default_weight() -> u32 {
    1
}

/// Parse a TOML string into an `LbConfig`.
///
/// # Errors
///
/// Returns `ConfigError::TomlParse` if deserialization fails.
pub fn parse_config(input: &str) -> Result<LbConfig, ConfigError> {
    let config: LbConfig = toml::from_str(input)?;
    Ok(config)
}

/// Validate a parsed configuration.
///
/// # Errors
///
/// Returns `ConfigError::Validation` if the config is invalid.
pub fn validate_config(config: &LbConfig) -> Result<(), ConfigError> {
    // S15 A2-8: passthrough is an independent datapath — a config
    // with only `[passthrough]` and no `[[listeners]]` is valid
    // (Mode-A-only deployment matching the
    // `quic-passthrough-only` feature build).
    if config.listeners.is_empty() && config.passthrough.is_none() {
        return Err(ConfigError::Validation(
            "at least one listener or [passthrough] is required".into(),
        ));
    }
    if let Some(pt) = config.passthrough.as_ref() {
        validate_passthrough(pt)?;
    }
    for (i, listener) in config.listeners.iter().enumerate() {
        validate_listener(i, listener)?;
    }
    if let Some(rt) = config.runtime.as_ref() {
        validate_runtime(rt)?;
    }
    // ROUND-8 OPS-02/OPS-10: cross-check that each listener's
    // *effective* jitter does not exceed its *effective* drain
    // budget once inheritance from [runtime] is resolved. This
    // catches the case where a listener sets drain_jitter_ms but
    // inherits a smaller [runtime].drain_timeout_ms (validate_listener
    // alone can't see the runtime block).
    for (i, listener) in config.listeners.iter().enumerate() {
        let eff_timeout = listener.effective_drain_timeout_ms(config.runtime.as_ref());
        let eff_jitter = listener.effective_drain_jitter_ms(config.runtime.as_ref());
        if eff_jitter > eff_timeout {
            return Err(ConfigError::Validation(format!(
                "listener {i} effective drain_jitter_ms={eff_jitter} exceeds \
                 effective drain_timeout_ms={eff_timeout} after [runtime] \
                 inheritance (jitter must be <= the drain budget)"
            )));
        }
    }
    if let Some(obs) = config.observability.as_ref() {
        validate_observability(obs)?;
    }
    Ok(())
}

fn validate_observability(obs: &ObservabilityConfig) -> Result<(), ConfigError> {
    if let Some(bind) = obs.metrics_bind.as_deref() {
        let trimmed = bind.trim();
        if trimmed.is_empty() {
            return Err(ConfigError::Validation(
                "observability.metrics_bind is empty — omit the key to disable".into(),
            ));
        }
        trimmed.parse::<std::net::SocketAddr>().map_err(|e| {
            ConfigError::Validation(format!(
                "observability.metrics_bind {trimmed:?} is not a valid SocketAddr: {e}"
            ))
        })?;
    }
    Ok(())
}

fn validate_runtime(rt: &RuntimeConfig) -> Result<(), ConfigError> {
    if rt.xdp_enabled {
        let iface = rt
            .xdp_interface
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if iface.is_none() {
            return Err(ConfigError::Validation(
                "runtime.xdp_enabled=true requires runtime.xdp_interface".into(),
            ));
        }
    }
    // CODE-2-03: drain budget must be a sane positive duration.
    // 100 ms floor avoids the operator-mistake "drain_timeout_ms = 1"
    // collapsing the drain to a no-op; 300 000 ms (5 min) ceiling
    // bounds the worst-case SIGTERM-to-exit latency for service
    // managers (systemd default TimeoutStopSec is 90 s, k8s default
    // terminationGracePeriodSeconds is 30 s — both well under the
    // ceiling).
    if !(100..=300_000).contains(&rt.drain_timeout_ms) {
        return Err(ConfigError::Validation(format!(
            "runtime.drain_timeout_ms={} out of range 100..=300000",
            rt.drain_timeout_ms
        )));
    }
    // ROUND-8 OPS-02: gateway-level jitter ceiling, when explicitly
    // set, must be 0..=drain_timeout_ms (jitter cannot exceed the
    // budget it is subdividing). `None` derives drain_timeout_ms/4
    // and is always in range by construction.
    if let Some(j) = rt.drain_jitter_ms {
        if j > rt.drain_timeout_ms {
            return Err(ConfigError::Validation(format!(
                "runtime.drain_jitter_ms={j} exceeds runtime.drain_timeout_ms={} \
                 (jitter must be <= the drain budget)",
                rt.drain_timeout_ms
            )));
        }
    }
    // CODE-2-03 Wave 2c: settle window may be 0 (skip the sleep) but
    // is capped at 30 s — beyond that operators are mis-using the
    // knob (k8s terminationGracePeriodSeconds usually <= 30).
    if rt.readiness_settle_ms > 30_000 {
        return Err(ConfigError::Validation(format!(
            "runtime.readiness_settle_ms={} out of range 0..=30000",
            rt.readiness_settle_ms
        )));
    }
    // SEC-2-10 Wave 2c: 100 ms floor avoids an accidental
    // zero-budget timeout starving every TLS connect; 60 s ceiling
    // bounds slow-loris exposure.
    if !(100..=60_000).contains(&rt.handshake_timeout_ms) {
        return Err(ConfigError::Validation(format!(
            "runtime.handshake_timeout_ms={} out of range 100..=60000",
            rt.handshake_timeout_ms
        )));
    }
    // CODE-2-05 / REL-2-09 Wave 2c-2: floor of 100 keeps the sentinel
    // semaphore from collapsing into a single-connection bottleneck;
    // ceiling of 2_000_000 bounds memory pressure (Semaphore stores
    // one waiter slot per permit + per waiter).
    if !(100..=2_000_000).contains(&rt.max_inflight_connections) {
        return Err(ConfigError::Validation(format!(
            "runtime.max_inflight_connections={} out of range 100..=2000000",
            rt.max_inflight_connections
        )));
    }
    // CODE-2-09 / REL-2-11 Wave 2c-2: same range as
    // `handshake_timeout_ms` — both bound stalls on a hot path that
    // would otherwise occupy a worker indefinitely.
    if !(100..=60_000).contains(&rt.connect_timeout_ms) {
        return Err(ConfigError::Validation(format!(
            "runtime.connect_timeout_ms={} out of range 100..=60000",
            rt.connect_timeout_ms
        )));
    }
    // SEC-2-04 Wave 2c-2: 1..=2_000_000 — zero would refuse every
    // connection; 2_000_000 ceiling is shared with the listener cap.
    if !(1..=2_000_000).contains(&rt.per_ip_connection_cap) {
        return Err(ConfigError::Validation(format!(
            "runtime.per_ip_connection_cap={} out of range 1..=2000000",
            rt.per_ip_connection_cap
        )));
    }
    // SEC-2-03 follow-on: validate the optional watchdog block. We
    // bound `header_deadline_ms` like `connect_timeout_ms` and
    // `sweep_interval_ms` to a similar range; `body_progress_min_bps`
    // is a soft rate floor with a 10 MB/s ceiling — anything above
    // would push false-positive evictions on slow mobile uplinks.
    if let Some(wd) = rt.watchdog.as_ref() {
        if !(100..=60_000).contains(&wd.header_deadline_ms) {
            return Err(ConfigError::Validation(format!(
                "runtime.watchdog.header_deadline_ms={} out of range 100..=60000",
                wd.header_deadline_ms
            )));
        }
        if wd.body_progress_min_bps > 10_000_000 {
            return Err(ConfigError::Validation(format!(
                "runtime.watchdog.body_progress_min_bps={} out of range 0..=10000000",
                wd.body_progress_min_bps
            )));
        }
        if !(100..=60_000).contains(&wd.sweep_interval_ms) {
            return Err(ConfigError::Validation(format!(
                "runtime.watchdog.sweep_interval_ms={} out of range 100..=60000",
                wd.sweep_interval_ms
            )));
        }
    }
    // ROUND8-L4-03: the new-flow cap is either 0 (disabled) or in
    // 1_000..=10_000_000 per CPU. Below 1k/s/CPU the data plane would
    // skip conntrack insertion for normal traffic (lookup misses →
    // packets fall to the kernel stack instead of XDP_TX); above
    // 10M/s/CPU is past line rate on any current NIC. The clamp keeps
    // the runtime footgun (finding "Risk / blast radius") off the
    // table — an out-of-range value is a hard config error, not a
    // silent traffic blackhole.
    let cap = rt.xdp_new_flow_cap_per_sec_per_cpu;
    if cap != 0 && !(1_000..=10_000_000).contains(&cap) {
        return Err(ConfigError::Validation(format!(
            "runtime.xdp_new_flow_cap_per_sec_per_cpu={cap} out of range \
             (0 to disable, else 1000..=10000000)",
        )));
    }
    Ok(())
}

fn validate_listener(i: usize, listener: &ListenerConfig) -> Result<(), ConfigError> {
    if listener.address.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} has an empty address"
        )));
    }
    let protocol = listener.protocol.trim();
    if protocol.is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} has an empty protocol"
        )));
    }
    match protocol {
        "tls" => validate_tls_listener(i, listener)?,
        "quic" => validate_quic_listener(i, listener)?,
        "h1s" => validate_h1s_listener(i, listener)?,
        "h1" => {
            // Plain HTTP/1.1 — must not declare TLS/QUIC blocks.
            if listener.tls.is_some() {
                return Err(ConfigError::Validation(format!(
                    "listener {i} has [listeners.tls] but protocol is \"h1\"; \
                     set protocol=\"h1s\" or remove the tls block"
                )));
            }
            if listener.quic.is_some() {
                return Err(ConfigError::Validation(format!(
                    "listener {i} has [listeners.quic] but protocol is \"h1\""
                )));
            }
        }
        "tcp" | "http" | "h2" | "h3" => {
            if listener.tls.is_some() {
                return Err(ConfigError::Validation(format!(
                    "listener {i} has [listeners.tls] but protocol is {protocol:?}; \
                     set protocol=\"tls\" or remove the tls block"
                )));
            }
            if listener.quic.is_some() {
                return Err(ConfigError::Validation(format!(
                    "listener {i} has [listeners.quic] but protocol is {protocol:?}; \
                     set protocol=\"quic\" or remove the quic block"
                )));
            }
        }
        other => {
            return Err(ConfigError::Validation(format!(
                "listener {i} has unknown protocol {other:?} \
                 (expected one of: tcp, tls, quic, h1, h1s, http, h2, h3)"
            )));
        }
    }
    validate_websocket_block(i, protocol, listener)?;
    validate_grpc_block(i, protocol, listener)?;
    validate_http_timeouts(i, listener)?;
    validate_backend_list(i, listener)?;
    // ROUND-8 OPS-10: per-listener drain budget override must satisfy
    // the same 100..=300_000 ms range as the gateway-level key.
    if let Some(t) = listener.drain_timeout_ms {
        if !(100..=300_000).contains(&t) {
            return Err(ConfigError::Validation(format!(
                "listener {i} drain_timeout_ms={t} out of range 100..=300000"
            )));
        }
    }
    // ROUND-8 OPS-02: per-listener jitter override must be in
    // 0..=effective-listener-drain-timeout. When the listener does
    // not override drain_timeout_ms the effective bound depends on
    // the [runtime] block (cross-checked in validate_config); here we
    // bound it by the per-listener override when present, else the
    // absolute 300_000 ms ceiling.
    if let Some(j) = listener.drain_jitter_ms {
        let upper = listener.drain_timeout_ms.unwrap_or(300_000);
        if j > upper {
            return Err(ConfigError::Validation(format!(
                "listener {i} drain_jitter_ms={j} exceeds the effective \
                 drain_timeout_ms={upper} (jitter must be <= drain budget)"
            )));
        }
    }
    Ok(())
}

fn validate_grpc_block(
    i: usize,
    protocol: &str,
    listener: &ListenerConfig,
) -> Result<(), ConfigError> {
    if listener.grpc.is_some() && !matches!(protocol, "h1s") {
        return Err(ConfigError::Validation(format!(
            "listener {i} has [listeners.grpc] but protocol is {protocol:?}; \
             gRPC requires protocol=\"h1s\" (HTTP/2 over TLS via ALPN)"
        )));
    }
    if let Some(grpc) = listener.grpc.as_ref() {
        if grpc.max_deadline_seconds == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} grpc.max_deadline_seconds must be > 0"
            )));
        }
    }
    Ok(())
}

fn validate_websocket_block(
    i: usize,
    protocol: &str,
    listener: &ListenerConfig,
) -> Result<(), ConfigError> {
    // WS-over-H1/H1s (RFC 6455) + WS-over-H2 (RFC 8441, on h1s via ALPN)
    // are H1/H1s listeners; WS-over-H3 (RFC 9220, SESSION 27) rides the
    // `quic` listener via H3 extended CONNECT. Any other protocol with a
    // websocket block is a misconfig.
    if listener.websocket.is_some() && !matches!(protocol, "h1" | "h1s" | "quic") {
        return Err(ConfigError::Validation(format!(
            "listener {i} has [listeners.websocket] but protocol is {protocol:?}; \
             WebSocket requires protocol=\"h1\", \"h1s\", or \"quic\""
        )));
    }
    if let Some(ws) = listener.websocket.as_ref() {
        if ws.idle_timeout_seconds == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} websocket.idle_timeout_seconds must be > 0"
            )));
        }
        if ws.max_message_size_bytes == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} websocket.max_message_size_bytes must be > 0"
            )));
        }
        if ws.ping_rate_limit_per_window == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} websocket.ping_rate_limit_per_window must be > 0"
            )));
        }
        if ws.ping_rate_limit_window_seconds == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} websocket.ping_rate_limit_window_seconds must be > 0"
            )));
        }
        if ws.read_frame_timeout_seconds == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} websocket.read_frame_timeout_seconds must be > 0"
            )));
        }
    }
    Ok(())
}

fn validate_http_timeouts(i: usize, listener: &ListenerConfig) -> Result<(), ConfigError> {
    if let Some(http) = listener.http.as_ref() {
        if http.header_timeout_ms == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} http.header_timeout_ms must be > 0"
            )));
        }
        if http.body_timeout_ms == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} http.body_timeout_ms must be > 0"
            )));
        }
        if http.total_timeout_ms == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} http.total_timeout_ms must be > 0"
            )));
        }
        if http.head_timeout_ms == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} http.head_timeout_ms must be > 0"
            )));
        }
    }
    Ok(())
}

fn validate_backend_list(i: usize, listener: &ListenerConfig) -> Result<(), ConfigError> {
    for (j, backend) in listener.backends.iter().enumerate() {
        if backend.address.trim().is_empty() {
            return Err(ConfigError::Validation(format!(
                "listener {i} backend {j} has an empty address"
            )));
        }
        match backend.protocol.as_str() {
            "tcp" | "h1" | "h2" | "h3" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "listener {i} backend {j} has unknown protocol {other:?} \
                     (expected one of: tcp, h1, h2, h3)"
                )));
            }
        }
        validate_backend_h3_tls(i, j, backend)?;
    }
    Ok(())
}

/// Validate the H3 backend TLS knobs (D4-4). Non-H3 backends are
/// unaffected; H3 backends must either supply a `tls_ca_path` or
/// explicitly opt out via `tls_verify_peer = false`.
fn validate_backend_h3_tls(i: usize, j: usize, backend: &BackendConfig) -> Result<(), ConfigError> {
    if backend.protocol != "h3" {
        if backend.tls_ca_path.is_some()
            || backend.tls_verify_hostname.is_some()
            || !backend.tls_verify_peer
        {
            return Err(ConfigError::Validation(format!(
                "listener {i} backend {j} sets tls_* knobs but protocol is {:?}; \
                 these knobs are only meaningful for protocol = \"h3\"",
                backend.protocol
            )));
        }
        return Ok(());
    }
    if backend.tls_verify_peer && backend.tls_ca_path.as_deref().is_none_or(str::is_empty) {
        return Err(ConfigError::Validation(format!(
            "listener {i} backend {j} (protocol=\"h3\") requires tls_ca_path \
             when tls_verify_peer is true; either set tls_ca_path or explicitly \
             opt out via tls_verify_peer = false (NOT RECOMMENDED)"
        )));
    }
    if let Some(sni) = backend.tls_verify_hostname.as_deref() {
        if sni.trim().is_empty() {
            return Err(ConfigError::Validation(format!(
                "listener {i} backend {j} tls_verify_hostname is empty"
            )));
        }
    }
    Ok(())
}

fn validate_tls_listener(i: usize, listener: &ListenerConfig) -> Result<(), ConfigError> {
    let tls = listener.tls.as_ref().ok_or_else(|| {
        ConfigError::Validation(format!(
            "listener {i} has protocol=tls but is missing [listeners.tls]"
        ))
    })?;
    if tls.cert_path.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} tls.cert_path is empty"
        )));
    }
    if tls.key_path.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} tls.key_path is empty"
        )));
    }
    if tls.ticket_rotation_interval_seconds == 0 {
        return Err(ConfigError::Validation(format!(
            "listener {i} tls.ticket_rotation_interval_seconds must be > 0"
        )));
    }
    if listener.quic.is_some() {
        return Err(ConfigError::Validation(format!(
            "listener {i} has [listeners.quic] but protocol is \"tls\""
        )));
    }
    Ok(())
}

fn validate_h1s_listener(i: usize, listener: &ListenerConfig) -> Result<(), ConfigError> {
    // h1s = HTTP/1.1 over TLS. Reuses the [listeners.tls] block.
    if listener.tls.is_none() {
        return Err(ConfigError::Validation(format!(
            "listener {i} has protocol=\"h1s\" but is missing [listeners.tls]"
        )));
    }
    if listener.quic.is_some() {
        return Err(ConfigError::Validation(format!(
            "listener {i} has [listeners.quic] but protocol is \"h1s\""
        )));
    }
    // Delegate to the TLS validator for cert/key path checks.
    validate_tls_listener(i, listener)
}

fn validate_quic_listener(i: usize, listener: &ListenerConfig) -> Result<(), ConfigError> {
    let quic = listener.quic.as_ref().ok_or_else(|| {
        ConfigError::Validation(format!(
            "listener {i} has protocol=quic but is missing [listeners.quic]"
        ))
    })?;
    if quic.cert_path.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} quic.cert_path is empty"
        )));
    }
    if quic.key_path.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} quic.key_path is empty"
        )));
    }
    if quic.retry_secret_path.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} quic.retry_secret_path is empty"
        )));
    }
    if quic.max_idle_timeout_ms == 0 {
        return Err(ConfigError::Validation(format!(
            "listener {i} quic.max_idle_timeout_ms must be > 0"
        )));
    }
    if quic.max_recv_udp_payload_size < 1_200 {
        return Err(ConfigError::Validation(format!(
            "listener {i} quic.max_recv_udp_payload_size must be >= 1200 (RFC 9000 §14)"
        )));
    }
    if listener.tls.is_some() {
        return Err(ConfigError::Validation(format!(
            "listener {i} has [listeners.tls] but protocol is \"quic\""
        )));
    }
    // F-S26-1: a QUIC listener is EITHER Mode B (terminate-and-
    // re-originate raw QUIC via `[listeners.quic.raw_proxy]`) OR
    // H3-terminate-and-forward (decode H3 → relay to `[[listeners.
    // backends]]`). Configuring both is a genuine conflict — raw_proxy
    // hands every accepted connection to the raw-proxy actor, so the
    // H3-terminate backend list would be silently ignored. Reject it at
    // startup rather than do something surprising.
    if quic.raw_proxy.is_some() && !listener.backends.is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} sets both [listeners.quic.raw_proxy] (Mode B raw-QUIC \
             proxy) and [[listeners.backends]] (H3-terminate forwarding); these \
             are mutually exclusive — remove one"
        )));
    }
    // F-S26-1: the H3-terminate forwarding path wires exactly ONE
    // backend protocol family onto the listener (the library's
    // `with_h2_backend` / `with_h3_backend` take a single address; the
    // H1 `with_backends` takes the resolved vector). The router's
    // dispatch precedence is h2 > h3 > h1, so a mixed-protocol backend
    // list would silently drop the lower-precedence backends. Require a
    // single family so an operator misconfig fails loudly at startup.
    if quic.raw_proxy.is_none() && !listener.backends.is_empty() {
        let mut saw_h1 = false;
        let mut saw_h2 = false;
        let mut saw_h3 = false;
        for b in &listener.backends {
            match b.protocol.as_str() {
                "tcp" | "h1" => saw_h1 = true,
                "h2" => saw_h2 = true,
                "h3" => saw_h3 = true,
                // Unknown protocols are already rejected by
                // `validate_backend_list`; nothing to do here.
                _ => {}
            }
        }
        let families = usize::from(saw_h1) + usize::from(saw_h2) + usize::from(saw_h3);
        if families > 1 {
            return Err(ConfigError::Validation(format!(
                "listener {i} (protocol=\"quic\", H3-terminate) mixes backend \
                 protocol families (h1/tcp, h2, h3); a QUIC listener forwards to \
                 exactly one backend protocol — split the listeners or pick one"
            )));
        }
    }
    Ok(())
}

/// S15 A2-8: validate the optional `[passthrough]` block.
///
/// Mirrors the shape of `validate_quic_listener` — clamp the
/// owner-ruling knobs so an operator typo (e.g.
/// `min_client_dcid_len = 0`) fails loudly at startup instead of
/// silently re-enabling the cross-flow prefix-collision attack
/// surface.
fn validate_passthrough(pt: &PassthroughConfig) -> Result<(), ConfigError> {
    if pt.backends.is_empty() {
        return Err(ConfigError::Validation(
            "passthrough.backends must be non-empty".into(),
        ));
    }
    // Owner ruling §9.4 — flow cap is meaningful only at >= 1; the
    // upper bound matches `RuntimeConfig::max_inflight_connections`'s
    // ceiling so the routing-table-entry cap (2 * cap) stays under
    // 4M entries even at the worst case.
    if !(1..=2_000_000).contains(&pt.max_quic_connections) {
        return Err(ConfigError::Validation(format!(
            "passthrough.max_quic_connections={} out of range 1..=2000000",
            pt.max_quic_connections
        )));
    }
    // Owner ruling §9.3 — anything below 8 re-opens the
    // CVE-2022-30592-style prefix-collision surface; above
    // `MAX_CID_LEN` (20, RFC 9000 §17.3) is impossible on the wire.
    if !(8..=20).contains(&pt.min_client_dcid_len) {
        return Err(ConfigError::Validation(format!(
            "passthrough.min_client_dcid_len={} out of range 8..=20",
            pt.min_client_dcid_len
        )));
    }
    if !(1..=8192).contains(&pt.per_flow_backlog) {
        return Err(ConfigError::Validation(format!(
            "passthrough.per_flow_backlog={} out of range 1..=8192",
            pt.per_flow_backlog
        )));
    }
    if pt.audit_throttle_window_secs == 0 {
        return Err(ConfigError::Validation(
            "passthrough.audit_throttle_window_secs must be > 0".into(),
        ));
    }
    if !(1..=20).contains(&pt.max_dcid_len_routed) {
        return Err(ConfigError::Validation(format!(
            "passthrough.max_dcid_len_routed={} out of range 1..=20",
            pt.max_dcid_len_routed
        )));
    }
    if pt.retry_secret_path.as_os_str().is_empty() {
        return Err(ConfigError::Validation(
            "passthrough.retry_secret_path is empty".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_toml() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"
"#;
        let config = parse_config(input).unwrap();
        assert_eq!(config.listeners.len(), 1);
        assert_eq!(config.listeners[0].address, "0.0.0.0:8080");
        assert_eq!(config.listeners[0].protocol, "tcp");
    }

    #[test]
    fn parse_invalid_toml() {
        let result = parse_config("not valid toml {{{{");
        assert!(result.is_err());
    }

    // S19 / Mode B (B6): a MINIMAL `[listeners.quic.raw_proxy]` block —
    // only `backend_addr` + `sni` — must deserialize with the two caps
    // defaulting to 1024 / 256. Covers `default_raw_proxy_dgram_queue_cap`
    // + `default_raw_proxy_max_relay_streams` (the serde-default helpers).
    #[test]
    fn raw_proxy_minimal_toml_defaults_caps() {
        // Deserialize a `QuicListenerConfig` whose `[raw_proxy]` sub-table
        // gives ONLY `backend_addr` + `sni` (the two caps + `backend_ca_path`
        // omitted), exactly as a minimal `[listeners.quic.raw_proxy]` block
        // would after TOML table-nesting resolves to this struct.
        let input = r#"
cert_path = "/c"
key_path = "/k"
retry_secret_path = "/r"

[raw_proxy]
backend_addr = "127.0.0.1:4443"
sni = "backend.test"
"#;
        let quic: QuicListenerConfig =
            toml::from_str(input).expect("minimal raw_proxy QuicListenerConfig must deserialize");
        let rp = quic
            .raw_proxy
            .expect("raw_proxy block present ⇒ Some after deserialize");
        assert_eq!(rp.backend_addr, "127.0.0.1:4443");
        assert_eq!(rp.sni, "backend.test");
        assert_eq!(
            rp.dgram_queue_cap,
            default_raw_proxy_dgram_queue_cap(),
            "omitted dgram_queue_cap must default via the serde helper"
        );
        assert_eq!(rp.dgram_queue_cap, 1024, "documented B4 default");
        assert_eq!(
            rp.max_relay_streams,
            default_raw_proxy_max_relay_streams(),
            "omitted max_relay_streams must default via the serde helper"
        );
        assert_eq!(rp.max_relay_streams, 256, "documented B5 default");
        // `backend_ca_path` is `#[serde(default)]` Option ⇒ None when omitted.
        assert!(
            rp.backend_ca_path.is_none(),
            "omitted backend_ca_path defaults to None"
        );
    }

    #[test]
    fn validate_empty_listeners() {
        let config = LbConfig {
            listeners: vec![],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_empty_address() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: String::new(),
                protocol: "tcp".into(),
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
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_ok() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "http".into(),
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
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_empty_backend_address() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "tcp".into(),
                tls: None,
                quic: None,
                alt_svc: None,
                http: None,
                h2_security: None,
                websocket: None,
                grpc: None,
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![BackendConfig {
                    address: String::new(),
                    protocol: "tcp".into(),
                    weight: 1,
                    tls_ca_path: None,
                    tls_verify_hostname: None,
                    tls_verify_peer: true,
                }],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn parse_config_with_backends() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"
weight = 2
"#;
        let config = parse_config(input).unwrap();
        assert_eq!(config.listeners.len(), 1);
        assert_eq!(config.listeners[0].backends.len(), 1);
        assert_eq!(config.listeners[0].backends[0].address, "127.0.0.1:3000");
        assert_eq!(config.listeners[0].backends[0].weight, 2);
    }

    #[test]
    fn parse_tls_listener() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:443"
protocol = "tls"

[listeners.tls]
cert_path = "/etc/expressgateway/tls/cert.pem"
key_path  = "/etc/expressgateway/tls/key.pem"

[[listeners.backends]]
address = "127.0.0.1:3000"
"#;
        let config = parse_config(input).unwrap();
        let tls = config.listeners[0].tls.as_ref().unwrap();
        assert_eq!(tls.cert_path, "/etc/expressgateway/tls/cert.pem");
        assert_eq!(tls.key_path, "/etc/expressgateway/tls/key.pem");
        assert_eq!(tls.ticket_rotation_interval_seconds, 86_400);
        assert_eq!(tls.ticket_rotation_overlap_seconds, 86_400);
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_tls_without_block_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "tls".into(),
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
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_unknown_protocol_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "ftp".into(),
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
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_tls_block_without_tls_protocol_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "tcp".into(),
                tls: Some(TlsConfig {
                    cert_path: "/x".into(),
                    key_path: "/y".into(),
                    ticket_rotation_interval_seconds: 86_400,
                    ticket_rotation_overlap_seconds: 86_400,
                }),
                quic: None,
                alt_svc: None,
                http: None,
                h2_security: None,
                websocket: None,
                grpc: None,
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_tls_empty_cert_path_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "tls".into(),
                tls: Some(TlsConfig {
                    cert_path: String::new(),
                    key_path: "/y".into(),
                    ticket_rotation_interval_seconds: 86_400,
                    ticket_rotation_overlap_seconds: 86_400,
                }),
                quic: None,
                alt_svc: None,
                http: None,
                h2_security: None,
                websocket: None,
                grpc: None,
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn parse_quic_listener() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:443"
protocol = "quic"

[listeners.quic]
cert_path = "/etc/expressgateway/tls/cert.pem"
key_path  = "/etc/expressgateway/tls/key.pem"
retry_secret_path = "/etc/expressgateway/quic/retry.key"

[[listeners.backends]]
address = "127.0.0.1:3000"
protocol = "h1"
"#;
        let config = parse_config(input).unwrap();
        let quic = config.listeners[0].quic.as_ref().unwrap();
        assert_eq!(quic.cert_path, "/etc/expressgateway/tls/cert.pem");
        assert_eq!(quic.max_idle_timeout_ms, 30_000);
        assert_eq!(quic.max_recv_udp_payload_size, 1_350);
        assert_eq!(config.listeners[0].backends[0].protocol, "h1");
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_quic_without_block_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "quic".into(),
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
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_quic_small_mtu_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "quic".into(),
                tls: None,
                quic: Some(QuicListenerConfig {
                    cert_path: "/x".into(),
                    key_path: "/y".into(),
                    retry_secret_path: "/z".into(),
                    max_idle_timeout_ms: 30_000,
                    max_recv_udp_payload_size: 500,
                    raw_proxy: None,
                }),
                alt_svc: None,
                http: None,
                h2_security: None,
                websocket: None,
                grpc: None,
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    // F-S26-1: a QUIC listener with a single h1 backend and NO raw_proxy
    // is the H3-terminate → H1 forwarding shape — it must VALIDATE (this
    // is the config the binary now wires; previously backends were
    // "allowed but ignored" on the quic path).
    #[test]
    fn validate_quic_h3_terminate_with_h1_backend_ok() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "quic".into(),
                tls: None,
                quic: Some(QuicListenerConfig {
                    cert_path: "/x".into(),
                    key_path: "/y".into(),
                    retry_secret_path: "/z".into(),
                    max_idle_timeout_ms: 30_000,
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
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "h1".into(),
                    weight: 1,
                    tls_ca_path: None,
                    tls_verify_hostname: None,
                    tls_verify_peer: true,
                }],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(
            validate_config(&config).is_ok(),
            "a quic H3-terminate listener with a single h1 backend must validate"
        );
    }

    // F-S26-1: a QUIC listener that sets BOTH a raw_proxy (Mode B) and
    // backends (H3-terminate forwarding) is a genuine conflict — reject.
    #[test]
    fn validate_quic_raw_proxy_with_backends_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "quic".into(),
                tls: None,
                quic: Some(QuicListenerConfig {
                    cert_path: "/x".into(),
                    key_path: "/y".into(),
                    retry_secret_path: "/z".into(),
                    max_idle_timeout_ms: 30_000,
                    max_recv_udp_payload_size: 1_350,
                    raw_proxy: Some(RawQuicProxyConfig {
                        backend_addr: "127.0.0.1:4443".into(),
                        sni: "backend.test".into(),
                        backend_ca_path: None,
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
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "h1".into(),
                    weight: 1,
                    tls_ca_path: None,
                    tls_verify_hostname: None,
                    tls_verify_peer: true,
                }],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        let err = validate_config(&config).unwrap_err();
        assert!(
            matches!(&err, ConfigError::Validation(m) if m.contains("mutually exclusive")),
            "raw_proxy + backends must be rejected as mutually exclusive, got: {err:?}"
        );
    }

    // F-S26-1: a QUIC listener whose H3-terminate backends mix protocol
    // families (h1 + h2) is ambiguous given the single-address h2/h3
    // forwarding API — reject so the misconfig fails loudly.
    #[test]
    fn validate_quic_h3_terminate_mixed_backend_families_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "quic".into(),
                tls: None,
                quic: Some(QuicListenerConfig {
                    cert_path: "/x".into(),
                    key_path: "/y".into(),
                    retry_secret_path: "/z".into(),
                    max_idle_timeout_ms: 30_000,
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
                backends: vec![
                    BackendConfig {
                        address: "127.0.0.1:3000".into(),
                        protocol: "h1".into(),
                        weight: 1,
                        tls_ca_path: None,
                        tls_verify_hostname: None,
                        tls_verify_peer: true,
                    },
                    BackendConfig {
                        address: "127.0.0.1:3001".into(),
                        protocol: "h2".into(),
                        weight: 1,
                        tls_ca_path: None,
                        tls_verify_hostname: None,
                        tls_verify_peer: true,
                    },
                ],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        let err = validate_config(&config).unwrap_err();
        assert!(
            matches!(&err, ConfigError::Validation(m) if m.contains("backend protocol families")),
            "mixed backend families on a quic listener must be rejected, got: {err:?}"
        );
    }

    #[test]
    fn validate_backend_unknown_protocol_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "tcp".into(),
                tls: None,
                quic: None,
                alt_svc: None,
                http: None,
                h2_security: None,
                websocket: None,
                grpc: None,
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "gopher".into(),
                    weight: 1,
                    tls_ca_path: None,
                    tls_verify_hostname: None,
                    tls_verify_peer: true,
                }],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn parse_h1_listener_with_alt_svc_and_timeouts() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:80"
protocol = "h1"

[listeners.alt_svc]
h3_port = 8443

[listeners.http]
header_timeout_ms = 5000

[[listeners.backends]]
address = "127.0.0.1:3000"
"#;
        let config = parse_config(input).unwrap();
        assert_eq!(config.listeners[0].protocol, "h1");
        let alt = config.listeners[0].alt_svc.as_ref().unwrap();
        assert_eq!(alt.h3_port, 8443);
        assert_eq!(alt.max_age, 3_600);
        let http = config.listeners[0].http.unwrap();
        assert_eq!(http.header_timeout_ms, 5_000);
        assert_eq!(http.body_timeout_ms, 30_000);
        assert_eq!(http.total_timeout_ms, 60_000);
        assert_eq!(http.head_timeout_ms, 60_000);
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_h1s_without_tls_block_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "h1s".into(),
                tls: None,
                quic: None,
                alt_svc: None,
                http: None,
                h2_security: None,
                websocket: None,
                grpc: None,
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "tcp".into(),
                    weight: 1,
                    tls_ca_path: None,
                    tls_verify_hostname: None,
                    tls_verify_peer: true,
                }],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        let err = validate_config(&config).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn validate_h1_with_tls_block_rejected() {
        // Plain "h1" must not carry a TLS block — that combination would
        // silently surprise an operator who meant "h1s".
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "h1".into(),
                tls: Some(TlsConfig {
                    cert_path: "/x".into(),
                    key_path: "/y".into(),
                    ticket_rotation_interval_seconds: 86_400,
                    ticket_rotation_overlap_seconds: 86_400,
                }),
                quic: None,
                alt_svc: None,
                http: None,
                h2_security: None,
                websocket: None,
                grpc: None,
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_h1s_with_tls_block_ok() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "h1s".into(),
                tls: Some(TlsConfig {
                    cert_path: "/etc/cert.pem".into(),
                    key_path: "/etc/key.pem".into(),
                    ticket_rotation_interval_seconds: 86_400,
                    ticket_rotation_overlap_seconds: 86_400,
                }),
                quic: None,
                alt_svc: Some(AltSvcConfig {
                    h3_port: 443,
                    max_age: 3_600,
                }),
                http: Some(HttpTimeoutsConfig::default()),
                h2_security: None,
                websocket: None,
                grpc: None,
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "tcp".into(),
                    weight: 1,
                    tls_ca_path: None,
                    tls_verify_hostname: None,
                    tls_verify_peer: true,
                }],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        validate_config(&config).unwrap();
    }

    #[test]
    fn validate_zero_http_timeout_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "h1".into(),
                tls: None,
                quic: None,
                alt_svc: None,
                http: Some(HttpTimeoutsConfig {
                    header_timeout_ms: 0,
                    body_timeout_ms: 30_000,
                    total_timeout_ms: 60_000,
                    head_timeout_ms: 60_000,
                }),
                h2_security: None,
                websocket: None,
                grpc: None,
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "tcp".into(),
                    weight: 1,
                    tls_ca_path: None,
                    tls_verify_hostname: None,
                    tls_verify_peer: true,
                }],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn parse_runtime_xdp_enabled() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:80"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"

[runtime]
xdp_enabled = true
xdp_interface = "eth0"
"#;
        let config = parse_config(input).unwrap();
        let rt = config.runtime.as_ref().unwrap();
        assert!(rt.xdp_enabled);
        assert_eq!(rt.xdp_interface.as_deref(), Some("eth0"));
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn runtime_xdp_enabled_without_interface_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "tcp".into(),
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
            }],
            runtime: Some(RuntimeConfig {
                xdp_enabled: true,
                xdp_interface: None,
                xdp_mode: XdpModeChoice::Auto,
                drain_timeout_ms: 10_000,
                readiness_settle_ms: 1_000,
                drain_jitter_ms: None,
                handshake_timeout_ms: 5_000,
                max_inflight_connections: 65_536,
                connect_timeout_ms: 5_000,
                per_ip_connection_cap: 1_024,
                tls: None,
                watchdog: None,
                header_underscore_policy: HeaderUnderscorePolicy::Reject,
                max_keepalive_requests: 100,
                xdp_new_flow_cap_per_sec_per_cpu: 125_000,
            }),
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        let err = validate_config(&config).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn runtime_xdp_disabled_does_not_require_interface() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "tcp".into(),
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
            }],
            runtime: Some(RuntimeConfig {
                xdp_enabled: false,
                xdp_interface: None,
                xdp_mode: XdpModeChoice::Auto,
                drain_timeout_ms: 10_000,
                readiness_settle_ms: 1_000,
                drain_jitter_ms: None,
                handshake_timeout_ms: 5_000,
                max_inflight_connections: 65_536,
                connect_timeout_ms: 5_000,
                per_ip_connection_cap: 1_024,
                tls: None,
                watchdog: None,
                header_underscore_policy: HeaderUnderscorePolicy::Reject,
                max_keepalive_requests: 100,
                xdp_new_flow_cap_per_sec_per_cpu: 125_000,
            }),
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        validate_config(&config).unwrap();
    }

    #[test]
    fn runtime_absent_keeps_parse_backward_compatible() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:80"
protocol = "tcp"
"#;
        let config = parse_config(input).unwrap();
        assert!(config.runtime.is_none());
        assert!(config.observability.is_none());
    }

    #[test]
    fn parse_observability_metrics_bind() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:80"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"

[observability]
metrics_bind = "127.0.0.1:9090"
"#;
        let config = parse_config(input).unwrap();
        let obs = config.observability.as_ref().unwrap();
        assert_eq!(obs.metrics_bind.as_deref(), Some("127.0.0.1:9090"));
        validate_config(&config).unwrap();
    }

    #[test]
    fn parse_h1_listener_with_websocket() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:80"
protocol = "h1"

[listeners.websocket]
idle_timeout_seconds = 30
max_message_size_bytes = 1048576

[[listeners.backends]]
address = "127.0.0.1:3000"
"#;
        let config = parse_config(input).unwrap();
        let ws = config.listeners[0].websocket.as_ref().unwrap();
        assert!(ws.enabled);
        assert_eq!(ws.idle_timeout_seconds, 30);
        assert_eq!(ws.max_message_size_bytes, 1_048_576);
        // CF-S27-2: WS-over-H2 (RFC 8441 extended CONNECT) is OFF unless the
        // operator explicitly sets `h2_extended_connect = true`. Omitting it
        // (as above) must default to `false`.
        assert!(
            !ws.h2_extended_connect,
            "h2_extended_connect must default to false (CF-S27-2: WS-over-H2 opt-in)"
        );
        validate_config(&config).unwrap();
    }

    #[test]
    fn parse_websocket_h2_extended_connect_opt_in() {
        // When the operator opts in, the flag round-trips as `true`.
        let input = r#"
[[listeners]]
address = "0.0.0.0:80"
protocol = "h1"

[listeners.websocket]
h2_extended_connect = true

[[listeners.backends]]
address = "127.0.0.1:3000"
"#;
        let config = parse_config(input).unwrap();
        let ws = config.listeners[0].websocket.as_ref().unwrap();
        assert!(
            ws.h2_extended_connect,
            "h2_extended_connect = true must parse as the opt-in"
        );
        validate_config(&config).unwrap();
    }

    #[test]
    fn validate_websocket_on_non_http_listener_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "tcp".into(),
                tls: None,
                quic: None,
                alt_svc: None,
                http: None,
                h2_security: None,
                websocket: Some(WebsocketConfig::default()),
                grpc: None,
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    // SESSION 27 / WS-over-H3 (RFC 9220): a `quic` listener may now carry
    // a `[listeners.websocket]` block (it was previously rejected — only
    // h1/h1s were allowed). h3_extended_connect defaults OFF.
    #[test]
    fn validate_websocket_on_quic_listener_ok() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:443"
protocol = "quic"

[listeners.quic]
cert_path = "/c"
key_path = "/k"
retry_secret_path = "/r"

[listeners.websocket]
"#;
        let config = parse_config(input).unwrap();
        let ws = config.listeners[0]
            .websocket
            .as_ref()
            .expect("websocket block present on quic listener");
        assert!(
            !ws.h3_extended_connect,
            "h3_extended_connect must default OFF (S27 gate, like h2_extended_connect)"
        );
        validate_config(&config)
            .expect("a quic listener with a websocket block must validate (WS-over-H3)");
    }

    // SESSION 27: `h3_extended_connect = true` round-trips as the opt-in
    // and the config validates.
    #[test]
    fn parse_websocket_h3_extended_connect_opt_in() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:443"
protocol = "quic"

[listeners.quic]
cert_path = "/c"
key_path = "/k"
retry_secret_path = "/r"

[listeners.websocket]
h3_extended_connect = true
"#;
        let config = parse_config(input).unwrap();
        let ws = config.listeners[0].websocket.as_ref().unwrap();
        assert!(
            ws.h3_extended_connect,
            "h3_extended_connect = true must parse as the opt-in"
        );
        assert!(
            !ws.h2_extended_connect,
            "the H2 gate is independent and stays OFF"
        );
        validate_config(&config).unwrap();
    }

    #[test]
    fn parse_h1s_listener_with_grpc() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:443"
protocol = "h1s"

[listeners.tls]
cert_path = "/etc/cert.pem"
key_path = "/etc/key.pem"

[listeners.grpc]
max_deadline_seconds = 60
health_synthesized = false

[[listeners.backends]]
address = "127.0.0.1:3000"
"#;
        let config = parse_config(input).unwrap();
        let grpc = config.listeners[0].grpc.as_ref().unwrap();
        assert!(grpc.enabled);
        assert_eq!(grpc.max_deadline_seconds, 60);
        assert!(!grpc.health_synthesized);
        validate_config(&config).unwrap();
    }

    #[test]
    fn validate_grpc_on_non_h1s_listener_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "h1".into(),
                tls: None,
                quic: None,
                alt_svc: None,
                http: None,
                h2_security: None,
                websocket: None,
                grpc: Some(GrpcListenerConfig::default()),
                drain_timeout_ms: None,
                drain_jitter_ms: None,
                backends: vec![],
            }],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&config).is_err());
    }

    // ── ROUND-8 OPS-10 / OPS-02: per-listener drain budget + jitter ──

    fn base_runtime() -> RuntimeConfig {
        RuntimeConfig {
            xdp_enabled: false,
            xdp_interface: None,
            xdp_mode: XdpModeChoice::Auto,
            drain_timeout_ms: 10_000,
            readiness_settle_ms: 1_000,
            drain_jitter_ms: None,
            handshake_timeout_ms: 5_000,
            max_inflight_connections: 65_536,
            connect_timeout_ms: 5_000,
            per_ip_connection_cap: 1_024,
            tls: None,
            watchdog: None,
            header_underscore_policy: HeaderUnderscorePolicy::Reject,
            max_keepalive_requests: 100,
            xdp_new_flow_cap_per_sec_per_cpu: 125_000,
        }
    }

    fn min_listener(addr: &str) -> ListenerConfig {
        ListenerConfig {
            address: addr.into(),
            protocol: "tcp".into(),
            tls: None,
            quic: None,
            alt_svc: None,
            http: None,
            h2_security: None,
            websocket: None,
            grpc: None,
            drain_timeout_ms: None,
            drain_jitter_ms: None,
            backends: vec![BackendConfig {
                address: "127.0.0.1:9000".into(),
                protocol: "tcp".into(),
                weight: 1,
                tls_ca_path: None,
                tls_verify_hostname: None,
                tls_verify_peer: true,
            }],
        }
    }

    #[test]
    fn ops10_override_takes_precedence_over_runtime() {
        let mut l = min_listener("0.0.0.0:443");
        l.drain_timeout_ms = Some(300_000);
        let rt = RuntimeConfig {
            drain_timeout_ms: 10_000,
            ..base_runtime()
        };
        // Per-listener override wins over the [runtime] default.
        assert_eq!(l.effective_drain_timeout_ms(Some(&rt)), 300_000);
        // No override → inherit [runtime].
        let l2 = min_listener("0.0.0.0:80");
        assert_eq!(l2.effective_drain_timeout_ms(Some(&rt)), 10_000);
        // No [runtime] block → lb-config default.
        assert_eq!(l2.effective_drain_timeout_ms(None), 10_000);
    }

    #[test]
    fn ops02_jitter_default_is_quarter_of_budget() {
        let l = min_listener("0.0.0.0:80");
        let rt = RuntimeConfig {
            drain_timeout_ms: 20_000,
            drain_jitter_ms: None,
            ..base_runtime()
        };
        // Derived: drain_timeout_ms / 4.
        assert_eq!(rt.effective_drain_jitter_ms(), 5_000);
        assert_eq!(l.effective_drain_jitter_ms(Some(&rt)), 5_000);
        // Explicit 0 disables jitter for the listener.
        let mut l0 = min_listener("0.0.0.0:81");
        l0.drain_jitter_ms = Some(0);
        assert_eq!(l0.effective_drain_jitter_ms(Some(&rt)), 0);
    }

    #[test]
    fn ops10_per_listener_timeout_range_rejected() {
        let mut l = min_listener("0.0.0.0:80");
        l.drain_timeout_ms = Some(50); // below 100 floor
        let cfg = LbConfig {
            listeners: vec![l],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn ops02_listener_jitter_exceeding_inherited_budget_rejected() {
        // Listener sets a big jitter but inherits a small [runtime]
        // budget — the validate_config cross-check must reject it.
        let mut l = min_listener("0.0.0.0:80");
        l.drain_jitter_ms = Some(9_000);
        let rt = RuntimeConfig {
            drain_timeout_ms: 5_000,
            ..base_runtime()
        };
        let cfg = LbConfig {
            listeners: vec![l],
            runtime: Some(rt),
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        let err = validate_config(&cfg).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn ops02_gateway_jitter_exceeding_budget_rejected() {
        let rt = RuntimeConfig {
            drain_timeout_ms: 5_000,
            drain_jitter_ms: Some(9_000),
            ..base_runtime()
        };
        let cfg = LbConfig {
            listeners: vec![min_listener("0.0.0.0:80")],
            runtime: Some(rt),
            observability: None,
            admin: None,
            security: None,
            passthrough: None,
        };
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn ops11_readiness_settle_default_is_kubelet_aligned() {
        // Regression guard for OPS-11: the default must be one full
        // kubelet probe period (10 s) + margin.
        assert_eq!(default_readiness_settle_ms(), 11_000);
        assert!(default_readiness_settle_ms() <= 30_000); // still in range
    }

    #[test]
    fn validate_observability_bad_bind_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "tcp".into(),
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
            }],
            runtime: None,
            observability: Some(ObservabilityConfig {
                metrics_bind: Some("not-an-address".into()),
            }),
            admin: None,
            security: None,
            passthrough: None,
        };
        let err = validate_config(&config).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    // S15 A2-8: PassthroughConfig validation tests.
    fn pt_min(bind: &str, backend: &str) -> PassthroughConfig {
        PassthroughConfig {
            bind_addr: bind.parse().unwrap(),
            backends: vec![backend.parse().unwrap()],
            retry_secret_path: std::path::PathBuf::from("/tmp/eg-pt-retry.bin"),
            max_quic_connections: default_passthrough_max_quic_connections(),
            min_client_dcid_len: default_passthrough_min_client_dcid_len(),
            per_flow_backlog: default_passthrough_per_flow_backlog(),
            strict_source_binding: false,
            audit_throttle_window_secs: default_passthrough_audit_throttle_window_secs(),
            max_dcid_len_routed: default_passthrough_max_dcid_len_routed(),
            mint_retry: default_passthrough_mint_retry(),
            flow_idle_timeout_ms: default_passthrough_flow_idle_timeout_ms(),
        }
    }

    #[test]
    fn passthrough_only_config_is_valid() {
        // Mode-A-only deployment: no [[listeners]] entries, just the
        // [passthrough] block. Verifies the `validate_config`
        // listeners-empty exemption (otherwise this would be rejected
        // as "no listeners").
        let cfg = LbConfig {
            listeners: vec![],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: Some(pt_min("0.0.0.0:4433", "127.0.0.1:5000")),
        };
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn passthrough_empty_backends_rejected() {
        let mut pt = pt_min("0.0.0.0:4433", "127.0.0.1:5000");
        pt.backends.clear();
        let cfg = LbConfig {
            listeners: vec![],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: Some(pt),
        };
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn passthrough_min_client_dcid_below_floor_rejected() {
        // Owner ruling §9.3: a config-time min_client_dcid_len below
        // 8 re-opens the cross-flow prefix-collision surface — must
        // fail loud, not silent.
        let mut pt = pt_min("0.0.0.0:4433", "127.0.0.1:5000");
        pt.min_client_dcid_len = 4;
        let cfg = LbConfig {
            listeners: vec![],
            runtime: None,
            observability: None,
            admin: None,
            security: None,
            passthrough: Some(pt),
        };
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn passthrough_defaults_match_owner_rulings() {
        assert_eq!(default_passthrough_max_quic_connections(), 100_000); // §9.4
        assert_eq!(default_passthrough_min_client_dcid_len(), 8); // §9.3
        assert_eq!(default_passthrough_per_flow_backlog(), 32); // §5.1
        assert_eq!(default_passthrough_audit_throttle_window_secs(), 60); // §6.2
        assert_eq!(default_passthrough_max_dcid_len_routed(), 20); // §3.3
    }

    #[test]
    fn parse_passthrough_block_round_trip() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"

[passthrough]
bind_addr = "0.0.0.0:4433"
backends = ["127.0.0.1:5000", "127.0.0.1:5001"]
retry_secret_path = "/var/run/eg-pt-retry.bin"
strict_source_binding = true
"#;
        let cfg = parse_config(input).expect("parse ok");
        let pt = cfg.passthrough.as_ref().expect("passthrough present");
        assert_eq!(pt.backends.len(), 2);
        assert!(pt.strict_source_binding);
        // Defaults flow through serde:
        assert_eq!(pt.max_quic_connections, 100_000);
        assert_eq!(pt.min_client_dcid_len, 8);
        assert!(validate_config(&cfg).is_ok());
    }
}
