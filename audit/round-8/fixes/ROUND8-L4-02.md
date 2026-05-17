# Plan for ROUND8-L4-02 — LRU + TCP-state pruning (RST/FIN) + flood-rate cap

Finding-ref:     ROUND8-L4-02 (high, Open)
Reference:       Cilium `bpf/lib/conntrack.h` lesson 7 (RST / FIN-FIN-ACK pruning + long-idle); handoff item 2; references co-cited with ROUND8-L4-03 (Katran `is_under_flood()`).
Coverage-gap:    Theme 1 (EBPF-2-03 swapped HASH→LRU, marked Verified-Fixed; the *eviction-as-DoS* class never audited). Theme 2 (operational-vs-laboratory: only visible at >100k flows/sec).

Files touched:
  - `crates/lb-l4-xdp/ebpf/src/main.rs`             (parse TCP `flags` at L4 offset; prune CT on RST/FIN-ACK; new stat slots)
  - `crates/lb-l4-xdp/src/stats_export.rs`          (`StatSlot::{CtRstPrune, CtFinPrune}` labels)
  - `audit/deferred.md`                             (document TCP FSM as Pillar 4b-3+ work; capture the consequence of pure-LRU at our target QPS — open question from finding text)
  - `crates/lb-l4-xdp/tests/round8_conntrack_state.rs`  (NEW — proof tests)

Approach:

1. **Read TCP flags byte** in IPv4 / IPv6 TCP paths
   (`crates/lb-l4-xdp/ebpf/src/main.rs`):
   - Today the TCP header parse reads only `src_port`/`dst_port` (lines
     421-429 IPv4, mirror in v6). Add a third read of `tcp.flags`
     (offset 13 of TCP header per `network-types::tcp::TcpHdr`). The
     packed-field read pattern is the existing
     `core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).flags))`.
   - `TCP_FLAG_RST: u8 = 0x04;` and `TCP_FLAG_FIN: u8 = 0x01;` and
     `TCP_FLAG_ACK: u8 = 0x10;` constants.

2. **Prune on RST** (post-CT-lookup, pre-rewrite):
   ```rust
   if (flags & TCP_FLAG_RST) != 0 {
       let _ = CONNTRACK.remove(&key);   // aya::ebpf LruHashMap::remove
       incr_stat(STAT_CT_RST_PRUNE);
       return Ok(xdp_action::XDP_PASS); // packet continues to kernel/stack
   }
   ```
   Returning `XDP_PASS` (not `XDP_DROP`) keeps the upstream's RST flowing
   end-to-end; we only stop *tracking* the flow.

3. **Prune on FIN-ACK** (minimal version — no full FSM yet):
   ```rust
   if (flags & TCP_FLAG_FIN) != 0 && (flags & TCP_FLAG_ACK) != 0 {
       let _ = CONNTRACK.remove(&key);
       incr_stat(STAT_CT_FIN_PRUNE);
       // continue to rewrite + XDP_TX — last FIN-ACK still needs to land
   }
   ```
   Note: this is the *less-strict* Cilium minimum. A full FSM with
   SYN_SENT/SYN_RECV/ESTABLISHED/TIME_WAIT states is deferred to
   Pillar 4b-3 per the open question in the finding text; this plan
   ships the RST and FIN-ACK pruning that closes the
   replay-old-segment LRU-thrash vector.

4. **New stat slots** (`crates/lb-l4-xdp/ebpf/src/main.rs`):
   - `STAT_CT_RST_PRUNE: u32 = 11;` (slot 10 reserved by L4-01).
   - `STAT_CT_FIN_PRUNE: u32 = 12;`.
   - Extend `stats_export.rs` `StatSlot` enum to emit
     `xdp_conntrack_pruned_total{reason="rst|fin"}`.

5. **Document the deferred FSM** (`audit/deferred.md`):
   - Add a section "TCP FSM in BPF conntrack": short-term we ship
     RST/FIN-ACK pruning; full FSM (state machine with timers) is
     Pillar 4b-3; consequence-at-scale documented (pure LRU is
     replay-attackable; sliding-RST flood evicts live flows). Open
     question owner: ADR-0004 update.

6. **Proof tests** (`crates/lb-l4-xdp/tests/round8_conntrack_state.rs`,
   NEW):
   - `sim_rst_prunes_conntrack`: simulator inserts a CT entry, sends a
     packet with the RST flag set, asserts (a) the CT entry is gone,
     (b) `STAT_CT_RST_PRUNE` incremented, (c) action is `XDP_PASS`.
   - `sim_fin_ack_prunes_conntrack`: same for `FIN | ACK`. Action is
     `XDP_TX` (last FIN-ACK forwarded), CT removed afterwards.
   - `sim_replay_rst_does_not_evict_unrelated_flow`: insert two CT
     entries; send RST for the first; assert only the first is gone.
   - `sim_packet_without_flags_unchanged`: a normal SYN-ACK does not
     prune.

Proof:

- `cargo test -p lb-l4-xdp --test round8_conntrack_state` (userspace
  sim, no kernel needed).
- After the eBPF patch, re-capture verifier logs per
  `scripts/verify-xdp.sh 5.15`, `6.1`, `6.6` and commit the diffed
  baselines (ROUND8-L4-10 cross-ref).
- A scale validation harness (separate work item under Pillar 4b
  acceptance gate per Theme 2): spray 1M old-flow RST packets/sec at
  a VIP with 10k live flows; assert live-flow eviction rate stays
  below pre-fix baseline.

Risk / blast radius:

- The extra TCP-flags read adds one packed-field read + two `and`/`jnz`
  per TCP packet. Verifier-trivial; benchmark expected delta < 1% on
  the hot path.
- `CONNTRACK.remove(&key)` from the BPF program returns an `Option`;
  we discard the result (mirror Cilium's pattern). Aya 0.13.x supports
  `LruHashMap::remove` from the eBPF crate.
- This patch alone does NOT close ROUND8-L4-03 (SYN flood new-flow-
  rate cap). Both findings are needed; they are orthogonal.

Cross-ref:
- ROUND8-L4-03 (SYN-flood new-flow-rate cap): orthogonal; needs its
  own plan. The finding text explicitly notes "the flood cap is
  orthogonal and both are needed."
- ROUND8-L4-10: verifier-log re-capture required after this patch.
- `audit/ebpf/round-2-review.md` EBPF-2-03: status should be amended
  to note that the LRU swap is necessary-but-not-sufficient; the
  state-aware pruning lands here.

Owner:           div-l4
Lead-approval: approved 2026-05-14 team-lead-r8
