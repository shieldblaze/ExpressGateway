# xdp-project — xdp-tutorial and xdp-tools

References:
- `https://github.com/xdp-project/xdp-tutorial`  (pedagogical)
- `https://github.com/xdp-project/xdp-tools`     (libxdp + utilities)

This is the canonical XDP-idioms reference, maintained by the people
who write the kernel's XDP subsystem (Jesper Brouer et al.). It's
where you learn *what XDP feels like to use correctly*.

## Architecture summary

`xdp-tutorial` is a sequence of compilable lessons:

- `basic01-04`  — pass/drop/map-counter/pinning
- `packet01-03` — parse/rewrite/redirect
- `advanced01-03` — XDP-TC interaction, AF_XDP
- `tracing01-04` — tracepoints, exception tracking

`xdp-tools` ships `libxdp` and CLI utilities (`xdp-loader`,
`xdp-filter`, `xdp-bench`, `xdp-dump`, `xdp-trafficgen`,
`xdp-monitor`). `libxdp` is the *multi-program dispatcher*: it
glues multiple XDP programs onto one netdev by installing a small
trampoline program at the device and tail-calling into each
registered subordinate. Without libxdp, only one XDP program can
attach to a netdev at a time.

## Lessons learned

1. **The verifier discards bounds tracking on
   `bpf_xdp_adjust_head`.** The `packet02-rewriting` README is
   explicit: "the verifier will discard all information about
   previous bounds checks after the packet size has been
   adjusted." Lesson: every bounds check before
   `adjust_head`/`adjust_tail` must be repeated after. Our
   encap code (none currently, but planned) must reset all
   bounds.
   Source: `xdp-tutorial/packet02-rewriting/README.org`.

2. **`bpf_xdp_adjust_head` can fail two ways.** Either the
   target packet would be smaller than ETH header, OR there
   isn't enough headroom. Both return non-zero. Lesson: must
   check the return and return XDP_DROP / XDP_ABORTED, not
   continue with a bogus pointer.
   Source: same.

3. **veth devices won't deliver XDP-redirected frames
   unless the peer also has an XDP program attached.** The
   `packet03-redirecting` README calls this out: "veth devices
   won't deliver redirected/retransmitted XDP frames unless
   there is an XDP program attached to the receiving side."
   Lesson: any integration test that uses veth pairs must
   install a `XDP_PASS` dummy on the peer.
   Source: `xdp-tutorial/packet03-redirecting/README.org`.

4. **`bpf_fib_lookup` requires `net.ipv4.conf.all.forwarding=1`.**
   The same lesson notes: forwarding sysctl must be on for
   FIB lookup to populate routes. Without it, every FIB
   lookup returns "not found". Lesson: a loader that requires
   FIB lookup must check/document this sysctl.
   Source: `packet03-redirecting/README.org`.

5. **Multiple BPF programs in one ELF — selectable by name.**
   `basic02-prog-by-name` teaches that the loader takes a
   `prog_name` parameter. Lesson: name your section/function
   stably; renaming silently breaks the loader.
   Source: `basic02-prog-by-name/README.org`.

6. **`mount -t bpf bpf /sys/fs/bpf/` is the prerequisite for
   pinning.** `basic04-pinning-maps` makes this explicit. Lesson:
   our loader must either mount bpffs or refuse to start with
   a clear error. Silent fallback to "no pin" leaves
   nothing-shared-with-userspace, which is hard to debug.
   Source: `basic04-pinning-maps/README.org`.

7. **Orphaned map pins on reload.** The same lesson warns:
   "When reloading XDP programs, existing pinned maps aren't
   automatically cleaned up." Lesson: a reload that doesn't
   unpin first leaves a path occupied; userspace tools that
   try to re-bind get the old fd and silently use stale state.
   Our pin-path tests (`xdp_pin_paths.rs`) should cover the
   reload case.
   Source: same.

8. **Map fd staleness across reload.** The lesson notes
   `xdp_stats` continues using old file descriptors and
   won't detect new maps without restart. Lesson: any tool
   that uses our pinned stats path should re-resolve via
   pinpath periodically, not cache the fd indefinitely.
   Source: same.

9. **PERCPU vs HASH atomicity.** `basic03-map-counter`
   teaches: a plain `BPF_MAP_TYPE_ARRAY` counter is racy across
   CPUs; you need `__sync_fetch_and_add` (lock_xadd) for
   atomic increments. PERCPU avoids the atomic by giving each
   CPU its own slot. Lesson: per-CPU array beats atomic
   hash for counters by a wide margin. (We use
   `PerCpuArray<u64>` — correct.)
   Source: `basic03-map-counter/README.org`.

