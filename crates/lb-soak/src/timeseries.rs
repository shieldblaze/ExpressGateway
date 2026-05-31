//! Time-series collection + the BOUNDED/DRIFT trend analyzer.
//!
//! This is the heart of the soak verdict (R8): given a per-metric series
//! sampled over the run, decide whether the metric stayed **bounded** (flat /
//! oscillating around a level — PASS) or **drifted** upward (a leak / unbounded
//! growth — a FINDING). It is the most verdict-critical module in the suite, so
//! the analyzer is unit-tested on synthetic flat / rising / sawtooth / warmup
//! series and the thresholds are documented inline.
//!
//! R15: the verdict is computed over the WHOLE completed series; the analyzer
//! also surfaces every input number (medians, slope, growth) so the verdict in
//! `verdict.json` is auditable, not a bare label.

use std::fmt::Write as _;

/// How a metric column should be judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// Should not climb over the run (RSS, fds, threads, and bounded-state
    /// gauges that oscillate around a level). Judged by the trend test.
    Trend,
    /// Monotonic counter (drops/evictions). Never "flat"; judged by reporting
    /// its rate. Always BOUNDED unless its rate is accelerating.
    Counter,
    /// A counter that MUST stay zero (e.g. `panic_total`). Any positive final
    /// value is a DRIFT/finding.
    CounterMustBeZero,
}

/// Per-column verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Bounded / flat — PASS for this metric.
    Bounded,
    /// Upward drift — a FINDING for this metric.
    Drift,
    /// Too few samples to judge.
    Inconclusive,
}

impl Verdict {
    /// Stable string for JSON/CSV.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Verdict::Bounded => "BOUNDED",
            Verdict::Drift => "DRIFT",
            Verdict::Inconclusive => "INCONCLUSIVE",
        }
    }
}

/// Thresholds for the trend test. Defaults are the documented soak bands.
#[derive(Debug, Clone, Copy)]
pub struct TrendConfig {
    /// Fraction of leading samples discarded as warmup before judging
    /// steady-state (post-warmup leaks are the target; warmup growth is not a
    /// leak). Default 0.10.
    pub warmup_frac: f64,
    /// Relative-growth band: last-third median may exceed first-third median by
    /// up to this fraction and still be BOUNDED. Default 0.10 (10%).
    pub band: f64,
    /// Minimum monotone fraction (share of non-negative consecutive deltas)
    /// required to call a growth a consistent climb (vs. noise). Default 0.60.
    pub monotone_min: f64,
    /// Minimum trimmed-sample count to render a verdict. Default 8.
    pub min_samples: usize,
}

impl Default for TrendConfig {
    fn default() -> Self {
        Self {
            warmup_frac: 0.10,
            band: 0.10,
            monotone_min: 0.60,
            min_samples: 8,
        }
    }
}

/// Full evidence behind one column's verdict (serialized into `verdict.json`).
#[derive(Debug, Clone)]
pub struct ColumnVerdict {
    pub column: String,
    pub kind_str: &'static str,
    pub verdict: Verdict,
    pub n: usize,
    pub first: f64,
    pub last: f64,
    pub min: f64,
    pub max: f64,
    /// Median of the first third of the warmup-trimmed series.
    pub first_third_median: f64,
    /// Median of the last third of the warmup-trimmed series.
    pub last_third_median: f64,
    /// (last_third_median - first_third_median) / max(first_third_median, eps).
    pub rel_growth: f64,
    /// Least-squares slope per sample over the trimmed series.
    pub slope_per_sample: f64,
    /// Fraction of consecutive deltas that are >= 0 (climb consistency).
    pub monotone_frac: f64,
    /// One-line human explanation.
    pub note: String,
}

/// A column-oriented time-series: a fixed set of named columns plus rows
/// carrying a timestamp and one value per column.
#[derive(Debug, Clone)]
pub struct TimeSeries {
    columns: Vec<String>,
    t: Vec<f64>,
    /// `rows[i][c]` is the value of column `c` at sample `i`.
    rows: Vec<Vec<f64>>,
}

