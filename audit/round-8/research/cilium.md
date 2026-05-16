# Cilium — eBPF datapath + kube-proxy replacement

Reference: `https://github.com/cilium/cilium`, especially `bpf/`.
Production: Isovalent, every major cloud k8s offering.

## Architecture summary

Cilium has the largest publicly-readable XDP+TC datapath in
existence. Five entry programs live in `bpf/`:

- `bpf_xdp.c`    — XDP ingress (early prefilter, LB acceleration)
- `bpf_host.c`   — TC ingress/egress on host netdev
- `bpf_lxc.c`    — TC ingress/egress on container veth
- `bpf_overlay.c`— VXLAN/Geneve overlay datapath
- `bpf_sock.c`   — cgroup socket-level LB (kube-proxy "socket LB")

Common logic in `bpf/lib/` includes `lb.h` (service map lookup +
backend selection), `lb4.h`/`lb6.h` (per-family), `conntrack.h`,
`nat.h`, `maglev.h`. The XDP datapath is intentionally a thin
prefilter that *tail-calls into* the LB code; the full policy/SNAT
state machine lives in TC. This matters: XDP cannot use
`bpf_skb_load_bytes` or `bpf_redirect_neigh` and cannot consult the
routing table at full fidelity, so the architecture deliberately
shoves the hard cases down to TC.

Source: `https://github.com/cilium/cilium/tree/main/bpf` and
`https://docs.cilium.io/en/stable/network/ebpf-datapath/`.

## Lessons learned

1. **XDP datapath is a *prefilter*, not the whole LB.** Cilium does
   NodePort acceleration in XDP — match VIP, pick backend, encap, TX
   — but ConnTrack, SNAT, and policy enforcement run in TC. The
   reason: many helpers (skb_load_bytes, redirect_neigh, fib_lookup
   on some kernels) are TC-only or XDP-late. The split is in
   `bpf_xdp.c::tail_lb_ipv4` calling into `lb4_xlate` from `lib/lb.h`
   then deferring the rest. Implication: a single XDP program trying
   to do "everything" hits insn limits and verifier complexity walls.
   Source: `bpf/bpf_xdp.c`, function chain `cil_xdp_entry -> check_filters -> tail_lb_ipv4`.

2. **`tableSize` must be prime, in {251 ... 131071}.** Cilium's
   Maglev implementation enumerates the allowed table sizes; you
   can't pick "10000" or "powers of two". The doc says: "M should
   be larger than 100 * N" where N=max backends. Non-prime sizes
   break the Maglev permutation (offset+skip stride must be coprime
   with M). Lesson: if your hashring size is not prime, your
   reshuffle rate on backend churn is much worse than the paper.
   Source: `https://docs.cilium.io/en/latest/network/kubernetes/kubeproxy-free/` (Maglev section).

3. **All agents in the cluster must share the Maglev `hashSeed`.**
   The seed is 12 bytes base64-encoded, generated once. If two
   agents pick different seeds, the same flow hashes to different
   backends on different nodes, and DSR returns will not match
   incoming flows. Production teams have wiped clusters over this.
   Source: same kubeproxy-free doc, `hashSeed` field.

4. **Maglev only applies to N-S (external) traffic; E-W uses socket
   LB.** Cilium has *two* LB paths: Maglev/XDP for north-south,
   `bpf_sock.c` cgroup hook for east-west. The reason: inside the
   cluster you can rewrite at connect() time and avoid every-packet
   datapath cost. Lesson: if your LB is the only path for both
   internal and external flows you pay datapath cost on every
   internal RPC.
   Source: `bpf/bpf_sock.c` + kubeproxy-free doc.

5. **`bpf.lbMapMax` default 65536 is wrong for big clusters.** The
   doc explicitly recommends increasing this for ">1000 services or
   >10000 endpoints". The map is sized at agent startup; ENOMEM at
   insert means a service silently fails to take traffic. Lesson:
   compute (services * backends * 2) and pick the cap based on
   that, not on a "feels small" default.
   Source: tuning doc.

