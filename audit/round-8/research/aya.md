# aya-rs / aya — Rust eBPF framework

Reference: `https://github.com/aya-rs/aya`
This is what our userspace loader depends on (`aya`, `aya-obj`,
`aya-ebpf` in the BPF crate). The lessons here are what bites
*our* code, not theoretical.

## Architecture summary

- `aya-ebpf` — `no_std` crate used inside the BPF program; maps,
  helpers, attribute macros (`#[xdp]`, `#[map]`).
- `aya-obj` — kernel-free ELF inspector (parses an object,
  enumerates programs/maps/BTF/relocations).
- `aya` — userspace, Linux-only: `EbpfLoader` builder, program
  attach/detach, map handles, link management.

The userspace flow:

1. `EbpfLoader::new()` reads object bytes.
2. `set_btf()` / autodetect BTF from `/sys/kernel/btf/vmlinux`.
3. `load(bytes)` -> parses ELF, creates maps, performs CO-RE
   relocations using BTF, loads each program via
   `BPF_PROG_LOAD`.
4. `Bpf::program_mut("name")` -> downcast to `Xdp` etc.
5. `Xdp::attach(iface, XdpMode)` -> tries `bpf_link_create`,
   falls back to legacy netlink if `EINVAL`.
6. Returned `XdpLinkId` — drop semantics: detach when dropped
   unless pinned.

CO-RE relocations are the load-time step that makes a single
`.o` work across kernel versions: each access to a kernel
struct field is patched to the right offset based on the running
kernel's BTF.

## Lessons learned

1. **`XdpFlags` (bitset) was unsafe; replaced by `XdpMode` enum.**
   The old API allowed combining mutually-exclusive flags
   (`Skb | Driver`). PR #1545 moved to a type-safe enum. Lesson:
   our code uses `XdpMode` (correct), but anything that
   constructs `XdpFlags` directly from a `u32` is a regression
   trap. Verify no code path bypasses the enum.
   Source: aya PR #1545.

2. **`TryFrom<FdLink>` for UProbe/KProbe/TracePoint was
   checking wrong link types.** PR #1532 found that three
   probe types checked `BPF_LINK_TYPE_TRACING` /
   `BPF_LINK_TYPE_KPROBE_MULTI` when they should have checked
   `BPF_LINK_TYPE_PERF_EVENT`. Latent bug for ~2 years.
   Lesson: any `TryFrom<FdLink>` for XDP we rely on must
   check `BPF_LINK_TYPE_XDP`. Worth grep-checking.
   Source: aya PR #1532.

3. **Rust LLVM emits `scalar += pkt_ptr` (verifier hostile);
   clang emits `pkt_ptr += scalar`.** Aya issue #1562
   documents that the BPF verifier preserves pointer
   provenance only when the pointer is the LHS of the
   addition. Rust's LLVM backend sometimes generates the
   wrong order, leading to "invalid access to packet"
   verifier rejections for code that's logically correct.
   Lesson: any packet-offset arithmetic in our BPF program
   that uses a non-trivial pattern can fail the verifier
   for reasons unrelated to our code; the fix is sometimes
   to rewrite the addition in a specific order or use
   `ptr_at` helpers.
   Source: aya issue #1562.

4. **`bpf_csum_diff` helpers don't work as expected.**
   Issue #1402: the checksum helpers in `aya-ebpf` have
   ergonomic issues that lead to wrong checksums. Lesson:
   if we ever do incremental checksum (we have helpers like
   `csum16_update` in `ebpf/src/main.rs` — these are
   handwritten, not aya helpers, which is good), verify by
   round-tripping a packet and checking it accepted by a
   real kernel.
   Source: aya issue #1402.

