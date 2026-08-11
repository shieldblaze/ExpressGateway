//! Metrics, tracing, and logging.
//!
//! [`MetricsRegistry`] adds a handle cache over [`prometheus::Registry`] so `counter(...)` and
//! friends are IDEMPOTENT — repeat calls return the same handle instead of splitting increments
//! across two registrations.
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
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::sync::Arc;

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use prometheus::{Histogram as PHistogram, core::Collector};
use prometheus::{
    HistogramOpts, HistogramVec, IntCounterVec, IntGaugeVec, Opts, Registry, proto::MetricFamily,
};

/// Re-export so downstream crates can name a counter handle without a direct `prometheus` edge.
pub use prometheus::IntCounter;
/// Re-export so downstream crates can name a gauge handle without a direct `prometheus` edge.
pub use prometheus::IntGauge;

pub mod admin_http;
pub mod label_budget;
pub mod log;
pub mod passthrough_metrics;
pub mod probes;
pub mod prometheus_exposition;
pub mod quic_h3_recycle_metrics;
pub mod quic_modeb_metrics;
pub mod tracing_propagation;
pub mod xdp_metrics;

pub use label_budget::{
    CANONICAL_LABELS, CardinalityErr, DEFAULT_MAX_LABEL_CARDINALITY, EnforcedLabelBudget,
    LabelBudget, LabelBudgetError, MAX_ROUTES_BUDGET,
};
pub use log::{LogFormat, TracingConfig, TracingError, init_tracing};
pub use passthrough_metrics::PassthroughMetrics;
pub use probes::{ProbeRegistry, ProbeState};
pub use quic_h3_recycle_metrics::QuicH3RecycleMetrics;
pub use quic_modeb_metrics::QuicModeBMetrics;
pub use xdp_metrics::{ConntrackFamily, SamplerBaseline, XdpMetrics, stat_slot_labels};

/// Advisory series cap; past it a warning is emitted but registration still SUCCEEDS.
const CARDINALITY_WARN_THRESHOLD: usize = 10_000;

/// Handle-cache entries, keeping the typed handle registered under each name.
#[derive(Clone)]
enum Handle {
    Counter(IntCounter),
    CounterVec(IntCounterVec),
    Histogram(PHistogram),
    HistogramVec(HistogramVec),
    Gauge(IntGauge),
    GaugeVec(IntGaugeVec),
}

/// Raised when a metric name is already registered under a different type.
#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    /// Name already registered under a different metric type.
    #[error("metric {name:?} already registered as a different type")]
    TypeMismatch {
        /// Offending metric name.
        name: String,
    },
    /// Registration failed inside `prometheus`.
    #[error("prometheus registration failed: {0}")]
    Prometheus(#[from] prometheus::Error),
}

/// Thread-safe metrics registry; repeat `counter(name, help)` calls return the same handle.
#[derive(Debug)]
pub struct MetricsRegistry {
    inner: Registry,
    handles: DashMap<String, Handle>,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Counter(_) => f.write_str("Counter"),
            Self::CounterVec(_) => f.write_str("CounterVec"),
            Self::Histogram(_) => f.write_str("Histogram"),
            Self::HistogramVec(_) => f.write_str("HistogramVec"),
            Self::Gauge(_) => f.write_str("Gauge"),
            Self::GaugeVec(_) => f.write_str("GaugeVec"),
        }
    }
}

