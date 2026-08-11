//! Every kernel `StatSlot` must have an `xdp_packets_total{action}` row — the STATS-map-to-
//! userspace mirror contract.

use lb_l4_xdp::stats_export::NUM_SLOTS;
use lb_observability::prometheus_exposition::render_text;
use lb_observability::xdp_metrics::{apply_packet_deltas, stat_slot_labels};
use lb_observability::{MetricsRegistry, XdpMetrics};

#[test]
fn all_stat_slots_are_exported_at_zero() {
    let reg = MetricsRegistry::new();
    let _m = XdpMetrics::register(&reg).expect("register XDP families");

    let body = render_text(&reg);
    let labels = stat_slot_labels();

    assert_eq!(
        labels.len(),
        NUM_SLOTS,
        "stat_slot_labels() length must match lb_l4_xdp::NUM_SLOTS",
    );

    for action in labels {
        let row = format!("xdp_packets_total{{action=\"{action}\"}} 0");
        assert!(
            body.contains(&row),
            "row for action={action:?} not pre-seeded; body:\n{body}",
        );
    }
}

#[test]
fn deltas_apply_per_slot() {
    let reg = MetricsRegistry::new();
    let m = XdpMetrics::register(&reg).unwrap();

    // Ordered as `stat_slot_labels()`, one distinct delta per slot.
    let deltas: Vec<u64> = (1..=NUM_SLOTS as u64).collect();
    apply_packet_deltas(&m, &deltas);

    let body = render_text(&reg);
    let labels = stat_slot_labels();
    for (i, action) in labels.iter().enumerate() {
        let expected_val = (i + 1) as u64;
        let row = format!("xdp_packets_total{{action=\"{action}\"}} {expected_val}");
        assert!(
            body.contains(&row),
            "row for action={action:?} value={expected_val} missing; body:\n{body}",
        );
    }
}

#[test]
fn label_key_is_action_not_result() {
    // The key is `action`, not `result` — recording rules depend on it.
    let reg = MetricsRegistry::new();
    let _m = XdpMetrics::register(&reg).unwrap();
    let body = render_text(&reg);
    assert!(
        body.contains("xdp_packets_total{action="),
        "label key must be `action`: {body}",
    );
    assert!(
        !body.contains("xdp_packets_total{result="),
        "label key must NOT be `result`: {body}",
    );
}
