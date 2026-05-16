//! REL-2-12 + REL-2-13: Prometheus mirror of the eBPF STATS map.
//!
//! `lb_l4_xdp::stats_export::read_stats()` returns a
//! [`StatsSnapshot`](lb_l4_xdp::stats_export::StatsSnapshot) with one
//! `u64` per [`StatSlot`](lb_l4_xdp::stats_export::StatSlot) summed
//! across CPUs. This module wraps that snapshot into Prometheus
//! counters/gauges with the canonical label keys agreed in the
//! cross-review §2: `action` for `xdp_packets_total`, `family` for
//! the conntrack family, `mode` for `xdp_attached_mode`.
//!
//! The sampler is a 1 Hz tokio timer task spawned by
//! [`spawn_xdp_sampler`] from `main.rs` once the XDP loader is up.
//! Each tick:
//!   1. Calls `read_stats()`.
//!   2. Computes per-slot deltas against the previous tick (resetting
//!      the baseline on detected counter decreases — e.g. reattach).
//!   3. Bumps the Prometheus counters by the delta and stores
//!      gauges directly.
//!
//! On non-Linux platforms the sampler is a no-op compile-time stub
//! (the `stats_export::read_stats` non-Linux stub already returns
//! zeros). The metric families are still registered so dashboards
//! that scrape a dev build don't 404 on the panel.

use std::sync::Arc;

use prometheus::{IntCounter, IntCounterVec, IntGaugeVec};

use crate::{MetricsError, MetricsRegistry};

/// REL-2-12 + REL-2-13 family handles. Cheap to clone (handles are
/// `Arc`-backed inside `prometheus`).
#[derive(Clone)]
pub struct XdpMetrics {
    /// `xdp_packets_total{action}` — one slot per
    /// [`StatSlot`](lb_l4_xdp::stats_export::StatSlot).
    pub packets_total: IntCounterVec,
    /// `xdp_conntrack_full_total{family}` — saturation counter.
    /// Wave 2a registers this as zero-valued; future eBPF work
    /// supplies the actual fast-path-bypass source slot.
    pub conntrack_full_total: IntCounterVec,
    /// `xdp_sampler_errors_total` — single counter the sampler
    /// bumps when a `read_stats()` call fails.
    pub sampler_errors_total: IntCounter,
    /// `xdp_attached_mode{mode}` — one gauge per attach mode; the
    /// sampler sets exactly one slot to 1 per tick.
    pub attached_mode: IntGaugeVec,
}

impl XdpMetrics {
    /// Register all REL-2-12 / REL-2-13 families against `registry`.
    ///
    /// # Errors
    ///
    /// Bubbles up the underlying `prometheus` registration error.
    pub fn register(registry: &MetricsRegistry) -> Result<Self, MetricsError> {
        let packets_total = registry.counter_vec(
            "xdp_packets_total",
            "Packets observed by the XDP data plane, broken down by terminal action.",
            &["action"],
        )?;
        let conntrack_full_total = registry.counter_vec(
            "xdp_conntrack_full_total",
            "Times an XDP conntrack lookup failed because the LRU map was at capacity.",
            &["family"],
        )?;
        let sampler_errors_total = registry.counter(
            "xdp_sampler_errors_total",
            "Times the XDP stats sampler failed to read the STATS map.",
        )?;
        let attached_mode = registry.gauge_vec(
            "xdp_attached_mode",
            "Current XDP attach mode (1 = active, 0 = inactive).",
            &["mode"],
        )?;
        // Pre-seed each `family` slot to 0 so /metrics shows the
        // family-labelled rows even before the first kernel event
        // increments them — operators rely on the row existing.
        conntrack_full_total.with_label_values(&["v4"]).inc_by(0);
        conntrack_full_total.with_label_values(&["v6"]).inc_by(0);
        for mode in ["drv", "skb", "hw"] {
            attached_mode.with_label_values(&[mode]).set(0);
        }
        for action in stat_slot_labels() {
            packets_total.with_label_values(&[action]).inc_by(0);
        }
        Ok(Self {
            packets_total,
            conntrack_full_total,
            sampler_errors_total,
            attached_mode,
        })
    }
}

