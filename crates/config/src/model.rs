//! Configuration model types for ExpressGateway.
//!
//! Every struct derives `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`
//! and provides sensible production defaults via the `Default` trait.
//!
//! Enum fields use `#[serde(rename_all = "snake_case")]` so that the TOML
//! representation uses lowercase underscore names (e.g. `"round_robin"`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// String-typed enum helpers
// ---------------------------------------------------------------------------

/// Environment preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Production,
    Development,
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Production => f.write_str("production"),
            Self::Development => f.write_str("development"),
        }
    }
}

/// Log level (mirrors `tracing::Level` names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        };
        f.write_str(s)
    }
}

/// I/O backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IoBackend {
    Auto,
    IoUring,
    Epoll,
}

impl fmt::Display for IoBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::IoUring => f.write_str("io_uring"),
            Self::Epoll => f.write_str("epoll"),
        }
    }
}

/// XDP attach mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XdpMode {
    Driver,
    Generic,
}

impl fmt::Display for XdpMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Driver => f.write_str("driver"),
            Self::Generic => f.write_str("generic"),
        }
    }
}

/// TLS profile preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsProfile {
    /// TLS 1.3 only.
    Modern,
    /// TLS 1.2 + 1.3.
    Intermediate,
}

impl fmt::Display for TlsProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Modern => f.write_str("modern"),
            Self::Intermediate => f.write_str("intermediate"),
        }
    }
}

/// Mutual TLS mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutualTlsMode {
    NotRequired,
    Optional,
    Required,
}

impl fmt::Display for MutualTlsMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRequired => f.write_str("not_required"),
            Self::Optional => f.write_str("optional"),
            Self::Required => f.write_str("required"),
        }
    }
}

/// Listener protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenerProtocol {
    Http,
    Https,
    Tcp,
    Udp,
}

impl fmt::Display for ListenerProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http => f.write_str("http"),
            Self::Https => f.write_str("https"),
            Self::Tcp => f.write_str("tcp"),
            Self::Udp => f.write_str("udp"),
        }
    }
}

/// Load-balancing strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LbStrategy {
    RoundRobin,
    LeastConnections,
    IpHash,
    Random,
    Weighted,
}

impl fmt::Display for LbStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RoundRobin => f.write_str("round_robin"),
            Self::LeastConnections => f.write_str("least_connections"),
            Self::IpHash => f.write_str("ip_hash"),
            Self::Random => f.write_str("random"),
            Self::Weighted => f.write_str("weighted"),
        }
    }
}

/// PROXY protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyProtoMode {
    Off,
    V1,
    V2,
}

/// PROXY protocol inbound mode (adds `auto`-detect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyProtoInbound {
    Off,
    V1,
    V2,
    Auto,
}

/// Security filter mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode {
    Allowlist,
    Denylist,
}

/// Action when a rate-limit is exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitAction {
    Drop,
    Reset,
    Tarpit,
}

/// Compression algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressionAlgorithm {
    Gzip,
    Brotli,
    Zstd,
}

impl fmt::Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gzip => f.write_str("gzip"),
            Self::Brotli => f.write_str("brotli"),
            Self::Zstd => f.write_str("zstd"),
        }
    }
}

/// HTTP method for health-check probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Options,
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Get => f.write_str("GET"),
            Self::Head => f.write_str("HEAD"),
            Self::Post => f.write_str("POST"),
            Self::Put => f.write_str("PUT"),
            Self::Delete => f.write_str("DELETE"),
            Self::Options => f.write_str("OPTIONS"),
        }
    }
}

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

