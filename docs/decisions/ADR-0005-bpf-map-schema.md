# ADR-0005: BPF map schema — real aya-ebpf maps + 5-tuple conntrack + prime-sized Maglev table

- Status: Accepted (realised 2026-04-22 via Pillar 4a)
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: Maglev paper (Eisenbud et al., NSDI 2016), Katran design
  notes, Google Maglev production experience, Linux `bpf(2)` man page
  (map types), Pingora backend-hash documentation.

## Context and problem statement

The L4 data plane needs a flow-affinity table (conntrack) and a
consistent-hash decision surface (Maglev). Pillar 4a makes these real in
two places:

- **In-kernel (BPF)**: maps declared in `crates/lb-l4-xdp/ebpf/src/main.rs`
  via `aya_ebpf::macros::map`.
- **Userspace (simulation)**: `crates/lb-l4-xdp/src/lib.rs` — identical
  semantics, used for CI-safe tests.

If the two schemas drift, every consumer has to change at once. This ADR
freezes the schema and aligns the simulation and the real BPF maps.

## Decision drivers

- The userspace simulation and the in-kernel BPF maps must agree on key
  and value types.
- Determinism: Maglev's disruption guarantees require a prime-sized
  lookup table.
- Memory cap: conntrack must be bounded under adversarial load.
- Testability: every invariant reproducible in a unit test.
- Verifier safety: BPF map sizes and types are what the kernel's BPF
  verifier accepts — no tricks required.

## Decision outcome

The real BPF map declarations as of Pillar 4a:

| Map name    | BPF type           | Key                         | Value                | Max entries | Purpose                                |
|-------------|--------------------|-----------------------------|-----------------------|-------------|----------------------------------------|
| `CONNTRACK` | `BPF_MAP_TYPE_HASH`| `FlowKey` (5-tuple, IPv4)   | `BackendEntry`        | 1 000 000   | Flow → backend pinning                 |
| `L7_PORTS`  | `BPF_MAP_TYPE_HASH`| `u16` (dst port, net order) | `u8` (flags)          | 256         | Ports that XDP must PASS to userspace  |
| `ACL_DENY`  | `BPF_MAP_TYPE_HASH`| `u32` (src IPv4, net order) | `u32` (rule id)       | 100 000     | /32 deny entries (LPM in Pillar 4b)    |
| `STATS`     | `BPF_MAP_TYPE_PERCPU_ARRAY` | `u32` (slot)       | `u64` (counter)       | 32          | Per-CPU counters; advisory             |

Types as declared in `crates/lb-l4-xdp/ebpf/src/main.rs`:

```rust
#[repr(C)]
pub struct FlowKey {
    pub src_addr: u32,   // network order
    pub dst_addr: u32,   // network order
    pub src_port: u16,   // network order
    pub dst_port: u16,   // network order
    pub protocol: u8,    // IPPROTO_TCP (6) / IPPROTO_UDP (17)
    pub _pad: [u8; 3],   // verifier-friendly alignment; keeps sizeof == 16
}

#[repr(C)]
pub struct BackendEntry {
    pub backend_idx: u32, // index into userspace Maglev table
    pub flags: u32,       // reserved; Pillar 4b uses bit 0 for "rewrite"
}
```

Stats slot layout (indices into `STATS`):

| Index | Name              | Meaning                                     |
|-------|-------------------|---------------------------------------------|
| 0     | `STAT_PASS`       | Packets returned `XDP_PASS`                 |
| 1     | `STAT_DROP`       | Packets returned `XDP_DROP` (ACL denied)    |
| 2     | `STAT_CT_HIT`     | Conntrack hits (flow pinned to backend)     |
| 3     | `STAT_L7`         | L7 bypass hits (port in L7_PORTS)           |
| 4     | `STAT_PARSE_FAIL` | Header parsing bounds-check failures        |

### Decisions the schema encodes

1. **Conntrack key = 5-tuple** (`src_addr`, `dst_addr`, `src_port`,
   `dst_port`, `protocol`), mirrored in `FlowKey` in the simulation
   (`crates/lb-l4-xdp/src/lib.rs`). The key is stored in network byte
   order in BPF so flow lookups are a raw byte compare.
2. **Eviction**: FIFO in the simulation (capacity 1 000 000). In-kernel
   the map declares `BPF_MAP_TYPE_HASH`; full-capacity inserts are
   rejected by the verifier. Pillar 4b will upgrade to
   `BPF_MAP_TYPE_LRU_HASH` (same key/value shape; only the map-type byte
   in the ELF changes) for adversarial-load resilience.
3. **Maglev table size must be prime** — validated at userspace
   construction time by `is_prime()`. The BPF side does not rebuild the
   Maglev table; userspace computes it and pushes backend-index entries
   into `CONNTRACK` when new flows arrive (Pillar 4a: XDP just pins,
   userspace decides; Pillar 4b will make the kernel path lookup-only
   with `XDP_TX`).
4. **Stale-entry recovery**: on a shrink, userspace iterates `CONNTRACK`
   and drops entries whose `backend_idx` is out of range for the new
   backend set. The simulation implements this in
   `HotSwapManager::route_flow` (`crates/lb-l4-xdp/src/lib.rs:361–373`);
   the real kernel-side cleanup is a userspace `bpf_map_lookup_batch` +
   `bpf_map_delete_elem` loop.

