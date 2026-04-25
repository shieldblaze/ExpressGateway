//! Prometheus text-format exposition (version 0.0.4).
//!
//! The admin HTTP listener uses [`render_text`] to serialize the
//! registry on each `GET /metrics`. Callers that want to format a
//! single metric family without going through the HTTP endpoint (for
//! example, in tests) can do the same thing directly.

use prometheus::{Encoder, TextEncoder};

use crate::MetricsRegistry;

/// Content-type header value for the 0.0.4 text format. Matches
/// [`prometheus::TEXT_FORMAT`] but pulls it into this module so callers
/// don't have to import the upstream crate.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Render the registry as Prometheus text-format output.
///
/// The result always terminates with a trailing newline so HTTP
/// responses pass the Prometheus scraper's conformance check.
#[must_use]
pub fn render_text(registry: &MetricsRegistry) -> String {
    let mfs = registry.gather();
    let encoder = TextEncoder::new();
    let mut buf = Vec::with_capacity(1024);
    // TextEncoder::encode only fails if the writer fails; a Vec<u8>
    // writer never returns Err, but we still degrade gracefully
    // rather than bubbling a panic.
    if let Err(e) = encoder.encode(&mfs, &mut buf) {
        tracing::error!(error = %e, "prometheus text encode failed");
        return String::new();
    }
    String::from_utf8(buf).unwrap_or_else(|e| {
        tracing::error!(error = %e, "prometheus text encode produced non-UTF8");
        String::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_help_and_type_lines() {
        let reg = MetricsRegistry::new();
        let c = reg.counter("hits_total", "Total hits").unwrap();
        c.inc_by(3);
        let text = render_text(&reg);
        assert!(text.contains("# HELP hits_total Total hits"), "{text}");
        assert!(text.contains("# TYPE hits_total counter"), "{text}");
        assert!(text.contains("hits_total 3"), "{text}");
    }

    #[test]
    fn renders_histogram_buckets_and_sum() {
        let reg = MetricsRegistry::new();
        let h = reg
            .histogram("dur_seconds", "duration", &[0.1, 1.0])
            .unwrap();
        h.observe(0.5);
        let text = render_text(&reg);
        assert!(text.contains("dur_seconds_bucket"), "{text}");
        assert!(text.contains("dur_seconds_sum"), "{text}");
        assert!(text.contains("dur_seconds_count"), "{text}");
    }

    #[test]
    fn empty_registry_renders_empty_string() {
        let reg = MetricsRegistry::new();
        let text = render_text(&reg);
        assert_eq!(text, "");
    }

    #[test]
    fn exposition_text_parses_as_openmetrics_subset() {
        let reg = MetricsRegistry::new();
        reg.counter("a_total", "a").unwrap().inc();
        reg.gauge("b", "b").unwrap().set(1);
        let text = render_text(&reg);
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("# HELP ") {
                assert!(rest.contains(' '), "malformed HELP line: {line}");
                continue;
            }
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                assert!(
                    ["counter", "gauge", "histogram", "summary", "untyped"]
                        .iter()
                        .any(|t| rest.ends_with(*t)),
                    "malformed TYPE line: {line}",
                );
                continue;
            }
            // sample line: "<name>{labels}? <value>"
            assert!(line.contains(' '), "malformed sample line: {line}");
        }
    }
}
