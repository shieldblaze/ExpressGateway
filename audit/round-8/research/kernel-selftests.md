# Linux kernel selftests for BPF / XDP

Reference: `https://github.com/torvalds/linux/tree/master/tools/testing/selftests/bpf`
The kernel's *own* regression tests for the BPF subsystem. These are
the gold standard for "what could go wrong with XDP that you didn't
anticipate" because each test usually corresponds to a kernel bug
that shipped to production once.

## Architecture summary

Three subdirectories matter for our scope:

- `progs/` — BPF programs (in C) compiled at test time
- `prog_tests/` — userspace harness functions, one per scenario
- `verifier/` — denser verifier-only test snippets

The tests run on real kernels; they exercise actual syscalls
(`BPF_PROG_LOAD`, `BPF_LINK_CREATE`, `BPF_MAP_*`) and check
behaviour against expected error codes. Most tests build veth pairs
in fresh netns to avoid touching the host.

When the kernel verifier rejects something, the test asserts
*exactly which error* the verifier returned, so a kernel change
that subtly relaxes the rules produces a test failure that
documents the change. This is the closest thing to a normative
"what XDP is allowed to do" reference.

## Lessons learned

1. **`xdp_attach.c` enforces `XDP_FLAGS_REPLACE` semantics.** The
   test verifies that replacing an attached XDP program *requires
   passing the old program's fd*; an unconditional replace is
   rejected. Lesson: a loader that "just attaches" without
   `BPF_F_REPLACE` will silently clobber another tenant's
   program. Our `loader.rs` must check whether something is
   already attached and use replace semantics.
   Source: `tools/testing/selftests/bpf/prog_tests/xdp_attach.c`.

2. **`xdp_link.c` forbids prog-style attach when a link is
   active.** Once a BPF link is taken on an interface, the
   legacy `setlink` netlink attach must fail. A loader that
   tries both paths in the wrong order can leave the interface
   in a state where the old program persists. Lesson: link
   semantics are exclusive with netlink-prog semantics.
   Source: `prog_tests/xdp_link.c`.

3. **`xdp_link.c` also checks "interface index goes to zero
   after detach".** This is a sanity check we should be doing
   in our integration test: query the interface and assert
   no program is attached, not just "we returned from detach".
   Source: same.

4. **`xdp_bonding.c` — XDP cannot attach to bond master AND
   slave.** The kernel explicitly rejects this combination.
   If our loader supports bonded interfaces it must detach
   from any slave with a program before attaching to master,
   and refuse to attach to a slave if the master has one.
   Source: `prog_tests/xdp_bonding.c`.

5. **`xdp_cpumap_attach.c` — program-type fragmentation
   matters.** A program is either fragment-capable or not;
   you cannot mix them in a CPUMAP. The kernel returns a
   non-obvious error. If our build chain ever produces a
   "frags" variant we must consistently attach the same kind.
   Source: `prog_tests/xdp_cpumap_attach.c`.

6. **`xdp_devmap_attach.c` — `BPF_XDP_DEVMAP` programs
   cannot attach to ifaces directly.** They live in DEVMAP
   entries. A loader that confuses prog-types loads
   successfully but never receives packets. Lesson: validate
   program-type matches its intended attach point.
   Source: `prog_tests/xdp_devmap_attach.c`.

7. **`xdp_adjust_tail.c` — tailroom is architecture-dependent.**
   Max grow is 320 on x86, 384 on POWER, 512 on s390x. A
   loader assuming "1500 bytes of tailroom" will silently drop
   on POWER. Lesson: never hard-code packet-size budgets in
   encap math; the kernel knows.
   Source: `prog_tests/xdp_adjust_tail.c`.

8. **`xdp_adjust_tail.c` also confirms newly allocated head
   bytes are zeroed.** The kernel zero-fills `bpf_xdp_adjust_head`
   space. But many XDP programs assume this and then *don't*
   zero-fill on their own; if a kernel regression breaks this,
   you leak old packet data into new headers. Lesson: defense
   in depth, zero your own headers.
   Source: same.

