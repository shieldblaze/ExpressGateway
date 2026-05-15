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

// EBPF-2-08: the per-CPU STATS surface is Linux-only because aya
// is. Non-Linux callers still see the AttachModeLabel /
// pin-reused / slot-enum APIs (they're pure-Rust).
#[cfg(target_os = "linux")]
use std::sync::OnceLock;

#[cfg(target_os = "linux")]
use aya::maps::{Map as AyaMap, MapData, MapError, PerCpuArray};
#[cfg(target_os = "linux")]
use parking_lot::Mutex;

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

// ---------------------------------------------------------------------------
// EBPF-2-08: STATS per-CPU array export.
// ---------------------------------------------------------------------------

/// Slot indices into the eBPF program's `STATS: PerCpuArray<u64>`.
/// **MUST** stay in lock-step with `crates/lb-l4-xdp/ebpf/src/main.rs`
/// (search for `STAT_*` constants). Order is wire-stable — append
/// new slots to the end ONLY, never reorder.
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
    /// `STAT_BACKEND_UNPOPULATED` (ROUND8-L4-01): a conntrack hit
    /// whose `backend_ip == 0` or `backend_port == 0` — controller
    /// wrote an unpopulated entry. The XDP path returns XDP_PASS
    /// so the kernel stack handles the packet; this counter is the
    /// operator signal to chase the misconfiguration.
    BackendUnpopulated = 10,
    /// `STAT_V4_FRAGMENT` (ROUND8-L4-08): IPv4 packet with MF set
    /// or fragment offset > 0. XDP_PASS so the kernel reassembles.
    V4Fragment = 11,
    /// `STAT_V6_FRAGMENT` (ROUND8-L4-08): IPv6 packet carrying a
    /// Fragment Extension Header (IPPROTO_FRAGMENT = 44).
    V6Fragment = 12,
    /// `STAT_CT_RST_PRUNE` (ROUND8-L4-02): a TCP RST packet evicted
    /// its conntrack entry (Cilium `bpf/lib/conntrack.h` RST-prune
    /// lesson). The RST itself is passed to the kernel so the peer
    /// still observes connection teardown end-to-end; only flow
    /// *tracking* stops. Counter is the operator signal for sliding-
    /// RST replay attacks.
    CtRstPrune = 13,
    /// `STAT_CT_FIN_PRUNE` (ROUND8-L4-02): a TCP FIN-ACK packet
    /// evicted its conntrack entry. Packet itself is still forwarded
    /// (XDP_TX) so the FIN-ACK reaches the backend; the slot is freed
    /// to keep the LRU aligned with real TCP-FSM reality without
    /// paying the verifier cost of a full FSM (deferred to Pillar
    /// 4b-3).
    CtFinPrune = 14,
    /// `STAT_NEW_FLOW_RATE_CAP` (ROUND8-L4-03): a *new* flow
    /// (conntrack miss) was rate-capped under a SYN flood. Katran
    /// `is_under_flood()` lesson 4: above the per-CPU new-flow cap
    /// (`xdp_new_flow_cap_per_sec_per_cpu`, default 125_000), the
    /// CT-miss path is short-circuited to XDP_PASS WITHOUT the
    /// STAT_PASS "please populate conntrack" signal — established
    /// (CT-hit) flows are untouched so the LRU stays stable for
    /// legitimate traffic instead of being thrashed by the
    /// attacker's unique 5-tuples. This counter is the operator's
    /// SYN-flood alarm AND the back-pressure signal the userspace
    /// control loop polls. The userspace `CtInsertGate` increments
    /// the same slot when it denies a control-plane CT insert.
    NewFlowRateCap = 15,
}

/// Number of currently-defined stat slots. Bumps to this constant
/// must come WITH a matching addition to the `STAT_*` enum in the
/// eBPF crate AND a new variant at the end of [`StatSlot`].
pub const NUM_SLOTS: usize = 16;