impl MetricsRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Registry::new(),
            handles: DashMap::new(),
        }
    }

    /// Get-or-create an [`IntCounter`]; on a cache hit `help` is IGNORED.
    ///
    /// # Errors
    ///
    /// [`MetricsError`].
    pub fn counter(&self, name: &str, help: &str) -> Result<IntCounter, MetricsError> {
        // Fast path: a previous registration is visible — return its handle.
        if let Some(entry) = self.handles.get(name) {
            if let Handle::Counter(c) = entry.value() {
                return Ok(c.clone());
            }
            return Err(MetricsError::TypeMismatch {
                name: name.to_owned(),
            });
        }
        // The write lock must cover registration, not just insertion: otherwise two threads
        // register separately and increments split across two handles or vanish via AlreadyReg.
        match self.handles.entry(name.to_owned()) {
            Entry::Occupied(occ) => match occ.get() {
                Handle::Counter(c) => Ok(c.clone()),
                _ => Err(MetricsError::TypeMismatch {
                    name: name.to_owned(),
                }),
            },
            Entry::Vacant(vac) => {
                let c = IntCounter::with_opts(Opts::new(name, help))?;
                self.inner.register(Box::new(c.clone()))?;
                vac.insert(Handle::Counter(c.clone()));
                self.check_cardinality();
                Ok(c)
            }
        }
    }

    /// Get-or-create a labeled [`IntCounterVec`]. The label set is FIXED at first registration.
    ///
    /// # Errors
    ///
    /// See [`Self::counter`].
    pub fn counter_vec(
        &self,
        name: &str,
        help: &str,
        labels: &[&str],
    ) -> Result<IntCounterVec, MetricsError> {
        if let Some(entry) = self.handles.get(name) {
            if let Handle::CounterVec(c) = entry.value() {
                return Ok(c.clone());
            }
            return Err(MetricsError::TypeMismatch {
                name: name.to_owned(),
            });
        }
        match self.handles.entry(name.to_owned()) {
            Entry::Occupied(occ) => match occ.get() {
                Handle::CounterVec(c) => Ok(c.clone()),
                _ => Err(MetricsError::TypeMismatch {
                    name: name.to_owned(),
                }),
            },
            Entry::Vacant(vac) => {
                let c = IntCounterVec::new(Opts::new(name, help), labels)?;
                self.inner.register(Box::new(c.clone()))?;
                vac.insert(Handle::CounterVec(c.clone()));
                self.check_cardinality();
                Ok(c)
            }
        }
    }

    /// Get-or-create a [`PHistogram`] with the given bucket boundaries.
    ///
    /// # Errors
    ///
    /// See [`Self::counter`].
    pub fn histogram(
        &self,
        name: &str,
        help: &str,
        buckets: &[f64],
    ) -> Result<PHistogram, MetricsError> {
        if let Some(entry) = self.handles.get(name) {
            if let Handle::Histogram(h) = entry.value() {
                return Ok(h.clone());
            }
            return Err(MetricsError::TypeMismatch {
                name: name.to_owned(),
            });
        }
        match self.handles.entry(name.to_owned()) {
            Entry::Occupied(occ) => match occ.get() {
                Handle::Histogram(h) => Ok(h.clone()),
                _ => Err(MetricsError::TypeMismatch {
                    name: name.to_owned(),
                }),
            },
            Entry::Vacant(vac) => {
                let h = PHistogram::with_opts(
                    HistogramOpts::new(name, help).buckets(buckets.to_vec()),
                )?;
                self.inner.register(Box::new(h.clone()))?;
                vac.insert(Handle::Histogram(h.clone()));
                self.check_cardinality();
                Ok(h)
            }
        }
    }

    /// Get-or-create a labeled [`HistogramVec`].
    ///
    /// # Errors
    ///
    /// See [`Self::counter`].
    pub fn histogram_vec(
        &self,
        name: &str,
        help: &str,
        labels: &[&str],
        buckets: &[f64],
    ) -> Result<HistogramVec, MetricsError> {
        if let Some(entry) = self.handles.get(name) {
            if let Handle::HistogramVec(h) = entry.value() {
                return Ok(h.clone());
            }
            return Err(MetricsError::TypeMismatch {
                name: name.to_owned(),
            });
        }
        match self.handles.entry(name.to_owned()) {
            Entry::Occupied(occ) => match occ.get() {
                Handle::HistogramVec(h) => Ok(h.clone()),
                _ => Err(MetricsError::TypeMismatch {
                    name: name.to_owned(),
                }),
            },
            Entry::Vacant(vac) => {
                let h = HistogramVec::new(
                    HistogramOpts::new(name, help).buckets(buckets.to_vec()),
                    labels,
                )?;
                self.inner.register(Box::new(h.clone()))?;
                vac.insert(Handle::HistogramVec(h.clone()));
                self.check_cardinality();
                Ok(h)
            }
        }
    }

    /// Get-or-create a labeled [`IntGaugeVec`]. The label set is FIXED at first registration.
    ///
    /// # Errors
    ///
    /// See [`Self::counter`].
    pub fn gauge_vec(
        &self,
        name: &str,
        help: &str,
        labels: &[&str],
    ) -> Result<IntGaugeVec, MetricsError> {
        if let Some(entry) = self.handles.get(name) {
            if let Handle::GaugeVec(g) = entry.value() {
                return Ok(g.clone());
            }
            return Err(MetricsError::TypeMismatch {
                name: name.to_owned(),
            });
        }
        match self.handles.entry(name.to_owned()) {
            Entry::Occupied(occ) => match occ.get() {
                Handle::GaugeVec(g) => Ok(g.clone()),
                _ => Err(MetricsError::TypeMismatch {
                    name: name.to_owned(),
                }),
            },
            Entry::Vacant(vac) => {
                let g = IntGaugeVec::new(Opts::new(name, help), labels)?;
                self.inner.register(Box::new(g.clone()))?;
                vac.insert(Handle::GaugeVec(g.clone()));
                self.check_cardinality();
                Ok(g)
            }
        }
    }

    /// Get-or-create an [`IntGauge`].
    ///
    /// # Errors
    ///
    /// See [`Self::counter`].
    pub fn gauge(&self, name: &str, help: &str) -> Result<IntGauge, MetricsError> {
        if let Some(entry) = self.handles.get(name) {
            if let Handle::Gauge(g) = entry.value() {
                return Ok(g.clone());
            }
            return Err(MetricsError::TypeMismatch {
                name: name.to_owned(),
            });
        }
        match self.handles.entry(name.to_owned()) {
            Entry::Occupied(occ) => match occ.get() {
                Handle::Gauge(g) => Ok(g.clone()),
                _ => Err(MetricsError::TypeMismatch {
                    name: name.to_owned(),
                }),
            },
            Entry::Vacant(vac) => {
                let g = IntGauge::with_opts(Opts::new(name, help))?;
                self.inner.register(Box::new(g.clone()))?;
                vac.insert(Handle::Gauge(g.clone()));
                self.check_cardinality();
                Ok(g)
            }
        }
    }

    /// Get-or-create the `accept_inflight{listener}` gauge family.
    ///
    /// # Errors
    ///
    /// The underlying `prometheus` registration error.
    pub fn accept_inflight_gauge(&self) -> Result<IntGaugeVec, MetricsError> {
        self.gauge_vec(
            "accept_inflight",
            "In-flight accepted connections currently held under the per-listener cap",
            &["listener"],
        )
    }

    /// Increment `accept_inflight{listener}`. BEST-EFFORT: a registration failure warns and drops
    /// the sample rather than failing the hot path.
    pub fn accept_inflight_inc(&self, listener: &str) {
        match self.accept_inflight_gauge() {
            Ok(g) => g.with_label_values(&[listener]).inc(),
            Err(e) => {
                tracing::warn!(metric = "accept_inflight", error = %e, "gauge inc failed");
            }
        }
    }

    /// Decrement `accept_inflight{listener}`; mirror of [`Self::accept_inflight_inc`].
    pub fn accept_inflight_dec(&self, listener: &str) {
        match self.accept_inflight_gauge() {
            Ok(g) => g.with_label_values(&[listener]).dec(),
            Err(e) => {
                tracing::warn!(metric = "accept_inflight", error = %e, "gauge dec failed");
            }
        }
    }

    /// Get-or-create the `panic_total` counter; the panic hook holds a clone of this handle.
    ///
    /// # Errors
    ///
    /// The underlying `prometheus` registration error.
    pub fn panic_total_counter(&self) -> Result<IntCounter, MetricsError> {
        self.counter(
            "panic_total",
            "Number of panics caught by the process-wide hook since startup.",
        )
    }

    /// Snapshot the registered metric families.
    #[must_use]
    pub fn gather(&self) -> Vec<MetricFamily> {
        self.inner.gather()
    }

    /// The inner registry, for collectors the helpers do not cover.
    #[must_use]
    pub const fn inner(&self) -> &Registry {
        &self.inner
    }

    /// Increment a counter, creating it on first touch. The help string is the NAME — use
    /// [`Self::counter`] for real help text.
    pub fn increment(&self, name: &str, value: u64) {
        match self.counter(name, name) {
            Ok(c) => c.inc_by(value),
            Err(e) => {
                tracing::warn!(metric = %name, error = %e, "counter increment failed");
            }
        }
    }

    /// Read a counter's value; `None` if unknown or not a counter.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<u64> {
        let handle = self.handles.get(name)?.value().clone();
        if let Handle::Counter(c) = handle {
            return Some(c.get());
        }
        None
    }

    /// Distinct metric FAMILIES registered, not series.
    #[must_use]
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// Whether no metrics have been registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    fn check_cardinality(&self) {
        let total: usize = self
            .inner
            .gather()
            .iter()
            .map(|fam| fam.get_metric().len())
            .sum();
        if total > CARDINALITY_WARN_THRESHOLD {
            tracing::warn!(
                total_series = total,
                threshold = CARDINALITY_WARN_THRESHOLD,
                "metrics cardinality exceeds threshold; review label usage",
            );
        }
    }
}

