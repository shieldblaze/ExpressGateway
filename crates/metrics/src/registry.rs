//! Prometheus metrics registry.
//!
//! Defines all gateway metrics and provides a global singleton registry.
//! All metric names are prefixed with `expressgateway_` per Prometheus
//! naming conventions.

use prometheus::{
    Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec,
    Opts, Registry,
};
use std::sync::OnceLock;

/// Namespace prefix for all metrics.
const NAMESPACE: &str = "expressgateway";

/// All Prometheus metrics exposed by the gateway.
pub struct MetricsRegistry {
    /// Prometheus registry that holds all collectors.
    pub registry: Registry,

    // ── Gauges ──────────────────────────────────────────────
    /// Active frontend connections, labelled by `listener` and `protocol`.
    pub connections_active: IntGaugeVec,
    /// Active backend connections, labelled by `cluster` and `backend`.
    pub backend_connections_active: IntGaugeVec,
    /// Backend health state (1 = healthy, 0 = unhealthy), labelled by
    /// `cluster` and `backend`.
    pub backend_health: IntGaugeVec,
    /// Write-buffer pending bytes, labelled by `listener`.
    pub backpressure_pending_bytes: IntGaugeVec,
    /// Whether backpressure is paused (1) or flowing (0), labelled by
    /// `listener`.
    pub backpressure_paused: IntGaugeVec,

    // ── Pool gauges ────────────────────────────────────────
    /// Connection pool size, labelled by `pool` (e.g. "h1", "tcp") and
    /// `backend`.
    pub pool_connections_idle: IntGaugeVec,
    /// Buffer pool available buffers.
    pub buffer_pool_available: IntGauge,
    /// Buffer pool capacity.
    pub buffer_pool_capacity: IntGauge,

    // ── Counters ────────────────────────────────────────────
    /// Total connections accepted, labelled by `listener` and `protocol`.
    pub connections_total: IntCounterVec,
    /// Total HTTP requests, labelled by `listener`, `method`, `status`,
    /// `protocol`.
    pub requests_total: IntCounterVec,
    /// Total bytes sent, labelled by `direction` (`upstream` / `downstream`).
    pub bytes_sent_total: IntCounterVec,
    /// Total bytes received, labelled by `direction`.
    pub bytes_received_total: IntCounterVec,
    /// Total errors, labelled by `error_class`.
    ///
    /// Error classes: `io`, `tls`, `config`, `backend`, `timeout`,
    /// `rate_limit`, `access_denied`, `protocol`, `http`, `other`.
    pub errors_total: IntCounterVec,
    /// Successful control-plane pushes.
    pub controlplane_push_success_total: IntCounter,
    /// Failed control-plane pushes.
    pub controlplane_push_failure_total: IntCounter,
    /// XDP packets processed, labelled by `action`.
    pub xdp_packets_total: IntCounterVec,
    /// Connection pool hits, labelled by `pool`.
    pub pool_hits_total: IntCounterVec,
    /// Connection pool misses, labelled by `pool`.
    pub pool_misses_total: IntCounterVec,
    /// Connection pool evictions, labelled by `pool`.
    pub pool_evictions_total: IntCounterVec,
    /// TLS handshake errors, labelled by `reason`.
    pub tls_handshake_errors_total: IntCounterVec,

    // ── Histograms ──────────────────────────────────────────
    /// Request duration in seconds, labelled by `listener`, `method`, and
    /// `protocol`.
    ///
    /// Bucket boundaries are tuned for a reverse proxy: sub-millisecond
    /// resolution at the low end for cache hits, and 30/60s buckets for
    /// long-poll / streaming endpoints.
    pub request_duration_seconds: HistogramVec,
    /// Control-plane push latency in seconds.
    pub controlplane_push_latency_seconds: Histogram,
    /// TLS handshake duration in seconds, labelled by `tls_version`.
    pub tls_handshake_duration_seconds: HistogramVec,
    /// Backend connection establishment time in seconds, labelled by
    /// `cluster` and `backend`.
    pub backend_connect_duration_seconds: HistogramVec,
}