5. **Errno corruption on kernels where `map_update_elem`
   return-type was `int` not `long`.** Issue #1331: on
   kernel 6.1 (and earlier), the return was `int`; on 6.8
   it's `long`. Aya treating it as `long` on the older
   kernel reads `i64=4294967279` for `EEXIST(-17)`. Lesson:
   any kernel-version-spanning fleet will see *wrong* errno
   values from our BPF program; if we react on errno (e.g.
   "if EEXIST, increment race counter"), the comparison
   silently always-false on older kernels.
   Source: aya issue #1331.

6. **MLX5/ConnectX-6 XDP_REDIRECT in DRV_MODE silently
   drops packets.** Issue #1193 has no fix; workaround is
   SKB_MODE. Lesson: our loader's "attach-fallback ladder"
   from EBPF-2-04 (per `audit/STATE`) tries DRV first
   then SKB. On affected NICs, DRV "succeeds" but
   `XDP_REDIRECT` doesn't egress. Need a *runtime probe*
   (send a packet, verify it arrives at peer) not just
   "attach returned OK".
   Source: aya issue #1193.

7. **BTF map definitions emitted by aya are NOT
   libbpf-compatible.** Issue #1455: extra wrapper struct
   layers (`__0` → `value`) break cilium/ebpf and other
   readers. Lesson: anyone who tries to introspect our
   pinned maps with a libbpf-based tool will fail. If
   ops want to use `bpftool` for live introspection, they
   need a version that handles aya's BTF.
   Source: aya issue #1455.

8. **BTF load fails inside Docker / Google COS.** Issue
   #1349 documents that `BPF_BTF_LOAD` syscall fails in
   containerized environments even when the program
   itself loads fine. Lesson: our loader must handle BTF-
   load-failed-but-program-loads gracefully (warning, not
   error), or our container deployments break. Aya's
   `EbpfLoader` already does this: BTF failures are only
   fatal for Extension/FEntry/FExit/LSM/BtfTracePoint
   program types, not for XDP.
   Source: aya issue #1349; aya `bpf.rs` BTF handling.

9. **`PinnedLink` Drop does NOT detach the program.**
   `define_link_wrapper!` macro generates a Drop that
   detaches *unless* the link is pinned. Once pinned, the
   program persists across process restart. Lesson: a
   pinned XdpLink is intentionally "leaked" from the
   process's POV — that's the whole point of pinning.
   Our `xdp_link_id_drop_safe.rs` test should distinguish
   the two cases.
   Source: aya `aya/src/programs/links.rs`.

10. **Macro-generated `Drop` swallows detach errors with
    `let _unused`.** From aya's link wrapper macro: a
    failure to detach during Drop is silently dropped.
    Lesson: a netdev that's gone (interface removed) will
    return an error on detach; we won't see it in logs.
    Our metrics should expose "detach during drop failed"
    if we want operability.
    Source: aya `links.rs` macro impl.

11. **`EbpfLoader` allows-unsupported-maps as an
    opt-in.** By default the loader fails on unknown map
    types. `allow_unsupported_maps()` lets you load
    BPF-only maps that userspace can't touch. Lesson: if
    a future kernel adds a map-type aya doesn't know
    yet, our loader fails to load entirely. Worth
    enabling this flag if we care about forward-compat.
    Source: aya `bpf.rs`.

12. **`set_global` constants must be set *before* `load()`.**
    Globals are patched into the ELF at load time via
    relocations. Setting them after `load()` has no effect
    silently. Lesson: any constant we pass via aya's
    global mechanism must be set in the right order; a
    refactor that moves the call is a silent bug.
    Source: aya `EbpfLoader::set_global`.

13. **`aya::Bpf::map()` returns `Option`, not error.** If
    a map name doesn't exist in the loaded program, you
    get `None`. Lesson: if our BPF map gets renamed
    (e.g. `conntrack` -> `ct`), userspace sees `None`
    and we get no stats — and no error.
    Source: aya `bpf.rs`.