/// Top-level gateway configuration.
///
/// Each section controls a different subsystem.  The default values are
/// suitable for a single-node development setup; production deployments
/// should tune transport, buffer, and security settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Global settings (environment, logging, metrics).
    pub global: GlobalConfig,
    /// Async runtime tuning.
    pub runtime: RuntimeConfig,
    /// TCP/socket transport options.
    pub transport: TransportConfig,
    /// Memory buffer pool sizing.
    pub buffer: BufferConfig,
    /// TLS termination and origination.
    pub tls: TlsConfig,
    /// Frontend listeners.
    pub listeners: Vec<ListenerConfig>,
    /// Backend clusters.
    pub clusters: Vec<ClusterConfig>,
    /// Routing rules that map requests to clusters.
    pub routes: Vec<RouteConfig>,
    /// HTTP/1.1 and HTTP/2 settings.
    pub http: HttpConfig,
    /// PROXY protocol inbound/outbound settings.
    pub proxy_protocol: ProxyProtocolConfig,
    /// Health-check tuning.
    pub health_check: HealthCheckGroupConfig,
    /// Circuit-breaker thresholds.
    pub circuit_breaker: CircuitBreakerCfg,
    /// Security: IP allow/deny, rate-limiting.
    pub security: SecurityConfig,
    /// Control-plane gRPC/REST settings.
    pub controlplane: ControlPlaneConfig,
    /// Graceful shutdown behaviour.
    pub graceful_shutdown: GracefulShutdownConfig,
    /// Backend retry policy.
    pub retry: RetryConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            global: GlobalConfig::default(),
            runtime: RuntimeConfig::default(),
            transport: TransportConfig::default(),
            buffer: BufferConfig::default(),
            tls: TlsConfig::default(),
            listeners: vec![ListenerConfig::default()],
            clusters: vec![ClusterConfig::default()],
            routes: vec![RouteConfig::default()],
            http: HttpConfig::default(),
            proxy_protocol: ProxyProtocolConfig::default(),
            health_check: HealthCheckGroupConfig::default(),
            circuit_breaker: CircuitBreakerCfg::default(),
            security: SecurityConfig::default(),
            controlplane: ControlPlaneConfig::default(),
            graceful_shutdown: GracefulShutdownConfig::default(),
            retry: RetryConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Global
// ---------------------------------------------------------------------------

/// Global gateway settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    /// `Production` or `Development`.
    pub environment: Environment,
    /// Log level.
    pub log_level: LogLevel,
    /// Bind address for the Prometheus metrics endpoint.
    pub metrics_bind: String,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            environment: Environment::Production,
            log_level: LogLevel::Info,
            metrics_bind: "0.0.0.0:9090".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// Async runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// I/O backend selection.
    pub backend: IoBackend,
    /// Worker thread count; 0 means use all available CPUs.
    pub worker_threads: usize,
    /// Enable XDP acceleration.
    pub xdp_enabled: bool,
    /// Network interface for XDP (e.g. `"eth0"`).
    pub xdp_interface: String,
    /// XDP attach mode.
    pub xdp_mode: XdpMode,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            backend: IoBackend::Auto,
            worker_threads: 0,
            xdp_enabled: false,
            xdp_interface: String::new(),
            xdp_mode: XdpMode::Driver,
        }
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// TCP/socket transport settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransportConfig {
    /// SO_RCVBUF size in bytes.
    pub recv_buf_size: usize,
    /// SO_SNDBUF size in bytes.
    pub send_buf_size: usize,
    /// TCP_NODELAY (disable Nagle).
    pub tcp_nodelay: bool,
    /// TCP_QUICKACK (Linux).
    pub tcp_quickack: bool,
    /// SO_KEEPALIVE.
    pub tcp_keepalive: bool,
    /// TCP_FASTOPEN.
    pub tcp_fastopen: bool,
    /// TCP_FASTOPEN queue length.
    pub tcp_fastopen_queue: u32,
    /// Listen backlog depth.
    pub so_backlog: u32,
    /// SO_REUSEPORT.
    pub so_reuseport: bool,
    /// Backend connect timeout in milliseconds.
    pub connect_timeout_ms: u64,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            recv_buf_size: 262_144,
            send_buf_size: 262_144,
            tcp_nodelay: true,
            tcp_quickack: true,
            tcp_keepalive: true,
            tcp_fastopen: true,
            tcp_fastopen_queue: 256,
            so_backlog: 50_000,
            so_reuseport: true,
            connect_timeout_ms: 10_000,
        }
    }
}

// ---------------------------------------------------------------------------
// Buffer
// ---------------------------------------------------------------------------

/// Memory buffer-pool settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BufferConfig {
    /// Base page size in bytes.
    pub page_size: usize,
    /// Maximum order for the buddy allocator.
    pub max_order: u32,
    /// Thread-local small-object cache capacity.
    pub small_cache_size: usize,
    /// Thread-local normal-object cache capacity.
    pub normal_cache_size: usize,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            page_size: 16_384,
            max_order: 11,
            small_cache_size: 256,
            normal_cache_size: 64,
        }
    }
}

