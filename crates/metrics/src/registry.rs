//! Prometheus metrics registry.
//!
//! Defines all gateway metrics and provides a global singleton registry.

use prometheus::{
    Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGaugeVec, Opts, Registry,
};
use std::sync::OnceLock;

/// All Prometheus metrics exposed by the gateway.
pub struct MetricsRegistry {
    /// Prometheus registry that holds all collectors.
    pub registry: Registry,

    // ── Gauges ──────────────────────────────────────────────
    /// Active frontend connections, labelled by `protocol`.
    pub connections_active: IntGaugeVec,
    /// Active backend connections, labelled by `backend`.
    pub backend_connections_active: IntGaugeVec,
    /// Backend health state, labelled by `backend` and `state`.
    pub backend_health: IntGaugeVec,

    // ── Counters ────────────────────────────────────────────
    /// Total connections accepted, labelled by `protocol`.
    pub connections_total: IntCounterVec,
    /// Total HTTP requests, labelled by `method`, `status`, `protocol`.
    pub requests_total: IntCounterVec,
    /// Total bytes sent, labelled by `direction`.
    pub bytes_sent_total: IntCounterVec,
    /// Total bytes received, labelled by `direction`.
    pub bytes_received_total: IntCounterVec,
    /// Successful control-plane pushes.
    pub controlplane_push_success: IntCounter,
    /// Failed control-plane pushes.
    pub controlplane_push_failure: IntCounter,
    /// XDP packets processed, labelled by `action`.
    pub xdp_packets_total: IntCounterVec,

    // ── Histograms ──────────────────────────────────────────
    /// Request duration in seconds, labelled by `method` and `protocol`.
    pub request_duration_seconds: HistogramVec,
    /// Control-plane push latency in seconds.
    pub controlplane_push_latency: Histogram,
}

/// Global singleton for the metrics registry.
static GLOBAL_REGISTRY: OnceLock<MetricsRegistry> = OnceLock::new();

impl MetricsRegistry {
    /// Build a new `MetricsRegistry` with all metrics registered against the
    /// supplied Prometheus [`Registry`].
    fn new(registry: Registry) -> prometheus::Result<Self> {
        // Gauges
        let connections_active = IntGaugeVec::new(
            Opts::new(
                "connections_active",
                "Number of active frontend connections",
            ),
            &["protocol"],
        )?;
        let backend_connections_active = IntGaugeVec::new(
            Opts::new(
                "backend_connections_active",
                "Number of active backend connections",
            ),
            &["backend"],
        )?;
        let backend_health = IntGaugeVec::new(
            Opts::new("backend_health", "Health state of backends (1 = healthy)"),
            &["backend", "state"],
        )?;

        // Counters
        let connections_total = IntCounterVec::new(
            Opts::new("connections_total", "Total connections accepted"),
            &["protocol"],
        )?;
        let requests_total = IntCounterVec::new(
            Opts::new("requests_total", "Total HTTP requests"),
            &["method", "status", "protocol"],
        )?;
        let bytes_sent_total = IntCounterVec::new(
            Opts::new("bytes_sent_total", "Total bytes sent"),
            &["direction"],
        )?;
        let bytes_received_total = IntCounterVec::new(
            Opts::new("bytes_received_total", "Total bytes received"),
            &["direction"],
        )?;
        let controlplane_push_success = IntCounter::new(
            "controlplane_push_success",
            "Successful control-plane configuration pushes",
        )?;
        let controlplane_push_failure = IntCounter::new(
            "controlplane_push_failure",
            "Failed control-plane configuration pushes",
        )?;
        let xdp_packets_total = IntCounterVec::new(
            Opts::new("xdp_packets_total", "XDP packets processed"),
            &["action"],
        )?;

        // Histograms
        let request_duration_seconds = HistogramVec::new(
            HistogramOpts::new("request_duration_seconds", "Request duration in seconds").buckets(
                vec![
                    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                ],
            ),
            &["method", "protocol"],
        )?;
        let controlplane_push_latency = Histogram::with_opts(
            HistogramOpts::new(
                "controlplane_push_latency_seconds",
                "Control-plane push latency in seconds",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]),
        )?;

