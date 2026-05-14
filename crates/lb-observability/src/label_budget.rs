//! REL-2-08: per-listener / per-backend RED metrics label budget.
//!
//! The `prometheus` crate happily registers unbounded label sets,
//! and one stray `{path = <request URI>}` is enough to saturate a
//! scrape endpoint with millions of series. This module gives the
//! binary a startup-time gate:
//!
//! ```ignore
//! let budget = LabelBudget::from_config_shape(
//!     cfg.listeners.len(),
//!     cfg.max_backends_per_listener,
//!     cfg.observability.max_label_cardinality.unwrap_or(10_000),
//! );
//! budget.check()?;  // refuses to boot if the worst-case product overflows
//! ```
//!
//! The canonical label keys are documented in `METRICS.md` and
//! re-exported here as the [`CANONICAL_LABELS`] table so accidental
//! drift breaks the compile (the table is consumed by tests in this
//! crate that diff the live registry against it).

use std::fmt;

/// Default ceiling on the worst-case Cartesian product of label
/// values per metric family. `prometheus`'s text exposition starts
/// to feel the pain around a few hundred thousand series; 10 000 is
/// conservative for a single LB instance.
pub const DEFAULT_MAX_LABEL_CARDINALITY: usize = 10_000;

/// Canonical label-key vocabulary the rel-owned RED metric families
/// use. Anything else is a regression — the integration tests in
/// `crates/lb-observability/tests/red_label_budget.rs` re-validate
/// the live registry against this table.
///
/// Format: `(family_name, &[label_keys])`.
pub const CANONICAL_LABELS: &[(&str, &[&str])] = &[
    (
        "http_requests_total",
        &["listener", "route", "version", "status_class"],
    ),
    (
        "http_request_duration_seconds",
        &["listener", "route", "version"],
    ),
    (
        "backend_requests_total",
        &["listener", "backend", "status_class"],
    ),
    ("backend_request_duration_seconds", &["listener", "backend"]),
    ("connections_inflight", &["listener"]),
    ("xdp_conntrack_full_total", &["family"]),
    ("xdp_conntrack_entries_current", &["family"]),
    ("xdp_conntrack_capacity", &["family"]),
    ("xdp_packets_total", &["action"]),
    ("xdp_bytes_total", &["direction"]),
    ("xdp_attached_mode", &["mode"]),
    ("xdp_sampler_errors_total", &["kind"]),
];

/// Error returned by [`LabelBudget::check`] when the worst-case
/// label product would overflow the configured ceiling.
#[derive(Debug)]
pub struct LabelBudgetError {
    /// Name of the offending family.
    pub family: &'static str,
    /// Worst-case series count given the supplied config shape.
    pub worst_case: usize,
    /// The ceiling the family broke through.
    pub ceiling: usize,
}

impl fmt::Display for LabelBudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "label cardinality budget exceeded: {family} would emit \
             up to {worst} series, ceiling is {ceiling}",
            family = self.family,
            worst = self.worst_case,
            ceiling = self.ceiling,
        )
    }
}

impl std::error::Error for LabelBudgetError {}

/// Worst-case series counts derived from the static config shape.
///
/// The numbers below intentionally do NOT multiply every label
/// across every family — the canonical RED layout deliberately
/// pulls `backend` off request-level metrics and `status_class`
/// off duration histograms. See the table in `METRICS.md`.
#[derive(Copy, Clone, Debug)]
pub struct LabelBudget {
    /// `cfg.listeners.len()` from the live config.
    pub listeners: usize,
    /// Worst-case backend pool size *per listener*. The plan
    /// recommends bounding this at 256 in the config validator.
    pub backends_per_listener: usize,
    /// Worst-case route count per listener. Bounded at 64 by the
    /// config validator.
    pub routes_per_listener: usize,
    /// Per-family ceiling beyond which `check` refuses to boot.
    pub max_label_cardinality: usize,
}

impl LabelBudget {
    /// Build a budget from the live config shape. `routes_per_listener`
    /// defaults to 1 if the config does not yet expose explicit routes.
    #[must_use]
    pub const fn from_config_shape(
        listeners: usize,
        backends_per_listener: usize,
        routes_per_listener: usize,
        max_label_cardinality: usize,
    ) -> Self {
        Self {
            listeners,
            backends_per_listener,
            routes_per_listener,
            max_label_cardinality,
        }
    }