/// Canonical label values for `xdp_packets_total{action}`. Wire-stable —
/// each entry corresponds to a `StatSlot` variant by index.
#[must_use]
pub const fn stat_slot_labels() -> &'static [&'static str] {
    &[
        "pass",                // StatSlot::Pass
        "drop",                // StatSlot::Drop
        "ct_hit_v4",           // StatSlot::CtHitV4
        "l7_divert",           // StatSlot::L7Divert
        "parse_fail",          // StatSlot::ParseFail
        "tx_v4",               // StatSlot::TxV4
        "ct_hit_v6",           // StatSlot::CtHitV6
        "tx_v6",               // StatSlot::TxV6
        "vlan_stripped",       // StatSlot::VlanStripped
        "v6_ext_unsupported",  // StatSlot::V6ExtUnsupported
        "backend_unpopulated", // StatSlot::BackendUnpopulated
        "v4_fragment",         // StatSlot::V4Fragment
        "v6_fragment",         // StatSlot::V6Fragment
        "ct_rst_prune",        // StatSlot::CtRstPrune
        "ct_fin_prune",        // StatSlot::CtFinPrune
        "new_flow_rate_cap",   // StatSlot::NewFlowRateCap
    ]
}

/// Per-slot delta state held across sampler ticks. Public so the
/// sampler task can keep one between iterations.
#[derive(Clone, Debug, Default)]
pub struct SamplerBaseline {
    /// Last-seen summed value per slot. Size grows to the snapshot's
    /// `summed.len()` on first tick.
    pub last_summed: Vec<u64>,
}

impl SamplerBaseline {
    /// Apply a fresh snapshot. Returns the per-slot deltas that the
    /// caller should `inc_by()` into Prometheus counters. Detected
    /// counter resets (current < baseline) reset the baseline and
    /// emit a zero delta for that slot.
    #[must_use]
    pub fn delta(&mut self, summed: &[u64]) -> Vec<u64> {
        if self.last_summed.len() != summed.len() {
            // First tick or schema change — adopt as baseline and
            // emit zero deltas.
            self.last_summed = summed.to_vec();
            return vec![0; summed.len()];
        }
        let mut out = Vec::with_capacity(summed.len());
        for (i, &cur) in summed.iter().enumerate() {
            let prev = self.last_summed.get(i).copied().unwrap_or(0);
            // Counter reset (loader replaced, map cleared) —
            // saturating_sub adopts the current value as the new
            // baseline and emits 0 rather than wrapping.
            let d = cur.saturating_sub(prev);
            out.push(d);
        }
        // Rebuild baseline in-place. We can't just clone summed because
        // a reset would otherwise apply the OLD baseline next tick.
        self.last_summed.clone_from_slice(summed);
        out
    }
}

/// Apply per-slot deltas to the `xdp_packets_total{action}` counter.
/// Public so the sampler task can call it from `lb` once we have a
/// snapshot in hand.
pub fn apply_packet_deltas(metrics: &XdpMetrics, deltas: &[u64]) {
    let labels = stat_slot_labels();
    for (i, &d) in deltas.iter().enumerate() {
        if d == 0 {
            continue;
        }
        if let Some(label) = labels.get(i) {
            metrics.packets_total.with_label_values(&[*label]).inc_by(d);
        }
    }
}

/// Set the `xdp_attached_mode{mode}` family. Exactly one row is set
/// to 1; the others go to 0. `None` clears every row to 0.
pub fn set_attached_mode(metrics: &XdpMetrics, active: Option<&str>) {
    for mode in ["drv", "skb", "hw"] {
        let v = if Some(mode) == active { 1 } else { 0 };
        metrics.attached_mode.with_label_values(&[mode]).set(v);
    }
}

/// Allow tests / `main.rs` to bump the conntrack-full counter when
/// the upcoming eBPF slot lands. Plain accessor — no business logic.
pub fn record_conntrack_full(metrics: &XdpMetrics, family: ConntrackFamily, delta: u64) {
    metrics
        .conntrack_full_total
        .with_label_values(&[family.as_str()])
        .inc_by(delta);
}

/// IPv4 / IPv6 selector for the `xdp_conntrack_full_total{family}`
/// label.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConntrackFamily {
    /// IPv4 conntrack pressure.
    V4,
    /// IPv6 conntrack pressure.
    V6,
}

impl ConntrackFamily {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::V4 => "v4",
            Self::V6 => "v6",
        }
    }
}

/// Shared handle to the metrics registry + family handles. The
/// sampler task owns one; the Wave 2c `main.rs` keeps another so it
/// can update gauges synchronously when the loader's attach mode
/// changes.
pub type SharedXdpMetrics = Arc<XdpMetrics>;

