# Plan for EBPF-2-03 — CONNTRACK + CONNTRACK_V6 → `BPF_MAP_TYPE_LRU_HASH`
Finding-ref:     EBPF-2-03 (high, Open) — joint with SEC-2-02 (high), REL-2-12 (high)
Files touched:
  - `crates/lb-l4-xdp/ebpf/src/main.rs`     (`#[map]` annotation: `HashMap` → `LruHashMap`)
  - `crates/lb-l4-xdp/src/loader.rs`        (userspace inserter: `aya::maps::HashMap` → `aya::maps::LruHashMap`)
  - `crates/lb-l4-xdp/Cargo.toml`           (no version change; aya 0.13.1 already exposes `LruHashMap`)
  - `crates/lb-l4-xdp/tests/l4_xdp_conntrack.rs`  (extend existing fixtures: LRU-eviction-order proof)
  - `crates/lb-l4-xdp/src/lb_xdp.bin`       (regenerated; size budget unchanged — map type change does not alter program text)

Approach:

1. **Aya support verification (preflight)**. aya 0.13.1 exposes both
   the kernel map type and the typed accessor:
   - eBPF crate: `aya_ebpf::maps::LruHashMap<K, V>` (registry at
     `~/.cargo/registry/src/.../aya-ebpf-0.1.1/src/maps/hash_map.rs`).
   - Userspace: `aya::maps::LruHashMap<&mut MapData, K, V>` in
     `aya-0.13.1/src/maps/hash_map/lru_hash_map.rs`.
   Confirmed present. No version bump required. If aya ever changes
   the typed accessor name, the preflight `cargo check` step in CI
   fires first.

2. **eBPF-side map declaration**. In
   `crates/lb-l4-xdp/ebpf/src/main.rs:209-215`, replace:
   ```rust
   #[map]
   static CONNTRACK:    HashMap<FlowKey,   BackendEntry>   = HashMap::with_max_entries(1_000_000, 0);
   #[map]
   static CONNTRACK_V6: HashMap<FlowKeyV6, BackendEntryV6> = HashMap::with_max_entries(  512_000, 0);
   ```
   with:
   ```rust
   #[map]
   static CONNTRACK:    LruHashMap<FlowKey,   BackendEntry>   = LruHashMap::with_max_entries(1_000_000, 0);
   #[map]
   static CONNTRACK_V6: LruHashMap<FlowKeyV6, BackendEntryV6> = LruHashMap::with_max_entries(  512_000, 0);
   ```
   The lookup call sites (`main.rs:339-348,370-419,604-631`) are
   API-compatible: `LruHashMap::get(&key)` has the same signature as
   `HashMap::get(&key)` in aya-ebpf. No call-site edits needed.

3. **Plain `LRU_HASH` vs. `LRU_PERCPU_HASH`** — choose plain
   `LRU_HASH`. Rationale: per-CPU LRU doubles the kernel memlock
   charge (1M × 28 B × nr_cpus) and the eviction is per-CPU which
   defeats the flow-affinity model — a flow that migrated CPUs would
   evict from one shard while the other still holds the stale
   entry. The plain `LRU_HASH` uses the kernel's global-with-NUMA-
   locality variant which is what Katran ships.

4. **Userspace inserter (`HotSwapManager`)**. In
   `crates/lb-l4-xdp/src/loader.rs` and `src/lib.rs:349-409` the
   userspace inserter today uses `aya::maps::HashMap::try_from(map)`.
   Change to `aya::maps::LruHashMap::try_from(map)`. The `insert`
   signature is identical (`(&K, &V, u64) -> Result<…>`). Error
   handling: the `ENOMEM` path goes away (LRU evicts instead of
   refusing). Keep the error-path branch but downgrade the log from
   ERROR to WARN since LRU eviction under sustained pressure is
   expected behaviour, not a bug.

5. **Sizing**. Keep 1 M (v4) + 512 k (v6). The kernel memlock
   charge for `LRU_HASH` is roughly `(key+value+24) × max_entries`
   = `(16+28+24) × 1_000_000 ≈ 68 MiB` for v4 and `40 MiB` for v6.
   On 5.15+ with `CAP_BPF`, the memlock rlimit is no longer charged
   so this is purely accounting.

Proof:

- Test name: `lb-l4-xdp/tests/l4_xdp_conntrack.rs::lru_evicts_oldest_under_flood`
- Test scaffold: extends existing `tests/l4_xdp_conntrack.rs`
  fixtures with a flood scenario:
  1. Set `MAX_ENTRIES = 64` via a `#[cfg(test)]` override (or fork
     the eBPF binary with a smaller cap built by `build-xdp.sh
     --max-entries=64`).
  2. Insert 64 distinct `FlowKey`s with monotonically increasing
     `dst_port` `0..64`. Record the timestamp on each.
  3. Touch (`get`) keys `0..32` to mark them recently-used.
  4. Insert 32 NEW keys (`dst_port = 64..96`).
  5. Read back every key. Invariant assertion:
     - All of `0..32` (recently-touched) MUST still be present.
     - All of `32..64` (oldest, untouched) MUST be gone (`ENOENT`).
     - All of `64..96` (newest) MUST be present.
  6. As a counter-check, the same test run against a
     `BPF_MAP_TYPE_HASH` map MUST fail at step 4 with `ENOMEM`
     (negative-control, gated behind `#[cfg(feature =
     "neg-control-plain-hash")]`).
- Test runs as `cargo test -p lb-l4-xdp --test l4_xdp_conntrack
  -- lru_evicts_oldest_under_flood --ignored` (requires CAP_BPF;
  marked `#[ignore]` by default per project convention from
  `tests/real_elf.rs`).

Risk / blast radius:

- **Verifier acceptance on 5.15** — `LRU_HASH` has been kernel-
  supported since 4.10, so 5.15 is safe. Confirmed by the BPF
  map-types feature matrix.
- **Eviction policy interaction with `STAT_PASS`** — once flows
  start evicting, the BPF program sees a `None` lookup for an
  evicted flow and falls through to `STAT_PASS` / `XDP_PASS`,
  which userspace must re-insert. This is the same code path as
  today's cold-start, so no new logic required. The eviction rate
  becomes the alerting signal (cross-ref REL-2-12).
- **Aya `LruHashMap` userspace iter behaviour**: aya's
  `MapIter` on LRU types is documented to be a best-effort
  snapshot. The `HotSwapManager` walk that exports CONNTRACK
  contents for debugging must accept that some entries may be
  evicted mid-walk; downgrade those misses to WARN.

Cross-ref:
- SEC-2-02 (joint authoring; sec owns the adversarial harness
  writeup).
- REL-2-12 (CONNTRACK saturation metric + alert — depends on
  EBPF-2-08 STATS export landing).
- ADR-0005 §Pillar 4b-3 (promise delivered).

Owner:          ebpf
Lead-approval: approved 2026-05-13 team-lead
