# ADR-0005: BPF map schema — real aya-ebpf maps + 5-tuple conntrack + prime-sized Maglev table

- Status: Accepted. Realised 2026-04-22 via Pillar 4a; extended
  2026-04-23 via Pillar 4b-2 to add `CONNTRACK_V6` and promote
  `ACL_DENY` to `BPF_MAP_TYPE_LPM_TRIE`, plus `BackendEntry` payload
  growth for `XDP_TX` rewrite.
- Date: 2026-04-22 (Pillar 4a), 2026-04-23 (Pillar 4b-2 revision)
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

The real BPF map declarations as of Pillar 4b-2:

| Map name         | BPF type                    | Key                           | Value            | Max entries | Purpose                                            |
|------------------|-----------------------------|-------------------------------|------------------|-------------|----------------------------------------------------|
| `CONNTRACK`      | `BPF_MAP_TYPE_HASH`         | `FlowKey` (5-tuple, IPv4)     | `BackendEntry`   | 1 000 000   | IPv4 flow → backend pinning + rewrite state       |
| `CONNTRACK_V6`   | `BPF_MAP_TYPE_HASH`         | `FlowKeyV6` (5-tuple, IPv6)   | `BackendEntryV6` | 512 000     | IPv6 flow → backend pinning + rewrite state       |
| `L7_PORTS`       | `BPF_MAP_TYPE_HASH`         | `u16` (dst port, net order)   | `u8` (flags)     | 256         | Ports that XDP must PASS to userspace             |
| `ACL_DENY_TRIE`  | `BPF_MAP_TYPE_LPM_TRIE`     | `LpmKey<u32>` (IPv4 CIDR)     | `u32` (rule id)  | 100 000     | IPv4 deny ACL with longest-prefix match           |
| `STATS`          | `BPF_MAP_TYPE_PERCPU_ARRAY` | `u32` (slot)                  | `u64` (counter)  | 32          | Per-CPU counters; advisory                        |

Types as declared in `crates/lb-l4-xdp/ebpf/src/main.rs`:

```rust
#[repr(C)]
pub struct FlowKey {        // 16 bytes
    pub src_addr: u32,
    pub dst_addr: u32,
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: u8,
    pub _pad: [u8; 3],
}

#[repr(C)]
pub struct FlowKeyV6 {      // 40 bytes
    pub src_addr: [u8; 16],
    pub dst_addr: [u8; 16],
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: u8,
    pub _pad: [u8; 3],
}

/// Pillar 4b-2: BackendEntry grew from 8 → 28 bytes to carry enough
/// rewrite state for an `XDP_TX` without a second map lookup.
#[repr(C)]
pub struct BackendEntry {
    pub backend_idx: u32,
    pub flags: u32,
    pub backend_ip: u32,      // destination IPv4 for the rewrite
    pub backend_port: u16,    // destination L4 port for the rewrite
    pub _pad: u16,
    pub backend_mac: [u8; 6], // destination MAC
    pub src_mac: [u8; 6],     // our NIC's MAC
}

#[repr(C)]
pub struct BackendEntryV6 {   // 40 bytes
    pub backend_idx: u32,
    pub flags: u32,
    pub backend_ip: [u8; 16],
    pub backend_port: u16,
    pub _pad: u16,
    pub backend_mac: [u8; 6],
    pub src_mac: [u8; 6],
}
```

Stats slot layout (indices into `STATS`):

