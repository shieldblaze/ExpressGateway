# Katran — Facebook's L4 XDP load balancer

Reference: `https://github.com/facebookincubator/katran`
Repo language: C (BPF), C++ (userspace), Python (manifest/build).
Production: Meta edge fleet since pre-2018.

## Architecture summary

Katran is an XDP-mode L4 load balancer with Direct Server Return.
Userspace is a C++ library (`lib/katran/`) that owns BPF map state; the
data plane is `lib/bpf/balancer.bpf.c` plus a chain of tail-call programs
(decapsulation, GUE, ICMP). The forwarding decision is:

  client packet -> XDP entry program -> tail-call decap if VIP-bound
  -> 5-tuple hashed against per-VIP "ring" (extended Maglev) ->
  encap (IPIP / GUE) -> XDP_TX back out the same NIC.

Connection affinity is preserved across LB restarts because Maglev is
deterministic given (seed, backend set). When the backend set changes,
Katran additionally consults a per-CPU LRU conntrack table to keep
existing flows on the previous backend. Userspace owns all maps; the
BPF program treats every map lookup as fallible.

Source: `https://engineering.fb.com/2018/05/22/open-source/open-sourcing-katran-a-scalable-network-load-balancer/`.

## Lessons learned

1. **Per-CPU LRU + a global fallback LRU.** A naive single LRU map is
   cache-line ping-pong under high RPS; pure per-CPU LRU loses flow
   affinity if RX-queue steering changes. Katran does both: per-CPU LRU
   on the fast path, then a shared "fallback_glru" lookup when per-CPU
   miss happens, before re-running the hash. This is in
   `balancer_consts.h` (`DEFAULT_LRU_SIZE`, `DEFAULT_GLOBAL_LRU_SIZE`)
   and is the same pattern the Cilium team eventually copied. Implies:
   if your conntrack is LRU-hash, you also need a "second chance"
   policy on miss, not just "rehash and pray."
   Source: `https://github.com/facebookincubator/katran/blob/main/katran/lib/bpf/balancer_consts.h`.

2. **MAX_PCKT_SIZE deliberately small (default 1.5k).** Katran refuses
   to load-balance anything larger than its compile-time max packet,
   because XDP `bpf_xdp_adjust_head` for encap can run off the packet
   tailroom and corrupt subsequent buffers. The README lists this as a
   non-goal: "Maximum packet size cannot be bigger than 3.5k (and 1.5k
   by default)." A larger-than-MTU packet is dropped, not fragmented.
   Source: README "Limitations" section,
   `https://github.com/facebookincubator/katran#limitations`.

3. **No IP fragmentation, by design.** The forwarding decision needs
   L4 ports, which are absent from non-first fragments. Rather than
   buffer-and-reassemble (which XDP cannot do), Katran drops or
   redirects non-first fragments. This is explicit in README
   non-goals. Implies: any L4 LB that pretends to handle fragments is
   either (a) lying, (b) doing reassembly in userspace, or (c)
   hashing fragments by L3 only, which is a covert channel.
   Source: `https://github.com/facebookincubator/katran#limitations`.

4. **`is_under_flood()` rate-limiter to skip LRU writes.** Under
   SYN-flood, blindly writing every new flow into the LRU evicts good
   flows. Katran's `balancer.bpf.c` short-circuits LRU insertion when
   per-CPU new-connection rate exceeds `MAX_CONN_RATE` (125k/s/core),
   keeping the LRU stable for real users. Without this, an attacker
   can knock established flows out of the LB by spraying SYNs.
   Source: search "is_under_flood" in `balancer.bpf.c`.

5. **`MAX_SUPPORTED_CPUS` is a hard cap of 128, not "online CPUs".**
   Per-CPU maps are sized at load time and can't grow. Sizing to
   `num_possible_cpus()` is correct; sizing to `num_online_cpus()`
   silently drops counters on hot-plugged CPUs. Katran picks a
   conservative fixed cap. (Compare to `STATS: PerCpuArray<u64>` in
   our `ebpf/src/main.rs` — we set `max_entries=32`, which is a
   stat-key count, not a CPU count; aya per-CPU maps are sized by
   the kernel.)
   Source: `balancer_consts.h` `MAX_SUPPORTED_CPUS`.

6. **Maglev "ring" size is RING_SIZE=65537 (prime) per VIP.** The
   Maglev paper says M >= 100*N for ~1% reassignment on backend churn;
   Katran picks 65537 (prime, > 100 * 4096 max reals) once and reuses
   it across all VIPs. The constant is hard-coded at compile time
   because the BPF stack budget doesn't allow per-VIP variable sizes.
   Implication: any LB that lets operators reconfigure ring size at
   runtime is doing something complex; you probably want a single
   prime, picked once.
   Source: `balancer_consts.h` `RING_SIZE`.

7. **GUE port 6080 is a magic number.** GUE encap on port 6080 is a
   Cloudflare/Facebook convention; receivers must be configured to
   strip it. If your downstream is unaware, traffic looks like
   random UDP and gets sent to L4 anti-DDoS. Hard-coded in
   `balancer_consts.h` GUE_DPORT. Lesson: encap port must be
   coordinated with the host stack of every backend; this is an
   operational, not technical, hazard.
   Source: `balancer_consts.h`.