/// HTTP latency buckets, 100 µs to ~10 s, matching the Prometheus guide's defaults.
#[must_use]
pub fn http_latency_buckets() -> Vec<f64> {
    vec![
        0.000_1, 0.000_5, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ]
}

/// Share a registry across tasks cheaply.
pub type SharedRegistry = Arc<MetricsRegistry>;

// Sole consumer of the `core::Collector` import; removing it orphans that `use`.
#[allow(dead_code)]
fn _force_collector_linkage(_: &dyn Collector) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_and_read_backcompat() {
        let reg = MetricsRegistry::new();
        assert!(reg.is_empty());

        reg.increment("requests_total", 1);
        reg.increment("requests_total", 1);
        assert_eq!(reg.get("requests_total"), Some(2));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn missing_counter_returns_none() {
        let reg = MetricsRegistry::new();
        assert_eq!(reg.get("nonexistent"), None);
    }

    #[test]
    fn counter_get_or_create_is_idempotent() {
        let reg = MetricsRegistry::new();
        let a = reg.counter("foo_total", "foo help").unwrap();
        let b = reg.counter("foo_total", "different help").unwrap();
        a.inc();
        b.inc();
        assert_eq!(a.get(), 2, "both handles must share state");
    }

    #[test]
    fn counter_vec_labels_increment_independently() {
        let reg = MetricsRegistry::new();
        let v = reg
            .counter_vec("requests_by_status", "requests by status", &["status"])
            .unwrap();
        v.with_label_values(&["200"]).inc();
        v.with_label_values(&["200"]).inc();
        v.with_label_values(&["500"]).inc();
        assert_eq!(v.with_label_values(&["200"]).get(), 2);
        assert_eq!(v.with_label_values(&["500"]).get(), 1);
    }

    #[test]
    fn histogram_observe_appears_in_exposition() {
        let reg = MetricsRegistry::new();
        let h = reg
            .histogram("latency_seconds", "latency", &[0.01, 0.1, 1.0])
            .unwrap();
        h.observe(0.05);
        h.observe(0.5);
        let text = prometheus_exposition::render_text(&reg);
        assert!(text.contains("latency_seconds_bucket"), "text was:\n{text}");
        assert!(
            text.contains("latency_seconds_count 2"),
            "text was:\n{text}"
        );
    }

    #[test]
    fn gather_snapshot_matches_registered_metrics() {
        let reg = MetricsRegistry::new();
        reg.counter("c1_total", "c1").unwrap().inc();
        reg.gauge("g1", "g1").unwrap().set(42);
        reg.histogram("h1_seconds", "h1", &[0.1, 1.0])
            .unwrap()
            .observe(0.2);
        let fams = reg.gather();
        let names: Vec<String> = fams.iter().map(|f| f.name().to_owned()).collect();
        assert!(names.contains(&"c1_total".to_string()));
        assert!(names.contains(&"g1".to_string()));
        assert!(names.contains(&"h1_seconds".to_string()));
    }

    #[test]
    fn type_mismatch_is_reported() {
        let reg = MetricsRegistry::new();
        reg.counter("same_name", "help").unwrap();
        let err = reg.gauge("same_name", "help").unwrap_err();
        assert!(matches!(err, MetricsError::TypeMismatch { .. }));
    }

    #[test]
    fn thread_safe_increment() {
        use std::sync::Arc as StdArc;

        let reg = StdArc::new(MetricsRegistry::new());
        let mut handles = Vec::new();

        for _ in 0..4 {
            let r = StdArc::clone(&reg);
            handles.push(std::thread::spawn(move || {
                for _ in 0..1000 {
                    r.increment("concurrent_total", 1);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(reg.get("concurrent_total"), Some(4000));
    }
}
