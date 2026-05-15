### ROUND8-L4-02 — Pure LRU conntrack with no TCP state awareness (Cilium lesson 7)

Reference: `audit/round-8/research/cilium.md` lesson 7 (`conntrack.h` invalidates entries on RST / FIN-FIN-ACK / long-idle) + handoff item 2
Our equivalent: `crates/lb-l4-xdp/ebpf/src/main.rs:235-241` (`CONNTRACK` / `CONNTRACK_V6` declared as `LruHashMap`), `handle_ipv4`/`handle_ipv6` decision sites do not inspect TCP flags

Severity: high
Status:   Proposed-Fix (div-l4, task#73, 2026-05-15, commit 67024106's sibling d4d81e40 `ROUND8-L4-02/08 — repair NUM_SLOTS sibling assertions`) — the push-back is addressed: `round8_conntrack_state.rs:221` no longer hardcodes `assert_eq!(CtFinPrune+1, NUM_SLOTS)`. Re-anchored to the enum: every prune slot must be `< NUM_SLOTS` (inside the `read_stats()` loop) AND `NewFlowRateCap+1 == NUM_SLOTS` (loop bounded exactly to the last STAT_*-backed slot). A future slot add now self-updates instead of silently dropping a kernel slot. Proof: `cargo test -p lb-l4-xdp --test round8_conntrack_state` 7/7 PASS (was 6/1-FAIL). L4-03 BPF source already changed under 43d250ee; verifier-log re-capture is the ROUND8-L4-10 diff-gate's job at first privileged CI run (cross-ref L4-10; no BPF source touched by this repair — tests only). Verify re-checks.
          [Prior: Push-back (verify, task#70, 2026-05-15) — eBPF RST/FIN-ACK prune logic correct, but proof `round8_conntrack_state.rs:221` FAILED at branch tip: L4-03 bumped NUM_SLOTS 15->16 without repairing this finding's `assert_eq!(CtFinPrune+1, NUM_SLOTS)`. See audit/round-8/verify/l4.md.]

Divergence:
- Cilium: tracks ESTABLISHED / SYN_SENT / SYN_RECV / CLOSE_WAIT transitions; entries pruned on RST, on FIN-FIN-ACK, and on long-idle.
- Us: `CONNTRACK: LruHashMap<FlowKey, BackendEntry>` (1M IPv4, 512k IPv6). No state machine. Entries are evicted only by the kernel's LRU policy. `TcpHdr` parses only `src_port`/`dst_port` — we don't even *look* at flags.

Impact:
- An attacker who has observed any segment of a closed flow can replay any old TCP byte to keep that flow's conntrack entry "recently used", evicting live flows.
- Closed flows (RST seen, FIN-FIN-ACK exchanged) remain resident until LRU eviction — slow drain pushes capacity downward.
- The previous Round-7 audit graded EBPF-2-03 (LRU swap) as Verified-Fixed but the swap from HASH to LRU only solves the ENOMEM-on-fill problem; the TCP-state-aware pruning problem the Cilium team solved in `conntrack.h` is wide open.

Reproduction:
- This is a "lesson-not-yet-paid-for". We haven't run at scale. The bug shape: write a script that replays one old SYN-ACK every second for every closed flow seen; observe live-flow miss rate climb as LRU evicts them.

Recommendation:
1. Short-term: in the IPv4/IPv6 paths, parse TCP `flags` (offset 13 of the TCP header). If `RST` (`0x04`) is set, do `CONNTRACK.remove(&key)` before deciding the action, and `incr_stat(STAT_CT_RST_PRUNE)`. This is cheap and closes the replay-old-RST eviction attack.
2. Medium-term (Pillar 4b-3 deferred): minimal TCP FSM. Add `BackendEntry.state: u8` field, store `SYN_SENT` on first SYN, `ESTABLISHED` on ACK, prune on FIN-ACK exchange. Mirrors Cilium's reduced state machine.
3. Add `STAT_CT_RST_PRUNE`, `STAT_CT_FIN_PRUNE` counters. Wire to `xdp_conntrack_pruned_total{reason}`.

Open question for the team: ADR-0004 deferred TCP option rewrite to Pillar 4b-3; the same ADR should explicitly document whether TCP state awareness is *also* deferred and what the consequence of "pure LRU + replay" is at our target QPS.
