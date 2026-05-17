//! REL-2-08 follow-on — the `http_requests_total` metric family must
//! carry the canonical `{listener, route, version, status_class}`
//! label set that the RUNBOOK `LbReq5xx` alert and the
//! `lb_observability::label_budget::CANONICAL_LABELS` table expect.
//!
//! The accept-site emit lives in `crates/lb/src/main.rs` and is
//! exercised end-to-end by the bridging integration tests; here we
//! pin the *shape* contract:
//!
//! 1. The `CANONICAL_LABELS` table declares the 4-label set.
//! 2. Registering `http_requests_total` with the 4-label set
//!    succeeds (no type-mismatch on re-register).
//! 3. Bumping the counter with the 4-label tuple renders to the
//!    exposition with all four labels present.
//!
//! Anyone who later trims a label off the metric will break this
//! test before the regression reaches a dashboard / alert.

use std::sync::Arc;

use lb_observability::{CANONICAL_LABELS, MetricsRegistry, prometheus_exposition};

#[test]
fn test_http_requests_total_has_listener_route_labels() {
    // 1. Canonical-label table must still declare the 4-label set.
    let canonical = CANONICAL_LABELS
        .iter()
        .find(|(n, _)| *n == "http_requests_total")
        .expect("http_requests_total must appear in CANONICAL_LABELS");
    assert_eq!(
        canonical.1,
        &["listener", "route", "version", "status_class"],
        "canonical label set drifted; RUNBOOK LbReq5xx alert depends on this exact tuple"
    );

    // 2. Registering with the production label shape must succeed.
    let reg = Arc::new(MetricsRegistry::new());
    let v = reg
        .counter_vec(
            "http_requests_total",
            "HTTP requests terminated by the L7 proxy",
            &["listener", "route", "version", "status_class"],
        )
        .expect("registration with canonical shape must succeed");

    // 3. Bumping with the 4-label tuple round-trips through the
    // exposition with every label key + value rendered.
    v.with_label_values(&["0.0.0.0:8443", "", "h1", "2xx"])
        .inc();
    v.with_label_values(&["0.0.0.0:8443", "", "h2", "5xx"])
        .inc();

    let text = prometheus_exposition::render_text(&reg);
    assert!(
        text.contains("http_requests_total{"),
        "exposition missing metric family:\n{text}",
    );
    // Order of labels in the rendered text is alphabetical
    // (Prometheus convention); assert each label key appears.
    for key in ["listener=", "route=", "version=", "status_class="] {
        assert!(
            text.contains(key),
            "exposition missing label key {key}:\n{text}",
        );
    }
    assert!(
        text.contains("listener=\"0.0.0.0:8443\""),
        "listener value missing from rendered series:\n{text}",
    );
    // The 5xx bump must surface so the RUNBOOK alert expression
    // `rate(http_requests_total{status_class="5xx"}[5m])` matches.
    assert!(
        text.contains("status_class=\"5xx\""),
        "status_class=\"5xx\" missing from rendered series:\n{text}",
    );
}

#[test]
fn test_http_request_duration_seconds_has_listener_route_labels() {
    // Mirror of the counter check for the duration histogram — same
    // canonical shape minus the status_class label per the
    // CANONICAL_LABELS table.
    let canonical = CANONICAL_LABELS
        .iter()
        .find(|(n, _)| *n == "http_request_duration_seconds")
        .expect("http_request_duration_seconds must appear in CANONICAL_LABELS");
    assert_eq!(
        canonical.1,
        &["listener", "route", "version"],
        "duration histogram label set drifted",
    );

    let reg = Arc::new(MetricsRegistry::new());
    let h = reg
        .histogram_vec(
            "http_request_duration_seconds",
            "L7 request duration",
            &["listener", "route", "version"],
            &lb_observability::http_latency_buckets(),
        )
        .expect("histogram registration with canonical shape must succeed");
    h.with_label_values(&["0.0.0.0:8443", "", "h1"])
        .observe(0.1);
    let text = prometheus_exposition::render_text(&reg);
    for key in ["listener=", "route=", "version="] {
        assert!(text.contains(key), "missing {key}:\n{text}");
    }
}
