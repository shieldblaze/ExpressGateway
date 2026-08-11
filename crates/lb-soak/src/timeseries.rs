//! Time-series collection + the BOUNDED/DRIFT trend analyzer that produces the soak verdict (R8).

use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// Should not climb over the run (RSS, fds, threads, and bounded-state gauges that oscillate around a level).
    Trend,
    /// Monotonic counter (drops/evictions).
    Counter,
    /// A counter that MUST stay zero (e.g. `panic_total`).
    CounterMustBeZero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Bounded,
    Drift,
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

#[derive(Debug, Clone, Copy)]
pub struct TrendConfig {
    /// Leading fraction discarded as warmup before judging steady state. Default 0.10.
    pub warmup_frac: f64,
    /// Last-third median may exceed the first-third median by this fraction and stay BOUNDED. Default 0.10.
    pub band: f64,
    /// Minimum monotone fraction (non-negative deltas) to call a climb consistent, not noise. Default 0.60.
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
    pub first_third_median: f64,
    pub last_third_median: f64,
    pub rel_growth: f64,
    pub slope_per_sample: f64,
    pub monotone_frac: f64,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct TimeSeries {
    columns: Vec<String>,
    t: Vec<f64>,
    rows: Vec<Vec<f64>>,
}

impl TimeSeries {
    #[must_use]
    pub fn new(columns: Vec<String>) -> Self {
        Self {
            columns,
            t: Vec::new(),
            rows: Vec::new(),
        }
    }

    #[must_use]
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.t.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.t.is_empty()
    }

    /// Append a sample. Missing trailing values are NaN-filled so a transient scrape miss
    /// does not desync the columns.
    pub fn push(&mut self, t_secs: f64, mut values: Vec<f64>) {
        values.resize(self.columns.len(), f64::NAN);
        values.truncate(self.columns.len());
        self.t.push(t_secs);
        self.rows.push(values);
    }

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

    /// Analyze every column under the given per-column [`MetricKind`] (a column not present in `kinds` defaults to [`MetricKind::Trend`]).
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
            // A constant-rate counter has first/second-half slopes ~equal; 1.8x cleanly
            // separates linear (1.0x) from quadratic (~2.4x) growth, so only an
            // accelerating counter is DRIFT.
            let first_half = slope(&trimmed[..tn / 2]);
            let second_half = slope(&trimmed[tn / 2..]);
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
        // Sawtooth: high monotone fraction but NO net trend. The rel-growth gate must
        // keep this from reading as a leak.
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
        // High warmup then flat: without trimming, first-third low / last-third high.
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
        let mut ts = TimeSeries::new(vec!["a".into(), "b".into(), "c".into()]);
        ts.push(0.0, vec![1.0]); // only column a present
        let row = ts.column_values("c").unwrap();
        assert!(row[0].is_nan(), "missing column must be NaN, kept aligned");
        let csv = ts.to_csv();
        assert!(csv.lines().nth(1).unwrap().starts_with("0.000,1,,"));
    }
}