/// Global singleton for the metrics registry.
static GLOBAL_REGISTRY: OnceLock<MetricsRegistry> = OnceLock::new();

/// Helper to build an `Opts` with the gateway namespace.
#[inline]
fn opts(name: &str, help: &str) -> Opts {
    Opts::new(name, help).namespace(NAMESPACE)
}

/// Helper to build a `HistogramOpts` with the gateway namespace.
#[inline]
fn hist_opts(name: &str, help: &str) -> HistogramOpts {
    HistogramOpts {
        common_opts: opts(name, help),
        buckets: Vec::new(), // caller sets buckets
    }
}

/// Exponential bucket boundaries tuned for proxy request latency.
///
/// Covers 100us to 60s.  Recoverable percentiles: p50, p90, p99, p99.9.
const REQUEST_DURATION_BUCKETS: &[f64] = &[
    0.000_1, // 100us
    0.000_5, // 500us
    0.001,   // 1ms
    0.005,   // 5ms
    0.01,    // 10ms
    0.025,   // 25ms
    0.05,    // 50ms
    0.1,     // 100ms
    0.25,    // 250ms
    0.5,     // 500ms
    1.0,     // 1s
    2.5,     // 2.5s
    5.0,     // 5s
    10.0,    // 10s
    30.0,    // 30s
    60.0,    // 60s
];

/// Bucket boundaries for TLS handshake duration.
const TLS_HANDSHAKE_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

/// Bucket boundaries for backend connect latency.
const BACKEND_CONNECT_BUCKETS: &[f64] = &[
    0.000_5, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0, 10.0,
];