| Index | Name                    | Meaning                                                        |
|-------|-------------------------|----------------------------------------------------------------|
| 0     | `STAT_PASS`             | Packets returned `XDP_PASS`                                    |
| 1     | `STAT_DROP`             | Packets returned `XDP_DROP` (ACL denied)                       |
| 2     | `STAT_CT_HIT_V4`        | IPv4 conntrack hits                                            |
| 3     | `STAT_L7`               | L7 bypass hits (port in `L7_PORTS`)                            |
| 4     | `STAT_PARSE_FAIL`       | Header parsing bounds-check failures                           |
| 5     | `STAT_TX_V4`            | IPv4 `XDP_TX` (rewritten and retransmitted)                    |
| 6     | `STAT_CT_HIT_V6`        | IPv6 conntrack hits                                            |
| 7     | `STAT_TX_V6`            | IPv6 `XDP_TX`                                                  |
| 8     | `STAT_VLAN`             | Frames where a single 802.1Q tag was stripped                  |
| 9     | `STAT_V6_EXT_UNSUPPORTED` | IPv6 packets with >2 or unsupported extension headers → PASS |

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
- **IPv6**: Pillar 4b-2 adds `FlowKeyV6` (40 bytes) and a separate
  `CONNTRACK_V6` map rather than widening `FlowKey` with a
  discriminant. Two maps are cheaper in the fast path (no
  tagged-union match) and the per-flow memory still fits within the
  kernel's per-map budget (512 k × 80 B ≈ 40 MiB for v6 conntrack).
- **VLAN**: single 802.1Q tag stripped by the parser before the L3
  branch; QinQ (stacked tags) is Pillar 4b-3. The VLAN tag is not
  stored in the flow key — only the ports/addresses after it.
- **`_pad: [u8; 3]`** pads `FlowKey` to 16 bytes. Without padding the
  verifier on some kernels rejects `HashMap<FlowKey, _>` because key
  size is not naturally aligned. 16 bytes is verifier-friendly and
  memcmp-cheap.
- **FIFO simulation vs `LRU_HASH` kernel**: FIFO is trivially correct,
  bounded, O(1). `LRU_HASH` has eviction heuristics the simulation
  cannot reproduce; FIFO is a strictly conservative model (if FIFO holds
  under load, LRU does at least as well). Acknowledged in ADR-0004.
- **`ACL_DENY_TRIE` (Pillar 4b-2)**: promoted from Pillar 4a's plain
  `HashMap<u32, u32>` to `BPF_MAP_TYPE_LPM_TRIE`. The key is aya's
  `LpmKey<u32>` (4-byte CIDR prefix length followed by a 4-byte IPv4
  address in network byte order). The verifier accepts this layout on
  every kernel ≥ 4.20. /32 entries still work — they're just the
  degenerate `prefix_len = 32` case. The LPM trie adds CIDR-prefix
  matching (e.g. `10.0.0.0/8` denies the whole RFC 1918 block with
  one entry).
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
- `CONNTRACK_V6` at 512 k × 80 B uses ~40 MiB — half of the v4 map's
  footprint but paid unconditionally even on v4-heavy deployments.
  Operators can shrink it via `max_entries` patching if it's wasted.
- FIFO eviction (simulation) hostile to long-lived flows under churn.
  In-kernel `LRU_HASH` (Pillar 4b-3) mitigates.

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

## Follow-ups / open questions (Pillar 4b-3)

- Migrate `CONNTRACK` + `CONNTRACK_V6` to `BPF_MAP_TYPE_LRU_HASH`.
- QinQ (stacked 802.1Q) parsing; current Pillar 4b-2 strips only one
  tag.
- `xtask xdp-verify` matrix exercising the verifier across 5.15 LTS,
  6.1 LTS, and 6.6 LTS kernels.
- Expose conntrack capacity, v6 conntrack capacity, and Maglev table
  size through `lb-config`.
- Hot-reload of `ACL_DENY_TRIE` entries from SIGHUP reload.
- TCP option rewrite (timestamps, MSS, SACK) for `XDP_TX`.

## Sources

- Eisenbud, D. et al. "Maglev: A Fast and Reliable Software Network
  Load Balancer." NSDI '16.
- Linux kernel `bpf(2)` — `BPF_MAP_TYPE_HASH`, `BPF_MAP_TYPE_LRU_HASH`,
  `BPF_MAP_TYPE_LPM_TRIE`, `BPF_MAP_TYPE_PERCPU_ARRAY`.
- Katran public architecture notes.
- Internal: `crates/lb-l4-xdp/ebpf/src/main.rs`,
  `crates/lb-l4-xdp/src/lib.rs`.