10. **VLAN parsing — disable NIC offload to see the tag.**
    `packet01-parsing` warns: "hardware offloads must be
    disabled so XDP sees the actual VLAN headers in packets."
    Lesson: if our parser tries to read VLAN but the NIC
    has `rxvlan` offload enabled, the tag is stripped before
    XDP sees the packet, and our `STAT_VLAN` counter never
    fires.
    Source: `packet01-parsing/README.org`.

11. **Network byte order pitfalls.** Same lesson:
    `bpf_ntohs()` / `bpf_htons()` must be used. Our Rust code
    uses `u16::from_be_bytes` / `u16::to_be` — verify all
    16-bit and 32-bit network-order conversions are correct.
    A wrong byte order silently fails to match.
    Source: same.

12. **Multi-program dispatcher requires libxdp or our own
    chain.** Without it, only one XDP program attaches. Two
    operators each trying to attach independently overwrite
    each other. Lesson: an XDP-only product co-existing with
    Cilium or other agents must use libxdp or refuse.
    Source: `xdp-tools` libxdp.

13. **AF_XDP RX-queue binding is the #1 mistake.** The
    `advanced03-AF_XDP` README: "AF_XDP sockets bind to
    specific RX-queue IDs. Since NICs distribute traffic via
    RSS-hashing across queues, packets may never reach your
    socket." Lesson: even though we don't use AF_XDP today,
    if we ever do, ethtool flow-steering or one-socket-per-
    queue is mandatory.
    Source: `advanced03-AF_XDP/README.org`.

14. **AF_XDP copy-mode vs zero-copy.** Some drivers do
    DMA-direct to UMEM (zero-copy), most do a single copy.
    Performance gap is large but copy-mode is portable.
    Lesson: don't promise zero-copy unless you've validated
    the driver list.
    Source: same.

15. **XDP-TC metadata channel.** `advanced01-xdp-tc-interact`
    teaches that XDP can write metadata that TC reads. The
    metadata area is on the same packet buffer; both sides
    must agree on the layout. Lesson: if we ever do XDP+TC
    co-design, the metadata struct is a versioned contract.
    Source: `advanced01-xdp-tc-interact/README.org`.

## Defensive patterns worth comparing against our code

- **D1. Reset bounds after every `adjust_head/tail`.**
  We do no head/tail adjustment today. If we add encap
  (GUE/IPIP), this becomes load-bearing.
  Source: `packet02-rewriting/README.org`.

- **D2. Mount bpffs as a precondition or refuse.** Our
  loader must verify `/sys/fs/bpf` is a `bpf` mount before
  attempting pin operations.
  Source: `basic04-pinning-maps/README.org`.

- **D3. Unpin before re-pin on reload.** Our pin-path tests
  (`xdp_pin_paths.rs`) — verify they cover the reload case.
  Source: same.

- **D4. Per-CPU array for hot counters; not atomic-hash.**
  We already do this (`STATS: PerCpuArray<u64>`).
  Source: `basic03-map-counter/README.org`.

- **D5. Ethtool offload audit on attach.** Our loader
  should probe and warn (or refuse) when offloads that
  hide protocol data from XDP are enabled (`rxvlan`,
  `gro`, `lro`).
  Source: `packet01-parsing/README.org` + Cilium scripts.

## Non-goals (explicit)

- xdp-tutorial is **not** a benchmarking suite.
- xdp-tools is **not** a control plane — no service
  abstraction, no health checks, no API server.
- libxdp **does not** solve the "two agents want to own
  the netdev" problem; it solves "one operator wants to
  run two programs". Multi-tenant requires governance
  outside libxdp.
- No support for XDP **offload** (true HW) in
  xdp-tutorial; only generic/native.

## Direct comparison ideas for `div-l4`

1. Verify `xdp_pin_paths.rs` test covers reload (load,
   pin, reload, verify path-content reflects new prog).
2. Verify our loader confirms `/sys/fs/bpf` mount-type
   before pinning, with a clean error.
3. Audit byte-order conversions on all 16/32-bit network
   fields. The Rust type system doesn't enforce big-endian.
4. Verify `STAT_VLAN` is exercised by a real VLAN-tagged
   test packet, not just by parsing logic.
5. Verify `STAT_PARSE_FAIL` / `STAT_V6_EXT_UNSUPPORTED`
   tests assert the *specific* counter incremented, not
   just "some counter incremented".
6. Consider whether we want libxdp-style multi-program
   dispatch. If a user installs Cilium and our LB on the
   same netdev today, one silently wins.