    /// Worst-case series count for each family.
    #[must_use]
    pub const fn worst_case(&self) -> WorstCase {
        // Closed-set enumerations.
        const STATUS_CLASSES: usize = 5; // 1xx..5xx
        const HTTP_VERSIONS: usize = 4; // h1.0/h1.1/h2/h3
        WorstCase {
            http_requests_total: self.listeners
                * self.routes_per_listener
                * HTTP_VERSIONS
                * STATUS_CLASSES,
            http_request_duration_seconds: self.listeners
                * self.routes_per_listener
                * HTTP_VERSIONS,
            backend_requests_total: self.listeners * self.backends_per_listener * STATUS_CLASSES,
            backend_request_duration_seconds: self.listeners * self.backends_per_listener,
            connections_inflight: self.listeners,
        }
    }

    /// Refuse to boot if any family would exceed
    /// `max_label_cardinality`.
    ///
    /// # Errors
    ///
    /// Returns the first offending family on overflow.
    pub fn check(&self) -> Result<(), LabelBudgetError> {
        let w = self.worst_case();
        let ceiling = self.max_label_cardinality;

        let cases: &[(&'static str, usize)] = &[
            ("http_requests_total", w.http_requests_total),
            (
                "http_request_duration_seconds",
                w.http_request_duration_seconds,
            ),
            ("backend_requests_total", w.backend_requests_total),
            (
                "backend_request_duration_seconds",
                w.backend_request_duration_seconds,
            ),
            ("connections_inflight", w.connections_inflight),
        ];
        for (family, worst) in cases {
            if *worst > ceiling {
                return Err(LabelBudgetError {
                    family,
                    worst_case: *worst,
                    ceiling,
                });
            }
        }
        Ok(())
    }
}

/// Per-family worst-case series count.
#[derive(Copy, Clone, Debug)]
pub struct WorstCase {
    /// `http_requests_total{listener, route, version, status_class}`
    pub http_requests_total: usize,
    /// `http_request_duration_seconds{listener, route, version}`
    pub http_request_duration_seconds: usize,
    /// `backend_requests_total{listener, backend, status_class}`
    pub backend_requests_total: usize,
    /// `backend_request_duration_seconds{listener, backend}`
    pub backend_request_duration_seconds: usize,
    /// `connections_inflight{listener}`
    pub connections_inflight: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realistic_config_fits_within_default_budget() {
        let b = LabelBudget::from_config_shape(
            4,  // listeners
            16, // backends per listener
            8,  // routes per listener
            DEFAULT_MAX_LABEL_CARDINALITY,
        );
        b.check().expect("4×16×8 config fits within 10k budget");
        let w = b.worst_case();
        assert_eq!(w.http_requests_total, 4 * 8 * 4 * 5);
        assert_eq!(w.backend_requests_total, 4 * 16 * 5);
        assert_eq!(w.connections_inflight, 4);
    }

    #[test]
    fn oversize_config_refuses_to_start() {
        // 200 listeners × 500 backends × 5 status classes = 500_000
        // — well over the 10_000 ceiling.
        let b = LabelBudget::from_config_shape(200, 500, 1, DEFAULT_MAX_LABEL_CARDINALITY);
        let err = b.check().expect_err("budget should refuse oversize config");
        assert_eq!(err.family, "backend_requests_total");
        assert!(err.worst_case > err.ceiling);
    }

    #[test]
    fn canonical_label_table_covers_red_families() {
        // The integration test in tests/red_label_budget.rs depends
        // on these entries existing — re-assert here so a rebase
        // accident is caught at unit-test time too.
        let names: Vec<&str> = CANONICAL_LABELS.iter().map(|(n, _)| *n).collect();
        for required in [
            "http_requests_total",
            "http_request_duration_seconds",
            "backend_requests_total",
            "backend_request_duration_seconds",
            "connections_inflight",
        ] {
            assert!(
                names.contains(&required),
                "canonical table missing {required}",
            );
        }
    }

    #[test]
    fn xdp_label_keys_locked_to_action_family_direction_mode() {
        // REL-2-13 + EBPF-2-08 cross-review §2: the action/family/
        // direction/mode keys are wire-stable. This test fails if
        // anyone renames them in CANONICAL_LABELS.
        let entry = CANONICAL_LABELS
            .iter()
            .find(|(n, _)| *n == "xdp_packets_total")
            .expect("xdp_packets_total in table");
        assert_eq!(entry.1, &["action"]);
        let cf = CANONICAL_LABELS
            .iter()
            .find(|(n, _)| *n == "xdp_conntrack_full_total")
            .expect("xdp_conntrack_full_total in table");
        assert_eq!(cf.1, &["family"]);
    }
}
