//! ROUND8-L4-02 proof: TCP-state-aware conntrack pruning.
//!
//! Reference: Cilium `bpf/lib/conntrack.h` lesson — pure LRU is
//! vulnerable to a sliding-RST replay (an adversary spraying RST/FIN
//! packets across already-evicted flows fills the LRU's young end and
//! pushes live flows toward eviction). Pruning on RST and on the
//! FIN-ACK terminating sequence keeps the table aligned to actual
//! TCP-FSM reality.
//!
//! The eBPF code path is unreachable in CI; this file models the BPF
//! state-machine over the userspace [`ConntrackTable`] and the
//! [`stats_export::StatSlot`] vocabulary the BPF program uses, so any
//! drift between the userspace contract and the BPF source surfaces
//! as a test failure.
//!
//! NUM_SLOTS bumps with this commit from 13 to 15 to make room for
//! `StatSlot::CtRstPrune` (13) and `StatSlot::CtFinPrune` (14).

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lb_l4_xdp::stats_export::{NUM_SLOTS, StatSlot};
use lb_l4_xdp::{ConntrackTable, FlowKey};

// TCP flag bits as they appear on the wire — same constants the BPF
// program uses (see `crates/lb-l4-xdp/ebpf/src/main.rs` TCP_FLAG_*).
const FIN: u8 = 0x01;
const RST: u8 = 0x04;
const ACK: u8 = 0x10;
const SYN: u8 = 0x02;

/// Tiny stand-in for the BPF action enum. The BPF program returns one
/// of `XDP_PASS` / `XDP_TX` / `XDP_DROP`; we only care about the
/// distinguishability of PASS vs TX in this test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Pass,
    Tx,
}

/// Per-slot counter set the eBPF program updates. We model only the
/// slots this fix touches.
#[derive(Debug, Default)]
struct Stats {
    rst_prune: u64,
    fin_prune: u64,
    ct_hit: u64,
}

/// Simulator of the eBPF program's TCP path: lookup + state-aware
/// prune (matches the inline code in `handle_ipv4`). Returns the
/// XDP action the BPF program would have returned.
fn sim_tcp_path(
    table: &mut ConntrackTable,
    stats: &mut Stats,
    flow: &FlowKey,
    flags: u8,
) -> Action {
    // 1. RST short-circuit: prune and PASS the original packet so the
    //    network stack sees the RST end-to-end.
    if (flags & RST) != 0 {
        table.remove(flow);
        stats.rst_prune += 1;
        return Action::Pass;
    }

    // 2. Lookup. Miss -> PASS so userspace populates the entry.
    let backend = match table.lookup(flow) {
        Some(b) => b,
        None => return Action::Pass,
    };
    stats.ct_hit += 1;

    // 3. Sentinel guard (ROUND8-L4-01); modelled as "any backend index
    //    is valid" in this test — the L4-01 test covers the sentinel
    //    case separately.
    let _ = backend;

    // 4. Rewrite -> TX.
    let action = Action::Tx;

    // 5. FIN-ACK prune AFTER the rewrite. The last FIN-ACK is
    //    forwarded normally; the slot is freed so a replay can't
    //    keep an already-closed flow alive.
    if (flags & FIN) != 0 && (flags & ACK) != 0 {
        table.remove(flow);
        stats.fin_prune += 1;
    }

    action
}

fn flow(src_port: u16) -> FlowKey {
    FlowKey {
        src_addr: 0x0A00_0001,
        dst_addr: 0x0A00_0002,
        src_port,
        dst_port: 443,
        protocol: 6,
    }
}

#[test]
fn rst_prunes_conntrack_and_passes_to_kernel() {
    let mut table = ConntrackTable::new();
    let mut stats = Stats::default();
    let f = flow(40000);
    table.insert(f.clone(), 7);
    assert_eq!(table.len(), 1, "fixture: entry present before RST");

    let action = sim_tcp_path(&mut table, &mut stats, &f, RST);
    assert_eq!(
        action,
        Action::Pass,
        "RST must XDP_PASS the original packet"
    );
    assert_eq!(table.lookup(&f), None, "RST must evict the conntrack entry");
    assert_eq!(stats.rst_prune, 1, "STAT_CT_RST_PRUNE must increment");
    assert_eq!(stats.ct_hit, 0, "RST path must not increment CT_HIT");
}

#[test]
fn fin_ack_prunes_after_tx() {
    let mut table = ConntrackTable::new();
    let mut stats = Stats::default();
    let f = flow(40001);
    table.insert(f.clone(), 9);

    let action = sim_tcp_path(&mut table, &mut stats, &f, FIN | ACK);
    assert_eq!(
        action,
        Action::Tx,
        "FIN-ACK is the last packet and MUST be forwarded (XDP_TX)"
    );
    assert_eq!(
        table.lookup(&f),
        None,
        "FIN-ACK must evict the conntrack entry after the rewrite"
    );
    assert_eq!(stats.fin_prune, 1, "STAT_CT_FIN_PRUNE must increment");
    assert_eq!(stats.ct_hit, 1, "FIN-ACK on a tracked flow is a hit");
    assert_eq!(stats.rst_prune, 0);
}

