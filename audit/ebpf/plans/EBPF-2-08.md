# Plan for EBPF-2-08 — Export `STATS` per-CPU array to Prometheus
Finding-ref:     EBPF-2-08 (medium, Open) — joint with REL-2-13 (medium); rel wires the Prom surface
Files touched:
  - `crates/lb-l4-xdp/src/stats_export.rs`  (NEW — the API boundary between ebpf and rel)
  - `crates/lb-l4-xdp/src/loader.rs`        (typed accessor on `XdpLoader::stats()`; `take_map("STATS")` plumbing)
  - `crates/lb-l4-xdp/src/lib.rs`           (`pub mod stats_export;`)
  - `crates/lb-l4-xdp/Cargo.toml`           (no new dep; aya 0.13 already has `PerCpuArray`)
  - `crates/lb-l4-xdp/tests/stats_export.rs` (NEW — proof test)
  - (rel-side) `crates/lb-observability/src/xdp_metrics.rs` — **NOT touched in this plan**; REL-2-13's plan calls into `stats_export.rs`

Approach:

1. **The API boundary**. `stats_export.rs` is a tiny module exposing
   a `Vec<u64>` per slot — the slot index matches the eBPF
   `STAT_*` constants. The full surface:

   ```rust
   // crates/lb-l4-xdp/src/stats_export.rs

   /// Slot indices match `crates/lb-l4-xdp/ebpf/src/main.rs:226`.
   #[repr(usize)]
   #[derive(Copy, Clone, Debug)]
   pub enum StatSlot {
       Pass                = 0,
       Drop                = 1,
       CtHitV4             = 2,
       L7Divert            = 3,
       ParseFail           = 4,
       TxV4                = 5,
       CtHitV6             = 6,
       TxV6                = 7,
       VlanStripped        = 8,
       V6ExtUnsupported    = 9,
   }
   pub const NUM_SLOTS: usize = 10;

   /// Owned snapshot — one `u64` per (slot, cpu).
   /// Returned shape: `Vec<u64>` of length `NUM_SLOTS`, each entry is
   /// the summed-across-cpus value (Prom-friendly).
   /// `per_cpu()` returns the un-summed `Vec<Vec<u64>>` for debugging.
   pub fn summed() -> Result<Vec<u64>, StatsExportError>;
   pub fn per_cpu() -> Result<Vec<Vec<u64>>, StatsExportError>;

   // Also: attach-mode reporting (shared with EBPF-2-04):
   pub enum AttachModeLabel { Drv, Skb, Hw }
   pub fn record_attach_mode(mode: AttachModeLabel);
   pub fn current_attach_mode() -> Option<AttachModeLabel>;

   // Pin reuse gauge (shared with EBPF-2-05):
   pub fn record_pin_reused(name: &'static str, reused: bool);
   pub fn pin_reused_snapshot() -> Vec<(&'static str, bool)>;
   ```

2. **Wire to the aya map**. Inside `XdpLoader::load_from_bytes`,
   after `ebpf.load(...)`, capture the STATS map handle:
   ```rust
   let stats_map = ebpf
       .take_map("stats")    // explicit pin name from EBPF-2-05
       .ok_or(XdpLoaderError::StatsMapMissing)?;
   let stats: PerCpuArray<MapData, u64> = PerCpuArray::try_from(stats_map)?;
   // Store inside a `OnceCell<Mutex<PerCpuArray<…>>>` reachable from
   // stats_export.rs via a `set_stats_handle()` setter called once at
   // startup. No globals in callable paths; the setter is called from
   // `XdpLoader::load_from_bytes` and panics-on-double-set.
   crate::stats_export::set_stats_handle(stats);
   ```
   The aya `PerCpuArray::get(&index, flags)` returns a `PerCpuValues<u64>`
   which derefs to `&[u64]` (one entry per online CPU). `summed()`
   iterates `0..NUM_SLOTS` and sums each.

3. **Sampling cadence is rel's decision**. `stats_export.rs`
   provides instantaneous reads only; the scrape cadence is set by
   `lb-observability`'s Prometheus registry (5 s by default per
   the round-2 review). The per-CPU read is one
   `bpf_map_lookup_elem` syscall per slot (aya does not batch
   here), so 10 slots × 5 s = 2 Hz syscall rate — negligible.

