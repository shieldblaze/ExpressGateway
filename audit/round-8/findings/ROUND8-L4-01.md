### ROUND8-L4-01 — Backend-index 0 / zero-IP is a valid backend (Katran lesson 10 absent)

Reference: `audit/round-8/research/katran.md` lesson 10 ("CH-ring-position 0 reserved as uninitialised") + handoff item 1
Our equivalent: `crates/lb-l4-xdp/ebpf/src/main.rs:183-196` (`BackendEntry`), `crates/lb-l4-xdp/ebpf/src/main.rs:464-470` (lookup path), `crates/lb-l4-xdp/src/loader.rs:130-176`

Severity: high
Status:   Verified-Fixed (verify, task#70, 2026-05-15) — eBPF data-plane sentinel (XDP_PASS + STAT_BACKEND_UNPOPULATED) closes the partial-population race; userspace `try_new` added (infallible `new` kept for back-compat — non-blocking caveat). See audit/round-8/verify/l4.md.

Divergence:
- Katran: `real_idx == 0` is reserved, hash never lands on it, drop with `increment_ch_drop_real_0()`. Real backends start at index 1.
- Us: `BackendEntry { backend_idx: 0, flags: 0, backend_ip: 0, backend_port: 0, ... }` is a valid lookup result. If userspace pushes a partially-zeroed entry (race, mis-merged update), `CONNTRACK.get(&key) -> Some(zero_entry)`, the BPF program then increments `STAT_CT_HIT_V4` and executes `rewrite_v4` against `entry.backend_ip = 0` — meaning destination IP gets rewritten to `0.0.0.0` and forwarded back out via `XDP_TX`. No sentinel check.

Impact:
- Mid-update race in `XdpLoader::conntrack_map().insert()` (which is multi-syscall — see ROUND8-L4-04): if the value bytes have been written but the BackendEntry's `backend_ip` not yet populated, a packet that matches the partial entry gets MAC-rewritten and re-emitted with `dst = 0.0.0.0`. The packet leaves the NIC as malformed traffic.
- An attacker with userspace write access (not the threat model here, but an operator typo) inserting `BackendEntry::new(0, 0, 0, 0, [0;6], [0;6])` would silently cause every flow that hits that conntrack key to be black-holed via `XDP_TX 0.0.0.0:0`.
- No counter for "zero-sentinel hit"; the operator sees `xdp_packets_total{result="ct_hit_v4"}` ticking but `xdp_packets_total{result="tx_v4"}` *also* ticking — looks healthy.

Reproduction:
- Insert `BackendEntry::new(0, 0, 0, 0, [0u8;6], [0u8;6])` into the conntrack map for a known flow.
- Send a packet matching that flow.
- Observe: program emits `XDP_TX` with rewritten dst = `00:00:00:00:00:00 -> 0.0.0.0:0`. There is no `STAT_BACKEND_UNPOPULATED` counter to flag this.

Recommendation:
1. Add `STAT_BACKEND_UNPOPULATED = 10` (extend `stats_export::StatSlot` accordingly).
2. In `handle_ipv4` / `handle_ipv6`, after the conntrack hit:
   ```
   if entry.backend_ip == 0 && entry.backend_port == 0 {
       incr_stat(STAT_BACKEND_UNPOPULATED);
       return Ok(xdp_action::XDP_PASS);
   }
   ```
   For IPv6, the equivalent is `entry.backend_ip == [0u8;16]`.
3. Belt-and-braces: have `BackendEntry::new` / `BackendEntryV6::new` in the userspace loader reject `backend_ip = 0` (debug_assert + Result), and add a unit test.
