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
#![warn(clippy::pedantic, clippy::nursery)]
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
}

impl Default for WebsocketConfig {
    fn default() -> Self {
        Self {
            enabled: default_ws_enabled(),
            idle_timeout_seconds: default_ws_idle_timeout(),
            max_message_size_bytes: default_ws_max_message_size(),
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
}

impl Default for HttpTimeoutsConfig {
    fn default() -> Self {
        Self {
            header_timeout_ms: default_header_timeout_ms(),
            body_timeout_ms: default_body_timeout_ms(),
            total_timeout_ms: default_total_timeout_ms(),
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
}

const fn default_quic_idle_timeout_ms() -> u64 {
    30_000
}

const fn default_quic_recv_udp_payload() -> u64 {
    1_350
}

/// Configuration for a single upstream backend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackendConfig {
    /// Backend address (e.g. `"127.0.0.1:3000"`).
    pub address: String,
    /// Wire protocol spoken to this backend. Defaults to `"tcp"`.
    /// Values accepted: `"tcp"` (raw stream, used by the plain-TCP and
    /// TLS-over-TCP listeners), `"h1"` (HTTP/1.1 over TCP — the QUIC
    /// listener's default bridge target in Pillar 3b.3c-2), `"h3"`
    /// (HTTP/3 over QUIC — consumed by the Pillar 3b.3c-3 upstream
    /// pool).
    #[serde(default = "default_backend_protocol")]
    pub protocol: String,
    /// Weight for weighted load-balancing algorithms (default 1).
    #[serde(default = "default_weight")]
    pub weight: u32,
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
    if config.listeners.is_empty() {
        return Err(ConfigError::Validation(
            "at least one listener is required".into(),
        ));
    }
    for (i, listener) in config.listeners.iter().enumerate() {
        validate_listener(i, listener)?;
    }
    if let Some(rt) = config.runtime.as_ref() {
        validate_runtime(rt)?;
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
    if listener.websocket.is_some() && !matches!(protocol, "h1" | "h1s") {
        return Err(ConfigError::Validation(format!(
            "listener {i} has [listeners.websocket] but protocol is {protocol:?}; \
             WebSocket requires protocol=\"h1\" or \"h1s\""
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
            "tcp" | "h1" | "h3" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "listener {i} backend {j} has unknown protocol {other:?} \
                     (expected one of: tcp, h1, h3)"
                )));
            }
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

    #[test]
    fn validate_empty_listeners() {
        let config = LbConfig {
            listeners: vec![],
            runtime: None,
            observability: None,
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
                backends: vec![],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![BackendConfig {
                    address: String::new(),
                    protocol: "tcp".into(),
                    weight: 1,
                }],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![],
            }],
            runtime: None,
            observability: None,
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
                }),
                alt_svc: None,
                http: None,
                h2_security: None,
                websocket: None,
                grpc: None,
                backends: vec![],
            }],
            runtime: None,
            observability: None,
        };
        assert!(validate_config(&config).is_err());
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
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "gopher".into(),
                    weight: 1,
                }],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "tcp".into(),
                    weight: 1,
                }],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "tcp".into(),
                    weight: 1,
                }],
            }],
            runtime: None,
            observability: None,
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
                }),
                h2_security: None,
                websocket: None,
                grpc: None,
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "tcp".into(),
                    weight: 1,
                }],
            }],
            runtime: None,
            observability: None,
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
                backends: vec![],
            }],
            runtime: Some(RuntimeConfig {
                xdp_enabled: true,
                xdp_interface: None,
            }),
            observability: None,
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
                backends: vec![],
            }],
            runtime: Some(RuntimeConfig {
                xdp_enabled: false,
                xdp_interface: None,
            }),
            observability: None,
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
                backends: vec![],
            }],
            runtime: None,
            observability: None,
        };
        assert!(validate_config(&config).is_err());
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
                backends: vec![],
            }],
            runtime: None,
            observability: None,
        };
        assert!(validate_config(&config).is_err());
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
                backends: vec![],
            }],
            runtime: None,
            observability: Some(ObservabilityConfig {
                metrics_bind: Some("not-an-address".into()),
            }),
        };
        let err = validate_config(&config).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }
}
