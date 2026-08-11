//! RED metrics label budget: `prometheus` registers unbounded label sets happily, and one stray
//! `{path = <request URI>}` saturates a scrape with millions of series. Two gates for two label
//! kinds — [`LabelBudget::check`] is a startup worst-case product check for CLOSED sets, and
//! [`EnforcedLabelBudget`] is the per-emission guard for OPEN sets driven by request data.

use std::collections::HashSet;
use std::fmt;
use std::sync::{Mutex, PoisonError};

/// Per-family worst-case series ceiling; text exposition degrades around a few hundred thousand.
pub const DEFAULT_MAX_LABEL_CARDINALITY: usize = 10_000;

/// Worst-case routes per listener. BOTH gates must use this value or the startup check under-counts.
pub const MAX_ROUTES_BUDGET: usize = 64;

/// Canonical `(family, label_keys)` vocabulary; `tests/red_label_budget.rs` diffs the live registry
/// against it, so anything unlisted is a regression.
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
    ("accept_inflight", &["listener"]),
    ("xdp_conntrack_full_total", &["family"]),
    ("xdp_conntrack_entries_current", &["family"]),
    ("xdp_conntrack_capacity", &["family"]),
    ("xdp_packets_total", &["action"]),
    ("xdp_bytes_total", &["direction"]),
    ("xdp_attached_mode", &["mode"]),
    ("xdp_sampler_errors_total", &["kind"]),
];

/// Raised by [`LabelBudget::check`] when a family would overflow its ceiling.
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

/// Worst-case series counts from the static config shape; deliberately NOT a full cross-product.
#[derive(Copy, Clone, Debug)]
pub struct LabelBudget {
    /// `cfg.listeners.len()` from the live config.
    pub listeners: usize,
    /// Worst-case backends per listener.
    pub backends_per_listener: usize,
    /// Worst-case routes per listener.
    pub routes_per_listener: usize,
    /// Per-family ceiling beyond which `check` refuses to boot.
    pub max_label_cardinality: usize,
}

impl LabelBudget {
    /// Build a budget from the live config shape.
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
            accept_inflight: self.listeners,
        }
    }

    /// Refuse to boot if any family exceeds its ceiling; reports the FIRST offender only.
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
            ("accept_inflight", w.accept_inflight),
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

/// Per-emission open-set cardinality guard for `route` / `backend` / `listener`: a misconfigured
/// route extractor can register millions of tuples between scrapes. A refused tuple is DROPPED, not
/// folded into an `"other"` bucket, because a placeholder masks the bug class.
pub struct EnforcedLabelBudget {
    family: &'static str,
    ceiling: usize,
    seen: Mutex<HashSet<u64>>,
}

impl fmt::Debug for EnforcedLabelBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnforcedLabelBudget")
            .field("family", &self.family)
            .field("ceiling", &self.ceiling)
            .field(
                "seen_len",
                &self.seen.lock().map(|s| s.len()).unwrap_or(usize::MAX),
            )
            .finish()
    }
}

/// Raised when an emit site would breach its per-family ceiling.
#[derive(Debug)]
pub enum CardinalityErr {
    /// The tuple was novel and the per-family budget is exhausted.
    Refused {
        /// Offending family.
        family: &'static str,
        /// Current size of the seen-set.
        seen: usize,
        /// Per-family ceiling.
        ceiling: usize,
    },
}

impl fmt::Display for CardinalityErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused {
                family,
                seen,
                ceiling,
            } => write!(
                f,
                "label cardinality refused: {family} would register a \
                 novel tuple but the per-family ceiling is exhausted \
                 (seen={seen}, ceiling={ceiling})",
            ),
        }
    }
}

impl std::error::Error for CardinalityErr {}

impl EnforcedLabelBudget {
    /// Build a guard for an open-set family with the given ceiling.
    #[must_use]
    pub fn new(family: &'static str, ceiling: usize) -> Self {
        Self {
            family,
            ceiling,
            seen: Mutex::new(HashSet::new()),
        }
    }

    fn hash_tuple(values: &[&str]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                0x1fu8.hash(&mut h);
            }
            v.hash(&mut h);
        }
        h.finish()
    }

    /// Admit a label tuple.
    pub fn admit(&self, values: &[&str]) -> Result<(), CardinalityErr> {
        let h = Self::hash_tuple(values);
        let mut guard = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.contains(&h) {
            return Ok(());
        }
        if guard.len() >= self.ceiling {
            return Err(CardinalityErr::Refused {
                family: self.family,
                seen: guard.len(),
                ceiling: self.ceiling,
            });
        }
        guard.insert(h);
        Ok(())
    }

    /// Current seen-set size, for `metrics_series_observed_total{family}`.
    #[must_use]
    pub fn observed(&self) -> usize {
        self.seen.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Per-family ceiling.
    #[must_use]
    pub const fn ceiling(&self) -> usize {
        self.ceiling
    }

    /// Family identifier the budget guards.
    #[must_use]
    pub const fn family(&self) -> &'static str {
        self.family
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
    /// `accept_inflight{listener}` — the accept-site saturation gauge.
    pub accept_inflight: usize,
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
        // 200 × 500 × 5 = 500_000, far over the ceiling.
        let b = LabelBudget::from_config_shape(200, 500, 1, DEFAULT_MAX_LABEL_CARDINALITY);
        let err = b.check().expect_err("budget should refuse oversize config");
        assert_eq!(err.family, "backend_requests_total");
        assert!(err.worst_case > err.ceiling);
    }

    #[test]
    fn canonical_label_table_covers_red_families() {
        // Re-asserted here so a rebase dropping these fails at unit-test time, not only integration.
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
    fn enforced_budget_admits_within_ceiling() {
        let g = EnforcedLabelBudget::new("http_requests_total", 3);
        assert!(g.admit(&["a", "h2", "2xx"]).is_ok());
        assert!(g.admit(&["b", "h2", "2xx"]).is_ok());
        assert!(g.admit(&["c", "h2", "2xx"]).is_ok());
        assert!(g.admit(&["a", "h2", "2xx"]).is_ok());
        assert_eq!(g.observed(), 3);
    }

    #[test]
    fn enforced_budget_refuses_overflow() {
        let g = EnforcedLabelBudget::new("http_requests_total", 2);
        g.admit(&["a"]).unwrap();
        g.admit(&["b"]).unwrap();
        let err = g
            .admit(&["c"])
            .expect_err("third novel tuple must refuse at ceiling=2");
        match err {
            CardinalityErr::Refused {
                family,
                seen,
                ceiling,
            } => {
                assert_eq!(family, "http_requests_total");
                assert_eq!(seen, 2);
                assert_eq!(ceiling, 2);
            }
        }
        assert!(g.admit(&["a"]).is_ok());
        assert_eq!(g.observed(), 2);
    }

    #[test]
    fn enforced_budget_distinguishes_arity_via_separator() {
        let g = EnforcedLabelBudget::new("http_requests_total", 4);
        g.admit(&["a", "b"]).unwrap();
        g.admit(&["ab"]).unwrap();
        assert_eq!(g.observed(), 2);
    }

    #[test]
    fn max_routes_budget_constant_matches_plan() {
        assert_eq!(MAX_ROUTES_BUDGET, 64);
    }

    #[test]
    fn xdp_label_keys_locked_to_action_family_direction_mode() {
        // These label keys are WIRE-STABLE — renaming one breaks every existing dashboard.
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