// ---------------------------------------------------------------------------
// TLS
// ---------------------------------------------------------------------------

/// TLS termination and origination settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    /// Whether TLS is enabled at all.
    pub enabled: bool,
    /// TLS profile preset.
    pub profile: TlsProfile,
    /// TLS handshake timeout in milliseconds.
    pub handshake_timeout_ms: u64,
    /// Session ticket lifetime in seconds.
    pub session_timeout_s: u64,
    /// Server-side session cache size.
    pub session_cache_size: usize,
    /// Server-side TLS settings.
    pub server: TlsServerConfig,
    /// Client-side (backend) TLS settings.
    pub client: TlsClientConfig,
    /// Per-hostname certificate overrides for SNI.
    pub sni_certs: Vec<SniCertConfig>,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: TlsProfile::Intermediate,
            handshake_timeout_ms: 10_000,
            session_timeout_s: 3600,
            session_cache_size: 20_000,
            server: TlsServerConfig::default(),
            client: TlsClientConfig::default(),
            sni_certs: Vec::new(),
        }
    }
}

/// Server-side TLS configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsServerConfig {
    /// Path to the default PEM certificate.
    pub default_cert: String,
    /// Path to the default PEM private key.
    pub default_key: String,
    /// Mutual TLS mode.
    pub mutual_tls: MutualTlsMode,
    /// Path to the trust CA bundle for mTLS.
    pub trust_ca: String,
    /// Path to a CRL file.
    pub crl_file: String,
}

impl Default for TlsServerConfig {
    fn default() -> Self {
        Self {
            default_cert: String::new(),
            default_key: String::new(),
            mutual_tls: MutualTlsMode::NotRequired,
            trust_ca: String::new(),
            crl_file: String::new(),
        }
    }
}

/// Client-side (backend origination) TLS configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsClientConfig {
    /// Verify the backend's hostname against its certificate.
    pub verify_hostname: bool,
    /// Client-side session cache lifetime in seconds.
    pub session_timeout_s: u64,
}

impl Default for TlsClientConfig {
    fn default() -> Self {
        Self {
            verify_hostname: true,
            session_timeout_s: 3600,
        }
    }
}

/// Per-hostname SNI certificate entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SniCertConfig {
    /// SNI hostname to match.
    pub hostname: String,
    /// Path to the PEM certificate.
    pub cert: String,
    /// Path to the PEM private key.
    pub key: String,
    /// Enable OCSP stapling for this certificate.
    pub ocsp_stapling: bool,
}

// ---------------------------------------------------------------------------
// Listener
// ---------------------------------------------------------------------------

/// A frontend listener that accepts connections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListenerConfig {
    /// Human-readable name.
    pub name: String,
    /// Protocol.
    pub protocol: ListenerProtocol,
    /// Socket bind address (e.g. `"0.0.0.0:8080"`).
    pub bind: String,
    /// Optional TLS profile override.
    pub tls_profile: Option<TlsProfile>,
    /// Accepted HTTP versions (e.g. `["h1", "h2"]`).
    pub http_versions: Option<Vec<String>>,
    /// Whether this listener uses XDP acceleration.
    pub xdp_accelerated: Option<bool>,
    /// Alt-Svc max-age for HTTP/3 advertisement.
    pub alt_svc_max_age: Option<u32>,
    /// Per-listener I/O backend override.
    /// When `None`, uses the global `runtime.backend` setting.
    pub io_backend: Option<IoBackend>,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            name: "default".into(),
            protocol: ListenerProtocol::Http,
            bind: "0.0.0.0:8080".into(),
            tls_profile: None,
            http_versions: None,
            xdp_accelerated: None,
            alt_svc_max_age: None,
            io_backend: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Cluster
// ---------------------------------------------------------------------------

/// A group of backend nodes behind a load-balancing strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClusterConfig {
    /// Unique cluster name referenced by routes.
    pub name: String,
    /// Load-balancing strategy.
    pub lb_strategy: LbStrategy,
    /// Maximum concurrent connections per node.
    pub max_connections_per_node: Option<u64>,
    /// Drain timeout in seconds before forcibly closing connections.
    pub drain_timeout_s: Option<u64>,
    /// Backend nodes belonging to this cluster.
    pub nodes: Vec<NodeConfig>,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            name: "default".into(),
            lb_strategy: LbStrategy::RoundRobin,
            max_connections_per_node: Some(10_000),
            drain_timeout_s: Some(30),
            nodes: vec![NodeConfig::default()],
        }
    }
}

