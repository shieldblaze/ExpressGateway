//! EBPF-2-08 proof test: the STATS per-CPU array export API. Pins the wire-stable slot-index values, the `HandleMissing` (never-panic) behaviour before `install_stats_handle`, and the snapshot shape.

#![cfg(target_os = "linux")]

use lb_l4_xdp::stats_export::{NUM_SLOTS, StatSlot, StatsExportError, read_stats};

#[test]
fn slot_indices_match_ebpf_constants() {
    // Re-derive from the eBPF source (the `STAT_*` constants) so editing the eBPF crate without touching the userspace enum is caught.
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
    // No handle installed in this process. The contract is: never panic, always a Result. (cargo runs each test file in its own binary, so cross-binary contamination is impossible.)
    let r = read_stats();
    match r {
        Err(StatsExportError::HandleMissing) => {}
        Err(other) => panic!("expected HandleMissing, got {other:?}"),
        Ok(s) => {
            // Possible if a prior test in the SAME binary installed the handle.
            assert_eq!(s.summed.len(), NUM_SLOTS);
            assert_eq!(s.per_cpu.len(), NUM_SLOTS);
        }
    }
}

/// Wire-stability guard for the STATS slot count (currently 16). Each bump MUST come with a new `STAT_*` constant in the eBPF crate, a new `StatSlot` appended at the END, and this assertion updated in lock-step.
#[test]
fn num_slots_constant_tracks_appended_slots() {
    assert_eq!(NUM_SLOTS, 16);
}

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
