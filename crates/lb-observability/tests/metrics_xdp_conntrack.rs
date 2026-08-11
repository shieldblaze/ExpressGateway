//! `xdp_conntrack_full_total{family}` advances on a recorded delta. The kernel-side source slot
//! is not wired yet, so the helper is driven directly to lock the family + label contract.

use lb_observability::prometheus_exposition::render_text;
use lb_observability::xdp_metrics::record_conntrack_full;
use lb_observability::{ConntrackFamily, MetricsRegistry, XdpMetrics};

#[test]
fn test_counter_reflects_stats_slot() {
    let reg = MetricsRegistry::new();
    let m = XdpMetrics::register(&reg).expect("register XDP families");

    let body = render_text(&reg);
    assert!(
        body.contains("xdp_conntrack_full_total{family=\"v4\"} 0"),
        "v4 row not pre-seeded: {body}",
    );
    assert!(
        body.contains("xdp_conntrack_full_total{family=\"v6\"} 0"),
        "v6 row not pre-seeded: {body}",
    );

    // Simulates a sampler slot delta.
    record_conntrack_full(&m, ConntrackFamily::V4, 7);
    record_conntrack_full(&m, ConntrackFamily::V6, 2);

    let body = render_text(&reg);
    assert!(
        body.contains("xdp_conntrack_full_total{family=\"v4\"} 7"),
        "v4 counter not updated: {body}",
    );
    assert!(
        body.contains("xdp_conntrack_full_total{family=\"v6\"} 2"),
        "v6 counter not updated: {body}",
    );
}

#[test]
fn test_counter_is_monotonically_increasing() {
    let reg = MetricsRegistry::new();
    let m = XdpMetrics::register(&reg).unwrap();

    for _ in 0..5 {
        record_conntrack_full(&m, ConntrackFamily::V4, 1);
    }
    let body = render_text(&reg);
    assert!(
        body.contains("xdp_conntrack_full_total{family=\"v4\"} 5"),
        "counter must be monotonically increasing: {body}",
    );
}
