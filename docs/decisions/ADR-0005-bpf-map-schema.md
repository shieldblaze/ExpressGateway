# ADR-0005: BPF map schema — 5-tuple conntrack, prime-sized Maglev table, FIFO eviction

- Status: Accepted
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: Maglev paper (Eisenbud et al., NSDI 2016), Katran design
  notes, Google Maglev production experience, Linux `bpf(2)` man page
  (map types), Pingora backend-hash documentation.

## Context and problem statement

The L4 data plane needs two data structures: a **flow-affinity table**
(conntrack) that remembers which backend a given 5-tuple already points
to, and a **consistent-hash table** (Maglev) that decides where a new
flow should go so that adding or removing a backend disturbs as few flows
as possible.

The schemas for these two structures are load-bearing: they appear in
three places — the userspace simulation (`crates/lb-l4-xdp/src/lib.rs`),
the eventual in-kernel BPF maps (deferred, see ADR-0004), and the
control-plane API that pushes backend sets down. If we get the schema
wrong, every consumer has to change at once. We therefore freeze the
schema here.

Specific questions to nail down:

1. What constitutes the conntrack key?
2. What is the eviction policy when the conntrack table is full?
3. What size does the Maglev lookup table use, and why a prime?
4. What is the consequence of a backend set being *smaller* than what an
   existing conntrack entry remembers?

## Decision drivers

- Consistency between the simulation and the future in-kernel BPF maps —
  the same invariants must hold.
- Determinism: Maglev's disruption guarantees depend on the table being
  prime-sized.
- Memory cap: conntrack must not grow unboundedly under DDoS.
- Hot-swap safety: switching backend sets must not black-hole in-flight
  flows.
- Testability: every invariant must be reproducible in a unit test.
- Space: BPF maps have per-map memory limits; we plan for ~1 M entries.
- Simplicity: the schema must fit in code a human can audit end-to-end.

## Considered options

- Conntrack key: 5-tuple vs 4-tuple (no protocol) vs 6-tuple (with VLAN).
- Eviction: FIFO vs LRU vs random vs TTL-based.
- Maglev table size: fixed 65 537 vs configurable prime vs power of two.
- Stale-entry handling on backend shrink: drop flow vs re-route via
  Maglev vs blacklist.

## Decision outcome

1. **Conntrack key = 5-tuple** (`src_addr`, `dst_addr`, `src_port`,
   `dst_port`, `protocol`), modelled as `FlowKey` in
   `crates/lb-l4-xdp/src/lib.rs` (struct definition lines 47–60).
2. **Eviction = FIFO** in the simulation, with a default capacity of
   1 000 000 entries (`DEFAULT_CONNTRACK_MAX_ENTRIES`,
   `crates/lb-l4-xdp/src/lib.rs:25`). In-kernel map will be
   `BPF_MAP_TYPE_LRU_HASH` — equivalent semantics under adversarial
   load.
3. **Maglev table size must be prime** — validated at construction time
   by `is_prime()` (`crates/lb-l4-xdp/src/lib.rs:152–171`);
   `MaglevTable::new` rejects non-prime sizes with
   `XdpError::InvalidTableSize`. Default test size: 65 537 (the smallest
   prime > 2^16).
4. **Backend-shrink semantics**: on lookup, a conntrack entry whose
   recorded backend index is ≥ the current Maglev backend count is
   treated as stale, removed, and the flow is re-routed via the current
   Maglev table. Implemented in `HotSwapManager::route_flow`
   (`crates/lb-l4-xdp/src/lib.rs:361–373`).

## Rationale

- **5-tuple** is the standard identification for a flow at L4. Including
  `protocol` is essential — a TCP and a UDP flow may share the same
  `(src, dst, sport, dport)` quadruple on the wire (e.g. DNS-over-QUIC
  vs DNS-over-TLS on :53).