## Rationale

- **5-tuple** is the standard L4 flow identifier. Including `protocol`
  matters — TCP and UDP flows may share the same
  `(src, dst, sport, dport)` (e.g. DNS on :53).
- **IPv4 only** for Pillar 4a. IPv6 (and VLAN) are Pillar 4b — they
  require a second `FlowKey` variant (`FlowKeyV6`) and a parser branch.
- **`_pad: [u8; 3]`** pads `FlowKey` to 16 bytes. Without padding the
  verifier on some kernels rejects `HashMap<FlowKey, _>` because key
  size is not naturally aligned. 16 bytes is verifier-friendly and
  memcmp-cheap.
- **FIFO simulation vs `LRU_HASH` kernel**: FIFO is trivially correct,
  bounded, O(1). `LRU_HASH` has eviction heuristics the simulation
  cannot reproduce; FIFO is a strictly conservative model (if FIFO holds
  under load, LRU does at least as well). Acknowledged in ADR-0004.
- **`ACL_DENY` is HashMap<u32, u32>, not LPM_TRIE**: aya-ebpf 0.1's
  `LpmTrie` ergonomics are fragile on older kernels; Pillar 4a sticks
  to exact /32 matches to avoid verifier surprises. Pillar 4b promotes
  to `BPF_MAP_TYPE_LPM_TRIE` with a `[u8; 5]` key (prefix len + 4 IP
  bytes). See ADR-0004 follow-ups.
- **Prime Maglev size**: `(offset + c * skip) mod table_size` sweeps
  the full residue class only when `table_size` is coprime with `skip`.
  Choosing `table_size` prime and `skip ∈ [1, table_size-1]` guarantees
  this; see `MaglevTable::permutation` in the simulation.

## Consequences

### Positive

- Schema frozen and replicated in both the BPF program and the
  userspace simulation; they cannot drift silently.
- Backend add/remove is safe: in-flight flows pinned by `CONNTRACK`
  keep reaching their original backend; new flows use the updated
  Maglev table.
- `STATS` slots are versioned (indices, not names) so adding a new
  counter does not break older readers.

### Negative

- `FlowKey` padding wastes 3 bytes per conntrack entry (~3 MiB across
  1 M entries) for verifier alignment. Unavoidable on current kernels.
- `ACL_DENY` is /32 exact match, not CIDR. Pillar 4b upgrade tracked.
- FIFO eviction (simulation) hostile to long-lived flows under churn.
  In-kernel `LRU_HASH` (Pillar 4b) mitigates.

### Neutral

- `STATS` sized 32 slots is far more than the five currently used;
  this is intentional headroom for Pillar 4b (add
  `STAT_CT_MISS`, `STAT_V6_UNSUPPORTED`, etc.).

## Implementation notes

- `crates/lb-l4-xdp/ebpf/src/main.rs` — real `#[map]` declarations and
  `FlowKey`/`BackendEntry` layouts.
- `crates/lb-l4-xdp/src/lib.rs:47–60` — userspace `FlowKey`.
- `crates/lb-l4-xdp/src/lib.rs:25` — `DEFAULT_CONNTRACK_MAX_ENTRIES = 1_000_000`.
- `crates/lb-l4-xdp/src/lib.rs:67–144` — `ConntrackTable` with FIFO
  eviction.
- `crates/lb-l4-xdp/src/lib.rs:152–171` — `is_prime` (6k±1 trial).
- `crates/lb-l4-xdp/src/lib.rs:178–306` — `MaglevTable` + populate loop.
- `crates/lb-l4-xdp/src/lib.rs:314–374` — `HotSwapManager` with stale-
  entry recovery.
- Tests: `tests/l4_xdp_conntrack.rs`, `tests/l4_xdp_maglev.rs`,
  `tests/l4_xdp_hotswap.rs` (manifest-locked); plus loader and module
  tests in `crates/lb-l4-xdp/src/`.

## Follow-ups / open questions (Pillar 4b)

- Add `FlowKeyV6` (IPv6 variant) + second conntrack map, or widen
  `FlowKey` to 36 bytes with a discriminant.
- Migrate `CONNTRACK` to `BPF_MAP_TYPE_LRU_HASH`.
- Promote `ACL_DENY` to `BPF_MAP_TYPE_LPM_TRIE` with `[u8; 5]` key.
- Expose conntrack capacity and Maglev table size through `lb-config`.
- Kernel-side `XDP_TX` rewrite: the `BackendEntry.flags` bit 0 becomes
  "rewrite and transmit".

## Sources

- Eisenbud, D. et al. "Maglev: A Fast and Reliable Software Network
  Load Balancer." NSDI '16.
- Linux kernel `bpf(2)` — `BPF_MAP_TYPE_HASH`, `BPF_MAP_TYPE_LRU_HASH`,
  `BPF_MAP_TYPE_LPM_TRIE`, `BPF_MAP_TYPE_PERCPU_ARRAY`.
- Katran public architecture notes.
- Internal: `crates/lb-l4-xdp/ebpf/src/main.rs`,
  `crates/lb-l4-xdp/src/lib.rs`.
