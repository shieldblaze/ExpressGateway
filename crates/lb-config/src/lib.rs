//! Typed configuration structures, TOML parsing and validation.
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

/// Config hot-reload diff/partition — swappable vs restart-required.
pub mod reload;
pub use reload::{ReloadPlan, RestartRequiredChange, SwappableChange};

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
///
/// Every config struct here carries `#[serde(deny_unknown_fields)]`. With 88 `#[serde(default)]`
/// keys and no such guard, a misspelled key used to parse clean and the operator never learned
/// their override was ignored. Do not add `#[serde(flatten)]` to these structs — it is
/// incompatible with `deny_unknown_fields` and would silently reopen the hole.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LbConfig {
    /// Configured listeners.
    #[serde(default)]
    pub listeners: Vec<ListenerConfig>,
    /// Global runtime knobs (optional). When absent all defaults apply.
    #[serde(default)]
    pub runtime: Option<RuntimeConfig>,
    /// Admin listener settings; absent means no admin HTTP listener and an in-process registry.
    #[serde(default)]
    pub observability: Option<ObservabilityConfig>,
    /// `[admin]` auth block. ABSENT means every admin request is served unauthenticated, which is
    /// why the listener then refuses to bind a non-loopback address.
    #[serde(default)]
    pub admin: Option<AdminConfig>,
    /// `[security]` toggles; absent means the lenient RFC 9112 baseline.
    #[serde(default)]
    pub security: Option<SecurityConfig>,
    /// `[passthrough]` block: routes QUIC by Connection ID WITHOUT decrypting — no TLS state, no
    /// handshake. Independent of `[[listeners]]`; coexists with terminating listeners.
    #[serde(default)]
    pub passthrough: Option<PassthroughConfig>,
}

/// Process-wide HTTP security toggles, kept out of the per-listener blocks because they are
/// deployment policy rather than listener configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    /// Reject any `Transfer-Encoding` codec other than `chunked`. The lenient default accepts
    /// anything hyper can parse, which is a smuggle vector against permissive backends — but
    /// strict mode breaks legitimate `gzip, chunked` clients, so it is opt-in.
    #[serde(default)]
    pub strict_te: bool,
}

/// Bearer-token + bind policy for the admin listener. The token is a SHA-256 digest, never the
/// plaintext, and a configured token alone does NOT permit a non-loopback bind.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConfig {
    /// 64-hex SHA-256 of the bearer token. `None` disables auth entirely, which forces a
    /// loopback-only bind.
    #[serde(default)]
    pub api_token_hash: Option<String>,
    /// Allow a non-loopback admin bind. Requires `api_token_hash`, or the gateway refuses to start.
    #[serde(default)]
    pub allow_non_loopback: bool,
}

/// Admin HTTP listener (`/metrics`, `/healthz`). Loopback-only is the expected posture — there is
/// NO built-in mTLS.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityConfig {
    /// Admin listener bind address; `None` starts no listener.
    #[serde(default)]
    pub metrics_bind: Option<String>,
}

/// Process-wide runtime configuration. Every field is opt-in, so a config with no `[runtime]`
/// block keeps pure-userspace behaviour.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Attach the XDP program at startup. Needs `CAP_BPF` + `CAP_NET_ADMIN` and a compiled ELF;
    /// missing either WARNS and continues without XDP rather than failing.
    #[serde(default)]
    pub xdp_enabled: bool,
    /// Interface to attach XDP to; required when `xdp_enabled`.
    #[serde(default)]
    pub xdp_interface: Option<String>,
    /// XDP attach mode. `Auto` probes Drv-then-Skb; use `"native"` on production NICs so startup
    /// FAILS instead of silently degrading to 1-3 Mpps SKB mode.
    #[serde(default)]
    pub xdp_mode: XdpModeChoice,
    /// Graceful-drain budget on SIGTERM; tasks still running past it are ABORTED with a warn.
    /// Range 100..=300_000 ms.
    #[serde(default = "default_drain_timeout_ms")]
    pub drain_timeout_ms: u64,
    /// Settle window between flipping `/readyz` to 503 and starting the cancel. MUST exceed the
    /// upstream health-check interval or traffic keeps arriving at a draining pod — the 11 s
    /// default is one kubelet `periodSeconds: 10` plus margin. Range 0..=30_000 ms.
    #[serde(default = "default_readiness_settle_ms")]
    pub readiness_settle_ms: u64,
    /// Drain-cancel jitter ceiling. Without it every replica cancels at the same instant on a
    /// deploy-wide SIGTERM and the reconnect storm hits the shared upstream LB (Envoy hit this in
    /// production past 2-3 replicas). `None` derives `drain_timeout_ms / 4`; `Some(0)` disables
    /// jitter for deterministic testing. Range `0..=drain_timeout_ms`.
    #[serde(default)]
    pub drain_jitter_ms: Option<u64>,
    /// TLS handshake budget, capping accept-side slowloris. Range 100..=60_000 ms.
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
    /// Per-listener inflight cap; saturation sheds with a 503, or a silent close before ALPN.
    /// Range `100..=2_000_000`.
    #[serde(default = "default_max_inflight_connections")]
    pub max_inflight_connections: u32,
    /// Upstream dial budget, so a SYN black hole cannot monopolise a worker. Range `100..=60_000`.
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    /// Per-source-IP concurrent-connection cap. Saturation closes the socket WITHOUT a response —
    /// replying would make the cap an amplification lever. Range `1..=2_000_000`.
    #[serde(default = "default_per_ip_cap")]
    pub per_ip_connection_cap: u32,
    /// `[runtime.tls]` policy block; absent means the rustls default `&[&TLS12, &TLS13]`.
    #[serde(default)]
    pub tls: Option<RuntimeTlsConfig>,
    /// `[runtime.watchdog]` block. Absent leaves the proxies on NoopHooks: hyper's header timeout
    /// still bites, but the rate floor is dormant.
    #[serde(default)]
    pub watchdog: Option<RuntimeWatchdogConfig>,
    /// How to handle `_` in header names. An underscore is an AUTH-BYPASS primitive against
    /// backends that normalise `_` <-> `-` (Java middleware, some Python frameworks, SAP
    /// gateways), which is why both Envoy and nginx refuse to pass it through at the edge.
    #[serde(default)]
    pub header_underscore_policy: HeaderUnderscorePolicy,
    /// Requests (H1) / lifetime streams (H2) per keep-alive connection before a proactive close.
    /// Cloudflare added this to Pingora after per-connection accounting growth, TLS-session-age
    /// and FD-pinning pain at the edge.
    ///
    /// `0` disables the cap. Otherwise `1..=10_000_000` — the ceiling exists so a fat-fingered
    /// value cannot leave an operator believing they set a bound when they did not.
    #[serde(default = "default_max_keepalive_requests")]
    pub max_keepalive_requests: u32,
    /// Request streams per HTTP/3 connection before a GOAWAY + graceful recycle (RFC 9114 §5.2).
    ///
    /// Deliberately SEPARATE from [`Self::max_keepalive_requests`]: an H3 recycle pays a full
    /// QUIC+TLS handshake plus congestion-control ramp, so it is tuned an order higher.
    ///
    /// The cap is what bounds quiche's insert-only per-connection `StreamMap::collected` set. `0`
    /// disables it and RE-OPENS both the RSS-staircase leak and a single-connection DoS vector —
    /// only safe on trusted listeners. Otherwise `1..=10_000_000`.
    #[serde(default = "default_max_requests_per_h3_connection")]
    pub max_requests_per_h3_connection: u32,
    /// Per-CPU new-flow-rate cap for XDP SYN-flood mitigation (Katran `MAX_CONN_RATE`). Excess
    /// new flows skip the conntrack populate, so a unique-5-tuple spray cannot thrash the LRU and
    /// evict established connections.
    ///
    /// `0` disables it. Otherwise `1_000..=10_000_000`: below 1k/s/CPU normal traffic stops being
    /// CT-inserted and falls to the kernel stack instead of `XDP_TX`; above 10M/s/CPU is past NIC
    /// line rate. The gate is PER-PROCESS, so multi-replica deployments size it per replica.
    #[serde(default = "default_xdp_new_flow_cap_per_sec_per_cpu")]
    pub xdp_new_flow_cap_per_sec_per_cpu: u32,
}

