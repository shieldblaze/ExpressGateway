//! Lock-step API boundary between the eBPF data plane and userspace observability. Everything here
//! is safe, lock-free and panic-free at steady state — telemetry must never be the reason
//! production aborts.
use std::sync::atomic::{AtomicU8, Ordering};

// EBPF-2-08: the per-CPU STATS surface is Linux-only because aya is. The label / pin-reused /
// slot-enum APIs are pure Rust and stay available everywhere.
#[cfg(target_os = "linux")]
use std::sync::OnceLock;

#[cfg(target_os = "linux")]
use aya::maps::{Map as AyaMap, MapData, MapError, PerCpuArray};
#[cfg(target_os = "linux")]
use parking_lot::Mutex;

/// Coarse-grained mode label for the Prometheus `xdp_attach_mode` gauge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachModeLabel {
    /// Native driver mode (`XDP_FLAGS_DRV_MODE`).
    Drv,
    /// Generic SKB mode (`XDP_FLAGS_SKB_MODE`).
    Skb,
    /// Hardware offload (`XDP_FLAGS_HW_MODE`).
    Hw,
}

impl AttachModeLabel {
    /// Stable byte encoding for atomic storage. Sentinel `0xFF` = not set.
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

    /// Prometheus label value, matching the kernel API vocabulary so an operator can compare it
    /// against `bpftool net show`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Drv => "drv",
            Self::Skb => "skb",
            Self::Hw => "hw",
        }
    }
}

/// Sentinel for "no attach mode recorded yet".
const ATTACH_MODE_UNSET: u8 = 0xFF;

/// Process-global atomic store of the current XDP attach mode. Single producer (the startup attach
/// path), many consumers; at most one XDP attach per process.
static ATTACH_MODE: AtomicU8 = AtomicU8::new(ATTACH_MODE_UNSET);

/// Record which mode the XDP loader successfully attached in. Latest call wins.
pub fn record_attach_mode(mode: AttachModeLabel) {
    ATTACH_MODE.store(mode.as_byte(), Ordering::Relaxed);
}

/// Read back the current attach mode. `None` when XDP has not been attached, so the gauge reports 0
/// for every mode rather than fabricating a value.
#[must_use]
pub fn current_attach_mode() -> Option<AttachModeLabel> {
    AttachModeLabel::from_byte(ATTACH_MODE.load(Ordering::Relaxed))
}

/// Process-global count of `Drv` attaches the blocklist refused or the probe found dead, forcing a
/// demotion to `Skb`. Userspace-only — the BPF program never runs when an attach silently drops, so
/// there is no kernel `STATS` slot.
static ATTACH_PROBE_FAILED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Increment the attach-probe-failed counter.
pub fn record_attach_probe_failed() {
    ATTACH_PROBE_FAILED.fetch_add(1, Ordering::Relaxed);
}

/// Read back the cumulative attach-probe-failed count for Prom exposition
/// (`xdp_attach_probe_failed_total`).
#[must_use]
pub fn attach_probe_failed_count() -> u64 {
    ATTACH_PROBE_FAILED.load(Ordering::Relaxed)
}

/// Snapshot of which pinned maps were reused vs. freshly created at startup. Bit `i` is `1` if the
/// `i`-th pin in [`pin_names()`] was reused; the packing keeps the Prom scrape a single atomic load
/// projected to per-name gauges, no Mutex.
static PIN_REUSED_BITMAP: AtomicU8 = AtomicU8::new(0);

/// Canonical pin-name ordering for the bitmap. Append to the END only — bit positions are
/// wire-stable.
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

/// Record whether the named pin was reused. Unknown names are silently dropped (forward
/// compatibility with future pin additions).
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

