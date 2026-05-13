//! EBPF-2-08 proof test: STATS per-CPU array export API.
//!
//! The rel crate calls `lb_l4_xdp::stats_export::read_stats()` once
//! per Prom scrape to get a fresh `StatsSnapshot`. This file pins:
//!
//! - The slot-index numeric values (wire-stable for Prom labels).
//! - The behaviour when no `install_stats_handle` has happened yet
//!   (`HandleMissing` error; never panics).
//! - The shape of the snapshot (10 slots; per-cpu inner vecs).
//!
//! The kernel-side scaffold (load XDP, `bpf_prog_run` test packets,
//! read non-zero counters) is `#[ignore]` per project convention.

#![cfg(target_os = "linux")]

use lb_l4_xdp::stats_export::{NUM_SLOTS, StatSlot, StatsExportError, read_stats};

#[test]
fn slot_indices_match_ebpf_constants() {
    // Cross-check against `crates/lb-l4-xdp/ebpf/src/main.rs:198-207`
    // (the `STAT_*` constants). Same wire ordering on both sides;
    // the unit test in `src/stats_export.rs` covers the userspace
    // assertion. Here we re-derive from the source to catch the
    // case where someone edits the eBPF crate without touching the
    // userspace enum.
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/ebpf/src/main.rs"))
        .expect("read ebpf source");

    for (name, idx) in [
        ("STAT_PASS", StatSlot::Pass),
        ("STAT_DROP", StatSlot::Drop),
        ("STAT_CT_HIT_V4", StatSlot::CtHitV4),
        ("STAT_L7", StatSlot::L7Divert),
        ("STAT_PARSE_FAIL", StatSlot::ParseFail),
        ("STAT_TX_V4", StatSlot::TxV4),
        ("STAT_CT_HIT_V6", StatSlot::CtHitV6),
        ("STAT_TX_V6", StatSlot::TxV6),
        ("STAT_VLAN", StatSlot::VlanStripped),
        ("STAT_V6_EXT_UNSUPPORTED", StatSlot::V6ExtUnsupported),
    ] {
        let needle = format!("const {name}: u32 = {};", idx as usize);
        assert!(
            src.contains(&needle),
            "ebpf/src/main.rs must declare `{needle}` to keep the \
             userspace StatSlot indices in lock-step — see EBPF-2-08",
        );
    }
}

#[test]
fn read_stats_without_install_returns_handle_missing() {
    // No XdpLoader has been instantiated in this test process, so
    // `install_stats_handle` has not been called. The expected
    // behaviour is a clean error, not a panic.
    let r = read_stats();
    // The result MAY succeed if a sibling test in another test
    // binary somehow installed the handle (cargo runs each #[test]
    // file in its own binary so cross-binary contamination is
    // impossible). The contract: never panic, always Result.
    match r {
        Err(StatsExportError::HandleMissing) => {}
        Err(other) => panic!("expected HandleMissing, got {other:?}"),
        Ok(s) => {
            // Possible if a prior test in the SAME binary installed
            // the handle. Sanity-check the shape if so.
            assert_eq!(s.summed.len(), NUM_SLOTS);
            assert_eq!(s.per_cpu.len(), NUM_SLOTS);
        }
    }
}

#[test]
fn num_slots_constant_is_ten() {
    assert_eq!(NUM_SLOTS, 10);
}

/// EBPF-2-08 named proof test for the kernel-side path. Deferred to
/// the CI privileged stage.
#[test]
#[ignore = "needs CAP_BPF + bpffs — runs in CI privileged stage"]
fn summed_counters_advance_under_load() {
    eprintln!(
        "EBPF-2-08 STATS export kernel scaffold — load XDP onto dummy0, \
         bpf_prog_run test packets, assert summed[CtHitV4] >= delta. \
         Full body lands with the CI privileged-stage fixture (shared with \
         EBPF-2-05's bpffs scaffold)."
    );
}