impl TimeSeries {
    /// Create an empty series with the given column names (order is the CSV
    /// column order, after the leading `t_secs`).
    #[must_use]
    pub fn new(columns: Vec<String>) -> Self {
        Self {
            columns,
            t: Vec::new(),
            rows: Vec::new(),
        }
    }

    /// Column names (without the leading `t_secs`).
    #[must_use]
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Number of samples recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.t.len()
    }

    /// Whether no samples have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.t.is_empty()
    }

    /// Append a sample. `values` must align with `columns` (extra values are
    /// ignored; missing trailing values are filled with NaN so a transient
    /// scrape miss does not desync the columns).
    pub fn push(&mut self, t_secs: f64, mut values: Vec<f64>) {
        values.resize(self.columns.len(), f64::NAN);
        values.truncate(self.columns.len());
        self.t.push(t_secs);
        self.rows.push(values);
    }

    /// Extract one column's values (NaNs included).
    #[must_use]
    pub fn column_values(&self, col: &str) -> Option<Vec<f64>> {
        let idx = self.columns.iter().position(|c| c == col)?;
        Some(
            self.rows
                .iter()
                .map(|r| r.get(idx).copied().unwrap_or(f64::NAN))
                .collect(),
        )
    }

    /// Render to CSV (`t_secs,<cols...>`). NaNs render as empty cells.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        out.push_str("t_secs");
        for c in &self.columns {
            let _ = write!(out, ",{c}");
        }
        out.push('\n');
        for (i, t) in self.t.iter().enumerate() {
            let _ = write!(out, "{t:.3}");
            if let Some(row) = self.rows.get(i) {
                for v in row {
                    if v.is_nan() {
                        out.push(',');
                    } else {
                        let _ = write!(out, ",{v}");
                    }
                }
            }
            out.push('\n');
        }
        out
    }

    /// Analyze every column under the given per-column [`MetricKind`] (a column
    /// not present in `kinds` defaults to [`MetricKind::Trend`]).
    #[must_use]
    pub fn analyze(&self, cfg: &TrendConfig, kinds: &[(String, MetricKind)]) -> Vec<ColumnVerdict> {
        self.columns
            .iter()
            .map(|c| {
                let kind = kinds
                    .iter()
                    .find(|(name, _)| name == c)
                    .map_or(MetricKind::Trend, |(_, k)| *k);
                let vals = self.column_values(c).unwrap_or_default();
                analyze_column(c, &vals, kind, cfg)
            })
            .collect()
    }
}