/// Slot indices into the eBPF program's `STATS: PerCpuArray<u64>`. MUST stay in lock-step with the
/// `STAT_*` constants in `ebpf/src/main.rs`; order is wire-stable, so append only.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum StatSlot {
    /// `STAT_PASS`: packet was passed to the kernel stack.
    Pass = 0,
    /// `STAT_DROP`: packet was dropped by an ACL deny.
    Drop = 1,
    /// `STAT_CT_HIT_V4`: IPv4 conntrack lookup hit.
    CtHitV4 = 2,
    /// `STAT_L7`: dst-port matched the L7 divert list.
    L7Divert = 3,
    /// `STAT_PARSE_FAIL`: header parse failed.
    ParseFail = 4,
    /// `STAT_TX_V4`: IPv4 rewrite + XDP_TX issued.
    TxV4 = 5,
    /// `STAT_CT_HIT_V6`: IPv6 conntrack lookup hit.
    CtHitV6 = 6,
    /// `STAT_TX_V6`: IPv6 rewrite + XDP_TX issued.
    TxV6 = 7,
    /// `STAT_VLAN`: a single 802.1Q tag was stripped.
    VlanStripped = 8,
    /// `STAT_V6_EXT_UNSUPPORTED`: too many IPv6 extension headers.
    V6ExtUnsupported = 9,
    /// `STAT_BACKEND_UNPOPULATED` (ROUND8-L4-01): a conntrack hit whose `backend_ip == 0` or
    /// `backend_port == 0` — controller wrote an unpopulated entry.
    BackendUnpopulated = 10,
    /// `STAT_V4_FRAGMENT` (ROUND8-L4-08): IPv4 packet with MF set or fragment offset > 0.
    V4Fragment = 11,
    /// `STAT_V6_FRAGMENT` (ROUND8-L4-08): IPv6 packet carrying a Fragment Extension Header
    /// (IPPROTO_FRAGMENT = 44).
    V6Fragment = 12,
    /// `STAT_CT_RST_PRUNE` (ROUND8-L4-02): a TCP RST packet evicted its conntrack entry (Cilium
    /// `bpf/lib/conntrack.h` RST-prune lesson).
    CtRstPrune = 13,
    /// `STAT_CT_FIN_PRUNE` (ROUND8-L4-02): a TCP FIN-ACK packet evicted its conntrack entry.
    CtFinPrune = 14,
    /// `STAT_NEW_FLOW_RATE_CAP` (ROUND8-L4-03): a new flow was rate-capped under a SYN flood
    /// (Katran `is_under_flood()`). The userspace `CtInsertGate` increments the SAME slot when it
    /// denies a control-plane CT insert.
    NewFlowRateCap = 15,
    /// `xdp_attach_probe_failed_total` (ROUND8-L4-05): the blocklist or probe found the requested
    /// `Drv` attach dead and the loader demoted to `Skb`. NOT a kernel per-CPU slot — no `STAT_*`
    /// constant, surfaced via [`attach_probe_failed_count`], never `read_stats()`, but it holds a
    /// wire-stable position so the slot vocabulary stays single-sourced.
    AttachProbeFailed = 16,
}

/// Number of KERNEL-side per-CPU `STATS` slots — the length of the `read_stats()` lookup loop. A
/// bump MUST come with a new `STAT_*` constant in the eBPF crate AND a new [`StatSlot`].
///
/// ROUND8-L4-05: `StatSlot::AttachProbeFailed` (16) is deliberately NOT counted — it is
/// userspace-only, and keeping `NUM_SLOTS == 16` is what bounds the kernel read loop to real kernel
/// slots.
pub const NUM_SLOTS: usize = 16;

/// Errors from the STATS read path.
#[derive(Debug, thiserror::Error)]
pub enum StatsExportError {
    /// The per-CPU array handle was never installed by `XdpLoader::load_from_bytes_pinned`.
    #[error("STATS handle not installed; load_from_bytes_pinned must be called first")]
    HandleMissing,
    /// `aya::maps::MapError` from the underlying read.
    #[error("bpf map error: {0}")]
    Map(String),
}

/// Owned snapshot of the STATS map at one moment. `summed[i]` is the cross-CPU sum (all the Prom
/// scraper publishes); `per_cpu[i]` is the un-summed slice for the debug endpoint.
#[derive(Debug, Clone)]
pub struct StatsSnapshot {
    /// Cross-CPU sum per slot.
    pub summed: Vec<u64>,
    /// Per-CPU breakdown.
    pub per_cpu: Vec<Vec<u64>>,
}

#[cfg(target_os = "linux")]
static STATS_HANDLE: OnceLock<Mutex<PerCpuArray<MapData, u64>>> = OnceLock::new();

/// Install the STATS map handle, once per process. EBPF-2-08 invariant: the typed wrapper is cached
/// but the VALUES never are — each `PerCpuArray::get` is a fresh `bpf_map_lookup_elem`, so the
/// scraper always sees live state.
#[cfg(target_os = "linux")]
pub fn install_stats_handle(map: AyaMap) -> Result<(), StatsExportError> {
    let pca: PerCpuArray<MapData, u64> =
        PerCpuArray::try_from(map).map_err(|e: MapError| StatsExportError::Map(format!("{e}")))?;
    STATS_HANDLE
        .set(Mutex::new(pca))
        .map_err(|_| StatsExportError::Map("STATS handle already installed".to_owned()))?;
    Ok(())
}

