# Meta Katran

## Why we study it
Katran is the reference implementation of an XDP-based L4 load balancer, running at Meta's edge since 2017. It defined the idioms — Maglev-in-eBPF, LRU connection tracking, DSR via IPIP encapsulation, BPF-map hot-swap — that every modern L4 LB (Cilium, Google Maglev follow-ups, Cloudflare Unimog) now reuses. Our `lb-l4-xdp` crate targets the same shape.

## Architecture in brief
Katran is a library (not a daemon) in C++ with eBPF programs in restricted C. The library (`katran/lib/`) exposes `KatranLb`, which loads the XDP program (`katran/lib/bpf/balancer_kern.c`), populates BPF maps, and reconciles VIP/backend/health state into those maps. Operators link `KatranLb` into their own control-plane binary; Meta runs a custom one, and the OSS tree ships example binaries (`katran_goto_test`).

The data plane lives entirely in the kernel. An XDP program attached to the NIC receives raw Ethernet frames before the kernel socket stack sees them. Per packet it: (1) parses Ethernet/IP/TCP-or-UDP headers, (2) looks up the destination VIP in a `BPF_MAP_TYPE_HASH` (`vip_map`), (3) computes a Maglev hash of the 5-tuple, (4) checks a percpu LRU conntrack map (`lru_map`) to honour connection stickiness across backend changes, (5) encapsulates the original packet in IPIP/IP6IP6 to the chosen backend, (6) `XDP_TX`s out the same NIC. Return traffic bypasses the LB entirely (Direct Server Return).

Maps are the IPC substrate. The userspace library writes to maps; the XDP program reads. Maglev tables are computed in userspace (C++) and pushed into a `BPF_MAP_TYPE_ARRAY` indexed by VIP. Hot-swap uses map-in-map (`BPF_MAP_TYPE_ARRAY_OF_MAPS`): a new Maglev table is published into a new inner map, then the outer map's pointer is atomically updated. Old flows carried by conntrack hit the old mapping; new flows hit the new mapping. This is the core trick that lets you add/remove backends without breaking sessions.

Control plane: out-of-tree. Katran exposes a Thrift API; operators feed it backend sets, weights, healthchecks. Health monitoring is external — Katran trusts the push.

Concurrency: XDP is SoftIRQ; each RX queue runs the program lock-free on its CPU. Percpu maps eliminate contention; shared maps use RCU-style read-side-lock-free access. Userspace uses standard C++ threading with map-level atomicity.

Language: C++17 for control, restricted C for BPF (no loops without bounded unrolling, no floats, no unbounded stack).

### ExpressGateway note: `lb-l4-xdp` is userspace simulation
Our crate `crates/lb-l4-xdp/src/lib.rs` is, by its own module doc, a **userspace simulation of the data plane**: "Real eBPF programs cannot be tested in CI, so we simulate the conntrack table, Maglev consistent hashing, and hot-swap behavior." This is an honest engineering decision — the Katran-equivalent eBPF lives at `crates/lb-l4-xdp/ebpf/src/main.rs` and cannot be integration-tested in a GitHub Actions runner, so we implement the same algorithms and invariants in safe Rust and assert them via `tests/l4_xdp_conntrack.rs`, `tests/l4_xdp_maglev.rs`, `tests/l4_xdp_hotswap.rs`. Any lesson below about kernel-level XDP quirks applies to the `ebpf/` program, not the simulation.

## Key design patterns
1. **Maglev consistent hashing with prime-sized lookup table.** 65537 is the Meta default — same prime our `crates/lb-balancer/src/maglev.rs` uses. The table is computed in userspace and pushed into BPF (`katran/lib/MaglevHashing.cpp`).
2. **LRU conntrack for connection stickiness across membership changes.** A 5-tuple → backend-id entry is inserted on first SYN; subsequent packets skip Maglev and go to the recorded backend. Size-bounded; oldest entry evicts on insert.
3. **Percpu maps for hot counters.** Metrics, per-backend connection counts, and the LRU itself are percpu to avoid cross-CPU cacheline bouncing. Summed in userspace for read.
4. **Map-in-map hot-swap.** New Maglev table pushed atomically, old table freed after RCU grace — zero packet loss during backend roster churn.
5. **Direct Server Return via IPIP encap.** VIP is preserved in the inner header; backend responds directly to client, saving LB bandwidth and halving latency.
6. **Hermetic XDP program: no helpers that block.** No `bpf_map_update_elem` in hot path without percpu; no `bpf_probe_read_str`; no loops the verifier can't bound.
7. **Healthcheck tunnel.** Katran also carries outbound healthcheck packets from userspace through the LB so health probes originate from the VIP and see what clients see.
8. **Userspace Thrift API instead of an embedded protocol.** Control-plane integration is someone else's problem; library stays a library.
9. **Backend-id indirection.** Maps store `u32` backend IDs, not IP addresses, so a backend IP change is a single map update, not a table rebuild.
10. **ICMP handling in XDP.** Fragmentation-needed messages are forwarded to the correct backend by parsing the embedded original header — critical for MTU discovery through the LB.

