//! Metrics, tracing, and logging for the load balancer.
//!
//! The [`MetricsRegistry`] wraps a [`prometheus::Registry`] with a handle
//! cache so `counter("foo", "...")`, `histogram(...)`, and friends are
//! idempotent — repeat calls return the same registered handle. The
//! legacy `increment` / `get` pair is kept for back-compat; it now routes
//! to an `IntCounter` internally so old call sites continue to work.
//!
//! Exposition is handled by [`prometheus_exposition::render_text`]; the
//! admin HTTP listener that serves the text format lives in
//! [`admin_http`].
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
    HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
    proto::MetricFamily,
};

pub mod admin_http;
pub mod prometheus_exposition;

/// Soft cap on the number of series a single registry will hold before a
/// tracing warning is emitted. Purely advisory — registration still
/// succeeds past the threshold.
const CARDINALITY_WARN_THRESHOLD: usize = 10_000;

/// Internal handle-cache entries. Each variant keeps the fully-typed
/// handle that was registered under a given name so repeat
/// `counter("foo", "...")` calls return the same instance.
#[derive(Clone)]
enum Handle {
    Counter(IntCounter),
    CounterVec(IntCounterVec),
    Histogram(PHistogram),
    HistogramVec(HistogramVec),
    Gauge(IntGauge),
}

/// Errors raised when a metric name is already registered with a
/// different underlying type.
#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    /// The name is already registered under a different metric type
    /// (for example, the caller asked for `counter("foo", ...)` after
    /// `histogram("foo", ...)` was already installed).
    #[error("metric {name:?} already registered as a different type")]
    TypeMismatch {
        /// Offending metric name.
        name: String,
    },
    /// Registration failed inside `prometheus`.
    #[error("prometheus registration failed: {0}")]
    Prometheus(#[from] prometheus::Error),
}

/// A thread-safe metrics registry, backed by [`prometheus::Registry`].
///
/// Repeat calls to `counter(name, help)` return the same handle so
/// instrumentation sites can fetch their metric cheaply on every hit
/// without external caching.
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

    /// Get-or-create an [`IntCounter`] registered under `name`.
    ///
    /// If the name was previously registered as a counter the existing
    /// handle is returned unchanged — `help` is ignored on the hot path.
    ///
    /// # Errors
    ///
    /// Returns [`MetricsError::TypeMismatch`] if `name` is already
    /// registered under a different metric type, or
    /// [`MetricsError::Prometheus`] if the underlying `prometheus`
    /// registry rejects the registration (for example, invalid label
    /// name).
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
        // Slow path: take the shard write lock for this key so only one
        // thread runs prometheus registration; the others wait, observe
        // the inserted handle, and clone it. This eliminates the
        // get-or-create race that could split increments across two
        // separately-registered handles or drop them via AlreadyReg.
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

    /// Get-or-create a labeled [`IntCounterVec`]. Labels are fixed at
    /// registration time; a second call with the same name must use
    /// the same label set.
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

    /// Snapshot the registered metric families. Used by the text
    /// exposition path.
    #[must_use]
    pub fn gather(&self) -> Vec<MetricFamily> {
        self.inner.gather()
    }

    /// Direct access to the inner [`prometheus::Registry`] for callers
    /// that want to register collectors the helpers above don't cover
    /// (for example a process collector).
    #[must_use]
    pub const fn inner(&self) -> &Registry {
        &self.inner
    }

    /// Back-compat helper: increment a counter by `value`, creating it
    /// on first touch. The `help` string is the metric name itself —
    /// callers that want a proper help text should use [`Self::counter`]
    /// directly.
    pub fn increment(&self, name: &str, value: u64) {
        match self.counter(name, name) {
            Ok(c) => c.inc_by(value),
            Err(e) => {
                tracing::warn!(metric = %name, error = %e, "counter increment failed");
            }
        }
    }

    /// Back-compat helper: read the current value of a counter
    /// previously touched via [`Self::increment`] or [`Self::counter`].
    /// Returns `None` if the name is unknown or bound to a non-counter
    /// handle.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<u64> {
        let handle = self.handles.get(name)?.value().clone();
        if let Handle::Counter(c) = handle {
            return Some(c.get());
        }
        None
    }

    /// Number of distinct metric names registered (families, not
    /// individual series).
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

/// Convenience bucket set for HTTP request latency in seconds, spanning
/// 100 µs to ~10 s on a (roughly) geometric progression. Mirrors the
/// Prometheus exposition guide's default HTTP latency buckets.
#[must_use]
pub fn http_latency_buckets() -> Vec<f64> {
    vec![
        0.000_1, 0.000_5, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ]
}

/// Share a registry across tasks cheaply.
pub type SharedRegistry = Arc<MetricsRegistry>;

// Keep prometheus::core::Collector in scope so trait bounds in the
// Collector-returning helpers resolve on call sites that only import
// this crate.
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
        let names: Vec<String> = fams.iter().map(|f| f.get_name().to_owned()).collect();
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