8. **ICMP "Packet Too Big" threshold = 1280 not 1500.** When Katran
   has to send a PTB (because of encap overhead), it uses 1280, the
   IPv6 minimum MTU, as the advertised MTU. Choosing 1500 here breaks
   PMTUD for any client behind a tunnel. Hard-coded constant.
   Source: `balancer_consts.h` ICMP_TOOBIG_SIZE.

9. **`MAX_VIPS = 512`, `MAX_REALS = 4096`.** Modest by modern standards,
   but reflects a deliberate choice: BPF stack/insns budget grows with
   table size. Forcing a small cap keeps the verifier happy and the
   forwarding loop bounded. If you uncap these you eventually trip
   the verifier's complexity limit.
   Source: `balancer_consts.h`.

10. **CH-ring-position 0 is reserved as "uninitialised".** Katran
    treats `real_idx == 0` as "ring not yet populated" and drops with
    a counter (`increment_ch_drop_real_0()`). This protects against
    races where userspace has registered a VIP but the Maglev table
    population is partial. The map can never legitimately map a flow
    to index 0; real backends start at 1.
    Source: `balancer.bpf.c` (search "ch_drop_real_0").

11. **Userspace pre-validates the BPF object before pinning.** The
    C++ library parses the ELF with libbpf, then issues
    `BPF_PROG_LOAD` with `prog_flags`, and only on success does it
    move the pin into bpffs. Failures are not "leave half-pinned
    state behind." Compare to our `XdpLoader` which also goes
    parse -> load -> attach -> pin.
    Source: `katran/lib/BpfLoader.cpp`.

12. **Tail-call programs are size-budgeted.** Katran splits ICMP
    handling, decap, and main forwarding into separate BPF programs
    chained via `bpf_tail_call`, because each program has its own
    insn limit. Inlining them all would exceed the verifier budget.
    Lesson: if your single XDP program grows past a few hundred
    insns of real work, you must plan for tail calls.
    Source: `katran/lib/bpf/balancer.bpf.c` `recirculate()` helper.

## Defensive patterns worth comparing against our code

- **D1. Validate every pointer arithmetic before deref.** Pattern
  `(void*)(eth + 1) > data_end` before reading `eth->h_proto`. We
  do this in `ebpf/src/main.rs` `handle_ipv4` etc., but Katran's
  helper `bpf_xdp_adjust_head` wrapper additionally zeroes the
  newly-allocated head bytes — we should check our encap path
  initialises every byte of the new head before sending.
  File: `katran/lib/bpf/balancer_helpers.h`.

- **D2. Counter on every drop reason.** Katran defines 22 stat
  indices; every `XDP_DROP` is preceded by `increment_<reason>()`.
  Our `ebpf/src/main.rs` has 10 stat indices (`STAT_DROP`,
  `STAT_PARSE_FAIL`, `STAT_V6_EXT_UNSUPPORTED`, etc.) — narrower than
  Katran. `div-l4` should verify that every distinct drop site has
  a distinct counter, not just `STAT_DROP`.
  File: `balancer_consts.h` (counter offsets).

- **D3. Real-index 0 reserved as sentinel.** See lesson 10.
  Worth checking our backend table for the same convention; if our
  index-0 is a valid backend, an off-by-one in userspace silently
  hashes flows to it.

- **D4. Per-CPU LRU + global LRU fallback chain.** See lesson 1.
  Worth comparing: does our `CONNTRACK: LruHashMap` saturate to
  a global fallback, or do we just rehash on miss and risk
  flow-stickiness loss?

- **D5. Rate-cap LRU writes under flood.** See lesson 4. Our
  conntrack does not appear to have a write-rate cap; this is a
  real DoS surface.

## Non-goals (explicit)

- **No fragmentation handling.** Drop fragmented packets, don't
  reassemble, don't fragment outgoing.
- **No IP options.** Drop. The verifier-acceptable parser would be
  too expensive.
- **Not a generic LB.** Specifically "LB on a stick" + DSR + L3
  routing. No NAT, no proxy mode, no socket lookup.
- **No L7 awareness.** Ports and 5-tuple only; QUIC connection IDs
  parsed for hashing but not for state.
- **No general TC fallback.** XDP-only; if XDP attach fails Katran
  errors out, it does not fall through to TC.
- **No statistics aggregation in kernel.** Per-CPU arrays, userspace
  sums.

Source for all: README "Limitations" + paper-style intro to
`engineering.fb.com/.../open-sourcing-katran-...`.

## Direct comparison ideas for `div-l4`

1. Compare `STAT_*` enumeration in `ebpf/src/main.rs` to Katran's
   22-counter catalogue. Identify drop sites that lack distinct
   counters. Specifically: do we count VLAN-tagged-untagged
   transitions, IPv6 ext-header rejects (we do — `STAT_V6_EXT_UNSUPPORTED`),
   conntrack-LRU-evict?
2. Compare `CONNTRACK` LRU sizing strategy. Katran uses per-CPU
   1000 + global 10000. We have `CONNTRACK` and `CONNTRACK_V6` —
   what max_entries? Does the user override at load time?
3. Look for a `MAX_PCKT_SIZE` equivalent. If we don't bound packet
   size before reading L4, we can't reject jumbo frames safely.
4. Look for `MAX_CONN_RATE` flood guard. If absent, a SYN-flood can
   wipe legitimate flows from our LRU.
5. Verify our Maglev/ring size (if we have Maglev). If we're not
   doing Maglev, we should at least state that and accept the
   reshuffling cost.
6. Verify backend-index sentinel (lesson 10). What does our backend
   map return for "not populated"?