/// Katran `MAX_CONN_RATE` parity; must stay in sync with the eBPF-side constant.
const fn default_xdp_new_flow_cap_per_sec_per_cpu() -> u32 {
    125_000
}

/// nginx-parity keep-alive request cap.
const fn default_max_keepalive_requests() -> u32 {
    100
}

/// Default H3 per-connection request cap; higher than the H1/H2 cap because a recycle costs a
/// full handshake. There is no upstream reference value — tokio-quiche ships uncapped.
const fn default_max_requests_per_h3_connection() -> u32 {
    1000
}

impl RuntimeConfig {
    /// Effective jitter ceiling: `drain_jitter_ms`, else `drain_timeout_ms / 4`.
    #[must_use]
    pub const fn effective_drain_jitter_ms(&self) -> u64 {
        match self.drain_jitter_ms {
            Some(j) => j,
            None => self.drain_timeout_ms / 4,
        }
    }
}

impl ListenerConfig {
    /// Effective drain budget: the per-listener override, else the gateway value, else the default.
    #[must_use]
    pub fn effective_drain_timeout_ms(&self, runtime: Option<&RuntimeConfig>) -> u64 {
        self.drain_timeout_ms.unwrap_or_else(|| {
            runtime.map_or_else(default_drain_timeout_ms, |r| r.drain_timeout_ms)
        })
    }

    /// Effective jitter ceiling: the per-listener override, else the gateway-derived value.
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

/// Policy for `_` in header names; mirrors Envoy `headers_with_underscores_action`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeaderUnderscorePolicy {
    /// 400 on any underscore-bearing header name. The default, matching Envoy edge practice.
    #[default]
    Reject,
    /// Drop underscore-bearing headers before forwarding, as nginx does by default.
    Drop,
    /// Pass them through verbatim — only safe when the backend does not normalise `_` to `-`.
    Allow,
}

/// Slowloris / slow-POST watchdog knobs; mirrors `lb_security::WatchdogConfig`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeWatchdogConfig {
    /// Header-phase deadline per request. Range `100..=60_000` ms.
    #[serde(default = "default_watchdog_header_deadline_ms")]
    pub header_deadline_ms: u64,
    /// Body-phase rate floor in B/s; `0` disables the check. Range `0..=10_000_000`.
    #[serde(default = "default_watchdog_body_progress_min_bps")]
    pub body_progress_min_bps: u64,
    /// Sweep cadence for fully stalled connections, which make no `progress` calls at all.
    /// Range `100..=60_000` ms.
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

/// Serde default for `RuntimeWatchdogConfig::header_deadline_ms`.
const fn default_watchdog_header_deadline_ms() -> u64 {
    5_000
}

/// Serde default for `RuntimeWatchdogConfig::body_progress_min_bps`.
const fn default_watchdog_body_progress_min_bps() -> u64 {
    64
}

/// Serde default for `RuntimeWatchdogConfig::sweep_interval_ms`.
const fn default_watchdog_sweep_interval_ms() -> u64 {
    1_000
}

/// Process-wide TLS policy, distinct from the per-listener `[listeners.tls]` cert/key paths.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTlsConfig {
    /// Restrict every TLS listener to TLS 1.3. This is a COMPLIANCE knob (PCI-DSS 4.0 §4.2.1.1,
    /// NIST SP 800-52r2), not a security gain — rustls's TLS 1.2 suites are downgrade-safe.
    #[serde(default)]
    pub tls13_only: bool,
}

/// Serde default for `RuntimeConfig::drain_timeout_ms`.
const fn default_drain_timeout_ms() -> u64 {
    10_000
}