## Edge cases / hard-won lessons
- **LRU eviction under DDoS.** A SYN flood fills the conntrack LRU faster than real flows can populate it; legitimate flows get evicted and hash to a new backend mid-session. Mitigation: SYN-cookie-like pre-filters, and LRU shards sized per-CPU not shared.
- **Map-in-map update visibility is not instantaneous across CPUs.** Some CPUs may see the old inner-map pointer for one softirq pass after the outer update. Katran accepts this and relies on conntrack to paper over the single-packet window.
- **IPIP MTU.** Encap adds 20 bytes (or 40 for v6); if the path MTU is 1500 and the backend path doesn't support 1520, packets silently drop. Katran clamps MSS on SYN.
- **Backend failure without conntrack purge** sends existing flows to a dead host until LRU eviction; a `quarantine` bit on the backend entry forces rehash.
- **Percpu map aggregation cost.** Reading 128 CPUs × N backends on every stats scrape cost 5%+ CPU; cache the last sum, incremental diff.
- **Verifier-friendly code is brittle to refactor.** A well-intentioned helper extraction can push the program past the 1M-instruction limit or the 512-byte stack limit. Keep hot paths flat.
- **RSS hash must agree with Maglev hash or you get wrong-CPU steering.** Katran configures NIC RSS to hash on the same fields.
- **Checksum recomputation on IPIP is a footgun.** Older kernels miscomputed inner-header csum deltas; Katran carries its own csum helper.
- **`XDP_TX` on an interface in bond/vlan hits kernel assumptions** — Katran requires a physical NIC or a correctly configured XDP-aware bond driver.
- **Concurrent map updates during hot-swap** must preserve backend-id stability: if you renumber IDs during rebuild, in-flight conntrack entries point at the wrong backend.

## Mapping to ExpressGateway
| Katran pattern | Our equivalent | Gap / status |
| --- | --- | --- |
| XDP Maglev | `crates/lb-l4-xdp/ebpf/src/main.rs` (eBPF) and `crates/lb-balancer/src/maglev.rs` (L7 reuse) | Present; same 65537 prime. |
| Userspace Maglev simulation for CI | `crates/lb-l4-xdp/src/lib.rs` + `tests/l4_xdp_maglev.rs` | Present — acknowledged simulation. |
| LRU conntrack | `crates/lb-l4-xdp/src/lib.rs` (`FlowKey`, `DEFAULT_CONNTRACK_MAX_ENTRIES` = 1M) + `tests/l4_xdp_conntrack.rs` | Present in simulation. |
| Hot-swap | `tests/l4_xdp_hotswap.rs` | Tested as a simulation invariant. |
| Backend-id indirection | `crates/lb-core/src/backend.rs` | Backends are value objects; integer-ID indirection for eBPF is only in the eBPF program. |
| Thrift control-plane | `crates/lb-cp-client/src/lib.rs`, `crates/lb-controlplane/src/lib.rs` | Different shape — we use a `ConfigBackend` trait with file-backed and in-memory implementations, not Thrift. |
| Direct Server Return | (none) | Not in scope for an L7-first proxy; would require kernel cooperation beyond this project. |
| Healthcheck tunnelling | `crates/lb-health/src/lib.rs` | Active healthchecks from the LB are present; "tunnelled through the data plane" is not. |

## Adoption recommendations
- We should keep the simulation's invariants (conntrack correctness across membership changes, Maglev minimal-disruption bound) as the spec that any future real-eBPF implementation must pass — the tests in `tests/l4_xdp_*.rs` then serve as a conformance suite for the kernel program.
- We could add a SYN-flood pre-filter hook to the simulation so that `lb-l4-xdp` exercises the LRU-under-DDoS pathology and the mitigation, even if only in Rust.
- We should document the backend-id-stability invariant in `crates/lb-core/src/backend.rs` so that a future L4 implementation does not silently renumber during reload.
- We could expose a "quarantine" flag on `Backend` so health-driven rehash is distinguishable from membership change, matching Katran's behavior.
- We should resist Direct Server Return; it is a protocol-level L4 optimisation that would break our L7 observability story.
- We could mirror Katran's per-CPU metrics aggregation pattern in `lb-observability` if we ever move to a multi-worker topology.

## Sources
- https://github.com/facebookincubator/katran — source tree. See `katran/lib/bpf/balancer_kern.c`, `katran/lib/MaglevHashing.cpp`, `katran/lib/KatranLb.cpp`.
- https://engineering.fb.com/2018/05/22/open-source/open-sourcing-katran-a-scalable-network-load-balancer/ — original announcement and architecture rationale.
- https://research.google/pubs/maglev-a-fast-and-reliable-software-network-load-balancer/ — the 2016 Google paper whose algorithm Katran implements.
- Katran NSDI talks and the follow-up Linux Plumbers BPF microconference sessions (search "Katran NSDI" and "LPC BPF Katran").
- https://github.com/aya-rs/aya — the Rust eBPF framework our `ebpf/` subcrate uses; relevant context for why our approach differs from Katran's C++.
- https://docs.kernel.org/bpf/map_lru_hash_update.html — kernel doc on LRU hash map semantics Katran relies on.