/// A single backend node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeConfig {
    /// Socket address (e.g. `"127.0.0.1:8081"`).
    pub address: String,
    /// Relative weight for weighted strategies (must be >= 1).
    pub weight: Option<u32>,
    /// Per-node connection limit.
    pub max_connections: Option<u64>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:8081".into(),
            weight: Some(1),
            max_connections: Some(10_000),
        }
    }
}

// ---------------------------------------------------------------------------
// Route
// ---------------------------------------------------------------------------

/// A routing rule that directs traffic to a cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RouteConfig {
    /// Match requests by `Host` header.
    pub host: Option<String>,
    /// Match requests by URI path prefix.
    pub path: Option<String>,
    /// Match requests by header key/value pairs.
    pub headers: Option<HashMap<String, String>>,
    /// Target cluster name.
    pub cluster: String,
    /// Evaluation priority (lower = higher priority).
    pub priority: u32,
    /// Per-route load-balancing override.
    pub lb_strategy: Option<LbStrategy>,
}

impl Default for RouteConfig {
    fn default() -> Self {
        Self {
            host: None,
            path: Some("/".into()),
            headers: None,
            cluster: "default".into(),
            priority: 100,
            lb_strategy: None,
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/// HTTP/1.1 and HTTP/2 protocol settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpConfig {
    /// Maximum request body size in bytes.
    pub max_request_body_size: u64,
    /// Maximum response body size in bytes.
    pub max_response_body_size: u64,
    /// Maximum total body bytes per connection.
    pub max_connection_body_size: u64,
    /// Maximum header block size in bytes.
    pub max_header_size: usize,
    /// Maximum URI length in bytes.
    pub max_uri_length: usize,
    /// Timeout for reading request headers (ms).
    pub request_header_timeout_ms: u64,
    /// Timeout for reading request body (ms).
    pub request_body_timeout_ms: u64,
    /// Idle connection timeout (seconds).
    pub idle_timeout_s: u64,
    /// HTTP keep-alive timeout (seconds).
    pub keepalive_timeout_s: u64,
    /// Maximum concurrent HTTP/2 streams.
    pub max_concurrent_streams: u32,
    /// HTTP/2 connection-level flow-control window size.
    pub h2_connection_window_size: u32,
    /// Response compression settings.
    pub compression: CompressionConfig,
    /// Sticky session / session affinity.
    pub sticky_session: StickySessionConfig,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            max_request_body_size: 10_485_760,
            max_response_body_size: 10_485_760,
            max_connection_body_size: 104_857_600,
            max_header_size: 8_192,
            max_uri_length: 4_096,
            request_header_timeout_ms: 30_000,
            request_body_timeout_ms: 60_000,
            idle_timeout_s: 300,
            keepalive_timeout_s: 60,
            max_concurrent_streams: 128,
            h2_connection_window_size: 1_048_576,
            compression: CompressionConfig::default(),
            sticky_session: StickySessionConfig::default(),
        }
    }
}

/// Response compression settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressionConfig {
    /// Whether compression is enabled.
    pub enabled: bool,
    /// Minimum response body size before compressing (bytes).
    pub min_size: usize,
    /// Ordered list of compression algorithms.
    pub algorithms: Vec<CompressionAlgorithm>,
    /// Compression level per algorithm (algorithm name -> level).
    /// Gzip: 1-9 (default 6), Brotli: 0-11 (default 4), Zstd: 1-22 (default 3).
    pub levels: HashMap<CompressionAlgorithm, u32>,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_size: 1_024,
            algorithms: vec![
                CompressionAlgorithm::Gzip,
                CompressionAlgorithm::Brotli,
                CompressionAlgorithm::Zstd,
            ],
            levels: HashMap::new(),
        }
    }
}

/// Sticky session (session affinity) settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StickySessionConfig {
    /// Whether sticky sessions are enabled.
    pub enabled: bool,
    /// Cookie name used for affinity.
    pub cookie_name: String,
}

