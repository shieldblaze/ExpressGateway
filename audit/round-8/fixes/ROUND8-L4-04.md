# Plan for ROUND8-L4-04 — Atomic per-VIP backend-table swap + generation field

Finding-ref:     ROUND8-L4-04 (medium, Open)
Reference:       Unimog (l4drop) D1 — atomic forwarding-table publication via prog-array swap; Unimog lesson 3 — daisy-chain "previous slot" so in-flight flows reach previous backend; handoff item 4.
Coverage-gap:    Theme 2 (operational-vs-laboratory: only visible at >>10s of backends + high rebalance frequency). No prior round audited backend publication atomicity.

Files touched:
  - `crates/lb-l4-xdp/ebpf/src/main.rs`             (NEW `backends_v4` map keyed by VIP -> `BackendTable { generation: u32, count: u32, entries: [BackendEntry; N] }`; lookup path consults backend-table before falling back to CT)
  - `crates/lb-l4-xdp/src/loader.rs`                (`XdpLoader::publish_backends_v4(vip, &[BackendEntry])` — single map_update with whole table; daisy-chain `previous_entries` field)
  - `crates/lb-l4-xdp/tests/round8_atomic_backends.rs` (NEW — proof)
  - `audit/deferred.md`                             (mark Maglev-style consistent hashing for Pillar 4b-3+)

Approach:

1. **`BackendTable` value struct** (`crates/lb-l4-xdp/ebpf/src/main.rs`):
   ```rust
   const MAX_BACKENDS_PER_VIP: usize = 64; // Pillar 4b-2 floor; raise later
   #[repr(C)]
   #[derive(Clone, Copy)]
   pub struct BackendTable {
       pub generation: u32,
       pub count: u32,
       pub entries: [BackendEntry; MAX_BACKENDS_PER_VIP],
       /// Unimog daisy-chain lesson 3: the previous generation's
       /// entries, kept for in-flight flows during a swap. Zeroed
       /// when not in transitional state.
       pub previous_count: u32,
       pub _pad: u32,
       pub previous_entries: [BackendEntry; MAX_BACKENDS_PER_VIP],
   }
   #[map(name = "backends_v4")]
   static BACKENDS_V4: HashMap<u32, BackendTable> =
       HashMap::<u32, BackendTable>::with_max_entries(1024, 0); // VIP -> table
   ```
   - The whole struct is one map value. A single `bpf_map_update_elem`
     publishes the new table atomically — readers either see the old
     value or the new value, never a half-merged one. (Kernel
     `BPF_MAP_TYPE_HASH` value updates are atomic w.r.t. concurrent
     lookups.)
   - `MAX_BACKENDS_PER_VIP = 64` is the verifier-tractable ceiling; if
     a VIP needs more, partition or wait for Pillar 4b-3 (Maglev).

2. **Lookup integration** (`handle_ipv4` / `handle_ipv6`):
   - The new flow path: on CT miss, look up `BACKENDS_V4[dst_addr]`
     (the VIP). If present, hash the 5-tuple modulo `count`, select
     `entries[idx]`. If the generation differs from the CT entry's
     remembered generation, fall back to `previous_entries[idx %
     previous_count]` for the daisy-chain window. Increment a CT
     entry with the picked backend + generation, then `XDP_TX`.
   - Existing CT-hit path is unchanged — CT-hit beats backend-table
     lookup (flow stickiness).

3. **Userspace publication** (`crates/lb-l4-xdp/src/loader.rs`):
   ```rust
   pub fn publish_backends_v4(
       &mut self,
       vip: Ipv4Addr,
       new_entries: &[BackendEntry],
   ) -> Result<(), XdpLoaderError> {
       if new_entries.len() > MAX_BACKENDS_PER_VIP {
           return Err(XdpLoaderError::TooManyBackends(new_entries.len()));
       }
       let mut table = self.backends_v4_map()?
           .get(&u32::from(vip).to_be(), 0).unwrap_or_default();
       // shift current → previous (daisy-chain)
       table.previous_entries = table.entries;
       table.previous_count = table.count;
       table.entries.fill(BackendEntry::ZERO);
       for (i, e) in new_entries.iter().enumerate() {
           table.entries[i] = *e;
       }
       table.count = new_entries.len() as u32;
       table.generation = table.generation.wrapping_add(1);
       self.backends_v4_map()?.insert(
           &u32::from(vip).to_be(), &table, 0)?;  // ATOMIC
       Ok(())
   }
   ```
   One syscall = one publication. No mid-swap window.

