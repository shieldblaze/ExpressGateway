### ROUND8-L4-04 — Non-atomic backend-table updates (Unimog daisy-chain lesson)

Reference: `audit/round-8/research/l4drop.md` D1 (atomic forwarding-table update) + handoff item 4
Our equivalent: `crates/lb-l4-xdp/src/loader.rs:746-758` (`conntrack_map` returns `AyaHashMap` — every `.insert(&k, &v, 0)` is one map_update syscall per key) and `:761-774` (v6)

Severity: medium
Status:   Verified-Fixed (verify, task#70, 2026-05-15) — single-syscall publish + generation/daisy-chain confirmed; data-plane touch behaviorally-inert per lead-approved Pillar-4b-3 scope split. See audit/round-8/verify/l4.md. [Original Proposed-Fix below.]
          Proposed-Fix — eBPF `backends_v4` HashMap + `BackendTable`
          value (generation + count + entries[64] + Unimog-lesson-3
          daisy-chain previous_entries[64]); userspace
          `XdpLoader::publish_backends_v4` does the atomic swap as a
          SINGLE `bpf_map_update_elem` (no half-populated-table
          window) with the daisy-chain current→previous shift +
          tail-zeroing on shrink; `TooManyBackends` error gate;
          `BACKEND_TABLE_SIZE` compile-time layout assertion (3088 B,
          matches eBPF byte-for-byte). The verifier-heavy data-plane
          *read* side (per-packet hash%count selection + generation
          compare) is explicitly deferred to Pillar-4b-3
          (`audit/deferred.md`) — a behaviorally-inert
          `backend_table_published(vip)` touch keeps the map + BTF
          alive and proves atomic-publish visibility end-to-end; the
          Pillar-4b-3 selection slots in at that call site. Proof
          `crates/lb-l4-xdp/tests/round8_atomic_backends.rs` (6 tests,
          green). BPF source changed → verifier-log re-capture pending
          (cross-ref ROUND8-L4-10). Awaiting verification sign-off.

Divergence:
- Unimog: forwarding-table publication is a *swap* — userspace builds a new bucket array, then atomically replaces the prog-array entry pointing the BPF program at the new table. No reader ever sees a half-populated table.
- Us: There is no single "backend table" map at all. The closest analogue is `CONNTRACK` (which is per-flow, not per-VIP). Backend population is "set N entries into CONNTRACK with N separate `bpf_map_update_elem` syscalls". During the N-syscall window the BPF program sees some entries old, some new. There is also no "generation" field, so a conntrack lookup that hit an old entry and a newer-flow that hit a partially-populated entry can race.
- More acute version of the bug than Unimog had: their Maglev table has the daisy-chain "previous slot" lesson 3 fallback so in-flight flows during a swap reach the previous server. We have nothing analogous; on a backend churn event, in-flight flows are stranded until the LRU evicts and a new conntrack entry is allocated.

Impact:
- Backend hot-swap is currently best-effort. The `HotSwapManager` in `crates/lb-l4-xdp/src/lib.rs:354-414` is the *userspace simulator* — the real BPF data plane has no equivalent atomicity guarantees.
- An operator-initiated rebalance (`PUT /admin/backends` adds/removes backends) walks through the conntrack map one-syscall-per-key. For the duration of the walk, flows mid-LRU-eviction-and-repopulation can be steered to a removed backend (existing CT entry still has the old `BackendEntry`) until userspace catches up to that key.

Reproduction:
- `lesson-not-yet-paid-for` again. At our test scale the rebalance is N=tens-of-keys and finishes in microseconds. At production scale N=millions and the wallclock window of inconsistency is seconds.

Recommendation:
1. Add a per-VIP `BackendTable` map keyed by VIP, value = `{generation: u32, entries: [BackendEntry; MAX_PER_VIP]}`. Atomic publication = a single map_update of the whole struct.
2. The BPF program first looks up `BackendTable[vip]`, then indexes into `entries[backend_idx % N]`, then optionally consults `CONNTRACK` for flow stickiness.
3. Add daisy-chain "previous slot" per Unimog lesson 3 so in-flight flows during a swap reach the previous backend.
4. Add an integration test that does a 1k-rebalance under simulated 100k packets/sec and asserts no flow sees a partial table.

This is a Pillar-4b-3-or-later piece of work; flag it now so it doesn't get punted to "after first incident".