impl Default for StickySessionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cookie_name: "EG_SESSION".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Proxy Protocol
// ---------------------------------------------------------------------------

/// PROXY protocol settings for preserving client addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyProtocolConfig {
    /// Inbound mode.
    pub inbound: ProxyProtoInbound,
    /// Outbound mode.
    pub outbound: ProxyProtoMode,
}

impl Default for ProxyProtocolConfig {
    fn default() -> Self {
        Self {
            inbound: ProxyProtoInbound::Off,
            outbound: ProxyProtoMode::Off,
        }
    }
}

// ---------------------------------------------------------------------------
// Health Check
// ---------------------------------------------------------------------------

/// Health-check configuration group including HTTP probe settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthCheckGroupConfig {
    /// Interval between probes (seconds).
    pub interval_s: u64,
    /// Per-probe timeout (milliseconds).
    pub timeout_ms: u64,
    /// Consecutive successes to mark healthy.
    pub rise: u32,
    /// Consecutive failures to mark unhealthy.
    pub fall: u32,
    /// Sample window size.
    pub samples: u32,
    /// HTTP-specific probe settings.
    pub http: HttpHealthCheckConfig,
}

impl Default for HealthCheckGroupConfig {
    fn default() -> Self {
        Self {
            interval_s: 10,
            timeout_ms: 5_000,
            rise: 2,
            fall: 3,
            samples: 100,
            http: HttpHealthCheckConfig::default(),
        }
    }
}

/// HTTP health-check probe settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpHealthCheckConfig {
    /// HTTP method for the probe.
    pub method: HttpMethod,
    /// URI path for the probe.
    pub path: String,
    /// Expected HTTP status codes (any match = healthy).
    pub expected_status: Vec<u16>,
}

impl Default for HttpHealthCheckConfig {
    fn default() -> Self {
        Self {
            method: HttpMethod::Get,
            path: "/healthz".into(),
            expected_status: vec![200],
        }
    }
}

// ---------------------------------------------------------------------------
// Circuit Breaker
// ---------------------------------------------------------------------------

/// Circuit-breaker thresholds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CircuitBreakerCfg {
    /// Failures before opening.
    pub failure_threshold: u32,
    /// Successes (in half-open) before closing.
    pub success_threshold: u32,
    /// Seconds to remain open before half-open.
    pub open_duration_s: u64,
}

impl Default for CircuitBreakerCfg {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 3,
            open_duration_s: 30,
        }
    }
}

// ---------------------------------------------------------------------------
// Security
// ---------------------------------------------------------------------------

/// Security settings for IP filtering and rate-limiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Filter mode.
    pub mode: SecurityMode,
    /// Maximum number of tracked unique IPs.
    pub max_tracked_ips: usize,
    /// Connection-rate limiting.
    pub connection_rate_limit: ConnectionRateLimitConfig,
    /// Packet-rate limiting.
    pub packet_rate_limit: PacketRateLimitConfig,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            mode: SecurityMode::Denylist,
            max_tracked_ips: 1_000_000,
            connection_rate_limit: ConnectionRateLimitConfig::default(),
            packet_rate_limit: PacketRateLimitConfig::default(),
        }
    }
}

/// Connection-rate limit configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionRateLimitConfig {
    /// Maximum new connections per IP within the window.
    pub max_per_ip: u32,
    /// Sliding window size in seconds.
    pub window_s: u64,
}

impl Default for ConnectionRateLimitConfig {
    fn default() -> Self {
        Self {
            max_per_ip: 100,
            window_s: 60,
        }
    }
}

/// Packet-rate limit configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PacketRateLimitConfig {
    /// Global packets-per-second limit.
    pub global_pps: u64,
    /// Per-IP packets-per-second limit.
    pub per_ip_pps: u64,
    /// Per-IP burst allowance.
    pub per_ip_burst: u64,
    /// Action when limit is exceeded.
    pub action: RateLimitAction,
}

impl Default for PacketRateLimitConfig {
    fn default() -> Self {
        Self {
            global_pps: 1_000_000,
            per_ip_pps: 10_000,
            per_ip_burst: 1_000,
            action: RateLimitAction::Drop,
        }
    }
}

// ---------------------------------------------------------------------------
// Control Plane
// ---------------------------------------------------------------------------