4. **Daisy-chain wind-down**: after a configurable hold (e.g.
   `drain_grace = 30s` from config), publish the table again with
   `previous_entries` zeroed. A simple userspace timer task. Out of
   scope of the loader itself — `lb-balancer` schedules it.

5. **HotSwapManager bridge** (`crates/lb-l4-xdp/src/lib.rs:354-414`):
   - The userspace simulator currently models hot-swap atomicity via
     `Arc<ArcSwap<_>>`. Now that the BPF data plane has the same
     guarantee, the simulator's role narrows to "ensure the
     userspace metadata layer atomically tracks what the BPF data
     plane published". Cross-reference the new
     `publish_backends_v4` from `HotSwapManager::swap`.

6. **Proof tests** (`crates/lb-l4-xdp/tests/round8_atomic_backends.rs`, NEW):
   - `sim_publish_atomic`: simulate two backends A,B; call
     `publish_backends_v4(vip, &[A, B])`; spawn 64 reader tasks
     issuing lookups while a writer thread calls `publish_backends_v4(vip, &[A, C])`
     1000×; assert no reader ever observes a `BackendEntry` whose
     `backend_ip == 0` (would mean torn write).
   - `sim_daisy_chain_previous`: publish [A]; record `gen=1`; mark
     `flow1` with backend A, generation 1 in CT. Publish [B] (`gen=2`,
     A moves to `previous_entries`). `flow1` next packet without CT:
     lookup yields B (current). But: a packet for `flow2` whose
     hashed-idx matches what would have been A under gen 1 is the
     transitional case — for the daisy-chain to be useful we route
     based on **flow-CT-recorded generation**; if `flow_gen != current_gen`,
     consult `previous_entries`. Assert behaviour.
   - `userspace_too_many_backends`: `publish_backends_v4` with 65
     entries returns `TooManyBackends(65)`.
   - `userspace_generation_increments`: two consecutive publishes;
     read back the table; assert `generation` increments mod 2^32.

Proof:

- `cargo test -p lb-l4-xdp --test round8_atomic_backends`.
- Re-capture verifier logs (L4-10): the new map lookup + generation
  comparison are added to the hot path.
- Scale gate (Pillar 4b acceptance per Theme 2): 1000-rebalance
  test under simulated 100k packets/sec; assert no flow sees a
  partial table.

Risk / blast radius:

- `BackendTable` size: `8 + 64 × sizeof(BackendEntry) × 2 + 8` ≈ 4 KB
  per VIP value. 1024 VIPs × 4 KB = 4 MB BPF-side memory. Acceptable.
- Verifier complexity grows: the lookup path now has an extra
  `HashMap.get` + bounded loop iteration (`for i in 0..count`,
  `count <= 64`). Capture in L4-10 logs.
- This is a "Pillar-4b-3-or-later" piece of work per finding text.
  Lead may choose to defer; the plan is here so the deferral is
  explicit, not silent.

Cross-ref:
- ROUND8-L4-01: sentinel guard remains — defence in depth. A
  half-populated `BackendTable.entries[i].backend_ip == 0` is still
  caught by L4-01's check on the resulting CT-hit.
- ROUND8-L4-07: `BackendEntry::flags` — decide its fate before
  freezing `BackendTable` layout.
- ROUND8-L4-10: verifier-log re-capture required.
- This finding aligns with the deferred Maglev work
  (`audit/deferred.md`) for consistent hashing; flag it in the plan.

Owner:           div-l4
Lead-approval: approved 2026-05-14 team-lead-r8
