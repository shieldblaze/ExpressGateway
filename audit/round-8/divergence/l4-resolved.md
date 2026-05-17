# L4 / XDP — divergences examined and resolved as "fine"

For each item below, a reference project does something we don't — and on
inspection we made a different choice deliberately, or the divergence
genuinely doesn't apply to us. Captured here for future-self reference so
the same questions don't get re-asked next round.

## R-L4-01 — No Maglev consistent-hashing in the BPF data plane

Reference: Katran lesson 6 (`RING_SIZE = 65537`); Cilium lesson 2 (table size must be prime in `{251 … 131071}`).

Our position: the userspace `crates/lb-l4-xdp/src/lib.rs:218-346` `MaglevTable` is *userspace simulator only*. The real BPF datapath does not pick backends — it only does conntrack lookup + rewrite. Backend selection happens in `crates/lb-balancer` userspace per ADR-0001-style architecture; the XDP path is "conntrack-hit -> XDP_TX, miss -> XDP_PASS-to-userspace".

Why it's fine: the Maglev-in-XDP pattern is Katran/Cilium's. Our pattern is closer to Unimog's first-packet-to-userspace + conntrack-cached-fast-path. The constants `is_prime` / `65537` are validated in `lib.rs:511-521`, so when userspace builds a Maglev table for control-plane purposes, it's a prime size.

Caveat we accept: backend churn does not preserve flow stickiness through XDP — that's a deliberate trade for not implementing Maglev in BPF. Documented obliquely in `audit/STATE`'s Pillar-4b deferrals. Add an explicit ADR if not already present.

## R-L4-02 — No tail-call program splitting

Reference: Katran lesson 12 (tail-call programs are size-budgeted); Cilium D1 (tail-call split).

Our position: the `lb_xdp` program inlines `handle_ipv4`, `handle_ipv6`, `rewrite_v4`, `rewrite_v6`. Built ELF is ~3 KiB. Verifier-insn budget on 5.15+ is 1M insns; we are nowhere near the limit at current scope.

Why it's fine for now: tail calls are needed once the program exceeds the verifier budget. We won't until Pillar 4b-3+ (SYN-cookie / TCP-state machine / encap). When that lands, this becomes "must do" not "should do".

Tripwire: if `lb_xdp.bin` grows past 30 KiB or the verifier log shows `processed > 500000 insns`, revisit.

## R-L4-03 — `PerCpuArray<u64>` for stats with `max_entries=32`, only 10 used

Reference: Katran lesson 5 (`MAX_SUPPORTED_CPUS = 128` is the per-CPU map dimension); xdp-tutorial lesson 9 (per-CPU array beats atomic hash).

Our position: `STATS: PerCpuArray<u64> = with_max_entries(32, 0)`. The `32` is the *slot count* (number of distinct counters), not the CPU count. aya / kernel size the per-CPU dimension at load time from `nr_cpu_ids` automatically; we don't size CPUs ourselves.

Why it's fine: this is the right shape. The extra 22 unused slots (10 defined, 32 declared) are slack for future stat additions without a map-schema change. Userspace `stats_export::NUM_SLOTS = 10` is the read-side bound; `read_stats()` only fetches slots 0..9.

Tripwire: when adding a stat slot, increment `NUM_SLOTS` and add the `StatSlot` enum variant. The unit test `num_slots_matches_enum` (`stats_export.rs:357`) is the regression net. Bump `max_entries` if/when we exceed 32.

## R-L4-04 — Common LRU (not `BPF_F_NO_COMMON_LRU` per-CPU)

Reference: kernel selftest `test_lru_map.c` lesson 11 (per-CPU LRU vs common LRU has different worst-case behaviour).

Our position: `LruHashMap::<...>::with_max_entries(1_000_000, 0)` — the `0` flags means common LRU.

Why it's fine: common LRU gives one global eviction order; per-CPU LRU is faster on hot caches but loses flow stickiness if RX-queue steering changes (the Katran lesson 1 problem). At our QPS target the common-LRU lock contention is acceptable; if we ever hit it, the right move is *per-CPU LRU + global fallback* (Katran's pattern), not naive per-CPU LRU. This is a deliberate trade; document it in the data-plane README.

