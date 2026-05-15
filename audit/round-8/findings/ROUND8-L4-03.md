### ROUND8-L4-03 — No SYN-flood / new-flow-rate cap on conntrack writes (Katran lesson 4)

Reference: `audit/round-8/research/katran.md` lesson 4 (`is_under_flood()`) + handoff item 3
Our equivalent: `crates/lb-l4-xdp/ebpf/src/main.rs:464-470` (CONNTRACK lookup; insert is userspace-side via `XdpLoader::conntrack_map`)

Severity: high
Status:   Proposed-Fix — eBPF per-CPU `is_under_flood()` gate (Katran
          lesson 4) on the CT-miss path of `handle_ipv4`/`handle_ipv6`
          (`new_flow_rate` per-CPU `RateWindow` + runtime-tunable
          `new_flow_cap_cfg`); userspace `CtInsertGate` leaky-bucket
          mirror in `loader.rs`; `xdp_new_flow_cap_per_sec_per_cpu`
          config knob (default 125_000, range 0|1k..=10M) with
          validation clamp; `StatSlot::NewFlowRateCap` (slot 15);
          proof `crates/lb-l4-xdp/tests/round8_synflood_cap.rs`
          (4 tests, green). BPF source changed → verifier-log
          baselines need CI re-capture (cross-ref ROUND8-L4-10).
          Awaiting verification teammate sign-off.

Divergence:
- Katran: per-CPU "new-connection rate" tracker (`MAX_CONN_RATE = 125k/s/core`). When the rate is exceeded, *new* LRU inserts are skipped — the LRU stays stable for established flows. The unrolled effect: under a SYN flood the LRU is not thrashed.
- Us: no rate tracker, period. Today, the LRU lookup in the BPF program is read-only (insertion happens in userspace via `XdpLoader::conntrack_map().insert()`). On the surface that looks like "we don't need the cap because we don't write from BPF". In reality, when userspace catches up on new flows (e.g. via the `lb-balancer` selecting backends and pushing entries), under a SYN flood it will be pushing millions of throwaway entries/sec into the LRU — same effect, different lever.
- More dangerous than Katran: our flow-control loop is in userspace, so the back-pressure path is async; the BPF map is the only buffer between an attacker's RPS and our control plane's ability to keep up.

Impact:
- A trivial SYN flood (≥125k flows/sec/core, achievable with `hping3` from a single host) saturates `XdpLoader::conntrack_map().insert()` writes; legitimate established flows (which already have entries) survive only until the LRU evicts them. Attack lasts as long as the attacker wants.
- The previous Round-7 audit's EBPF-2-03 fix (HASH -> LRU) made eviction graceful (no ENOMEM), but graceful eviction *is* the attack surface — old established flows are the LRU losers.

Reproduction:
- Lesson-not-yet-paid-for. We haven't run at SYN-flood scale. Bug shape: spray 1M unique 5-tuples/sec at a VIP, watch established TCP sessions stutter as their conntrack entries get evicted.

Recommendation:
1. Add a per-CPU "new-flow-rate" counter to the BPF program, sampled by `bpf_ktime_get_ns()` or a 1-second sliding window. If `new_flows_this_second > NEW_FLOW_CAP`, return `XDP_PASS` for the *new* flow without notifying userspace; established flows keep using their existing conntrack entries.
2. The control-plane side (`lb-balancer` driving `XdpLoader::conntrack_map().insert()`) must also rate-limit pushes. Today there is no such rate limit in `crates/lb-l4-xdp/src/loader.rs`.
3. Add `STAT_NEW_FLOW_RATE_CAP` counter.

Cross-ref ROUND8-L4-02: even with TCP-state-aware pruning, a SYN flood doesn't hit RST/FIN — every connection is "open" forever from our state machine's POV. The flood cap is orthogonal and both are needed.
