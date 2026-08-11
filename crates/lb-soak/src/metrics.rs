//! Minimal Prometheus text-exposition parser, scoped to the gauge/counter subset the product emits.
//! A wrong parse is a wrong bound verdict, so it is unit-tested.

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub name: String,
    pub labels: Vec<(String, String)>,
    pub value: f64,
}

#[derive(Debug, Clone, Default)]
pub struct MetricSet {
    pub samples: Vec<Sample>,
}

impl MetricSet {
    /// Sum every series with family name `name`; `None` (absent) is distinct from `Some(0.0)`.
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

    #[must_use]
    pub fn max(&self, name: &str) -> Option<f64> {
        self.samples
            .iter()
            .filter(|s| s.name == name)
            .map(|s| s.value)
            .fold(None, |acc, v| Some(acc.map_or(v, |a: f64| a.max(v))))
    }
}

/// Parse a Prometheus text-exposition body. Unparseable lines are skipped, not fatal.
#[must_use]
pub fn parse(body: &str) -> MetricSet {
    let mut samples = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // The metric+labels token ends at the first whitespace OUTSIDE `{...}`.
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
        assert_eq!(m.sum("panic_total"), Some(0.0));
    }

    #[test]
    fn comments_and_blanks_skipped() {
        let m = parse("# just a comment\n\n  \n# TYPE x gauge\n");
        assert!(m.samples.is_empty());
    }

    #[test]
    fn value_with_trailing_timestamp() {
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