        // Register all collectors
        registry.register(Box::new(connections_active.clone()))?;
        registry.register(Box::new(backend_connections_active.clone()))?;
        registry.register(Box::new(backend_health.clone()))?;
        registry.register(Box::new(connections_total.clone()))?;
        registry.register(Box::new(requests_total.clone()))?;
        registry.register(Box::new(bytes_sent_total.clone()))?;
        registry.register(Box::new(bytes_received_total.clone()))?;
        registry.register(Box::new(controlplane_push_success.clone()))?;
        registry.register(Box::new(controlplane_push_failure.clone()))?;
        registry.register(Box::new(xdp_packets_total.clone()))?;
        registry.register(Box::new(request_duration_seconds.clone()))?;
        registry.register(Box::new(controlplane_push_latency.clone()))?;

        Ok(Self {
            registry,
            connections_active,
            backend_connections_active,
            backend_health,
            connections_total,
            requests_total,
            bytes_sent_total,
            bytes_received_total,
            controlplane_push_success,
            controlplane_push_failure,
            xdp_packets_total,
            request_duration_seconds,
            controlplane_push_latency,
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

/// Return a reference to the global metrics registry.
///
/// Panics if [`register_all`] has not been called yet.
pub fn global_registry() -> &'static MetricsRegistry {
    GLOBAL_REGISTRY
        .get()
        .expect("metrics registry not initialised — call register_all() first")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_all_creates_metrics() {
        // Create a fresh registry (not the global singleton) so the test is
        // self-contained and does not conflict with other tests.
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        // Verify gauges work
        m.connections_active.with_label_values(&["http"]).set(42);
        assert_eq!(m.connections_active.with_label_values(&["http"]).get(), 42);

        m.backend_connections_active
            .with_label_values(&["backend-1"])
            .set(10);
        assert_eq!(
            m.backend_connections_active
                .with_label_values(&["backend-1"])
                .get(),
            10
        );

        m.backend_health
            .with_label_values(&["backend-1", "healthy"])
            .set(1);
        assert_eq!(
            m.backend_health
                .with_label_values(&["backend-1", "healthy"])
                .get(),
            1
        );
    }

    #[test]
    fn test_counters() {
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        m.connections_total.with_label_values(&["tcp"]).inc();
        assert_eq!(m.connections_total.with_label_values(&["tcp"]).get(), 1);

        m.requests_total
            .with_label_values(&["GET", "200", "http"])
            .inc_by(5);
        assert_eq!(
            m.requests_total
                .with_label_values(&["GET", "200", "http"])
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

        m.controlplane_push_success.inc();
        assert_eq!(m.controlplane_push_success.get(), 1);

        m.controlplane_push_failure.inc();
        assert_eq!(m.controlplane_push_failure.get(), 1);

        m.xdp_packets_total.with_label_values(&["pass"]).inc();
        assert_eq!(m.xdp_packets_total.with_label_values(&["pass"]).get(), 1);
    }

    #[test]
    fn test_histograms() {
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        m.request_duration_seconds
            .with_label_values(&["GET", "http"])
            .observe(0.042);
        assert_eq!(
            m.request_duration_seconds
                .with_label_values(&["GET", "http"])
                .get_sample_count(),
            1
        );

        m.controlplane_push_latency.observe(0.15);
        assert_eq!(m.controlplane_push_latency.get_sample_count(), 1);
    }

    #[test]
    fn test_prometheus_text_encoding() {
        let registry = Registry::new();
        let m = MetricsRegistry::new(registry).expect("registration should succeed");

        // Record some data
        m.connections_total.with_label_values(&["http"]).inc();

        let encoder = prometheus::TextEncoder::new();
        let families = m.registry.gather();
        let text = encoder.encode_to_string(&families).unwrap();

        assert!(text.contains("connections_total"));
        assert!(text.contains("http"));
    }

    #[test]
    fn test_global_singleton() {
        let r1 = register_all();
        let r2 = register_all();
        // Both should return the same pointer.
        assert!(std::ptr::eq(r1, r2));
    }
}
