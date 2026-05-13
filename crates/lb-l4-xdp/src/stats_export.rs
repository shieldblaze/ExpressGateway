//! Lock-step API boundary between the eBPF data plane and userspace
//! observability (rel's `lb-observability` crate consumes this in
//! Wave-2 per REL-2-13).
//!
//! Created by EBPF-2-04 (XDP attach mode reporting) and extended by
//! EBPF-2-05 (pinned-map reuse reporting) and EBPF-2-08 (per-CPU
//! STATS array export). Everything in this file is **safe**, **lock-
//! free**, and **panic-free** at steady state — telemetry must never
//! be the reason production aborts.
//!
//! File ownership: `ebpf` owns this file. `rel` reads from it via the
//! `pub fn` accessors below; rel MUST NOT edit this file.

use std::sync::atomic::{AtomicU8, Ordering};

// ---------------------------------------------------------------------------
// EBPF-2-04: XDP attach mode reporting.
// ---------------------------------------------------------------------------

/// Coarse-grained mode label for the Prometheus `xdp_attach_mode`
/// gauge. Matches the kernel's `XDP_FLAGS_*` mode bits one-for-one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachModeLabel {
    /// Native driver mode (`XDP_FLAGS_DRV_MODE`). 40-80 Mpps single-core.
    Drv,
    /// Generic SKB mode (`XDP_FLAGS_SKB_MODE`). 1-3 Mpps single-core; CI
    /// / dev path.
    Skb,
    /// Hardware offload (`XDP_FLAGS_HW_MODE`). mlx5 / nfp only.
    Hw,
}

impl AttachModeLabel {
    /// Stable byte encoding for atomic storage. Sentinel `0xFF` =
    /// "not set" (i.e. XDP not attached in this process yet).
    const fn as_byte(self) -> u8 {
        match self {
            Self::Drv => 1,
            Self::Skb => 2,
            Self::Hw => 3,
        }
    }

    const fn from_byte(b: u8) -> Option<Self> {
        match b {
            1 => Some(Self::Drv),
            2 => Some(Self::Skb),
            3 => Some(Self::Hw),
            _ => None,
        }
    }

    /// Prometheus label value (lower-case, matches the kernel API
    /// vocabulary so an operator can compare to `bpftool net show`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Drv => "drv",
            Self::Skb => "skb",
            Self::Hw => "hw",
        }
    }
}

/// Sentinel for "no attach mode recorded yet". Distinct from any
/// valid `AttachModeLabel::as_byte()` value.
const ATTACH_MODE_UNSET: u8 = 0xFF;

/// Process-global atomic store of the current XDP attach mode.
/// Single producer (the `xdp.rs::try_attach_xdp` startup path);
/// many consumers (the Prom scraper, status endpoints, tests).
/// Atomic byte is sufficient — there is at most one XDP attach per
/// process for the foreseeable future.
static ATTACH_MODE: AtomicU8 = AtomicU8::new(ATTACH_MODE_UNSET);

/// Record which mode the XDP loader successfully attached in.
/// Called from `crates/lb/src/xdp.rs` after `attach_with_fallback`
/// returns Ok. Safe to call repeatedly; latest call wins.
pub fn record_attach_mode(mode: AttachModeLabel) {
    ATTACH_MODE.store(mode.as_byte(), Ordering::Relaxed);
}

/// Read back the current attach mode for telemetry exposition.
/// Returns `None` when XDP has not been attached yet (so rel's gauge
/// reports `0` for every mode rather than fabricating a value).
#[must_use]
pub fn current_attach_mode() -> Option<AttachModeLabel> {
    AttachModeLabel::from_byte(ATTACH_MODE.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// EBPF-2-05: pinned-map reuse reporting.
// ---------------------------------------------------------------------------

/// Snapshot of which pinned maps were reused vs. freshly-created on
/// process startup. Read by rel's Prom layer; written once at startup
/// from `crates/lb-l4-xdp/src/loader.rs::load_from_bytes_pinned`.
///
/// Bit layout in the underlying atomic: bit `i` is `1` if the
/// `i`-th pin in [`pin_names()`] was reused from a prior process.
/// The packing is intentional: rel's Prom scrape pulls a single
/// atomic load and projects to per-name gauges, no Mutex required.
static PIN_REUSED_BITMAP: AtomicU8 = AtomicU8::new(0);

/// Canonical pin-name ordering for the bitmap. Add new entries to
/// the END only — bit positions are wire-stable.
#[must_use]
pub fn pin_names() -> &'static [&'static str] {
    &[
        "conntrack",
        "conntrack_v6",
        "l7_ports",
        "acl_deny_trie",
        "stats",
    ]
}

/// Record whether the named pin was reused.
/// Unknown names are silently dropped (forward compatibility with
/// future pin additions).
pub fn record_pin_reused(name: &str, reused: bool) {
    if let Some(idx) = pin_names().iter().position(|n| *n == name) {
        let mask = 1u8 << idx;
        if reused {
            PIN_REUSED_BITMAP.fetch_or(mask, Ordering::Relaxed);
        } else {
            PIN_REUSED_BITMAP.fetch_and(!mask, Ordering::Relaxed);
        }
    }
}

/// Read back the `(name, reused?)` pairs for every known pin.
#[must_use]
pub fn pin_reused_snapshot() -> Vec<(&'static str, bool)> {
    let bits = PIN_REUSED_BITMAP.load(Ordering::Relaxed);
    pin_names()
        .iter()
        .enumerate()
        .map(|(i, n)| (*n, (bits >> i) & 1 == 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_returns_none() {
        // Independent of other tests' record-calls: read after reset.
        ATTACH_MODE.store(ATTACH_MODE_UNSET, Ordering::Relaxed);
        assert_eq!(current_attach_mode(), None);
    }

    #[test]
    fn round_trip_all_modes() {
        for &m in &[
            AttachModeLabel::Drv,
            AttachModeLabel::Skb,
            AttachModeLabel::Hw,
        ] {
            record_attach_mode(m);
            assert_eq!(current_attach_mode(), Some(m));
        }
    }

    #[test]
    fn label_strings_match_kernel_vocab() {
        assert_eq!(AttachModeLabel::Drv.as_str(), "drv");
        assert_eq!(AttachModeLabel::Skb.as_str(), "skb");
        assert_eq!(AttachModeLabel::Hw.as_str(), "hw");
    }

    #[test]
    fn pin_reuse_records_round_trip() {
        // Reset bitmap so prior tests don't contaminate.
        PIN_REUSED_BITMAP.store(0, Ordering::Relaxed);
        record_pin_reused("conntrack", true);
        record_pin_reused("stats", true);
        let snap = pin_reused_snapshot();
        let conntrack = snap.iter().find(|(n, _)| *n == "conntrack");
        let stats = snap.iter().find(|(n, _)| *n == "stats");
        let conntrack_v6 = snap.iter().find(|(n, _)| *n == "conntrack_v6");
        assert_eq!(conntrack.map(|(_, r)| *r), Some(true));
        assert_eq!(stats.map(|(_, r)| *r), Some(true));
        assert_eq!(conntrack_v6.map(|(_, r)| *r), Some(false));
    }

    #[test]
    fn pin_reuse_unknown_name_is_silent() {
        // Forward-compat: a future pin name added in the eBPF crate
        // but not yet in `pin_names()` must not panic.
        record_pin_reused("future_map", true);
        // No observable effect — just check the call doesn't blow up.
    }
}