/// Median of a slice (NaNs filtered). Returns 0.0 for an empty input.
fn median(xs: &[f64]) -> f64 {
    let mut v: Vec<f64> = xs.iter().copied().filter(|x| !x.is_nan()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Least-squares slope of `ys` against sample index 0..n. NaNs are skipped
/// (their indices too). Returns 0.0 if fewer than two finite points.
fn slope(ys: &[f64]) -> f64 {
    let pts: Vec<(f64, f64)> = ys
        .iter()
        .enumerate()
        .filter(|(_, y)| !y.is_nan())
        .map(|(i, y)| (i as f64, *y))
        .collect();
    let n = pts.len() as f64;
    if pts.len() < 2 {
        return 0.0;
    }
    let sx: f64 = pts.iter().map(|(x, _)| x).sum();
    let sy: f64 = pts.iter().map(|(_, y)| y).sum();
    let sxx: f64 = pts.iter().map(|(x, _)| x * x).sum();
    let sxy: f64 = pts.iter().map(|(x, y)| x * y).sum();
    let denom = n * sxx - sx * sx;
    if denom.abs() < f64::EPSILON {
        return 0.0;
    }
    (n * sxy - sx * sy) / denom
}

/// Fraction of consecutive deltas that are >= 0 (climb consistency). A steady
/// leak approaches 1.0; symmetric noise approaches 0.5.
fn monotone_frac(ys: &[f64]) -> f64 {
    let finite: Vec<f64> = ys.iter().copied().filter(|y| !y.is_nan()).collect();
    if finite.len() < 2 {
        return 0.0;
    }
    let mut nonneg = 0usize;
    let mut total = 0usize;
    for w in finite.windows(2) {
        total += 1;
        if w[1] - w[0] >= 0.0 {
            nonneg += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        nonneg as f64 / total as f64
    }
}

/// Judge a single column.
#[must_use]
pub fn analyze_column(
    column: &str,
    values: &[f64],
    kind: MetricKind,
    cfg: &TrendConfig,
) -> ColumnVerdict {
    let finite: Vec<f64> = values.iter().copied().filter(|v| !v.is_nan()).collect();
    let n = finite.len();
    let first = finite.first().copied().unwrap_or(0.0);
    let last = finite.last().copied().unwrap_or(0.0);
    let min = finite.iter().copied().fold(f64::INFINITY, f64::min);
    let max = finite.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (min, max) = if n == 0 { (0.0, 0.0) } else { (min, max) };

    // CounterMustBeZero: any positive value at all is a finding.
    if kind == MetricKind::CounterMustBeZero {
        let v = if max > 0.0 {
            Verdict::Drift
        } else {
            Verdict::Bounded
        };
        let note = if max > 0.0 {
            format!("{column}: must-be-zero counter reached {max}")
        } else {
            format!("{column}: stayed zero across the run")
        };
        return ColumnVerdict {
            column: column.into(),
            kind_str: "counter_must_be_zero",
            verdict: v,
            n,
            first,
            last,
            min,
            max,
            first_third_median: 0.0,
            last_third_median: 0.0,
            rel_growth: 0.0,
            slope_per_sample: slope(&finite),
            monotone_frac: 1.0,
            note,
        };
    }

    // Warmup-trim then split into thirds.
    let trim = ((n as f64) * cfg.warmup_frac).floor() as usize;
    let trimmed = if trim < n {
        &finite[trim..]
    } else {
        &finite[..]
    };
    let tn = trimmed.len();

    if tn < cfg.min_samples {
        return ColumnVerdict {
            column: column.into(),
            kind_str: kind_str(kind),
            verdict: Verdict::Inconclusive,
            n,
            first,
            last,
            min,
            max,
            first_third_median: 0.0,
            last_third_median: 0.0,
            rel_growth: 0.0,
            slope_per_sample: slope(trimmed),
            monotone_frac: monotone_frac(trimmed),
            note: format!(
                "{column}: only {tn} steady-state samples (< {} required)",
                cfg.min_samples
            ),
        };
    }

    let third = tn / 3;
    let f3 = median(&trimmed[..third.max(1)]);
    let l3 = median(&trimmed[tn - third.max(1)..]);
    let eps = 1.0; // gauges are integer counts; 1 avoids div-by-zero blowups.
    let rel_growth = (l3 - f3) / f3.abs().max(eps);
    let slp = slope(trimmed);
    let mono = monotone_frac(trimmed);

    let (verdict, note) = match kind {
        MetricKind::Counter => {
            // Counters only rise; the question is whether the RATE is bounded.
            // We report it as BOUNDED (the absolute bound, e.g. drop-newest, is
            // asserted separately by the scenario against the product cap). An
            // accelerating counter — second half climbing much faster than the
            // first — is flagged as DRIFT.
            let first_half = slope(&trimmed[..tn / 2]);
            let second_half = slope(&trimmed[tn / 2..]);
            // A constant-rate counter (e.g. drop-newest under a sustained
            // flood) has first/second-half slopes ~equal (ratio ~1.0). A
            // genuinely unbounded/accelerating counter has a second-half slope
            // materially steeper. 1.8x cleanly separates the two (quadratic
            // growth is ~2.4x in a typical window; linear is 1.0x).
            if second_half > first_half * 1.8 + eps && second_half > 0.0 {
                (
                    Verdict::Drift,
                    format!(
                        "{column}: counter rate accelerating (slope {first_half:.3}→{second_half:.3}/sample)"
                    ),
                )
            } else {
                (
                    Verdict::Bounded,
                    format!(
                        "{column}: counter rose {} over run at bounded rate (slope {slp:.4}/sample)",
                        last - first
                    ),
                )
            }
        }
        MetricKind::Trend => {
            if rel_growth > cfg.band && mono >= cfg.monotone_min {
                (
                    Verdict::Drift,
                    format!(
                        "{column}: drift — last-third median {l3} vs first-third {f3} (+{:.1}%), \
                         monotone {:.0}% , slope {slp:.4}/sample",
                        rel_growth * 100.0,
                        mono * 100.0
                    ),
                )
            } else {
                (
                    Verdict::Bounded,
                    format!(
                        "{column}: bounded — last-third median {l3} vs first-third {f3} \
                         ({:+.1}%, within band {:.0}% or non-monotone {:.0}%)",
                        rel_growth * 100.0,
                        cfg.band * 100.0,
                        mono * 100.0
                    ),
                )
            }
        }
        MetricKind::CounterMustBeZero => unreachable!("handled above"),
    };

    ColumnVerdict {
        column: column.into(),
        kind_str: kind_str(kind),
        verdict,
        n,
        first,
        last,
        min,
        max,
        first_third_median: f3,
        last_third_median: l3,
        rel_growth,
        slope_per_sample: slp,
        monotone_frac: mono,
        note,
    }
}

const fn kind_str(k: MetricKind) -> &'static str {
    match k {
        MetricKind::Trend => "trend",
        MetricKind::Counter => "counter",
        MetricKind::CounterMustBeZero => "counter_must_be_zero",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TrendConfig {
        TrendConfig::default()
    }

    #[test]
    fn flat_series_is_bounded() {
        // RSS hovering around 50_000 KiB with small noise.
        let vals: Vec<f64> = (0..40)
            .map(|i| 50_000.0 + if i % 2 == 0 { 50.0 } else { -50.0 })
            .collect();
        let v = analyze_column("rss_kb", &vals, MetricKind::Trend, &cfg());
        assert_eq!(
            v.verdict,
            Verdict::Bounded,
            "flat must be BOUNDED: {}",
            v.note
        );
    }

    #[test]
    fn linear_climb_is_drift() {
        // A real leak: RSS rising ~1% per sample, steadily.
        let vals: Vec<f64> = (0..40).map(|i| 50_000.0 + (i as f64) * 500.0).collect();
        let v = analyze_column("rss_kb", &vals, MetricKind::Trend, &cfg());
        assert_eq!(
            v.verdict,
            Verdict::Drift,
            "steady climb must be DRIFT: {}",
            v.note
        );
        assert!(v.rel_growth > 0.10);
        assert!(v.monotone_frac > 0.9);
    }

    #[test]
    fn sawtooth_around_constant_is_bounded() {
        // Connection table under churn: a clean sawtooth rising 0..190 then
        // resetting every 20 samples — high monotone fraction (mostly +10
        // steps) but NO net upward trend, so first-/last-third medians match.
        // This is the case the rel-growth gate must protect against a false
        // leak even when the climb-consistency is high.
        let vals: Vec<f64> = (0..60).map(|i| ((i % 20) * 10) as f64).collect();
        let v = analyze_column("conns", &vals, MetricKind::Trend, &cfg());
        assert_eq!(
            v.verdict,
            Verdict::Bounded,
            "sawtooth must be BOUNDED: {}",
            v.note
        );
    }

    #[test]
    fn warmup_spike_then_flat_is_bounded() {
        // High warmup then settles flat — trimming the warmup avoids a false
        // leak. Without trimming, first-third would be low and last-third
        // high. The warmup here is the first 10%.
        let mut vals: Vec<f64> = vec![10_000.0, 80_000.0, 70_000.0, 60_000.0];
        vals.extend((0..40).map(|i| 50_000.0 + if i % 2 == 0 { 30.0 } else { -30.0 }));
        let v = analyze_column("rss_kb", &vals, MetricKind::Trend, &cfg());
        assert_eq!(
            v.verdict,
            Verdict::Bounded,
            "warmup spike must not be DRIFT: {}",
            v.note
        );
    }

    #[test]
    fn counter_constant_rate_is_bounded() {
        // Datagram drops accumulating at a constant rate (drop-newest under a
        // sustained flood) — bounded, not accelerating.
        let vals: Vec<f64> = (0..40).map(|i| (i as f64) * 10.0).collect();
        let v = analyze_column("drops_total", &vals, MetricKind::Counter, &cfg());
        assert_eq!(
            v.verdict,
            Verdict::Bounded,
            "constant-rate counter BOUNDED: {}",
            v.note
        );
    }

    #[test]
    fn counter_accelerating_is_drift() {
        // Quadratic growth — rate doubling — flags unbounded behaviour.
        let vals: Vec<f64> = (0..40).map(|i| (i as f64).powi(2)).collect();
        let v = analyze_column("growth_total", &vals, MetricKind::Counter, &cfg());
        assert_eq!(
            v.verdict,
            Verdict::Drift,
            "accelerating counter DRIFT: {}",
            v.note
        );
    }

    #[test]
    fn panic_counter_zero_is_bounded() {
        let vals: Vec<f64> = vec![0.0; 30];
        let v = analyze_column("panic_total", &vals, MetricKind::CounterMustBeZero, &cfg());
        assert_eq!(v.verdict, Verdict::Bounded);
    }

    #[test]
    fn panic_counter_nonzero_is_drift() {
        let mut vals: Vec<f64> = vec![0.0; 20];
        vals.push(1.0);
        vals.extend(vec![1.0; 9]);
        let v = analyze_column("panic_total", &vals, MetricKind::CounterMustBeZero, &cfg());
        assert_eq!(
            v.verdict,
            Verdict::Drift,
            "any panic must be DRIFT: {}",
            v.note
        );
    }

    #[test]
    fn too_few_samples_inconclusive() {
        let vals: Vec<f64> = vec![1.0, 2.0, 3.0];
        let v = analyze_column("x", &vals, MetricKind::Trend, &cfg());
        assert_eq!(v.verdict, Verdict::Inconclusive);
    }

    #[test]
    fn csv_roundtrip_shape() {
        let mut ts = TimeSeries::new(vec!["rss_kb".into(), "fds".into()]);
        ts.push(0.0, vec![100.0, 5.0]);
        ts.push(15.0, vec![110.0, 5.0]);
        let csv = ts.to_csv();
        assert_eq!(csv.lines().next().unwrap(), "t_secs,rss_kb,fds");
        assert_eq!(csv.lines().count(), 3, "header + 2 rows");
        assert_eq!(ts.len(), 2);
        assert_eq!(ts.column_values("fds").unwrap(), vec![5.0, 5.0]);
    }

    #[test]
    fn push_fills_missing_columns_with_nan() {
        // A scrape miss (short row) must not desync columns.
        let mut ts = TimeSeries::new(vec!["a".into(), "b".into(), "c".into()]);
        ts.push(0.0, vec![1.0]); // only column a present
        let row = ts.column_values("c").unwrap();
        assert!(row[0].is_nan(), "missing column must be NaN, kept aligned");
        // NaN renders as an empty cell.
        let csv = ts.to_csv();
        assert!(csv.lines().nth(1).unwrap().starts_with("0.000,1,,"));
    }
}
