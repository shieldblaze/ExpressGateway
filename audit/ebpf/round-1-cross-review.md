# Round 1 — eBPF cross-review notes

Owner: `ebpf`.

This file substitutes for `SendMessage` (no such tool is available in
this harness). Each block is the summary that would have been sent to
the named teammate. Disagreements (if any surface in round 2) go in
the same block as a `Reply:` subsection.

---

## To `sec`

Summary: the BPF object as committed has **no `license` ELF section
and no `.BTF`**. aya 0.13 may default-set the kernel license string at
`BPF_PROG_LOAD` time, but `XdpLoader::load_from_bytes`
(`crates/lb-l4-xdp/src/loader.rs:212`) does not call
`EbpfLoader::set_license`, so the kernel-accepted license is whatever
aya's `LoadOptions::license` default is. I could not verify that
default in this sandbox (no cargo registry cached). If the default is
not `"GPL"`, every real-NIC attach fails with -EINVAL and we log
`xdp disabled — kernel_load(lb_xdp) failed`.

Action requested: confirm aya 0.13's default license; if not `"GPL"`,
this is a SEV-1 startup-only failure under CAP_BPF.

Also: `caps::has_cap(None, CapSet::Effective, …)` does not probe
CAP_SYS_ADMIN — that's the only thing kernels < 5.8 accept. README
declares 5.15 floor, but if an operator on a 5.4 distro enables
xdp_enabled the warning text will be misleading.

Verifier-bypass surface: every `unsafe` in
`crates/lb-l4-xdp/ebpf/src/main.rs` is bounds-checked via `ptr_at` /
`ptr_at_mut`. No raw pointer arithmetic that bypasses the verifier-
visible `data_end` compare. Manual `read_unaligned` is used for every
packed field — no native references into the packet. Probably clean.

bpffs perms: nothing is pinned today. If pinning is added later,
design-needed: mount path, mode (likely 0750), owner (the LB's uid,
not root).

---

## To `code`

Summary: focus areas for Round 2 from the userspace XDP loader:

1. **Dropped `XdpLinkId`** at `crates/lb-l4-xdp/src/loader.rs:275`.
   Comment claims aya keeps the link alive via `self.ebpf`. Need to
   confirm against aya 0.13 source. If wrong, the attach silently
   detaches at function exit.
2. **`&mut self.ebpf` borrow** through every map accessor. Forces
   serialised single-task map access. Confirm the control plane in
   `crates/lb` does not race-update from concurrent Tokio tasks; if
   it does, `XdpLoader` needs interior locking.
3. **`Pod` impls for FlowKey/BackendEntry/FlowKeyV6/BackendEntryV6**
   (loader.rs:53, 76, 97, 120). Byte-for-byte layout vs.
   `ebpf/src/main.rs:143–195`. Public `pad`/`_pad` fields are not
   constructor-zeroed in either side — uninitialised padding will
   land in BPF map keys and break hashing. The ebpf-side reads use
   `core::ptr::read_unaligned` on `addr_of!` which is OK, but
   userspace inserters need to write zero-initialised structs.
4. **Committed ELF is 9864 B, ADRs say "~3 kB"**. Either the ADR is
   stale or debug info accreted. `strip --strip-debug` would shrink
   meaningfully; `bpf-linker` may not have stripped.
5. `LB_XDP_ELF` is `#[cfg(lb_xdp_elf)]`-gated and only emitted by the
   `lb-l4-xdp` crate. `crates/lb/src/xdp.rs` references
   `lb_l4_xdp::LB_XDP_ELF` (line 104) without a corresponding cfg
   gate at the use site (it's in an `#[cfg(lb_xdp_elf)] fn
   attach_with_elf` body, so the reference is only compiled when
   `lb`'s own `build.rs` also detects the blob — check this is
   mirrored in `crates/lb/build.rs`).

---

## To `rel`

Summary: reload, restart, and operational-readiness gaps.

1. **No map pinning** anywhere. Process restart drops every
   conntrack entry. `reload_zero_drop` test claim is fine for the L7
   path but the L4 XDP path is **cold on restart**. Reload docs do
   not mention this.
2. **SKB mode hard-coded** at `crates/lb/src/xdp.rs:115`. There is
   no Drv-mode probe, no Hw-mode probe, no config knob, no fallback.
   Operationally this is a 10–50× perf gap relative to native XDP.
3. **No multi-kernel verifier matrix.** ADR-0005 promised
   `xtask xdp-verify` over 5.15 / 6.1 / 6.6 — does not exist.
   `crates/lb-l4-xdp/ebpf/verifier-logs/` is empty.
4. **Conntrack map is HASH not LRU_HASH.** Under flood, inserts fail
   and STAT_PASS climbs. No alert / metric on
   `bpf_map_update_elem -> ENOMEM` rate.
5. **No graceful detach on shutdown.** Aya `Ebpf` drop tears down the
   attach asynchronously through the netlink path. SIGTERM →
   immediate detach → in-flight XDP_TX packets at the NIC may be
   lost. May be acceptable; needs an explicit decision.
6. **Per-CPU `STATS` map is never exported.** No metrics pipeline reads
   it, no Prometheus counter mirrors it. The slot constants
   `STAT_PASS`..`STAT_V6_EXT_UNSUPPORTED` (main.rs:198–207) exist
   purely as in-kernel counters today.

---

## To `proto`

Summary: protocol-correctness items in the XDP fast path.

1. **VLAN tag preserved on egress.** Single-tag 802.1Q is consumed
   by offset arithmetic at parse, then the rewrite path writes a
   fresh L2 header at offset 0, so the original VLAN tag bytes
   remain at offset 12. Confirm this is the intended on-wire shape.
2. **IPv6 extension headers:** only Hop-by-Hop (0) and Routing (43)
   are skipped, at most twice. Fragment (44), AH (51), ESP (50),
   Dest Opts (60) → XDP_PASS. Confirm the matrix.
3. **UDP/IPv4 zero-checksum left untouched** (RFC 768 OK).
   **UDP/IPv6 zero-checksum left untouched** (RFC 8200 §8.1
   forbids — but we deliver the violating packet unchanged, we don't
   originate it). Confirm semantics.
4. **TCP options unmodified.** TCP timestamps, MSS, SACK pointers
   continue across the rewrite. Pillar 4b deferred per ADR-0004.
5. **No SYN-cookie / SYN-flood mitigation in XDP.** Documented as
   Pillar 4b-3 deferred work.
6. **First-packet latency = userspace round trip.** The BPF program
   never picks a backend on cache miss — it XDP_PASSes and lets
   userspace decide, then userspace inserts into CONNTRACK. Confirm
   this matches the perf promise.

---

## Disagreements

(Empty — to be filled if other teammates push back in round 2.)