impl MetricsRegistry {
    /// Build a new `MetricsRegistry` with all metrics registered against the
    /// supplied Prometheus [`Registry`].
    fn new(registry: Registry) -> prometheus::Result<Self> {
        // ── Gauges ──────────────────────────────────────────
        let connections_active = IntGaugeVec::new(
            opts(
                "connections_active",
                "Number of active frontend connections",
            ),
            &["listener", "protocol"],
        )?;
        let backend_connections_active = IntGaugeVec::new(
            opts(
                "backend_connections_active",
                "Number of active backend connections",
            ),
            &["cluster", "backend"],
        )?;
        let backend_health = IntGaugeVec::new(
            opts("backend_health", "Health state of backends (1 = healthy)"),
            &["cluster", "backend"],
        )?;
        let backpressure_pending_bytes = IntGaugeVec::new(
            opts(
                "backpressure_pending_bytes",
                "Pending bytes in write buffer",
            ),
            &["listener"],
        )?;
        let backpressure_paused = IntGaugeVec::new(
            opts(
                "backpressure_paused",
                "Whether backpressure is active (1 = paused)",
            ),
            &["listener"],
        )?;

        // ── Pool gauges ────────────────────────────────────
        let pool_connections_idle = IntGaugeVec::new(
            opts(
                "pool_connections_idle",
                "Idle connections currently in pool",
            ),
            &["pool", "backend"],
        )?;
        let buffer_pool_available = IntGauge::with_opts(opts(
            "buffer_pool_available",
            "Available buffers in the buffer pool",
        ))?;
        let buffer_pool_capacity = IntGauge::with_opts(opts(
            "buffer_pool_capacity",
            "Maximum capacity of the buffer pool",
        ))?;

        // ── Counters ────────────────────────────────────────
        let connections_total = IntCounterVec::new(
            opts("connections_total", "Total connections accepted"),
            &["listener", "protocol"],
        )?;
        let requests_total = IntCounterVec::new(
            opts("requests_total", "Total HTTP requests"),
            &["listener", "method", "status", "protocol"],
        )?;
        let bytes_sent_total = IntCounterVec::new(
            opts("bytes_sent_total", "Total bytes sent"),
            &["direction"],
        )?;
        let bytes_received_total = IntCounterVec::new(
            opts("bytes_received_total", "Total bytes received"),
            &["direction"],
        )?;
        let errors_total = IntCounterVec::new(
            opts("errors_total", "Total errors by error class"),
            &["error_class"],
        )?;
        let controlplane_push_success_total = IntCounter::with_opts(opts(
            "controlplane_push_success_total",
            "Successful control-plane configuration pushes",
        ))?;
        let controlplane_push_failure_total = IntCounter::with_opts(opts(
            "controlplane_push_failure_total",
            "Failed control-plane configuration pushes",
        ))?;
        let xdp_packets_total = IntCounterVec::new(
            opts("xdp_packets_total", "XDP packets processed"),
            &["action"],
        )?;
        let pool_hits_total = IntCounterVec::new(
            opts("pool_hits_total", "Connection pool hits"),
            &["pool"],
        )?;
        let pool_misses_total = IntCounterVec::new(
            opts("pool_misses_total", "Connection pool misses"),
            &["pool"],
        )?;
        let pool_evictions_total = IntCounterVec::new(
            opts("pool_evictions_total", "Connection pool evictions"),
            &["pool"],
        )?;
        let tls_handshake_errors_total = IntCounterVec::new(
            opts("tls_handshake_errors_total", "TLS handshake errors"),
            &["reason"],
        )?;

        // ── Histograms ──────────────────────────────────────
        let request_duration_seconds = HistogramVec::new(
            hist_opts("request_duration_seconds", "Request duration in seconds")
                .buckets(REQUEST_DURATION_BUCKETS.to_vec()),
            &["listener", "method", "protocol"],
        )?;
        let controlplane_push_latency_seconds = Histogram::with_opts(
            hist_opts(
                "controlplane_push_latency_seconds",
                "Control-plane push latency in seconds",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]),
        )?;
        let tls_handshake_duration_seconds = HistogramVec::new(
            hist_opts(
                "tls_handshake_duration_seconds",
                "TLS handshake duration in seconds",
            )
            .buckets(TLS_HANDSHAKE_BUCKETS.to_vec()),
            &["tls_version"],
        )?;
        let backend_connect_duration_seconds = HistogramVec::new(
            hist_opts(
                "backend_connect_duration_seconds",
                "Backend connection establishment time in seconds",
            )
            .buckets(BACKEND_CONNECT_BUCKETS.to_vec()),
            &["cluster", "backend"],
        )?;

        // ── Register all collectors ─────────────────────────
        // Gauges
        registry.register(Box::new(connections_active.clone()))?;
        registry.register(Box::new(backend_connections_active.clone()))?;
        registry.register(Box::new(backend_health.clone()))?;
        registry.register(Box::new(backpressure_pending_bytes.clone()))?;
        registry.register(Box::new(backpressure_paused.clone()))?;
        registry.register(Box::new(pool_connections_idle.clone()))?;
        registry.register(Box::new(buffer_pool_available.clone()))?;
        registry.register(Box::new(buffer_pool_capacity.clone()))?;
        // Counters
        registry.register(Box::new(connections_total.clone()))?;
        registry.register(Box::new(requests_total.clone()))?;
        registry.register(Box::new(bytes_sent_total.clone()))?;
        registry.register(Box::new(bytes_received_total.clone()))?;
        registry.register(Box::new(errors_total.clone()))?;
        registry.register(Box::new(controlplane_push_success_total.clone()))?;
        registry.register(Box::new(controlplane_push_failure_total.clone()))?;
        registry.register(Box::new(xdp_packets_total.clone()))?;
        registry.register(Box::new(pool_hits_total.clone()))?;
        registry.register(Box::new(pool_misses_total.clone()))?;
        registry.register(Box::new(pool_evictions_total.clone()))?;
        registry.register(Box::new(tls_handshake_errors_total.clone()))?;
        // Histograms
        registry.register(Box::new(request_duration_seconds.clone()))?;
        registry.register(Box::new(controlplane_push_latency_seconds.clone()))?;
        registry.register(Box::new(tls_handshake_duration_seconds.clone()))?;
        registry.register(Box::new(backend_connect_duration_seconds.clone()))?;

