//! The [`LabelBudget`] startup gate must refuse an overflowing worst-case product.

use lb_observability::{CANONICAL_LABELS, DEFAULT_MAX_LABEL_CARDINALITY, LabelBudget};

#[test]
fn test_budget_refuses_start_on_overflow() {
    // 200 × 500 × 100 — the named failure case.
    let b = LabelBudget::from_config_shape(200, 500, 100, DEFAULT_MAX_LABEL_CARDINALITY);
    let err = b
        .check()
        .expect_err("budget must refuse oversize config shape");
    assert!(
        err.to_string().contains("backend_requests_total")
            || err.to_string().contains("http_requests_total"),
        "expected family-named error, got: {err}",
    );
    assert!(err.worst_case > err.ceiling);
}

#[test]
fn test_realistic_budget_passes() {
    let b = LabelBudget::from_config_shape(8, 32, 16, DEFAULT_MAX_LABEL_CARDINALITY);
    b.check().expect("realistic config should pass");
}

#[test]
fn test_canonical_label_table_documents_red_families() {
    let names: Vec<&str> = CANONICAL_LABELS.iter().map(|(n, _)| *n).collect();
    assert!(names.contains(&"http_requests_total"));
    assert!(names.contains(&"backend_requests_total"));
    assert!(names.contains(&"connections_inflight"));
    assert!(names.contains(&"xdp_conntrack_full_total"));
    // `action`, not `result` — drift breaks recording rules.
    let xpt = CANONICAL_LABELS
        .iter()
        .find(|(n, _)| *n == "xdp_packets_total")
        .expect("xdp_packets_total registered");
    assert_eq!(xpt.1, &["action"]);
}

#[test]
fn test_zero_listeners_is_zero_series() {
    let b = LabelBudget::from_config_shape(0, 0, 0, DEFAULT_MAX_LABEL_CARDINALITY);
    b.check().expect("empty config is trivially under budget");
    let w = b.worst_case();
    assert_eq!(w.http_requests_total, 0);
    assert_eq!(w.backend_requests_total, 0);
    assert_eq!(w.connections_inflight, 0);
}