6. **Source-range allowlist is LPM-TRIE, not LIST.** Service
   `loadBalancerSourceRanges` populates a per-service LPM trie
   pointed to by `BPF_MAP_TYPE_HASH_OF_MAPS`. The XDP path looks
   up the inner map by service ID, then does LPM. Reason: list
   scan is O(n) per packet, LPM is O(prefix-bits). Lesson: if our
   ACL is iterated, it's a DoS surface; LPM trie is the right
   shape.
   Source: `bpf/lib/lb.h` source-range lookup.

7. **Per-CPU conntrack with explicit "RST observed" state.** Cilium's
   `conntrack.h` tracks ESTABLISHED/SYN_SENT/SYN_RECV/CLOSE_WAIT
   transitions and prunes on RST. Naive LRU conntrack keeps closed
   flows around until eviction; that's a leak. Lesson: TCP state
   machine in the BPF program is not optional if you want LRU
   capacity to survive flow churn.
   Source: `bpf/lib/conntrack.h`.

8. **Mellanox ConnectX-4 Lx VF + native XDP = bug.** Cilium's
   issue tracker has a long-standing ticket where Mellanox VF
   drivers silently fail XDP_REDIRECT. Cilium added a runtime
   detection that falls back to generic (skb) mode on these NICs.
   Lesson: native XDP cannot be assumed; the loader must probe
   and fall back. Aya issue #1193 documents the same MLX5/CX6
   case.
   Source: search Cilium issues "mellanox ConnectX-4 Lx xdp".

9. **`bpf_xdp_adjust_head` + GRO/LRO offload interact badly.**
   When the NIC coalesces packets via LRO, XDP only sees one
   combined skb; `bpf_xdp_adjust_head` then expands a single
   buffer that the kernel later cannot split. Cilium's install
   script disables LRO/GRO on managed interfaces. Lesson: an XDP
   loader that doesn't touch NIC offloads is leaving a footgun.
   Source: Cilium install docs `ethtool -K $dev gro off lro off`.

10. **DSR has two variants (IPv4 option vs Geneve).** Cilium
    initially shipped only the IPv4-options DSR (encode return IP
    in a custom IP option). That breaks on clouds that strip IP
    options (AWS, anything with a sane firewall). The fix was to
    add Geneve-encap DSR mode. Lesson: DSR is not portable;
    "we do DSR" doesn't tell you anything without the encap
    choice.
    Source: kubeproxy-free doc, DSR-options vs DSR-geneve.