/// Serde default for `RuntimeConfig::readiness_settle_ms`: one kubelet probe period (10 s) plus
/// margin. Anything under the probe interval lets a pod cancel connections while still listed
/// Ready in Endpoints, so new connections keep landing on it.
const fn default_readiness_settle_ms() -> u64 {
    11_000
}

/// Serde default for `RuntimeConfig::handshake_timeout_ms`; a healthy TLS 1.3 handshake is
/// <100 ms, so this bites only on stalled clients.
const fn default_handshake_timeout_ms() -> u64 {
    5_000
}

/// Serde default for `RuntimeConfig::max_inflight_connections`.
const fn default_max_inflight_connections() -> u32 {
    65_536
}

/// Serde default for `RuntimeConfig::connect_timeout_ms`; cuts the SYN-black-hole tail.
const fn default_connect_timeout_ms() -> u64 {
    5_000
}

/// Serde default for `RuntimeConfig::per_ip_connection_cap`.
const fn default_per_ip_cap() -> u32 {
    1_024
}

/// XDP attach-mode selector, using the kernel's own vocabulary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum XdpModeChoice {
    /// Drv, falling back to Skb; never Hw. The default, so veth dev boxes still work.
    #[default]
    Auto,
    /// Drv only — ABORTS startup rather than degrading to SKB, which costs 10-50x throughput.
    Native,
    /// Generic SKB mode only.
    Skb,
    /// Hardware offload (mlx5 / nfp); loud-fails on unsupported NICs.
    Hw,
}

/// Configuration for a single listener.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenerConfig {
    /// Bind address (e.g. `"0.0.0.0:8080"`).
    pub address: String,
    /// Protocol selector: `"tcp"`, `"tls"`, `"h1"`, `"h1s"`, or `"quic"`.
    ///
    /// There is NO `"h2"` or `"h3"` listener — H2 rides `"h1s"` via ALPN and H3 rides `"quic"`,
    /// so those tokens (and `"http"`) are rejected here. They ARE valid as
    /// [`BackendConfig::protocol`] values, which is a different axis.
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
    /// HTTP/2 security thresholds; only meaningful on `"h1s"`, where H2 is reached via ALPN.
    #[serde(default)]
    pub h2_security: Option<H2SecurityConfig>,
    /// WebSocket capability block. ABSENT means upgrades fall through to the plain HTTP path as
    /// a GET with unknown headers, not an explicit rejection.
    #[serde(default)]
    pub websocket: Option<WebsocketConfig>,
    /// gRPC capability block; `"h1s"` only, since gRPC needs the ALPN-negotiated H2. ABSENT
    /// sends gRPC down the H2→H1 forward path, which usually 502s a tonic client.
    #[serde(default)]
    pub grpc: Option<GrpcListenerConfig>,
    /// Per-listener drain budget override, inheriting `[runtime].drain_timeout_ms` when `None`.
    /// Raise it HERE for long-poll / gRPC bidi / SSE / WebSocket listeners rather than raising the
    /// gateway default, which would slow every short-request listener's restart. Range
    /// `100..=300_000` ms.
    #[serde(default)]
    pub drain_timeout_ms: Option<u64>,
    /// Per-listener jitter override; `None` inherits the derived gateway value, `Some(0)`
    /// disables jitter. Must be `0..=` the EFFECTIVE per-listener `drain_timeout_ms`.
    #[serde(default)]
    pub drain_jitter_ms: Option<u64>,
    /// Upstream backends to load-balance across.
    #[serde(default)]
    pub backends: Vec<BackendConfig>,
}

/// gRPC capability config (Item 3, PROMPT.md §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrpcListenerConfig {
    /// Master switch. Defaults to true when the block is present.
    #[serde(default = "default_grpc_enabled")]
    pub enabled: bool,
    /// Ceiling on `grpc-timeout`; larger client values are CLAMPED, not rejected.
    #[serde(default = "default_grpc_max_deadline")]
    pub max_deadline_seconds: u64,
    /// Serve `/grpc.health.v1.Health/Check` locally instead of forwarding it.
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

/// WebSocket capability config; an absent block means the listener accepts no upgrades at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebsocketConfig {
    /// Master switch, defaulting TRUE when the block is present, so an empty table enables WS.
    #[serde(default = "default_ws_enabled")]
    pub enabled: bool,
    /// Idle timeout (no frames in EITHER direction) before a `1001 Going Away` close.
    #[serde(default = "default_ws_idle_timeout")]
    pub idle_timeout_seconds: u64,
    /// Upper bound on a single incoming WebSocket message (bytes).
    /// Defaults to 16 MiB.
    #[serde(default = "default_ws_max_message_size")]
    pub max_message_size_bytes: usize,
    /// Client `Ping` frames per window before `Close 1008` and an upstream shutdown (WS-001).
    #[serde(default = "default_ws_ping_rate_limit_per_window")]
    pub ping_rate_limit_per_window: u32,
    /// Rolling window for the client-Ping rate limit, in seconds.
    #[serde(default = "default_ws_ping_rate_limit_window_seconds")]
    pub ping_rate_limit_window_seconds: u64,
    /// PER-DIRECTION read-frame watchdog bounding pinned-buffer dwell (WS-002). Distinct from
    /// `idle_timeout_seconds`, which needs BOTH directions silent.
    #[serde(default = "default_ws_read_frame_timeout_seconds")]
    pub read_frame_timeout_seconds: u64,
    /// RFC 8441 WebSocket-over-HTTP/2. OFF because the H2 upgraded-stream write path has no
    /// end-to-end backpressure — a non-reading client forces unbounded gateway memory (CF-S27-2).
    /// Trusted populations only. Does not affect WS over HTTP/1.1 or HTTP/3.
    #[serde(default)]
    pub h2_extended_connect: bool,
    /// RFC 9220 WebSocket-over-HTTP/3, off by default. A SEPARATE gate from
    /// [`Self::h2_extended_connect`] — different datapath, different backpressure story, so
    /// enabling one must not enable the other.
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
            h2_extended_connect: false,
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