14. **Aya does not zero-init pinned-map values on
    re-load.** If a map is pinned with prior content and
    the loader re-binds, the old values are still there.
    Lesson: pin-aware reload must explicitly clear or
    explicitly preserve. Our pin paths test should
    document which it chose.
    Source: aya `Bpf::take_map` behaviour.

15. **`load_from_bytes` does NOT verify GPL license.**
    Aya parses the `license` ELF section but is happy
    to pass anything to the kernel. The kernel rejects
    non-GPL programs that use GPL-only helpers, *at
    load time*, with a non-obvious error. Lesson: our
    SEC-2-12 fix that asserts `GPL\0` in userspace
    before kernel-load is the right pattern — fail
    fast, fail loud.
    Source: aya `aya-obj/src/btf.rs` (license parse).

## Defensive patterns worth comparing against our code

- **D1. Type-safe `XdpMode` (not bitset).** Already
  using `XdpMode`. Check that no code path constructs
  `XdpFlags` directly.
  File: `crates/lb-l4-xdp/src/loader.rs`.

- **D2. Runtime probe after attach.** Lesson 6. Our
  attach-fallback ladder should not stop at "attach
  succeeded"; for `XdpMode::Driver` on suspect NICs we
  should optionally send a probe packet.
  File: `loader.rs` attach path.

- **D3. Userspace GPL-license assertion.** Lesson 15.
  We do this (SEC-2-12 fix, `LICENSE: [u8; 4] = *b"GPL\0"`
  in BPF + userspace check). Verify the test
  `loader_license_assert.rs` covers both
  presence and value.
  File: `tests/loader_license_assert.rs`.

- **D4. Map-name absence is fatal, not silent.**
  Lesson 13. Our loader should call `Bpf::map("name")
  .ok_or(...)` and error rather than silently skip
  stats binding.
  File: `loader.rs` stats binding.

- **D5. `PinnedLink` Drop semantics test.** Lesson 9.
  Our `xdp_link_id_drop_safe.rs` test should cover
  both unpinned (detach on drop) and pinned (persist
  on drop) cases.
  File: `tests/xdp_link_id_drop_safe.rs`,
  `tests/xdp_pin_paths.rs`.

## Non-goals (explicit)

- aya **does not** solve the multi-program-on-one-netdev
  problem; that's libxdp.
- aya **does not** support non-Linux targets at userspace
  (uses `libc`/syscalls). Cross-compile-from-mac for
  Linux is supported; running on mac is not.
- aya **does not** ship a verifier emulator; you find out
  about verifier failures at `BPF_PROG_LOAD` time, not at
  Rust compile time. Issue #1562 is exactly this gap.
- aya **does not** auto-clean orphaned pins; that's the
  loader's responsibility.
- aya **does not** support all kernel features
  immediately on release — see issue #1540 for Netkit
  attach, #1413 for bloom-filter maps.

## Direct comparison ideas for `div-l4`

1. Grep our codebase for `XdpFlags`; only `XdpMode`
   should appear at the API surface.
2. Verify our attach ladder logs the chosen mode at
   INFO (per EBPF-2-04) and the rejected modes at WARN.
3. For each map we register, verify userspace fails-fast
   if the map name isn't found. A renamed map should be
   a load error, not silent stat-loss.
4. Verify `xdp_link_id_drop_safe.rs` distinguishes
   pinned vs unpinned Drop semantics.
5. Verify the BTF-load failure path is non-fatal for
   XDP programs (our deployments may run inside
   containers without `/sys/kernel/btf/vmlinux`).
6. Verify our userspace GPL-license assertion runs in
   production (not just in tests) — SEC-2-12.
7. For any incremental-checksum code we have, write a
   real-packet round-trip test that loads on a kernel
   and verifies the checksum is accepted.
8. Consider an aya-version pin: aya's API has had
   breaking changes (`XdpFlags` -> `XdpMode`, link
   wrappers, etc.). Pinning a known-good minor version
   avoids surprise breakage on `cargo update`.
