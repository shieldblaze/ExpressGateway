//! Minimal Prometheus text-exposition (0.0.4) parser, scoped to what the soak
//! sampler needs: pull named gauge/counter values out of the gateway's
//! `/metrics` body so the soak can watch state-table sizes over time.
//!
//! Verdict-critical: the state-table gauges (`quic_modeb_streams_active`,
//! `quic_passthrough_flows`, `accept_inflight{listener=…}`, …) are the
//! product's own bounded-state signal. A wrong parse = a wrong bound verdict,
//! so the parser is unit-tested on a captured exposition body that includes
//! HELP/TYPE comments, labelled series, floats, and counters.
//!
//! This is intentionally NOT a full Prometheus parser — it handles the subset
//! the product actually emits (no exemplars, no histogram-bucket reassembly
//! beyond treating each line as an independent series).

/// One exposition line: a metric family name, its label set, and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// Metric family name (e.g. `quic_modeb_streams_active`).
    pub name: String,
    /// Label key/value pairs, in source order. Empty for unlabelled series.
    pub labels: Vec<(String, String)>,
    /// Parsed value. `NaN`/`+Inf`/`-Inf` are preserved.
    pub value: f64,
}

/// A parsed `/metrics` body.
#[derive(Debug, Clone, Default)]
pub struct MetricSet {
    /// Every value-bearing line, in source order.
    pub samples: Vec<Sample>,
}

impl MetricSet {
    /// Sum the values of every series whose family name is `name`. Returns
    /// `None` if the name is absent (distinct from `Some(0.0)`, which means
    /// present-and-zero). Summing makes a labelled gauge like
    /// `accept_inflight{listener=…}` collapse to a single total — the bound
    /// signal the soak wants.
    #[must_use]
    pub fn sum(&self, name: &str) -> Option<f64> {
        let mut found = false;
        let mut total = 0.0;
        for s in &self.samples {
            if s.name == name {
                found = true;
                total += s.value;
            }
        }
        found.then_some(total)
    }

    /// The maximum value across series with family name `name` (e.g. the
    /// largest per-listener `accept_inflight`). `None` if absent.
    #[must_use]
    pub fn max(&self, name: &str) -> Option<f64> {
        self.samples
            .iter()
            .filter(|s| s.name == name)
            .map(|s| s.value)
            .fold(None, |acc, v| Some(acc.map_or(v, |a: f64| a.max(v))))
    }
}

/// Parse a Prometheus text-exposition body into a [`MetricSet`].
///
/// Skips blank lines and `#`-prefixed HELP/TYPE comments. For each value line
/// it splits the metric (with optional `{labels}`) from the value, parses the
/// value as `f64` (the optional trailing timestamp token is ignored), and
/// records a [`Sample`]. Unparseable lines are skipped rather than fatal — a
/// soak must not die because one exposition line was odd.
#[must_use]
pub fn parse(body: &str) -> MetricSet {
    let mut samples = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Split `metric[{labels}] value [timestamp]`. The metric+labels token
        // ends at the first whitespace that is OUTSIDE a `{...}` label block
        // (label values may contain spaces).
        let (metric, rest) = match split_metric_value(line) {
            Some(pair) => pair,
            None => continue,
        };
        let value_tok = rest.split_whitespace().next().unwrap_or("");
        let value = match value_tok.parse::<f64>() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (name, labels) = match metric.split_once('{') {
            Some((name, label_blob)) => {
                let label_blob = label_blob.strip_suffix('}').unwrap_or(label_blob);
                (name.to_string(), parse_labels(label_blob))
            }
            None => (metric.to_string(), Vec::new()),
        };
        if name.is_empty() {
            continue;
        }
        samples.push(Sample {
            name,
            labels,
            value,
        });
    }
    MetricSet { samples }
}

/// Split a line into the `metric[{labels}]` token and the trailing
/// `value [timestamp]` remainder, honoring spaces inside a `{...}` block.
fn split_metric_value(line: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (i, c) in line.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            c if c.is_whitespace() && depth == 0 => {
                let metric = &line[..i];
                let rest = line[i..].trim_start();
                if metric.is_empty() {
                    return None;
                }
                return Some((metric, rest));
            }
            _ => {}
        }
    }
    None
}

/// Parse `k1="v1",k2="v2"` (the inside of a `{...}` block) into pairs. Tolerant
/// of missing quotes and trailing commas.
fn parse_labels(blob: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pair in blob.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        if let Some((k, v)) = pair.split_once('=') {
            let v = v.trim().trim_matches('"');
            out.push((k.trim().to_string(), v.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# HELP quic_modeb_streams_active Active Mode B relay streams
# TYPE quic_modeb_streams_active gauge
quic_modeb_streams_active 12
# HELP quic_modeb_connections Active Mode B connections
# TYPE quic_modeb_connections gauge
quic_modeb_connections 3
# HELP accept_inflight Inflight connections per listener
# TYPE accept_inflight gauge
accept_inflight{listener=\"127.0.0.1:8080\"} 7
accept_inflight{listener=\"127.0.0.1:8443\"} 5
# HELP quic_modeb_datagrams_dropped_total Dropped datagrams
# TYPE quic_modeb_datagrams_dropped_total counter
quic_modeb_datagrams_dropped_total 41
# HELP panic_total panics
# TYPE panic_total counter
panic_total 0
http_request_seconds_sum 1.5
";

    #[test]
    fn parses_unlabelled_gauge() {
        let m = parse(SAMPLE);
        assert_eq!(m.sum("quic_modeb_streams_active"), Some(12.0));
        assert_eq!(m.sum("quic_modeb_connections"), Some(3.0));
        assert_eq!(m.sum("panic_total"), Some(0.0));
    }

    #[test]
    fn sums_labelled_series() {
        let m = parse(SAMPLE);
        // accept_inflight has two listener series — sum is the total bound.
        assert_eq!(m.sum("accept_inflight"), Some(12.0));
        assert_eq!(m.max("accept_inflight"), Some(7.0));
    }

    #[test]
    fn labels_are_captured() {
        let m = parse(SAMPLE);
        let s = m
            .samples
            .iter()
            .find(|s| s.name == "accept_inflight" && s.value == 7.0)
            .expect("first accept_inflight series");
        assert_eq!(s.labels, vec![("listener".into(), "127.0.0.1:8080".into())]);
    }

    #[test]
    fn float_values_parse() {
        let m = parse(SAMPLE);
        assert_eq!(m.sum("http_request_seconds_sum"), Some(1.5));
    }

    #[test]
    fn absent_name_is_none_not_zero() {
        let m = parse(SAMPLE);
        assert_eq!(m.sum("quic_passthrough_flows"), None);
        // Present-and-zero is distinguishable from absent.
        assert_eq!(m.sum("panic_total"), Some(0.0));
    }

    #[test]
    fn comments_and_blanks_skipped() {
        let m = parse("# just a comment\n\n  \n# TYPE x gauge\n");
        assert!(m.samples.is_empty());
    }

    #[test]
    fn value_with_trailing_timestamp() {
        // Prometheus allows `metric value timestamp`; we take the value.
        let m = parse("foo_total 99 1700000000000\n");
        assert_eq!(m.sum("foo_total"), Some(99.0));
    }

    #[test]
    fn malformed_value_line_skipped_not_fatal() {
        let m = parse("good 1\nbad notanumber\nalsogood 2\n");
        assert_eq!(m.sum("good"), Some(1.0));
        assert_eq!(m.sum("alsogood"), Some(2.0));
        assert_eq!(m.sum("bad"), None);
    }
}