/// Read a fresh STATS snapshot — the public Prom-side entry point. Cost: one `bpf_map_lookup_elem`
/// per slot per scrape; on a 256-CPU host each syscall returns 2 KiB, so a scrape copies ~20 KiB of
/// kernel memory.
#[cfg(target_os = "linux")]
pub fn read_stats() -> Result<StatsSnapshot, StatsExportError> {
    let handle = STATS_HANDLE.get().ok_or(StatsExportError::HandleMissing)?;
    let guard = handle.lock();
    let mut per_cpu = Vec::with_capacity(NUM_SLOTS);
    let mut summed = Vec::with_capacity(NUM_SLOTS);
    for i in 0..(NUM_SLOTS as u32) {
        let values = guard
            .get(&i, 0)
            .map_err(|e: MapError| StatsExportError::Map(format!("{e}")))?;
        let slice: &[u64] = &values;
        let sum: u64 = slice.iter().copied().fold(0u64, u64::wrapping_add);
        per_cpu.push(slice.to_vec());
        summed.push(sum);
    }
    Ok(StatsSnapshot { summed, per_cpu })
}

/// Non-Linux stub returning zeros sized to [`NUM_SLOTS`], so cross-platform consumers need no `cfg`
/// gates.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn read_stats() -> Result<StatsSnapshot, StatsExportError> {
    Ok(StatsSnapshot {
        summed: vec![0u64; NUM_SLOTS],
        per_cpu: vec![Vec::new(); NUM_SLOTS],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_returns_none() {
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
        // Forward-compat: a pin name added in the eBPF crate but not yet in `pin_names()` must not
        // panic.
        record_pin_reused("future_map", true);
    }

    #[test]
    fn stat_slot_indices_are_wire_stable() {
        // Wire-stability invariant: each slot's numeric value is published to operators via
        // `xdp_packets_total{result}` labels; reordering breaks Prom recording rules.
        assert_eq!(StatSlot::Pass as usize, 0);
        assert_eq!(StatSlot::Drop as usize, 1);
        assert_eq!(StatSlot::CtHitV4 as usize, 2);
        assert_eq!(StatSlot::L7Divert as usize, 3);
        assert_eq!(StatSlot::ParseFail as usize, 4);
        assert_eq!(StatSlot::TxV4 as usize, 5);
        assert_eq!(StatSlot::CtHitV6 as usize, 6);
        assert_eq!(StatSlot::TxV6 as usize, 7);
        assert_eq!(StatSlot::VlanStripped as usize, 8);
        assert_eq!(StatSlot::V6ExtUnsupported as usize, 9);
        assert_eq!(StatSlot::BackendUnpopulated as usize, 10);
        assert_eq!(StatSlot::V4Fragment as usize, 11);
        assert_eq!(StatSlot::V6Fragment as usize, 12);
        assert_eq!(StatSlot::CtRstPrune as usize, 13);
        assert_eq!(StatSlot::CtFinPrune as usize, 14);
        assert_eq!(StatSlot::NewFlowRateCap as usize, 15);
        // ROUND8-L4-05: userspace-only slot (NOT counted in NUM_SLOTS — see the const doc). Wire
        // position is still stable so the exposition vocabulary is single-sourced.
        assert_eq!(StatSlot::AttachProbeFailed as usize, 16);
    }

    #[test]
    fn attach_probe_failed_counter_round_trip() {
        let before = attach_probe_failed_count();
        record_attach_probe_failed();
        record_attach_probe_failed();
        assert_eq!(attach_probe_failed_count(), before + 2);
    }

    #[test]
    fn num_slots_matches_enum() {
        // A new StatSlot variant without a NUM_SLOTS bump would be silently skipped by the
        // `read_stats` read loop — this assertion guards that.
        assert_eq!(NUM_SLOTS, 16);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_stub_returns_zeros() {
        let snap = read_stats().expect("non-linux stub is infallible");
        assert_eq!(snap.summed.len(), NUM_SLOTS);
        assert!(snap.summed.iter().all(|&v| v == 0));
    }
}