## R-L4-05 — `unsafe(no_mangle)` and `unsafe(link_section = "license")` on `LICENSE` static (`ebpf/src/main.rs:45-47`)

Reference: aya lesson 15 (userspace GPL-license assertion); selftest unsafe-attributes pattern.

Our position: both attributes use the `unsafe(...)` syntax under the 2024 edition — these are *attribute*-level unsafety markers; they're required by the language. This is not a runtime unsafe block. Round-7's unsafe-justification table correctly counts both as benign (`audit/unsafe-justifications.md:71-73`).

Why it's fine: no action required. Just noting the pattern is correct and intentional.

## R-L4-06 — Aya GPL license check runs at userspace load (`assert_license_is_gpl`)

Reference: aya lesson 15 (`load_from_bytes` does NOT verify GPL license).

Our position: `crates/lb-l4-xdp/src/loader.rs:421-442` parses the ELF with `object` crate and rejects non-`"GPL\0"` license. Three unit tests in the loader cover missing-section, wrong-payload, and happy-path.

Why it's fine: this is the recommended defensive pattern from the references. The previous audit SEC-2-12 covered this and it's correctly implemented.

## R-L4-07 — Map-name fail-fast pattern (handoff item 10)

Reference: aya lesson 13 (`Bpf::map()` returns `Option`).

Our position: every `take_map(name).ok_or(XdpLoaderError::MapNotFound(name))` in `loader.rs:719-723` and similar at `:746`, `:766`, `:784`. Renaming a BPF-side map breaks userspace at startup with a clear `MapNotFound("conntrack")` error.

Why it's fine: this is exactly the references' D4 pattern. Test `pin_name_constants_match_ebpf_source` (`tests/xdp_pin_paths.rs:32-50`) additionally guards the BPF-side name string against drift.

## R-L4-08 — Sentinel `0xFF` for `ATTACH_MODE_UNSET` (`stats_export.rs:78`)

Reference: aya error-handling lessons + handoff cross-cutting item 5.

Our position: `AtomicU8::new(0xFF)` as "no attach yet"; `from_byte(0xFF) -> None`. The Prom gauge correctly reports no value (rather than fabricating one) before attach.

Why it's fine: cleanly handles "haven't tried attach yet" and "attach succeeded". The `as_byte()` mapping {1=Drv, 2=Skb, 3=Hw} is distinct from `0xFF`. Good defensive pattern.

## R-L4-09 — IPv6 extension headers: only Hop-by-Hop and Routing handled, max 2

Reference: kernel selftests cover IPv6 ext-header parsing edges.

Our position: `handle_ipv6` parses `IPPROTO_HOPOPTS` (0) and `IPPROTO_ROUTING` (43), max 2 iterations, then either falls through to TCP/UDP or returns `STAT_V6_EXT_UNSUPPORTED` + `XDP_PASS`. **Note**: this *resolves* the "exotic ext-header" case but does NOT cover `IPPROTO_FRAGMENT = 44` (see ROUND8-L4-08, which is open).

Why it's fine for routing/HBH: the verifier won't accept an unbounded loop; a fixed `extensions_consumed < 2` bound is the standard pattern. Counter `STAT_V6_EXT_UNSUPPORTED` lets ops alarm if a flood of exotic v6 traffic shows up. The 2-iter cap matches what `xdp-tutorial` examples do.

## R-L4-10 — `BackendEntry::pad`/`_pad` parity between BPF and userspace, `new()` constructor zero-inits

Reference: kernel selftest hash-key sensitivity to padding.

Our position: BPF side `_pad: [u8; 3]` / userspace side `pad: [u8; 3]` (or `pad: u16`) with `BackendEntry::new` etc. constructors that zero-init. EBPF-2-09 / CODE-2-07 fix landed. Byte-size const-asserts in `loader.rs:284-287`.

Why it's fine: addressed in the previous round (CODE-2-07). The risk is captured: the `pad` is `pub` so a careless caller could still construct with garbage, but the `new()` constructor guarantees zero. Round-8 audit confirms no in-tree caller bypasses `new()`.
