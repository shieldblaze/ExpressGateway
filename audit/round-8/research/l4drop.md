# Cloudflare L4Drop / Unimog

Reference: blog posts (Unimog is not open-source). What we have:
- `https://blog.cloudflare.com/unimog-cloudflares-edge-load-balancer/`
- `https://blog.cloudflare.com/l4drop-xdp-ebpf-based-ddos-mitigations/`
- `https://blog.cloudflare.com/how-to-drop-10-million-packets/`

**Closed-source caveat.** Unimog's source is private. The lessons
below are extracted from blog posts and Cloudflare's public
talks. We can sustain ~12 distinct lessons; the rest of the
"10 distinct" bar would be padding. The blog posts are also
sometimes vague on specifics for competitive reasons.

## Architecture summary

Unimog is Cloudflare's edge L4 LB. The novel choice: every edge
server is *also* an Unimog forwarder, not a dedicated LB tier.
Packets arrive at any server; that server either handles them
locally or GUE-encaps and forwards to another edge server in the
same datacenter that should handle them. The forwarding table is
a single XDP-mode bucket-array sized at "100x the number of
servers" (so per-bucket churn on a server change is ~1%).

Daisy-chain (from the Beamer paper): each bucket has *two* slots,
current and previous. When the current server doesn't recognise
the flow, it forwards to the previous server — the "second hop"
— which presumably owns the established connection. This preserves
in-flight connections through rebalancing.

L4Drop is a sibling pre-pipeline: drop volumetric attacks before
they reach the forwarder. L4Drop compiles rules to inline BPF
constants via ELF relocations, not maps, to stay within the
verifier's insn budget.

## Lessons learned

1. **"Every server is a load balancer" deployment model.** Unimog
   avoids the dedicated-LB-tier pattern entirely. Every server runs
   the XDP forwarder; ECMP from the routers spreads ingress; each
   server independently decides "mine or yours". This eliminates a
   coordination problem (LB state sync across LB hosts) but
   introduces a new one (every server must agree on the forwarding
   table). Cloudflare publishes the table from xdpd to every server
   atomically.
   Source: Unimog blog.

2. **Forwarding-table buckets = 100x server count.** Mirrors
   Maglev's M >= 100*N. Cloudflare's table is a flat array indexed
   by `low_N_bits(hash(5tuple))`. Changing one server's assignment
   shifts ~1/server fraction of flows. Lesson: bucket array works
   even without Maglev permutation logic if you accept that table
   updates require careful sequencing.
   Source: Unimog blog "Forwarding tables".

3. **Daisy-chain (two slots per bucket).** When the forwarding
   table is updated, in-flight connections are preserved by the
   "previous server" pointer. New server gets the flow, looks it
   up in its socket table, finds no socket, GUE-forwards to
   previous. After a configurable drain window the previous
   slot is cleared. Lesson: any rebalance has a "second-hop
   window" of N seconds during which two servers know about
   the flow. Without this you drop every connection on rebalance.
   Source: Unimog blog "Daisy chaining".

4. **GUE port is per-deployment configurable, not hard-coded.**
   Unlike Katran (port 6080), Unimog's GUE port is set from
   config so it doesn't collide with other UDP services on the
   server. Lesson: even small constants need to be configurable.
   Source: Unimog blog (GUE section).

5. **xdpd is a Go daemon that owns the BPF chain.** Userspace is
   Go, not C++. xdpd loads the XDP program, populates maps, owns
   the lifecycle. Lesson: language at the userspace layer is
   *not* the performance bottleneck; BPF is. Pick something safe.
   Source: Unimog blog "xdpd".

6. **"Constant injection" via ELF relocations beats maps for
   small static config.** L4Drop compiles rules to BPF and
   patches relocated constants into the ELF at load time
   (instead of map lookups), keeping the verifier-insn budget
   for the hot path. Lesson: if a value is set at load and
   never changes, a relocation is cheaper than a map lookup.
   For runtime-changing values, use maps.
   Source: L4Drop blog.

7. **Random-sampling perf-event output for dropped packets.**
   L4Drop uses `bpf_get_prandom_u32() < threshold` then
   `bpf_xdp_event_output` to ship a copy of a sampled dropped
   packet to userspace. Lesson: full-rate ring-buffer of
   dropped packets is a DoS amplifier; sample.
   Source: L4Drop blog "Sampling".

