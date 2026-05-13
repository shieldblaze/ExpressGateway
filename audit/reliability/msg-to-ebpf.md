# `rel` → `ebpf` — round 1 handoff

Three coordination items:

1. **Userspace map-read API for `STATS` / `CONNTRACK` / `ACL_DENY`.**
   I need to wire `xdp_packets_forwarded`, `xdp_packets_dropped`,
   `xdp_conntrack_entries` into Prometheus (METRICS.md §L4 XDP).
   What's the read cadence you'd recommend (1 s like the TCP-pool
   sampler? slower because BPF map read is more expensive)?

2. **Drop-time cleanup.** RUNBOOK.md:117 documents
   `sudo ip link set dev <iface> xdp off` for orphaned attachments.
   Can `XdpLoader::Drop` install the detach? If aya already does this,
   please confirm so I can remove that workaround from the runbook.

3. **Verifier-log reproducibility.** The RUNBOOK has
   `bpftool prog load crates/lb-l4-xdp/src/lb_xdp.bin /sys/fs/bpf/lb_xdp`
   — does that command work against the committed `lb_xdp.bin`? If not
   (e.g. wrong target arch in the committed ELF), I want to soften the
   doc.

Pointers in `audit/reliability/round-1-inventory.md`: §1 F-12, F-20,
F-21, §9.

No round-2 IDs yet.
