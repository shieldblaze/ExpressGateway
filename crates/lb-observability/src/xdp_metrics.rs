//! Prometheus mirror of the eBPF STATS map, sampled at 1 Hz.
//!
//! The eBPF side exposes absolute counters, so each tick must DELTA against the previous one —
//! and must detect a decrease (loader reattach clears the map) and re-baseline instead of
//! wrapping.
//!
//! On non-Linux the sampler is a stub, but the families are still registered so a dev-build
//! scrape does not 404 the dashboard panels.

use std::sync::Arc;

use prometheus::{IntCounter, IntCounterVec, IntGaugeVec};

use crate::{MetricsError, MetricsRegistry};

/// XDP metric family handles; cheap to clone.
#[derive(Clone)]
pub struct XdpMetrics {
    /// `xdp_packets_total{action}`, one slot per `StatSlot`.
    pub packets_total: IntCounterVec,
    /// `xdp_conntrack_full_total{family}` — registered but NOT yet fed by an eBPF slot.
    pub conntrack_full_total: IntCounterVec,
    /// `xdp_sampler_errors_total`, bumped on a failed `read_stats()`.
    pub sampler_errors_total: IntCounter,
    /// `xdp_attached_mode{mode}`; exactly one slot is 1 per tick.
    pub attached_mode: IntGaugeVec,
}

impl XdpMetrics {
    /// Register every XDP family.
    ///
    /// # Errors
    ///
    /// The `prometheus` registration error.
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
        // Pre-seed so the labelled rows exist before the first kernel event — operators alert on
        // the row, and a missing row reads as a broken scrape.
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

/// `xdp_packets_total{action}` label values. WIRE-STABLE and index-matched to `StatSlot`.
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

/// Per-slot delta baseline carried between sampler ticks.
#[derive(Clone, Debug, Default)]
pub struct SamplerBaseline {
    /// Last-seen summed value per slot.
    pub last_summed: Vec<u64>,
}

impl SamplerBaseline {
    /// Apply a snapshot and return the per-slot deltas. A DECREASE means the map was cleared, so
    /// the baseline is re-adopted and the delta is 0 rather than a wrapped huge number.
    #[must_use]
    pub fn delta(&mut self, summed: &[u64]) -> Vec<u64> {
        if self.last_summed.len() != summed.len() {
            // First tick or schema change.
            self.last_summed = summed.to_vec();
            return vec![0; summed.len()];
        }
        let mut out = Vec::with_capacity(summed.len());
        for (i, &cur) in summed.iter().enumerate() {
            let prev = self.last_summed.get(i).copied().unwrap_or(0);
            // `saturating_sub` is what turns a cleared map into 0 instead of a wrapped value.
            let d = cur.saturating_sub(prev);
            out.push(d);
        }
        // Rebuilt, not cloned from `summed` — cloning would re-apply the OLD baseline next tick.
        self.last_summed.clone_from_slice(summed);
        out
    }
}

/// Apply per-slot deltas to `xdp_packets_total{action}`.
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

/// Set `xdp_attached_mode{mode}`: one row to 1, the rest to 0; `None` clears all.
pub fn set_attached_mode(metrics: &XdpMetrics, active: Option<&str>) {
    for mode in ["drv", "skb", "hw"] {
        let v = if Some(mode) == active { 1 } else { 0 };
        metrics.attached_mode.with_label_values(&[mode]).set(v);
    }
}

/// Bump the conntrack-full counter.
pub fn record_conntrack_full(metrics: &XdpMetrics, family: ConntrackFamily, delta: u64) {
    metrics
        .conntrack_full_total
        .with_label_values(&[family.as_str()])
        .inc_by(delta);
}

/// Family label selector for the conntrack metrics.
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

/// Shared XDP metrics handle: one in the sampler, one in `main.rs` for synchronous mode updates.
pub type SharedXdpMetrics = Arc<XdpMetrics>;

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