/// Control-plane gRPC and REST API settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlPlaneConfig {
    /// Whether the control-plane API is enabled.
    pub enabled: bool,
    /// gRPC listen address.
    pub grpc_bind: String,
    /// REST listen address.
    pub rest_bind: String,
    /// Heartbeat interval (seconds).
    pub heartbeat_interval_s: u64,
    /// Missed heartbeats before warning.
    pub heartbeat_miss_threshold: u32,
    /// Missed heartbeats before disconnect.
    pub heartbeat_disconnect_threshold: u32,
}

impl Default for ControlPlaneConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            grpc_bind: "0.0.0.0:50051".into(),
            rest_bind: "0.0.0.0:9091".into(),
            heartbeat_interval_s: 10,
            heartbeat_miss_threshold: 3,
            heartbeat_disconnect_threshold: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Graceful Shutdown
// ---------------------------------------------------------------------------

/// Graceful shutdown configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GracefulShutdownConfig {
    /// Drain timeout in seconds before force-closing connections.
    pub drain_timeout_s: u64,
}

impl Default for GracefulShutdownConfig {
    fn default() -> Self {
        Self {
            drain_timeout_s: 30,
        }
    }
}

// ---------------------------------------------------------------------------
// Retry
// ---------------------------------------------------------------------------

/// Backend retry policy configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// Whether retries are enabled.
    pub enabled: bool,
    /// Maximum number of retry attempts (0 = no retries).
    pub max_retries: u32,
    /// Initial backoff delay in milliseconds.
    pub initial_backoff_ms: u64,
    /// Maximum backoff delay in milliseconds.
    pub max_backoff_ms: u64,
    /// Backoff multiplier (fixed-point: 2 means 2x, 3 means 3x).
    pub backoff_multiplier: u32,
    /// Whether to retry on connection errors.
    pub retry_on_connect_failure: bool,
    /// Whether to retry on 5xx responses.
    pub retry_on_5xx: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_retries: 2,
            initial_backoff_ms: 100,
            max_backoff_ms: 10_000,
            backoff_multiplier: 2,
            retry_on_connect_failure: true,
            retry_on_5xx: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips_through_toml() {
        let cfg = Config::default();
        let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
        let parsed: Config = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn empty_toml_uses_defaults() {
        let cfg: Config = toml::from_str("").expect("empty toml");
        assert_eq!(cfg.global.environment, Environment::Production);
        assert!(cfg.transport.tcp_nodelay);
        assert_eq!(cfg.buffer.page_size, 16_384);
    }

    #[test]
    fn partial_override() {
        let toml_str = r#"
[global]
log_level = "debug"
"#;
        let cfg: Config = toml::from_str(toml_str).expect("partial");
        assert_eq!(cfg.global.log_level, LogLevel::Debug);
        // everything else stays at default
        assert_eq!(cfg.global.environment, Environment::Production);
    }

    #[test]
    fn invalid_enum_value_rejected_by_serde() {
        let toml_str = r#"
[global]
environment = "staging"
"#;
        let result: Result<Config, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn lb_strategy_round_trips() {
        let toml_str = r#"
[[clusters]]
name = "backend"
lb_strategy = "least_connections"

[[clusters.nodes]]
address = "127.0.0.1:8081"
"#;
        let cfg: Config = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.clusters[0].lb_strategy, LbStrategy::LeastConnections);
        let serialized = toml::to_string_pretty(&cfg).expect("serialize");
        assert!(serialized.contains("least_connections"));
    }

    #[test]
    fn retry_config_defaults() {
        let cfg = Config::default();
        assert!(cfg.retry.enabled);
        assert_eq!(cfg.retry.max_retries, 2);
        assert_eq!(cfg.retry.initial_backoff_ms, 100);
    }

    #[test]
    fn compression_levels_configurable() {
        let toml_str = r#"
[http.compression]
enabled = true
min_size = 512
algorithms = ["gzip", "zstd"]

[http.compression.levels]
gzip = 9
zstd = 5
"#;
        let cfg: Config = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.http.compression.algorithms.len(), 2);
        assert_eq!(
            cfg.http.compression.levels.get(&CompressionAlgorithm::Gzip),
            Some(&9)
        );
        assert_eq!(
            cfg.http.compression.levels.get(&CompressionAlgorithm::Zstd),
            Some(&5)
        );
    }
}