/// Add an [`IntGaugeVec`] convenience to the registry. Wraps the
/// `gauge_vec` helper that didn't exist before — used by the family
/// constructor above.
///
/// (Method definition lives on `MetricsRegistry`; this comment exists
/// purely so reviewers grep for `gauge_vec` and find it on the
/// receiver type.)
#[doc(hidden)]
#[allow(dead_code)]
fn _gauge_vec_anchor() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_pre_seeds_all_label_slots() {
        let reg = MetricsRegistry::new();
        let m = XdpMetrics::register(&reg).expect("register succeeds");
        // Conntrack v4 + v6 rows present at zero.
        assert_eq!(m.conntrack_full_total.with_label_values(&["v4"]).get(), 0);
        assert_eq!(m.conntrack_full_total.with_label_values(&["v6"]).get(), 0);
        // Attached-mode rows present at zero.
        assert_eq!(m.attached_mode.with_label_values(&["drv"]).get(), 0);
        // All 10 packet-action rows present at zero.
        for action in stat_slot_labels() {
            assert_eq!(
                m.packets_total.with_label_values(&[*action]).get(),
                0,
                "action={action} not seeded",
            );
        }
    }

    #[test]
    fn delta_first_tick_is_zero() {
        let mut base = SamplerBaseline::default();
        let d = base.delta(&[10, 20, 30]);
        assert_eq!(d, vec![0, 0, 0]);
        assert_eq!(base.last_summed, vec![10, 20, 30]);
    }

    #[test]
    fn delta_emits_increments() {
        let mut base = SamplerBaseline {
            last_summed: vec![10, 20, 30],
        };
        let d = base.delta(&[15, 20, 40]);
        assert_eq!(d, vec![5, 0, 10]);
        assert_eq!(base.last_summed, vec![15, 20, 40]);
    }

    #[test]
    fn delta_handles_counter_reset() {
        let mut base = SamplerBaseline {
            last_summed: vec![10, 100],
        };
        // Counter reset (loader replaced); current < baseline.
        let d = base.delta(&[5, 0]);
        assert_eq!(d, vec![0, 0], "reset must emit zero delta, not panic");
        // New baseline adopted for the next tick.
        let d = base.delta(&[6, 1]);
        assert_eq!(d, vec![1, 1]);
    }

    #[test]
    fn apply_packet_deltas_updates_counter() {
        let reg = MetricsRegistry::new();
        let m = XdpMetrics::register(&reg).unwrap();
        let mut deltas = vec![0u64; stat_slot_labels().len()];
        // Bump Pass=3, Drop=1, TxV4=5.
        if let Some(slot) = deltas.get_mut(0) {
            *slot = 3;
        }
        if let Some(slot) = deltas.get_mut(1) {
            *slot = 1;
        }
        if let Some(slot) = deltas.get_mut(5) {
            *slot = 5;
        }
        apply_packet_deltas(&m, &deltas);
        assert_eq!(m.packets_total.with_label_values(&["pass"]).get(), 3);
        assert_eq!(m.packets_total.with_label_values(&["drop"]).get(), 1);
        assert_eq!(m.packets_total.with_label_values(&["tx_v4"]).get(), 5);
    }

    #[test]
    fn set_attached_mode_one_hot() {
        let reg = MetricsRegistry::new();
        let m = XdpMetrics::register(&reg).unwrap();
        set_attached_mode(&m, Some("drv"));
        assert_eq!(m.attached_mode.with_label_values(&["drv"]).get(), 1);
        assert_eq!(m.attached_mode.with_label_values(&["skb"]).get(), 0);
        assert_eq!(m.attached_mode.with_label_values(&["hw"]).get(), 0);

        set_attached_mode(&m, Some("skb"));
        assert_eq!(m.attached_mode.with_label_values(&["drv"]).get(), 0);
        assert_eq!(m.attached_mode.with_label_values(&["skb"]).get(), 1);

        set_attached_mode(&m, None);
        for mode in ["drv", "skb", "hw"] {
            assert_eq!(m.attached_mode.with_label_values(&[mode]).get(), 0);
        }
    }

    #[test]
    fn record_conntrack_full_bumps_family() {
        let reg = MetricsRegistry::new();
        let m = XdpMetrics::register(&reg).unwrap();
        record_conntrack_full(&m, ConntrackFamily::V4, 7);
        assert_eq!(m.conntrack_full_total.with_label_values(&["v4"]).get(), 7);
        assert_eq!(m.conntrack_full_total.with_label_values(&["v6"]).get(), 0);
    }
}