11. **`externalTrafficPolicy=Local` + local-backend = needs
    explicit hairpin.** Cilium had a recurring bug
    (#45252, #44630) where external LB IPs failed when the
    backend pod was on the same node. The XDP program rewrites
    the DIP but the response source IP must be rewritten too,
    or the client sees an unfamiliar source. This needed
    coordinated XDP-ingress + TC-egress rewrites.
    Source: Cilium issues #45252, #44630.

12. **eBPF LB intercept conflicts with conntrack reply path
    (#44348).** When the LB lookup runs on traffic that's
    already a conntrack reply, source-port rewriting on UDP
    breaks the reply. Cilium added a "this is a reply, skip LB"
    short-circuit. Lesson: LB lookup must check conntrack
    state before doing any rewrite, even on the ingress path.
    Source: Cilium issue #44348.

13. **Cross-node TCP after WireGuard decrypt was silently
    dropped (#45497).** When WG decrypts and reinjects, the
    post-decrypt skb's source IP no longer matches the BPF
    program's expectations. Lesson: any encryption hop in front
    of XDP breaks the assumption that "packet's source is the
    real client". Worth checking our path for the same.
    Source: Cilium issue #45497.

14. **XDP on bonded interfaces has strict attach rules.** Cilium
    documents (and the kernel selftest `xdp_bonding.c`
    enforces): you cannot attach to both master and slave
    simultaneously. Cilium's installer probes the bond mode
    and refuses to attach on incompatible modes. Lesson: our
    loader needs to detect bond/team interfaces and refuse
    (or detach from slave first).
    Source: `tools/testing/selftests/bpf/prog_tests/xdp_bonding.c`.

## Defensive patterns worth comparing against our code

- **D1. Tail-call split.** `bpf_xdp.c` deliberately tail-calls
  `tail_lb_ipv4` instead of inlining. Our `ebpf/src/main.rs`
  inlines `handle_ipv4`/`handle_ipv6` — fine for size now, but
  the moment we add SNAT/conntrack-state-machine we'll exceed
  insn budget. Worth comparing how close we are to 1M insns at
  current scale.
  File: `bpf/bpf_xdp.c::tail_lb_ipv4`.

- **D2. Service-map sentinel for wildcard.** Cilium uses
  `IPPROTO_ANY` + port-zero as the wildcard service-entry
  encoding. Our code stores backends in `BackendEntry`/`BackendEntryV6`
  in `ebpf/src/main.rs`; check whether a zero entry is
  interpreted as "wildcard" anywhere.
  File: `bpf/lib/lb.h`.

- **D3. Conntrack TCP-state-aware.** Cilium's `conntrack.h`
  invalidates entries on RST, on FIN-FIN-ACK, and on long-idle.
  Our `CONNTRACK` is a pure LRU. Even if we don't implement
  the full TCP FSM, a "drop entry on RST" rule is cheap and
  prevents one class of attack (replay-old-RST to evict).
  File: `bpf/lib/conntrack.h`.

- **D4. LPM-TRIE allowlist.** Our `ACL_DENY_TRIE: LpmTrie`
  matches Cilium's pattern. Verify the trie key is in
  network byte order and zero-prefix entries are rejected
  by userspace (a zero-prefix `0.0.0.0/0` deny would block
  everything).
  File: `bpf/lib/lb.h` source-range trie.

- **D5. NIC offload assertion.** Cilium disables GRO/LRO on
  XDP-attached interfaces. Our loader (`crates/lb-l4-xdp/
  src/loader.rs`) — does it touch ethtool offloads? If not,
  a hostile operator can re-enable LRO and silently break
  our forwarder.
  Reference: Cilium install scripts.

## Non-goals (explicit)

- **Not a general L4 LB outside k8s.** The map shapes assume
  service IDs and CiliumNode identities; standalone use is
  unsupported.
- **No SCTP load balancing in XDP** (TC only).
- **No fragment reassembly.** Same as Katran.
- **No userspace fast path.** All forwarding stays in BPF.
- **No support for non-Linux dataplanes.** No DPDK/VPP backstop.
- **No "single binary" deployment.** Requires daemon + CRDs.

## Direct comparison ideas for `div-l4`

1. Verify our Maglev table size (if any) is prime. If we
   don't have Maglev, document the reshuffling cost on
   backend change.
2. Verify `ACL_DENY_TRIE` rejects `prefix_len=0` at
   userspace insert; otherwise an operator can install a
   default-deny by accident.
3. Check whether our conntrack tracks TCP state at all, or
   is pure LRU. If pure LRU, an attacker can keep dead
   flows alive by replaying any byte.
4. Verify the loader probes the netdev type — bond, team,
   bridge — and either refuses or attaches to the right
   layer. Cilium has scars from this.
5. Verify our forwarder uses LPM for ACLs, not a list scan
   (we do — `ACL_DENY_TRIE` is `LpmTrie<u32, u32>`), and
   that the value (`u32`) is meaningful (action code, not
   "any non-zero = deny", which is fragile).
6. Verify NIC offload handling: if our loader doesn't
   disable LRO/GRO, our packet parser sees coalesced
   buffers and computes wrong checksums.