/// HTTP/2 security thresholds. Deliberately mirrors
/// `lb_l7::h2_security::H2SecurityThresholds` instead of importing it, so lb-config stays free of
/// a hyper dependency — keep the two shapes in sync.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Keep-alive PING interval in ms; `0` disables keep-alive.
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

/// `Alt-Svc` injection: how a TLS-terminated H1 listener advertises its HTTP/3 endpoint.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpTimeoutsConfig {
    /// Budget for the request line + headers.
    #[serde(default = "default_header_timeout_ms")]
    pub header_timeout_ms: u64,
    /// Budget for draining the request body or awaiting upstream body bytes.
    #[serde(default = "default_body_timeout_ms")]
    pub body_timeout_ms: u64,
    /// Hard upper bound on total request lifetime.
    #[serde(default = "default_total_timeout_ms")]
    pub total_timeout_ms: u64,
    /// Fixed cap on the POST-UPLOAD head wait, separate from the Phase-A idle deadline derived
    /// from `body_timeout_ms`.
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

/// TLS listener configuration (rustls + `ring`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    /// Filesystem path to the PEM-encoded certificate chain.
    pub cert_path: String,
    /// Filesystem path to the PEM-encoded private key (PKCS#8 or SEC1).
    pub key_path: String,
    /// Session-ticket key rotation interval, in seconds.
    #[serde(default = "default_ticket_interval")]
    pub ticket_rotation_interval_seconds: u64,
    /// Grace period during which the previous ticket key still decrypts. Interval plus overlap is
    /// the true ticket lifetime.
    #[serde(default = "default_ticket_overlap")]
    pub ticket_rotation_overlap_seconds: u64,
}

const fn default_ticket_interval() -> u64 {
    86_400
}

const fn default_ticket_overlap() -> u64 {
    86_400
}

/// QUIC listener configuration (quiche + `BoringSSL`). `retry_secret_path` holds the 32-byte
/// stateless-retry HMAC key, auto-generated at mode 0600 on first boot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuicListenerConfig {
    /// Filesystem path to the PEM-encoded certificate chain.
    pub cert_path: String,
    /// Filesystem path to the PEM-encoded private key (PKCS#8 or SEC1).
    pub key_path: String,
    /// 32-byte retry-token signing key; auto-generated if absent.
    pub retry_secret_path: String,
    /// Connection idle timeout in milliseconds. Defaults to 30 seconds.
    #[serde(default = "default_quic_idle_timeout_ms")]
    pub max_idle_timeout_ms: u64,
    /// Maximum accepted UDP payload. MUST be ≥1200 (RFC 9000 §14); 1350 fits a 1500-byte MTU.
    #[serde(default = "default_quic_recv_udp_payload")]
    pub max_recv_udp_payload_size: u64,
    /// Mode B raw-QUIC proxy. PRESENCE of this block switches the listener from H3-termination to
    /// terminate-and-re-originate: two distinct `quiche::Connection`s relaying raw streams and
    /// datagrams, never a CID bridge. Absent keeps H3-terminate.
    #[serde(default)]
    pub raw_proxy: Option<RawQuicProxyConfig>,
}

const fn default_quic_idle_timeout_ms() -> u64 {
    30_000
}

const fn default_quic_recv_udp_payload() -> u64 {
    1_350
}

/// Mode B raw-QUIC backend. Every cap here is PRE-AUTH — it applies before the client is
/// authenticated — so the defaults are deliberately conservative.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawQuicProxyConfig {
    /// Upstream QUIC backend. An unparseable value FAILS startup rather than silently disabling
    /// Mode B.
    pub backend_addr: String,
    /// SNI presented to the upstream on the re-originated TLS handshake.
    pub sni: String,
    /// CA bundle for verifying the upstream cert. ABSENT DOES NOT DISABLE VERIFICATION —
    /// `verify_peer(true)` is engaged either way, falling back to system trust roots. There is
    /// deliberately no knob to turn verification off in Mode B.
    #[serde(default)]
    pub backend_ca_path: Option<String>,
    /// Per-direction DATAGRAM relay queue depth, also advertised to both peers. Over-cap is
    /// drop-newest, which is what stops a flooding peer growing relay memory unbounded.
    #[serde(default = "default_raw_proxy_dgram_queue_cap")]
    pub dgram_queue_cap: usize,
    /// Relay stream-table ceiling. Sits above the negotiated stream grant on purpose, so
    /// worst-case relay memory stays a hard constant even if `max_streams` is mis-set.
    #[serde(default = "default_raw_proxy_max_relay_streams")]
    pub max_relay_streams: usize,
}

/// Default DATAGRAM relay queue capacity; mirrors `lb_quic::raw_proxy::DGRAM_QUEUE_CAP`.
const fn default_raw_proxy_dgram_queue_cap() -> usize {
    1_024
}

/// Default relay stream-table ceiling; mirrors `lb_quic::raw_proxy::MAX_RELAY_STREAMS`.
const fn default_raw_proxy_max_relay_streams() -> usize {
    256
}

