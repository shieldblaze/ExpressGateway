# Plan for ROUND8-L4-03 — SYN-flood new-flow-rate cap on conntrack inserts (Katran pattern)

Finding-ref:     ROUND8-L4-03 (high, Open)
Reference:       Katran `katran/lib/bpf/balancer_kern.c` `is_under_flood()` lesson 4 (per-CPU `MAX_CONN_RATE = 125k/s/core`, skip new LRU inserts above threshold); handoff item 3.
Coverage-gap:    Theme 2 (operational-vs-laboratory: only visible at >125k flows/sec/core; Round-7 test posture is single-instance). Defense in depth orthogonal to ROUND8-L4-02 (TCP-state pruning).

Files touched:
  - `crates/lb-l4-xdp/ebpf/src/main.rs`             (NEW per-CPU rate-tracker map; `is_under_flood()` helper; gate on CT-miss `new flow` path; new stat slot)
  - `crates/lb-l4-xdp/src/loader.rs`                (userspace rate limit on `XdpLoader::conntrack_map().insert()`; expose tunable through `RuntimeConfig`)
  - `crates/lb-config/src/lib.rs`                   (new field `xdp_new_flow_cap_per_sec: u32`, default 125_000)
  - `crates/lb-l4-xdp/src/stats_export.rs`          (`StatSlot::NewFlowRateCap`)
  - `crates/lb-l4-xdp/tests/round8_synflood_cap.rs` (NEW — proof)

Approach:

1. **eBPF-side rate tracker** (`crates/lb-l4-xdp/ebpf/src/main.rs`):
   - Add a per-CPU map:
     ```rust
     #[repr(C)]
     #[derive(Clone, Copy, Default)]
     pub struct RateWindow {
         window_start_ns: u64,
         flows_this_window: u32,
         _pad: u32,
     }
     #[map(name = "new_flow_rate")]
     static NEW_FLOW_RATE: PerCpuArray<RateWindow> =
         PerCpuArray::<RateWindow>::with_max_entries(1, 0);
     ```
     Per-CPU avoids cross-CPU coherence cost. One-slot array is the
     idiomatic per-CPU singleton.
   - `is_under_flood()` helper using `bpf_ktime_get_ns()`:
     - If `now - window_start_ns > 1_000_000_000`, reset window:
       `window_start_ns = now; flows_this_window = 0;`.
     - Increment `flows_this_window`. If `flows_this_window > CAP`,
       return `true`.
   - **Gate site**: in the CT-miss path of `handle_ipv4`/`handle_ipv6`
     — currently the BPF-side never inserts into CT (userspace does),
     but the BPF program returns `XDP_PASS` on miss. Today's CT miss
     becomes the natural rate-limit point for the *outbound* "tell
     userspace to populate" signal. Concretely: when CT misses and
     `is_under_flood()` returns true, increment
     `STAT_NEW_FLOW_RATE_CAP` and skip the path that would otherwise
     trigger a userspace conntrack insert (today that path is no-op,
     but the userspace control loop polls statsmap and reacts; the
     stat slot becomes the back-pressure feedback signal).
   - `CAP` is *not* read from a map in v1 (verifier-impact and keeps
     hot path single-source). Compile-time constant 125_000 / sec / CPU
     mirrors Katran. If we want a runtime knob (recommended), expose
     a one-entry per-CPU map `NEW_FLOW_CAP_CFG` whose value the BPF
     program reads once per packet — verifier-cheap.

2. **Userspace rate limiter on CT inserts**
   (`crates/lb-l4-xdp/src/loader.rs`):
   - The control plane (`lb-balancer`) drives
     `XdpLoader::conntrack_map().insert()`. Add a userspace
     rate-limiter wrapper:
     ```rust
     pub struct CtInsertGate {
         tokens: AtomicU32,
         refill_per_sec: u32,
         last_refill_ns: AtomicU64,
     }
     impl CtInsertGate {
         pub fn try_admit(&self) -> bool { /* leaky-bucket */ }
     }
     ```
     Exposed via `XdpLoader::insert_conntrack_v4(&mut self, key, entry)` (composes with L4-01's same wrapper).
   - Default `refill_per_sec = 125_000 * num_cpus::get()`. Loud WARN
     when admission denied; `STAT_NEW_FLOW_RATE_CAP` incremented on
     userspace path too.

3. **Config knob** (`crates/lb-config/src/lib.rs`):
   - Add `xdp_new_flow_cap_per_sec_per_cpu: u32` default 125_000 to
     `RuntimeConfig`. Operator override path for ENA / mlx5 / smaller
     test boxes.
   - Reload coordination: changing the cap at runtime is permitted;
     the loader writes the per-CPU `NEW_FLOW_CAP_CFG` map (item 1).

4. **stats_export wiring**: add `StatSlot::NewFlowRateCap` ->
   `"new_flow_rate_cap"`. Stat slot index `13` (10/11/12 reserved by
   L4-01/-02).

5. **Proof tests** (`crates/lb-l4-xdp/tests/round8_synflood_cap.rs`,
   NEW):
   - `sim_rate_cap_blocks_burst`: drive the simulator with 200k
     unique 5-tuples in a 1s window with `CAP = 100k`; assert
     `STAT_NEW_FLOW_RATE_CAP` >= 100k; CT-map size <= 100k.
   - `sim_rate_cap_resets_at_window_boundary`: 50k flows in t=0..1s,
     then 50k flows in t=1..2s; assert all 100k admitted, zero
     blocks.
   - `userspace_gate_leaky_bucket`: unit-test the `CtInsertGate`
     directly; permits at refill rate, denies above.

Proof:

- `cargo test -p lb-l4-xdp --test round8_synflood_cap` (userspace
  sim).
- Re-capture verifier-log baselines per L4-10 since the BPF program
  now reads `bpf_ktime_get_ns()` and writes to a per-CPU map — both
  the verifier and bpftool see new operations.
- Scale gate (Pillar 4b acceptance per Theme 2): hping3 spray 200k
  unique flows/sec for 60s; assert live established TCP flow miss
  rate stays below baseline.

Risk / blast radius:

- Per-packet `bpf_ktime_get_ns()` adds a single helper call (~5ns on
  most kernels); benchmark target < 1% throughput loss.
- The per-CPU `NEW_FLOW_RATE` map is 1 entry × num_cpus × 16 bytes —
  ~512 bytes worst-case. Negligible.
- The CAP knob is a runtime footgun: setting it too low causes
  legitimate traffic to skip CT insertion → repeated lookup misses →
  packets pass to kernel stack instead of XDP_TX. Mitigation: clamp
  config to `[1_000, 10_000_000]` per CPU; loud WARN on apply.
- Userspace `CtInsertGate` is per-process; multi-replica deployment
  needs the cap to be set per-replica (cluster-aware sizing).
  Document in RUNBOOK.

Cross-ref:
- ROUND8-L4-02: pruning closes the *eviction-as-DoS* angle; this
  finding closes the *write-amplification* angle. Both needed.
- ROUND8-L4-04: an atomic backend-table publication does not help
  the SYN-flood case (the attacker generates unique CT keys, not
  backend churn). Independent.
- ROUND8-L4-10: verifier-log re-capture required.

Owner:           div-l4
Lead-approval: approved 2026-05-14 team-lead-r8