9. **`lru_bug.c` — `prealloc_lru_pop` MUST init value.** The
   kernel regression test for CVE-class bug: preallocated LRU
   entries were re-used with stale data. Lesson: if we read an
   LRU value, we cannot assume it's been initialised in
   userspace (when the BPF program is the only writer, this is
   fine, but it's worth documenting).
   Source: `prog_tests/lru_bug.c`.

10. **`test_lru_map.c` — ref-bit semantics.** LRU maps have
    two distinct lookup paths: "datapath lookup" sets the ref
    bit, "syscall lookup" does not. Userspace iteration to
    drain stats does *not* protect entries from eviction. A
    naive `for entry in map { read }` from userspace can race
    with the BPF program evicting entries it had marked
    "hot". Lesson: stat-export should iterate over a stable
    `PerCpuArray`, not over the LRU.
    Source: `test_lru_map.c`.

11. **`test_lru_map.c` — `BPF_F_NO_COMMON_LRU` flag changes
    behaviour fundamentally.** Per-CPU eviction (each CPU has
    its own LRU list) vs common LRU has different worst-case
    behaviour under skewed traffic. Lesson: pick the flag at
    map creation, document the choice; default is common LRU.
    Source: same.

12. **`xdp_metadata.c` — dev-bound programs cannot be in
    devmap.** Hardware-offload-bound XDP programs are
    explicitly different and the kernel enforces this. If
    our future plans include `XdpMode::Hardware`, the maps
    we put it into must be different from `XdpMode::Driver`.
    Source: `prog_tests/xdp_metadata.c`.

13. **`xdp_metadata.c` — VLAN metadata only on supporting
    drivers.** The metadata kfuncs return `-EOPNOTSUPP` on
    drivers that don't implement them. A BPF program that
    blindly trusts the metadata gets uninit data on
    unsupported drivers (in unprivileged cases) or just a
    failed call. Lesson: probe driver caps at load.
    Source: same.

14. **`btf_dedup_split.c` — split-BTF deduplication is
    fragile.** When loading a BTF that references base
    kernel BTF, the dedup must collapse identical types.
    A bug here means a program loads on one kernel and not
    another. Lesson: aya's BTF loader runs this gauntlet
    every time you load; failures are kernel/aya version
    mismatches, not your code.
    Source: `prog_tests/btf_dedup_split.c`.

15. **`tc_redirect.c` — `bpf_redirect_neigh` vs
    `bpf_redirect_peer`.** Two different semantics: `_neigh`
    re-runs the neighbor cache for the new dst; `_peer`
    crosses netns at the veth pair. Wrong choice silently
    forwards to wrong egress. Worth knowing if our future
    plans include cross-namespace forwarding.
    Source: `prog_tests/tc_redirect.c`.

## Defensive patterns worth comparing against our code

- **D1. Always test detach as part of the integration test.**
  Selftests do `attach -> verify -> detach -> verify-zero`.
  Our test suite has `xdp_attach_mode.rs` and
  `xdp_link_id_drop_safe.rs` — verify the latter does a
  post-drop query and asserts no program remains.
  File: `tools/testing/selftests/bpf/prog_tests/xdp_attach.c`.

- **D2. Probe attach mode capability before commit.** The
  selftests use `bpf_xdp_query` to confirm the mode; our
  loader's "attach-fallback ladder" (per EBPF-2-04 in
  audit notes) is the same idea. Verify the ladder
  *logs* which mode it landed on (EBPF-2-04 says it
  does — confirm).
  File: `xdp_attach.c`.

- **D3. Per-CPU array for stats, not LRU.** Lesson 10.
  Our `STATS: PerCpuArray<u64>` is correct shape.
  File: `prog_tests/lru_bug.c`.

- **D4. Asserts on max-tailroom platform-dep.** Lesson 7.
  Our encap path must use `bpf_xdp_adjust_tail` return
  value, not assume a constant.
  File: `xdp_adjust_tail.c`.

- **D5. Refuse to attach when an incompatible attach
  already exists.** The selftest exercises every
  combination of "prog already there, link tries to
  attach". Our loader should mirror this.
  File: `xdp_link.c`.

## Non-goals (explicit)

- selftests don't test **performance**, only correctness.
- selftests don't test **multi-tenant attach conflict
  resolution beyond the kernel's enforcement**; user-space
  policy is out of scope.
- selftests don't test **graceful kernel-version
  fallback**; each test is gated on the kernel feature
  flags and skipped if unsupported.
- selftests don't test **NIC driver bugs** — they use
  veth or loopback. The MLX5/CX6 bug (aya #1193) won't
  show up in selftests.

## Direct comparison ideas for `div-l4`

1. Run our `xdp_link_id_drop_safe.rs` and verify it
   includes a post-drop `bpf_xdp_query` check.
2. Run `xdp_attach_mode.rs` and verify it tests all
   three modes (Skb, Driver, default-fallback) and
   that the chosen mode is *logged*.
3. Survey our LRU map sizing: we use `LruHashMap`
   (`CONNTRACK`, `CONNTRACK_V6`) — is it common LRU or
   per-CPU? If common, document the eviction model.
4. Verify our stat-iteration in `stats_export.rs`
   uses the per-CPU array, not LRU iteration.
5. Verify our encap path (if any — we don't appear to
   do encap currently) does not assume a fixed
   tailroom number.
6. Add a `xdp_bonding`-style test if we ever support
   bonded interfaces.