/// Mode A QUIC passthrough listener. Top-level rather than a `[[listeners]]` variant because it
/// is a parallel datapath: it cannot share a UDP port with a terminating QUIC listener and has no
/// cert/key or drain knobs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassthroughConfig {
    /// Bind address for the listener UDP socket.
    pub bind_addr: std::net::SocketAddr,
    /// Backend addresses, hashed by Maglev on every Initial. Must be non-empty.
    pub backends: Vec<std::net::SocketAddr>,
    /// 32-byte retry-secret file, generated at mode 0600 if missing.
    pub retry_secret_path: std::path::PathBuf,
    /// Concurrent QUIC flow cap; the routing table is bounded at `2 *` this.
    #[serde(default = "default_passthrough_max_quic_connections")]
    pub max_quic_connections: usize,
    /// Minimum accepted client DCID length — the defence against CVE-2022-30592-style cross-flow
    /// prefix collisions.
    #[serde(default = "default_passthrough_min_client_dcid_len")]
    pub min_client_dcid_len: usize,
    /// Per-flow datagram backlog; drop-newest when full.
    #[serde(default = "default_passthrough_per_flow_backlog")]
    pub per_flow_backlog: usize,
    /// Drop short-header packets whose 4-tuple has changed. Catches off-path spoofed-CID
    /// injection but BREAKS NAT-rebind path migration, so it defaults off for mobile clients.
    #[serde(default)]
    pub strict_source_binding: bool,
    /// Audit-log throttle window, in seconds.
    #[serde(default = "default_passthrough_audit_throttle_window_secs")]
    pub audit_throttle_window_secs: u64,
    /// DCID length tried first when no per-flow length is known; 20 is the RFC 9000 §17.3 max.
    #[serde(default = "default_passthrough_max_dcid_len_routed")]
    pub max_dcid_len_routed: usize,
    /// Mint stateless Retry on no-token Initials (Initial-flood defence).
    ///
    /// KNOWN INTEROP BREAK (CF-S15-PASSTHROUGH-RETRY-ODCID): with this on, real-quiche backends
    /// REJECT the post-Retry `original_destination_connection_id` transport param, because the
    /// LB-chosen new_scid hides the client's ODCID. RFC 9000 §17.2.5's token-embedded-ODCID
    /// "Retry Service" pattern is the fix and is not implemented. Setting `false` forwards
    /// no-token Initials verbatim and delegates flood defence to the backend.
    #[serde(default = "default_passthrough_mint_retry")]
    pub mint_retry: bool,
    /// Idle-flow reaper threshold in ms (F-S20-2). Passthrough cannot see the encrypted
    /// CONNECTION_CLOSE, so WITHOUT this sweep a closed connection's flow persists forever — the
    /// S20 soak measured flows 0→56k, fds→28k, RSS→331 MB, evicted=0. `0` disables it (LRU-only).
    #[serde(default = "default_passthrough_flow_idle_timeout_ms")]
    pub flow_idle_timeout_ms: u64,
}

/// Default passthrough flow cap; routing entries are bounded at `2 *` this.
const fn default_passthrough_max_quic_connections() -> usize {
    100_000
}

/// Minimum client DCID length — the CVE-2022-30592 prefix-collision defence floor.
const fn default_passthrough_min_client_dcid_len() -> usize {
    8
}

/// Per-flow datagram backlog between the recv loop and the forward task.
const fn default_passthrough_per_flow_backlog() -> usize {
    32
}

/// Audit-log throttle window, so a misbehaving peer cannot flood the audit stream.
const fn default_passthrough_audit_throttle_window_secs() -> u64 {
    60
}

/// Short-header DCID fast-path length; 20 is RFC 9000 §17.3's maximum.
const fn default_passthrough_max_dcid_len_routed() -> usize {
    20
}

/// Initial-flood defence defaults ON; see `PassthroughConfig::mint_retry` for the interop caveat.
const fn default_passthrough_mint_retry() -> bool {
    true
}

/// Idle-flow reaper default; the standard stateless-passthrough reclamation window.
const fn default_passthrough_flow_idle_timeout_ms() -> u64 {
    60_000
}

/// Configuration for a single upstream backend.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    /// Backend address (e.g. `"127.0.0.1:3000"`).
    pub address: String,
    /// Upstream wire protocol: `"tcp"`, `"h1"`, `"h2"`, or `"h3"`. A different axis from
    /// [`ListenerConfig::protocol`] — the tokens overlap but mean the upstream leg.
    #[serde(default = "default_backend_protocol")]
    pub protocol: String,
    /// Weight for weighted load-balancing algorithms (default 1).
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// CA bundle verifying an H3 backend's cert. Required for `protocol = "h3"` unless
    /// `tls_verify_peer = false`.
    #[serde(default)]
    pub tls_ca_path: Option<String>,
    /// SNI override for backend verification, for when the cert name differs from the dial
    /// address. H3 only; defaults to the host part of `address`.
    #[serde(default)]
    pub tls_verify_hostname: Option<String>,
    /// Verify the H3 backend's certificate. `false` disables peer-cert verification ENTIRELY and
    /// is only defensible when a separate mesh layer authenticates the underlay.
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
/// `ConfigError::TomlParse`.
pub fn parse_config(input: &str) -> Result<LbConfig, ConfigError> {
    let config: LbConfig = toml::from_str(input)?;
    Ok(config)
}