- **FIFO vs LRU**: FIFO is trivially correct, bounded-memory, and O(1)
  with a `VecDeque` of insert order — see `ConntrackTable::insert`
  (lines 103–122). Under sustained load LRU is strictly better (keeps
  hot flows), but the BPF `LRU_HASH` has eviction heuristics we cannot
  reproduce in userspace with the same fidelity. FIFO gives a *lower
  bound* on the behaviour: if the simulation survives FIFO under
  load, the kernel's LRU will do at least as well. The simulation's
  update-existing-flow path (lines 105–109) deliberately does not touch
  the eviction queue, matching LRU's "update doesn't change eviction
  position" property poorly — this is a documented asymmetry and a
  follow-up for the in-kernel migration.
- **Prime table size**: Maglev's permutation construction uses
  `(offset + c * skip) mod table_size`. When `table_size` is coprime
  with `skip`, the full residue class is swept before a revisit; choosing
  prime guarantees coprimality for any `skip ∈ [1, table_size-1]`. This
  is why `MaglevTable::permutation`
  (`crates/lb-l4-xdp/src/lib.rs:230–241`) produces `skip = (h2 %
  (table_size - 1)) + 1` — strictly in `[1, table_size-1]`, coprime
  with a prime `table_size`.
- **Hash function**: two independent multiply-shift hashes (`hash_str`
  with seed 0 and with golden-ratio-derived seed 0x9e37_79b9_7f4a_7c15)
  produce `(offset, skip)`. This is algorithmically identical to the
  Maglev paper and intentionally matches `lb-balancer`'s software
  Maglev so that L4 and L7 paths route new flows consistently.
- **Stale-entry recovery**: without it, shrinking the backend set would
  either panic (indexing out of range — forbidden by lint) or black-hole
  traffic. The test `hotswap_evicts_stale_conntrack_after_shrink`
  (lines 557+) pins this behaviour.

## Consequences

### Positive
- Schema is stable across simulation and in-kernel data plane.
- Backend add/remove is safe: in-flight flows pinned by conntrack
  continue to their original backend; new flows use the new Maglev
  table.
- Memory is capped at ~1 M conntrack entries by default — roughly
  24 B × 1 M = 24 MiB for keys plus allocator overhead, well within a
  reasonable per-CPU BPF map budget.

### Negative
- FIFO eviction in the simulation is unfriendly to long-lived flows
  under churn — an attacker can shove out a legitimate flow with a
  1 M-packet-per-second burst. In-kernel LRU mitigates this partially.
- Maglev table size is a tunable but all tunings must be prime —
  operators cannot pick "nice" numbers like 100 000.

### Neutral
- The exact default (65 537) is conservative; production will likely
  want a larger prime (65 521 × n). Documented in `lb-config`.

## Implementation notes

- `crates/lb-l4-xdp/src/lib.rs:47–60` — `FlowKey`.
- `crates/lb-l4-xdp/src/lib.rs:25` — `DEFAULT_CONNTRACK_MAX_ENTRIES = 1_000_000`.
- `crates/lb-l4-xdp/src/lib.rs:67–144` — `ConntrackTable` with FIFO
  eviction (`insert_order: VecDeque<FlowKey>`).
- `crates/lb-l4-xdp/src/lib.rs:152–171` — `is_prime` (6k±1 trial).
- `crates/lb-l4-xdp/src/lib.rs:178–306` — `MaglevTable` + populate loop.
- `crates/lb-l4-xdp/src/lib.rs:314–374` — `HotSwapManager` with stale-
  entry recovery.
- Tests: `tests/l4_xdp_conntrack.rs`, `tests/l4_xdp_maglev.rs` (see
  `tests/` directory), plus module tests in `lib.rs`.

## Follow-ups / open questions

- Migrate simulation to LRU semantics to more closely match in-kernel
  `LRU_HASH`.
- Decide whether the Maglev table should be per-service or shared; our
  current shape is per-`HotSwapManager`, which maps to per-service.
- Expose conntrack capacity through `lb-config`.

## Sources

- Eisenbud, D. et al. "Maglev: A Fast and Reliable Software Network
  Load Balancer." NSDI '16.
- Linux kernel `bpf(2)` — `BPF_MAP_TYPE_LRU_HASH`.
- Katran public architecture notes.
- Internal: `crates/lb-l4-xdp/src/lib.rs`.