4. **Metric names** (REL-2-13's plan owns the exposition; recorded
   here for traceability):
   ```
   xdp_packets_total{result="pass"}              # STAT_PASS
   xdp_packets_total{result="drop"}              # STAT_DROP
   xdp_packets_total{result="tx_v4"}             # STAT_TX_V4
   xdp_packets_total{result="tx_v6"}             # STAT_TX_V6
   xdp_packets_total{result="l7"}                # STAT_L7_DIVERT
   xdp_packets_total{result="parse_fail"}        # STAT_PARSE_FAIL
   xdp_conntrack_hits_total{family="v4"}         # STAT_CT_HIT_V4
   xdp_conntrack_hits_total{family="v6"}         # STAT_CT_HIT_V6
   xdp_vlan_stripped_total                       # STAT_VLAN_STRIPPED
   xdp_v6_ext_unsupported_total                  # STAT_V6_EXT_UNSUPPORTED
   ```
   Plus the derived alerting signal (rel writes the recording rule):
   ```
   xdp_conntrack_used_entries{family}            # walk via bpf_map_get_next_key
   ```
   The walk needed for `used_entries` is also part of
   `stats_export.rs` but as a separate function
   `pub fn conntrack_used_entries(family: IpFamily) -> u64`.

5. **No double counting**. Per-CPU array counts are independently
   incremented from each CPU; summing them is correct and is what
   Cilium / Katran do. `summed()` is the only thing rel exposes
   as a Prom counter.

Proof:

- Test name: `lb-l4-xdp/tests/stats_export.rs::summed_counters_advance_under_load`
- Test scaffold:
  1. Load XDP onto `dummy0` (reuse `tests/real_elf.rs` fixture).
  2. Read baseline `stats_export::summed()` — store as `before`.
  3. Inject 1 000 packets via `bpf_prog_run` (userspace test
     helper `bpf_test_run` syscall, aya wraps as
     `Xdp::test_run(input)`) crafted as parseable TCP/IPv4
     packets to a known backend. Half are "match an existing
     CONNTRACK entry" (CT hit), half are "no entry" (parse-OK,
     CT miss → STAT_PASS).
  4. Read `stats_export::summed()` — store as `after`.
  5. Assert `after[StatSlot::CtHitV4 as usize] >= before[…] + 500`.
  6. Assert `after[StatSlot::Pass    as usize] >= before[…] + 500`.
  7. Assert the per-CPU shape via `per_cpu()`: at least one CPU
     reports a non-zero delta (proves we aren't reading slot 0
     only).
- Marked `#[ignore]` (CAP_BPF).

Risk / blast radius:

- **`set_stats_handle` panic-on-double-set** could trip if
  `load_from_bytes` is ever called twice in-process. Mitigation:
  `XdpLoader` already enforces single-load via its own state
  machine; document the invariant.
- **Read latency on hosts with 256+ CPUs**: per-CPU read is
  `O(nr_cpus)` syscalls × 10 slots. At 256 CPUs that's 2 560
  syscalls per scrape, ~10 ms. Still well under a 5 s scrape
  budget but worth a benchmark in the proof test if we ever
  ship on huge boxes.
- **Backwards compat of slot indices**: any future addition to
  the `STAT_*` enum in the eBPF crate MUST add to the *end* of
  `StatSlot` to keep wire-compat with already-pinned `stats`
  maps. Document this in `stats_export.rs` doc comment.

Cross-ref:
- REL-2-13: rel's plan reads via `stats_export::summed()` and
  publishes Prom metrics. **rel does NOT edit
  `crates/lb-l4-xdp/src/`** — the file-ownership boundary holds.
- EBPF-2-03: STATS export is the prerequisite for the
  CONNTRACK-saturation alert. Without it, the LRU fix is invisible
  to operators.
- EBPF-2-04: `stats_export.rs` also hosts `record_attach_mode()`
  / `current_attach_mode()` — created jointly in this plan
  because the file is new and shared.
- EBPF-2-05: hosts `record_pin_reused()` / `pin_reused_snapshot()`
  for the reload-zero-drop assertion.

Owner:          ebpf  (rel does the Prom-side wiring in REL-2-13)
Lead-approval: approved 2026-05-13 team-lead