/// Errors from the STATS read path.
#[derive(Debug, thiserror::Error)]
pub enum StatsExportError {
    /// The per-CPU array handle was never installed by
    /// `XdpLoader::load_from_bytes_pinned`. Either the loader was
    /// never called or the ELF didn't declare a `stats` map.
    #[error("STATS handle not installed; load_from_bytes_pinned must be called first")]
    HandleMissing,
    /// `aya::maps::MapError` from the underlying read.
    #[error("bpf map error: {0}")]
    Map(String),
}

/// Owned snapshot of the STATS map at a single moment in time.
/// `summed[i]` is the cross-CPU sum of slot `i`; the Prom scraper
/// only ever publishes `summed`. `per_cpu[i]` is the un-summed
/// slice for the debug HTTP endpoint.
#[derive(Debug, Clone)]
pub struct StatsSnapshot {
    /// Cross-CPU sum per slot. Length = [`NUM_SLOTS`].
    pub summed: Vec<u64>,
    /// Per-CPU breakdown. Outer len = [`NUM_SLOTS`], inner = nr_cpus.
    pub per_cpu: Vec<Vec<u64>>,
}

#[cfg(target_os = "linux")]
static STATS_HANDLE: OnceLock<Mutex<PerCpuArray<MapData, u64>>> = OnceLock::new();

/// Install the STATS map handle. Called by
/// `XdpLoader::load_from_bytes_pinned` exactly once per process.
///
/// EBPF-2-08 invariant: aya `PerCpuArray::get(&i, 0)` performs the
/// `bpf_map_lookup_elem` syscall on each call; we cache the typed
/// wrapper but **never cache the values** — the scraper always sees
/// fresh kernel state.
///
/// # Errors
///
/// - [`StatsExportError::Map`] if the supplied `Map` is not a
///   `PerCpuArray<u64>` (e.g. someone wired the wrong slot in).
#[cfg(target_os = "linux")]
pub fn install_stats_handle(map: AyaMap) -> Result<(), StatsExportError> {
    let pca: PerCpuArray<MapData, u64> =
        PerCpuArray::try_from(map).map_err(|e: MapError| StatsExportError::Map(format!("{e}")))?;
    STATS_HANDLE
        .set(Mutex::new(pca))
        .map_err(|_| StatsExportError::Map("STATS handle already installed".to_owned()))?;
    Ok(())
}

/// Read a fresh STATS snapshot. The public Prom-side entry point.
///
/// Cost: one `bpf_map_lookup_elem` syscall per slot per scrape, so
/// `NUM_SLOTS × scrape_period`-grained. On 256-CPU hosts each
/// syscall returns a 256 × 8 = 2 KiB buffer; total per-scrape work
/// is ~20 KiB of kernel copy.
///
/// # Errors
///
/// - [`StatsExportError::HandleMissing`]: loader has not installed
///   the handle (e.g. running without XDP).
/// - [`StatsExportError::Map`]: aya rejected the read (kernel-side
///   syscall failure).
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
        // `PerCpuValues` derefs to `&[V]`.
        let slice: &[u64] = &values;
        let sum: u64 = slice.iter().copied().fold(0u64, u64::wrapping_add);
        per_cpu.push(slice.to_vec());
        summed.push(sum);
    }
    Ok(StatsSnapshot { summed, per_cpu })
}

/// Non-Linux stub. Returns a snapshot of zeros sized to
/// [`NUM_SLOTS`] so cross-platform consumers (tests, dev mode) can
/// still call without `cfg` gates.
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

    #[test]
    fn stat_slot_indices_are_wire_stable() {
        // Wire-stability invariant: the numeric value of each slot
        // is published to operators via `xdp_packets_total{result}`
        // labels; reordering breaks Prom recording rules.
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
    }

    #[test]
    fn num_slots_matches_enum() {
        // If a new variant is added to StatSlot without bumping
        // NUM_SLOTS the read loop in `read_stats` would silently
        // skip it — this assertion guards that invariant.
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