/// Validate a parsed configuration.
///
/// # Errors
///
/// `ConfigError::Validation`.
pub fn validate_config(config: &LbConfig) -> Result<(), ConfigError> {
    // Passthrough is an independent datapath, so `[passthrough]` with no `[[listeners]]` is a
    // valid Mode-A-only deployment.
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
    // `validate_listener` cannot see the runtime block, so a listener jitter larger than an
    // INHERITED smaller drain budget only surfaces here.
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
    // The floor stops `drain_timeout_ms = 1` collapsing the drain to a no-op; the ceiling keeps
    // SIGTERM-to-exit under systemd's 90 s TimeoutStopSec.
    if !(100..=300_000).contains(&rt.drain_timeout_ms) {
        return Err(ConfigError::Validation(format!(
            "runtime.drain_timeout_ms={} out of range 100..=300000",
            rt.drain_timeout_ms
        )));
    }
    // Jitter cannot exceed the budget it subdivides. `None` derives /4 and is in range by
    // construction.
    if let Some(j) = rt.drain_jitter_ms {
        if j > rt.drain_timeout_ms {
            return Err(ConfigError::Validation(format!(
                "runtime.drain_jitter_ms={j} exceeds runtime.drain_timeout_ms={} \
                 (jitter must be <= the drain budget)",
                rt.drain_timeout_ms
            )));
        }
    }
    // 0 skips the sleep; past 30 s the knob outlives the usual k8s terminationGracePeriodSeconds.
    if rt.readiness_settle_ms > 30_000 {
        return Err(ConfigError::Validation(format!(
            "runtime.readiness_settle_ms={} out of range 0..=30000",
            rt.readiness_settle_ms
        )));
    }
    // The floor stops a near-zero budget starving every TLS connect; the ceiling bounds slowloris
    // exposure.
    if !(100..=60_000).contains(&rt.handshake_timeout_ms) {
        return Err(ConfigError::Validation(format!(
            "runtime.handshake_timeout_ms={} out of range 100..=60000",
            rt.handshake_timeout_ms
        )));
    }
    // The floor stops the semaphore collapsing to a single-connection bottleneck; the ceiling
    // bounds its per-permit and per-waiter memory.
    if !(100..=2_000_000).contains(&rt.max_inflight_connections) {
        return Err(ConfigError::Validation(format!(
            "runtime.max_inflight_connections={} out of range 100..=2000000",
            rt.max_inflight_connections
        )));
    }
    // Same range as `handshake_timeout_ms`; both bound a stall that would pin a worker.
    if !(100..=60_000).contains(&rt.connect_timeout_ms) {
        return Err(ConfigError::Validation(format!(
            "runtime.connect_timeout_ms={} out of range 100..=60000",
            rt.connect_timeout_ms
        )));
    }
    // Zero would refuse every connection; the ceiling is shared with the listener cap.
    if !(1..=2_000_000).contains(&rt.per_ip_connection_cap) {
        return Err(ConfigError::Validation(format!(
            "runtime.per_ip_connection_cap={} out of range 1..=2000000",
            rt.per_ip_connection_cap
        )));
    }
    // The 10 MB/s rate-floor ceiling exists because higher values false-positive-evict slow
    // mobile uplinks.
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
    // Below 1k/s/CPU normal traffic stops being conntrack-inserted and falls to the kernel stack
    // instead of XDP_TX; above 10M/s/CPU is past NIC line rate. Out-of-range is a hard error
    // rather than a silent traffic blackhole.
    let cap = rt.xdp_new_flow_cap_per_sec_per_cpu;
    if cap != 0 && !(1_000..=10_000_000).contains(&cap) {
        return Err(ConfigError::Validation(format!(
            "runtime.xdp_new_flow_cap_per_sec_per_cpu={cap} out of range \
             (0 to disable, else 1000..=10000000)",
        )));
    }
    // `0` is the documented disable sentinel; the 10M ceiling rejects a fat-finger value that
    // would otherwise be an effectively-unlimited cap the operator believes is a bound.
    if rt.max_keepalive_requests != 0 && rt.max_keepalive_requests > 10_000_000 {
        return Err(ConfigError::Validation(format!(
            "runtime.max_keepalive_requests={} out of range \
             (0 to disable the cap, else 1..=10000000)",
            rt.max_keepalive_requests
        )));
    }
    // Same shape as `max_keepalive_requests`. `0` disables the H3 recycle and re-opens the
    // StreamMap::collected leak and single-connection DoS vector.
    if rt.max_requests_per_h3_connection != 0 && rt.max_requests_per_h3_connection > 10_000_000 {
        return Err(ConfigError::Validation(format!(
            "runtime.max_requests_per_h3_connection={} out of range \
             (0 to disable the recycle, else 1..=10000000)",
            rt.max_requests_per_h3_connection
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
        "tcp" => {
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
        // `http`, `h2` and `h3` are NEVER served as listener protocols — H2 rides `h1s` via ALPN
        // and H3 rides `quic`. Rejecting at config time moves what used to be a boot-time "no
        // runtime implementation" abort to where the operator can act on it.
        "http" | "h2" | "h3" => {
            return Err(ConfigError::Validation(format!(
                "listener {i} has protocol {protocol:?} which is not a served \
                 listener protocol; served protocols are: tcp, tls, h1, h1s, quic. \
                 HTTP/2 is served over the \"h1s\" listener via ALPN (h2 preferred, \
                 http/1.1 fallback); HTTP/3 is served over the \"quic\" listener. \
                 (\"http\"/\"h2\"/\"h3\" are valid BACKEND protocols but not listener \
                 protocols.)"
            )));
        }
        other => {
            return Err(ConfigError::Validation(format!(
                "listener {i} has unknown protocol {other:?} \
                 (expected one of: tcp, tls, h1, h1s, quic)"
            )));
        }
    }
    validate_websocket_block(i, protocol, listener)?;
    validate_grpc_block(i, protocol, listener)?;
    validate_http_timeouts(i, listener)?;
    validate_backend_list(i, listener)?;
    // Same range as the gateway-level key.
    if let Some(t) = listener.drain_timeout_ms {
        if !(100..=300_000).contains(&t) {
            return Err(ConfigError::Validation(format!(
                "listener {i} drain_timeout_ms={t} out of range 100..=300000"
            )));
        }
    }
    // Without a per-listener drain override the real bound depends on the [runtime] block, which
    // is only visible in `validate_config`; here the absolute ceiling is the best available.
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
    // WS rides h1/h1s (RFC 6455, and RFC 8441 via ALPN) or `quic` (RFC 9220); anything else
    // carrying a websocket block is a misconfig.
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

/// H3 backends must supply a `tls_ca_path` or explicitly opt out via `tls_verify_peer = false`.
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
    // raw_proxy hands every accepted connection to the raw-proxy actor, so an H3-terminate
    // backend list alongside it would be SILENTLY IGNORED.
    if quic.raw_proxy.is_some() && !listener.backends.is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} sets both [listeners.quic.raw_proxy] (Mode B raw-QUIC \
             proxy) and [[listeners.backends]] (H3-terminate forwarding); these \
             are mutually exclusive — remove one"
        )));
    }
    // Dispatch precedence is h2 > h3 > h1, so a mixed-family backend list would SILENTLY DROP
    // the lower-precedence backends.
    if quic.raw_proxy.is_none() && !listener.backends.is_empty() {
        let mut saw_h1 = false;
        let mut saw_h2 = false;
        let mut saw_h3 = false;
        for b in &listener.backends {
            match b.protocol.as_str() {
                "tcp" | "h1" => saw_h1 = true,
                "h2" => saw_h2 = true,
                "h3" => saw_h3 = true,
                // Already rejected by `validate_backend_list`.
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

/// Validate `[passthrough]`. The clamps exist so a typo like `min_client_dcid_len = 0` fails
/// loudly instead of silently re-opening the cross-flow prefix-collision surface.
fn validate_passthrough(pt: &PassthroughConfig) -> Result<(), ConfigError> {
    if pt.backends.is_empty() {
        return Err(ConfigError::Validation(
            "passthrough.backends must be non-empty".into(),
        ));
    }
    // Upper bound matches the inflight ceiling, keeping the `2 * cap` routing table under 4M.
    if !(1..=2_000_000).contains(&pt.max_quic_connections) {
        return Err(ConfigError::Validation(format!(
            "passthrough.max_quic_connections={} out of range 1..=2000000",
            pt.max_quic_connections
        )));
    }
    // Below 8 re-opens the CVE-2022-30592 prefix-collision surface; above 20 is impossible on
    // the wire (RFC 9000 §17.3).
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

    // A minimal raw_proxy block must still get both caps from their serde defaults.
    #[test]
    fn raw_proxy_minimal_toml_defaults_caps() {
        // Shaped exactly as TOML table-nesting resolves a minimal block into this struct.
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
        // `tcp` is a SERVED protocol; `http` would now be rejected.
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

    // The served listener set is tcp/tls/h1/h1s/quic; http/h2/h3 must be rejected at config time.

    #[test]
    fn validate_unserved_listener_protocol_rejected() {
        for proto in ["http", "h2", "h3"] {
            let input = format!(
                r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "{proto}"

[[listeners.backends]]
address = "127.0.0.1:3000"
"#
            );
            // Parses fine as a String; only validation rejects it.
            let cfg = parse_config(&input).expect("parse ok (protocol is a String)");
            let err = validate_config(&cfg).expect_err("unserved listener protocol must reject");
            assert!(
                matches!(&err, ConfigError::Validation(m)
                    if m.contains("not a served") && m.contains(proto)),
                "protocol {proto:?} must be rejected as unserved, got: {err:?}"
            );
        }
    }

    #[test]
    fn validate_served_listener_protocols_named_in_unserved_error() {
        // The error must name the served set, or the operator cannot act on it.
        let input = r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "h2"

[[listeners.backends]]
address = "127.0.0.1:3000"
"#;
        let cfg = parse_config(input).expect("parse ok");
        let err = validate_config(&cfg).unwrap_err();
        let msg = match &err {
            ConfigError::Validation(m) => m.clone(),
            ConfigError::TomlParse(_) => String::new(),
        };
        assert!(
            matches!(&err, ConfigError::Validation(_)),
            "expected a Validation error, got: {err:?}"
        );
        for served in ["tcp", "tls", "h1", "h1s", "quic"] {
            assert!(
                msg.contains(served),
                "served-protocol error must name {served:?}; got: {msg}"
            );
        }
        assert!(
            msg.contains("ALPN"),
            "must explain H2 is served via ALPN: {msg}"
        );
    }

    #[test]
    fn unknown_top_level_key_rejected() {
        // A misspelled top-level table used to be silently dropped.
        let input = r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"

bogus_top_level_key = true
"#;
        let err = parse_config(input).expect_err("unknown top-level key must reject");
        assert!(
            matches!(&err, ConfigError::TomlParse(_)),
            "unknown key is a parse-time (deny_unknown_fields) error, got: {err:?}"
        );
    }

    #[test]
    fn unknown_runtime_key_rejected() {
        // The headline gap: a typo'd knob used to parse clean and be ignored.
        let input = r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"

[runtime]
max_keepalv_requests = 5
"#;
        let err = parse_config(input).expect_err("typo'd runtime key must reject");
        assert!(
            matches!(&err, ConfigError::TomlParse(_)),
            "expected a TomlParse error, got: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("max_keepalv_requests") || msg.contains("unknown field"),
            "error should name the unknown field, got: {msg}"
        );
    }

    #[test]
    fn unknown_listener_key_rejected() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"
prtocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"
"#;
        let err = parse_config(input).expect_err("typo'd listener key must reject");
        assert!(matches!(&err, ConfigError::TomlParse(_)), "got: {err:?}");
    }

    #[test]
    fn unknown_nested_block_key_rejected() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:443"
protocol = "tls"

[listeners.tls]
cert_path = "/c"
key_path = "/k"
ticket_rotaton_interval_seconds = 100

[[listeners.backends]]
address = "127.0.0.1:3000"
"#;
        let err = parse_config(input).expect_err("typo'd nested key must reject");
        assert!(matches!(&err, ConfigError::TomlParse(_)), "got: {err:?}");
    }

    #[test]
    fn valid_config_still_parses_under_deny_unknown_fields() {
        // Regression guard: a previously-valid config must still parse byte-identically.
        let input = r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"
weight = 1

[[listeners.backends]]
address = "127.0.0.1:3001"
weight = 1

[runtime]
drain_timeout_ms = 30000
readiness_settle_ms = 1000
handshake_timeout_ms = 5000
max_inflight_connections = 65536
connect_timeout_ms = 5000
per_ip_connection_cap = 1024
max_keepalive_requests = 100
max_requests_per_h3_connection = 1000
header_underscore_policy = "reject"
"#;
        let cfg = parse_config(input).expect("valid config must parse");
        validate_config(&cfg).expect("valid config must validate");
        let rt = cfg.runtime.as_ref().expect("runtime present");
        assert_eq!(rt.max_keepalive_requests, 100);
        assert_eq!(rt.max_requests_per_h3_connection, 1000);
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

    // The H3-terminate → H1 forwarding shape must validate; backends used to be allowed-but-
    // ignored on the quic path.
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

    // Both raw_proxy and backends is a genuine conflict.
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

    // Mixed backend families are ambiguous against the single-address h2/h3 forwarding API.
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
        // A TLS block on plain "h1" almost certainly means the operator wanted "h1s".
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
                max_requests_per_h3_connection: 1000,
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
                max_requests_per_h3_connection: 1000,
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
        // WS-over-H2 must stay OFF unless explicitly enabled (CF-S27-2).
        assert!(
            !ws.h2_extended_connect,
            "h2_extended_connect must default to false (CF-S27-2: WS-over-H2 opt-in)"
        );
        validate_config(&config).unwrap();
    }

    #[test]
    fn parse_websocket_h2_extended_connect_opt_in() {
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

    // A `quic` listener may carry a websocket block; h3_extended_connect still defaults OFF.
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

    // The H3 opt-in round-trips and validates.
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
            max_requests_per_h3_connection: 1000,
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
        assert_eq!(l.effective_drain_timeout_ms(Some(&rt)), 300_000);
        let l2 = min_listener("0.0.0.0:80");
        assert_eq!(l2.effective_drain_timeout_ms(Some(&rt)), 10_000);
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
        assert_eq!(rt.effective_drain_jitter_ms(), 5_000);
        assert_eq!(l.effective_drain_jitter_ms(Some(&rt)), 5_000);
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
        // Only the `validate_config` cross-check can catch a listener jitter above an INHERITED
        // budget.
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
        // Regression guard: the default must exceed one kubelet probe period.
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
        // A Mode-A-only deployment has no `[[listeners]]`, so it needs the listeners-empty
        // exemption or it is rejected as "no listeners".
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
        // Below 8 re-opens the cross-flow prefix-collision surface; must fail loud.
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
        assert_eq!(pt.max_quic_connections, 100_000);
        assert_eq!(pt.min_client_dcid_len, 8);
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn h3_request_cap_defaults_to_1000_when_absent() {
        // Omitting the knob must serde-default to the safe recycling value.
        let input = r#"
[[listeners]]
address = "0.0.0.0:80"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"

[runtime]
drain_timeout_ms = 10000
"#;
        let cfg = parse_config(input).expect("parse ok");
        let rt = cfg.runtime.as_ref().expect("runtime present");
        assert_eq!(
            rt.max_requests_per_h3_connection, 1000,
            "the H3 request cap must serde-default to 1000 when absent"
        );
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn h3_request_cap_explicit_value_parses() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:80"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"

[runtime]
drain_timeout_ms = 10000
max_requests_per_h3_connection = 250
"#;
        let cfg = parse_config(input).expect("parse ok");
        let rt = cfg.runtime.as_ref().expect("runtime present");
        assert_eq!(rt.max_requests_per_h3_connection, 250);
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn h3_request_cap_zero_is_valid_disabled() {
        // `0` is an accepted sentinel even though it re-opens the leak/DoS vector.
        let rt = RuntimeConfig {
            max_requests_per_h3_connection: 0,
            ..base_runtime()
        };
        assert!(
            validate_runtime(&rt).is_ok(),
            "max_requests_per_h3_connection=0 (disabled) must validate"
        );
    }

    #[test]
    fn h3_request_cap_in_range_is_valid() {
        // Values above `u32::MAX` are a TOML type error and never reach validation.
        for v in [1u32, 100, 1000, 1_000_000, 10_000_000] {
            let rt = RuntimeConfig {
                max_requests_per_h3_connection: v,
                ..base_runtime()
            };
            assert!(
                validate_runtime(&rt).is_ok(),
                "max_requests_per_h3_connection={v} must validate"
            );
        }
    }

    #[test]
    fn h3_request_cap_above_ceiling_rejected() {
        // A fat-finger value must not read as a bound while being effectively unlimited.
        let rt = RuntimeConfig {
            max_requests_per_h3_connection: 10_000_001,
            ..base_runtime()
        };
        let err = validate_runtime(&rt).unwrap_err();
        assert!(
            matches!(&err, ConfigError::Validation(m)
                if m.contains("max_requests_per_h3_connection") && m.contains("out of range")),
            "above-ceiling H3 cap must be rejected, got: {err:?}"
        );
    }

    #[test]
    fn keepalive_requests_range_validated() {
        // `0` and `1..=10_000_000` accepted; above 10M rejected.
        for v in [0u32, 1, 100, 10_000_000] {
            let rt = RuntimeConfig {
                max_keepalive_requests: v,
                ..base_runtime()
            };
            assert!(
                validate_runtime(&rt).is_ok(),
                "max_keepalive_requests={v} must validate"
            );
        }
        let rt = RuntimeConfig {
            max_keepalive_requests: 10_000_001,
            ..base_runtime()
        };
        let err = validate_runtime(&rt).unwrap_err();
        assert!(
            matches!(&err, ConfigError::Validation(m)
                if m.contains("max_keepalive_requests") && m.contains("out of range")),
            "above-ceiling keepalive cap must be rejected, got: {err:?}"
        );
    }

    // Every shipped config MUST parse and validate — this is what catches a schema drift or a
    // new validation rule breaking a documented example.

    fn repo_config_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("config")
    }

    /// Parse + validate one shipped config. Uses `assert!` rather than `expect` because the
    /// crate denies `clippy::panic` even in tests and `expect_fun_call` rejects a formatted arg.
    fn assert_config_file_ok(path: &std::path::Path) {
        let label = path.display().to_string();
        let read = std::fs::read_to_string(path);
        assert!(read.is_ok(), "read {label}: {read:?}");
        let text = read.unwrap_or_default();
        let parsed = parse_config(&text);
        assert!(
            parsed.is_ok(),
            "{label} must parse under deny_unknown_fields: {parsed:?}"
        );
        let cfg = parsed.unwrap_or_default();
        let validated = validate_config(&cfg);
        assert!(validated.is_ok(), "{label} must validate: {validated:?}");
    }

    #[test]
    fn shipped_default_config_parses_and_validates() {
        assert_config_file_ok(&repo_config_dir().join("default.toml"));
    }

    #[test]
    fn shipped_example_configs_parse_and_validate() {
        let dir = repo_config_dir().join("examples");
        let entries =
            std::fs::read_dir(&dir).expect("config/examples must be a readable directory");
        let mut seen = 0usize;
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            seen += 1;
            assert_config_file_ok(&path);
        }
        // An empty or missing examples dir would otherwise pass vacuously.
        assert!(
            seen >= 7,
            "expected at least 7 example configs, found {seen}"
        );
    }
}