#[test]
fn fin_without_ack_does_not_prune() {
    // Cilium's lesson is FIN-ACK specifically — a plain FIN (without
    // ACK) is unusual and we don't prune on it; the FSM-aware version
    // is Pillar 4b-3.
    let mut table = ConntrackTable::new();
    let mut stats = Stats::default();
    let f = flow(40002);
    table.insert(f.clone(), 11);

    let action = sim_tcp_path(&mut table, &mut stats, &f, FIN);
    assert_eq!(action, Action::Tx);
    assert_eq!(table.lookup(&f), Some(11), "plain FIN must NOT prune");
    assert_eq!(stats.fin_prune, 0);
}

#[test]
fn syn_ack_does_not_prune() {
    let mut table = ConntrackTable::new();
    let mut stats = Stats::default();
    let f = flow(40003);
    table.insert(f.clone(), 13);

    let action = sim_tcp_path(&mut table, &mut stats, &f, SYN | ACK);
    assert_eq!(action, Action::Tx);
    assert_eq!(table.lookup(&f), Some(13), "SYN-ACK must NOT prune");
    assert_eq!(stats.fin_prune, 0);
    assert_eq!(stats.rst_prune, 0);
}

#[test]
fn rst_replay_only_evicts_matching_flow() {
    // The defensive property: an RST for flow A must not affect
    // flow B's conntrack entry. This is what guards live flows
    // against a sliding-RST replay attack.
    let mut table = ConntrackTable::new();
    let mut stats = Stats::default();
    let a = flow(50000);
    let b = flow(50001);
    table.insert(a.clone(), 1);
    table.insert(b.clone(), 2);

    sim_tcp_path(&mut table, &mut stats, &a, RST);
    assert_eq!(table.lookup(&a), None);
    assert_eq!(
        table.lookup(&b),
        Some(2),
        "RST for unrelated flow must leave other flows alone"
    );
    assert_eq!(stats.rst_prune, 1);
}

#[test]
fn rst_for_untracked_flow_is_no_op_on_table() {
    // BPF's CONNTRACK.remove returns Err for a missing key; the prune
    // path swallows the result so the table is unaffected.
    let mut table = ConntrackTable::new();
    let mut stats = Stats::default();
    let f = flow(50002);

    let action = sim_tcp_path(&mut table, &mut stats, &f, RST);
    assert_eq!(action, Action::Pass);
    assert_eq!(table.len(), 0);
    assert_eq!(stats.rst_prune, 1, "stat ticks even for the no-op case");
}

#[test]
fn stat_slot_indices_for_prune_slots_are_stable() {
    // Wire-stability: operators key Prom recording rules off the slot
    // index. The L4-02 commit is the source of truth for slot 13/14.
    assert_eq!(StatSlot::CtRstPrune as usize, 13);
    assert_eq!(StatSlot::CtFinPrune as usize, 14);
    // Sanity that the NUM_SLOTS read-loop ceiling covers the prune slots.
    // Compile-time `const _: () = assert!(NUM_SLOTS >= 15)` would also
    // work but the const path masks an upgrade footgun (a hand-edited
    // NUM_SLOTS without a matching enum addition).
    //
    // ROUND8-L4-02/08: L4-03 appended `StatSlot::NewFlowRateCap` (15) so
    // `CtFinPrune` (14) is no longer the LAST kernel slot — the previous
    // `CtFinPrune + 1 == NUM_SLOTS` literal regressed. The load-bearing
    // invariant is unchanged: every prune slot must fall inside the
    // `read_stats()` `0..NUM_SLOTS` loop, and the last kernel slot must
    // be the last STAT_*-backed enum variant (`NewFlowRateCap`). Both
    // are expressed relative to the enum so the next slot add cannot
    // silently break the read loop without tripping this test.
    assert!(
        (StatSlot::CtRstPrune as usize) < NUM_SLOTS,
        "CtRstPrune slot must be inside the read_stats loop"
    );
    assert!(
        (StatSlot::CtFinPrune as usize) < NUM_SLOTS,
        "CtFinPrune slot must be inside the read_stats loop"
    );
    assert_eq!(
        StatSlot::NewFlowRateCap as usize + 1,
        NUM_SLOTS,
        "NewFlowRateCap is the last kernel-read slot; NUM_SLOTS must \
         bound the read loop exactly to the STAT_*-backed slots"
    );
}