8. **CPU cost target: <1%.** Cloudflare publishes that Unimog
   uses less than 1% CPU per server. The constraint shapes
   every design choice: no per-packet hash recompute when LRU
   hits, GUE checksum is RSS-friendly, no per-flow state writes
   on the hot path.
   Source: Unimog blog ("less than 1% of the processor
   utilization").

9. **eBPF "same binary, multiple kernels" is *the* reason
   chosen over kernel modules.** The blog calls this out
   explicitly. Lesson: this is the operational argument for
   eBPF on a kernel-version-diverse fleet. CO-RE relocations
   are doing the work.
   Source: Unimog blog ("range of kernel versions without
   recompilation").

10. **XDP_DROP performance: 10 mpps per core.** The "how to
    drop 10M packets" post benchmarks all the alternatives
    (userspace, iptables, nftables, tc, XDP) and finds XDP_DROP
    wins by ~6x over the next-best (tc-ingress). Lesson:
    drop-decisions belong in XDP regardless of L7 plans.
    Source: "How to drop 10 million packets" blog.

11. **Conntrack disabled in the benchmark.** Cloudflare's 14
    mpps test specifically *disables conntrack* on the receiving
    server because conntrack's own state machine is the
    bottleneck. Lesson: any conntrack you add to the XDP
    path measurably slows it down; sample/cap accordingly.
    Source: "How to drop 10M packets" blog.

12. **Hardware-flow-steering to a single core for the
    benchmark.** Cloudflare's benchmark pins all the test
    traffic to one RX queue and measures one core's cost.
    Lesson: published mpps numbers are "per core, single
    flow", not "per-server aggregate". When you read "10M
    pps" the number after scale-out is "10M * cores".
    Source: same blog.

## Defensive patterns worth comparing against our code

- **D1. Atomic forwarding-table update.** Unimog updates
  the full bucket array via a swap (new map, then
  `BPF_MAP_UPDATE` of the prog-array entry), never partial
  in-place. Our loader's backend-update path
  (`loader.rs`) — does it do CAS-style swap or in-place
  update? In-place leaves a window where some flows hash
  to old, some to new.
  Source: Unimog blog (table publication).

- **D2. Daisy-chain (previous-slot) on rebalance.** See
  lesson 3. Our path apparently has no "previous backend"
  concept; on backend churn we'll drop in-flight flows
  unless the conntrack saves them.
  Source: Unimog blog.

- **D3. Sample, don't stream, dropped-packet metadata.**
  Our `stats_export.rs` exposes per-CPU stats; if we add
  a "packet sample for ops" channel later, sample at
  ~1/1000 not full rate.
  Source: L4Drop sampling.

- **D4. Disable conntrack on the bench server, not in
  production.** This is an inverse pattern: do not let the
  *test* environment shape the production conntrack policy.
  Document explicitly that perf numbers from disabled-CT
  benches don't apply.
  Source: 10M blog.

- **D5. Constant injection via ELF relocations.** Our
  loader could exploit this: if a value (e.g. "is L7 port
  N") changes rarely, patch the constant on load rather
  than looking up in a map per-packet. We currently use
  `L7_PORTS: HashMap` (in `ebpf/src/main.rs`), which is
  the right shape for changing-at-runtime, but the
  question is what fraction of these values genuinely
  change.
  Source: L4Drop "compile rules to BPF".

## Non-goals (explicit, from blogs)

- Unimog is **not** intended to support arbitrary
  topologies — assumes ECMP-flat datacenter.
- Unimog is **not** an L7 proxy — that's pingora's job.
- L4Drop is **not** stateful — no conntrack, no flow tracking,
  rules-only.
- Unimog **does not** publish its source. Operators of
  Cloudflare-shaped infra are expected to either roll their
  own or use Cloudflare's hosted product.
- L4Drop **does not** do per-IP rate limiting in the XDP path
  beyond what the compiled rule says; complex shapers run in
  TC or userspace.

## Direct comparison ideas for `div-l4`

1. Verify our backend-table update is atomic from the BPF
   program's perspective (single map write, not multi-key).
2. Consider a "previous backend" slot per bucket for
   rebalance scenarios — without it our rebalance drops
   flows.
3. Verify our stats-export sampling rate. If we ever stream
   per-packet drop metadata, sample.
4. Check that perf-comparison numbers cited in our docs
   match the *with-conntrack* path, not the disabled-CT
   bench.
5. Survey our `L7_PORTS`, `ACL_DENY_TRIE`, backend maps —
   any whose contents are effectively immutable could be
   ELF-relocation constants instead, freeing insn budget.