        Ok(Self {
            registry,
            connections_active,
            backend_connections_active,
            backend_health,
            backpressure_pending_bytes,
            backpressure_paused,
            pool_connections_idle,
            buffer_pool_available,
            buffer_pool_capacity,
            connections_total,
            requests_total,
            bytes_sent_total,
            bytes_received_total,
            errors_total,
            controlplane_push_success_total,
            controlplane_push_failure_total,
            xdp_packets_total,
            pool_hits_total,
            pool_misses_total,
            pool_evictions_total,
            tls_handshake_errors_total,
            request_duration_seconds,
            controlplane_push_latency_seconds,
            tls_handshake_duration_seconds,
            backend_connect_duration_seconds,
        })
    }
}

/// Initialise the global metrics registry.
///
/// This must be called once at startup. Subsequent calls are no-ops and the
/// previously initialised registry is returned.
pub fn register_all() -> &'static MetricsRegistry {
    GLOBAL_REGISTRY.get_or_init(|| {
        let registry = Registry::new();
        MetricsRegistry::new(registry).expect("failed to register metrics")
    })
}

/// Return a reference to the global metrics registry, or `None` if
/// [`register_all`] has not been called yet.
///
/// Prefer this over a panicking accessor on any code path that might execute
/// before the startup sequence completes.
pub fn try_global_registry() -> Option<&'static MetricsRegistry> {
    GLOBAL_REGISTRY.get()
}

/// Return a reference to the global metrics registry.
///
/// # Panics
///
/// Panics if [`register_all`] has not been called yet.  Use
/// [`try_global_registry`] on paths where this cannot be guaranteed.
pub fn global_registry() -> &'static MetricsRegistry {
    GLOBAL_REGISTRY
        .get()
        .expect("metrics registry not initialised -- call register_all() first")
}

/// Map an [`expressgateway_core::Error`] variant to a bounded error class
/// label for the `errors_total` counter.
///
/// The returned `&'static str` values form a closed set so label cardinality
/// stays constant regardless of traffic patterns.
pub fn error_class(err: &expressgateway_core::Error) -> &'static str {
    use expressgateway_core::Error;
    match err {
        Error::Io(_) => "io",
        Error::Tls(_) => "tls",
        Error::Config(_) => "config",
        Error::NoHealthyBackend
        | Error::BackendConnectionFailed { .. }
        | Error::CircuitBreakerOpen { .. } => "backend",
        Error::BackendTimeout { .. } | Error::RequestTimeout => "timeout",
        Error::ConnectionLimitReached { .. } => "connection_limit",
        Error::RateLimitExceeded { .. } => "rate_limit",
        Error::AccessDenied { .. } => "access_denied",
        Error::Protocol(_) | Error::ProxyProtocol(_) => "protocol",
        Error::Http { .. }
        | Error::BodyTooLarge { .. }
        | Error::UriTooLong { .. }
        | Error::HeaderTooLarge { .. }
        | Error::PathTraversal { .. } => "http",
        Error::Grpc { .. } => "grpc",
        Error::WebSocket(_) => "websocket",
        Error::ShuttingDown => "shutdown",
        Error::NodeNotFound(_)
        | Error::ClusterNotFound(_)
        | Error::InvalidStateTransition { .. }
        | Error::Runtime(_)
        | Error::Other(_) => "internal",
    }
}

/// Record an error against the `errors_total` counter with the correct class
/// label. This is the primary entry point for error counting from data-path
/// code.
#[inline]
pub fn record_error(err: &expressgateway_core::Error) {
    if let Some(m) = try_global_registry() {
        m.errors_total
            .with_label_values(&[error_class(err)])
            .inc();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_all_creates_metrics() {
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        // Verify gauges work
        m.connections_active
            .with_label_values(&["default", "http"])
            .set(42);
        assert_eq!(
            m.connections_active
                .with_label_values(&["default", "http"])
                .get(),
            42
        );

        m.backend_connections_active
            .with_label_values(&["cluster-1", "backend-1"])
            .set(10);
        assert_eq!(
            m.backend_connections_active
                .with_label_values(&["cluster-1", "backend-1"])
                .get(),
            10
        );

        m.backend_health
            .with_label_values(&["cluster-1", "backend-1"])
            .set(1);
        assert_eq!(
            m.backend_health
                .with_label_values(&["cluster-1", "backend-1"])
                .get(),
            1
        );
    }

    #[test]
    fn test_counters() {
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        m.connections_total
            .with_label_values(&["default", "tcp"])
            .inc();
        assert_eq!(
            m.connections_total
                .with_label_values(&["default", "tcp"])
                .get(),
            1
        );

        m.requests_total
            .with_label_values(&["default", "GET", "200", "http"])
            .inc_by(5);
        assert_eq!(
            m.requests_total
                .with_label_values(&["default", "GET", "200", "http"])
                .get(),
            5
        );

        m.bytes_sent_total
            .with_label_values(&["upstream"])
            .inc_by(1024);
        assert_eq!(
            m.bytes_sent_total.with_label_values(&["upstream"]).get(),
            1024
        );

        m.bytes_received_total
            .with_label_values(&["downstream"])
            .inc_by(2048);
        assert_eq!(
            m.bytes_received_total
                .with_label_values(&["downstream"])
                .get(),
            2048
        );

        m.controlplane_push_success_total.inc();
        assert_eq!(m.controlplane_push_success_total.get(), 1);

        m.controlplane_push_failure_total.inc();
        assert_eq!(m.controlplane_push_failure_total.get(), 1);

        m.xdp_packets_total.with_label_values(&["pass"]).inc();
        assert_eq!(m.xdp_packets_total.with_label_values(&["pass"]).get(), 1);
    }

    #[test]
    fn test_error_counters() {
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        m.errors_total.with_label_values(&["io"]).inc();
        m.errors_total.with_label_values(&["backend"]).inc_by(3);
        m.errors_total.with_label_values(&["timeout"]).inc();

        assert_eq!(m.errors_total.with_label_values(&["io"]).get(), 1);
        assert_eq!(m.errors_total.with_label_values(&["backend"]).get(), 3);
        assert_eq!(m.errors_total.with_label_values(&["timeout"]).get(), 1);
    }

    #[test]
    fn test_histograms() {
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        m.request_duration_seconds
            .with_label_values(&["default", "GET", "http"])
            .observe(0.042);
        assert_eq!(
            m.request_duration_seconds
                .with_label_values(&["default", "GET", "http"])
                .get_sample_count(),
            1
        );

        m.controlplane_push_latency_seconds.observe(0.15);
        assert_eq!(m.controlplane_push_latency_seconds.get_sample_count(), 1);

        m.tls_handshake_duration_seconds
            .with_label_values(&["TLSv1.3"])
            .observe(0.005);
        assert_eq!(
            m.tls_handshake_duration_seconds
                .with_label_values(&["TLSv1.3"])
                .get_sample_count(),
            1
        );

        m.backend_connect_duration_seconds
            .with_label_values(&["cluster-1", "backend-1"])
            .observe(0.002);
        assert_eq!(
            m.backend_connect_duration_seconds
                .with_label_values(&["cluster-1", "backend-1"])
                .get_sample_count(),
            1
        );
    }

    #[test]
    fn test_pool_metrics() {
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        m.pool_connections_idle
            .with_label_values(&["h1", "backend-1"])
            .set(5);
        m.pool_hits_total.with_label_values(&["h1"]).inc_by(100);
        m.pool_misses_total.with_label_values(&["h1"]).inc_by(10);
        m.pool_evictions_total.with_label_values(&["h1"]).inc_by(2);

        assert_eq!(
            m.pool_connections_idle
                .with_label_values(&["h1", "backend-1"])
                .get(),
            5
        );
        assert_eq!(m.pool_hits_total.with_label_values(&["h1"]).get(), 100);
        assert_eq!(m.pool_misses_total.with_label_values(&["h1"]).get(), 10);
        assert_eq!(m.pool_evictions_total.with_label_values(&["h1"]).get(), 2);

        m.buffer_pool_available.set(512);
        m.buffer_pool_capacity.set(1024);
        assert_eq!(m.buffer_pool_available.get(), 512);
        assert_eq!(m.buffer_pool_capacity.get(), 1024);
    }

    #[test]
    fn test_backpressure_metrics() {
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        m.backpressure_pending_bytes
            .with_label_values(&["default"])
            .set(32768);
        m.backpressure_paused
            .with_label_values(&["default"])
            .set(1);

        assert_eq!(
            m.backpressure_pending_bytes
                .with_label_values(&["default"])
                .get(),
            32768
        );
        assert_eq!(
            m.backpressure_paused
                .with_label_values(&["default"])
                .get(),
            1
        );
    }

    #[test]
    fn test_prometheus_text_encoding() {
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        m.connections_total
            .with_label_values(&["default", "http"])
            .inc();

        let encoder = prometheus::TextEncoder::new();
        let families = m.registry.gather();
        let text = encoder.encode_to_string(&families).unwrap();

        assert!(
            text.contains("expressgateway_connections_total"),
            "metric should have namespace prefix"
        );
        assert!(text.contains("http"));
    }

    #[test]
    fn test_request_duration_bucket_boundaries() {
        // Verify the bucket count and ordering.
        assert_eq!(REQUEST_DURATION_BUCKETS.len(), 16);
        for window in REQUEST_DURATION_BUCKETS.windows(2) {
            assert!(
                window[0] < window[1],
                "buckets must be strictly increasing: {} >= {}",
                window[0],
                window[1]
            );
        }
        // Sub-millisecond coverage
        assert_eq!(REQUEST_DURATION_BUCKETS[0], 0.0001);
        // Long-tail coverage
        assert_eq!(*REQUEST_DURATION_BUCKETS.last().unwrap(), 60.0);
    }

    #[test]
    fn test_error_class_mapping() {
        use expressgateway_core::Error;

        assert_eq!(
            error_class(&Error::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "test"
            ))),
            "io"
        );
        assert_eq!(error_class(&Error::Tls("bad cert".into())), "tls");
        assert_eq!(error_class(&Error::NoHealthyBackend), "backend");
        assert_eq!(error_class(&Error::RequestTimeout), "timeout");
        assert_eq!(
            error_class(&Error::RateLimitExceeded {
                remote: "1.2.3.4".into()
            }),
            "rate_limit"
        );
        assert_eq!(
            error_class(&Error::AccessDenied {
                remote: "1.2.3.4".into()
            }),
            "access_denied"
        );
        assert_eq!(
            error_class(&Error::Http {
                status: 400,
                message: "bad".into()
            }),
            "http"
        );
        assert_eq!(
            error_class(&Error::Grpc {
                code: 2,
                message: "unknown".into()
            }),
            "grpc"
        );
        assert_eq!(error_class(&Error::WebSocket("err".into())), "websocket");
        assert_eq!(error_class(&Error::ShuttingDown), "shutdown");
        assert_eq!(error_class(&Error::Other("x".into())), "internal");
    }

    #[test]
    fn test_try_global_registry() {
        // After register_all, try_global_registry must return Some.
        let _ = register_all();
        assert!(try_global_registry().is_some());
    }

    #[test]
    fn test_global_singleton() {
        let r1 = register_all();
        let r2 = register_all();
        assert!(std::ptr::eq(r1, r2));
    }
}
