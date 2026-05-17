# L4 / XDP — handoff to `div-l4` and `div-ops`

Source files in this round:
- `audit/round-8/research/katran.md`
- `audit/round-8/research/cilium.md`
- `audit/round-8/research/l4drop.md`
- `audit/round-8/research/kernel-selftests.md`
- `audit/round-8/research/xdp-tutorial.md`
- `audit/round-8/research/aya.md`

Cross-reference (where our code lives):
- BPF program: `crates/lb-l4-xdp/ebpf/src/main.rs`
- Userspace loader: `crates/lb-l4-xdp/src/loader.rs`
- Stats export: `crates/lb-l4-xdp/src/stats_export.rs`
- Integration tests: `crates/lb-l4-xdp/tests/*.rs`

The previous (Round-7) verdict was CONDITIONAL GO. Round 8 is
adversarial. The patterns below are what to *look for in our code*
and challenge.

---

## Top-10 patterns `div-l4` should hunt

1. **Backend-index sentinel.** (Katran lesson 10.) Katran reserves
   ring-position 0 as "uninitialised" and counters drops. We
   need to check whether our `BackendEntry` / `BackendEntryV6`
   admit an "index 0 / zero-IP" value as a legitimate backend.
   If userspace pushes a partial population, `lookup` returns
   `Some(zeroed)` and we silently forward to `0.0.0.0:0`. Our
   `STAT_*` enum has no `STAT_BACKEND_UNPOPULATED` equivalent
   (file `ebpf/src/main.rs` lines 212–221).

2. **TCP-state-aware conntrack.** (Cilium lesson 7.) Our
   `CONNTRACK: LruHashMap<FlowKey, BackendEntry>` (sized
   1,000,000) is pure LRU. No RST/FIN handling. An attacker can
   replay any old segment to keep dead flows resident, pushing
   live flows out of the LRU. Even minimal "drop entry on RST"
   logic would be a measurable improvement.
   File: `ebpf/src/main.rs` line 236.

3. **Flood-rate cap on conntrack writes.** (Katran lesson 4.)
   Our hot path writes to `CONNTRACK` on every new flow. Under
   SYN flood that's 100% writes; the LRU thrashes and evicts
   legit flows. Katran's `is_under_flood()` short-circuits LRU
   inserts above `MAX_CONN_RATE = 125k/s/core`. We have no
   equivalent. The 1M `max_entries` only delays the problem.

4. **Atomic backend-table swap.** (Unimog lesson 1+2.)
   `loader.rs` userspace pushes backends — is the update atomic
   per-VIP (single map_update) or multi-key non-atomic? If
   non-atomic, mid-update flows see a half-populated table.
   We should map each VIP to a value that includes a
   {generation, entry} tuple, atomically swapped.

5. **Driver-mode silent-drop on MLX5/CX6.** (aya issue #1193.)
   Our `attach_with_fallback` ladder (Drv -> Skb) calls
   `xdp.attach()` and treats success as success. On the
   MLX5/CX6 bug, attach succeeds but `XDP_REDIRECT` silently
   drops. We have no runtime probe. Worth at least documenting
   the known-bad NIC list and emitting a WARN.

6. **CO-RE bounds-tracking footgun in Rust LLVM.**
   (aya issue #1562.) Our `ebpf/src/main.rs` does manual
   pointer arithmetic before every header access. If LLVM
   emits `scalar += pkt_ptr` instead of `pkt_ptr += scalar`,
   the verifier rejects. Verify by building with current
   stable LLVM and inspecting the verifier log for the
   "loses packet-pointer provenance" pattern. This is a
   silent-fail-at-load bug.

7. **`prefix_len = 0` admit-or-reject in `ACL_DENY_TRIE`.**
   (Cilium lesson 6, our lpm-trie use.) `LpmTrie<u32, u32>`
   sized 100,000. Userspace must reject an insert with
   `prefix_len=0` or an operator accidentally installs a
   global default-deny. Check `loader.rs` ACL push paths.

8. **NIC offload audit on attach.** (Cilium lesson 9 +
   xdp-tutorial lesson 10.) Our loader attaches and walks
   away. If LRO/GRO/rxvlan offloads are enabled, our parser
   sees coalesced or de-VLAN'd packets — `STAT_VLAN` never
   fires, `STAT_PARSE_FAIL` fires on partial-coalesced
   buffers. Need an ethtool probe at attach with WARN-or-
   refuse.

9. **Bonded-interface refusal.** (kernel selftest
   `xdp_bonding.c`.) The kernel rejects attach to a slave
   if master has a program, and vice versa. Our loader does
   not detect bond/team interfaces. A user running our LB on
   `bond0` will get attach failures that look like aya
   bugs. Either probe and document, or attach to the
   master only.

10. **Map-name fail-fast vs silent-skip.** (aya lesson 13.)
    `aya::Bpf::map("conntrack")` returns `Option`. If a
    refactor renames the BPF-side map, userspace silently
    skips that binding — stats and conntrack pinning go
    quiet without error. Audit `loader.rs` for every
    `.map(...)` call: must be `.ok_or(LoaderError::...)`.

## Top-5 cross-cutting items for `div-ops`

These are operability concerns that span loader lifecycle, metrics,
and BTF/kernel mismatch — `div-ops`' territory.

1. **XDP attach-mode lifecycle is more than "did it attach?"**
   (kernel selftest `xdp_attach.c`, aya issue #1193.) Our
   attach ladder logs the chosen mode (EBPF-2-04). Three gaps:
   (a) detach must verify post-detach that `bpf_xdp_query`
   returns zero prog-id; (b) on driver-mode silent-drop NICs
   our "attached" status is a lie; (c) reload must explicitly
   `BPF_F_REPLACE` with the old prog fd or it clobbers
   third-party programs. Verify `xdp_link_id_drop_safe.rs`
   and `xdp_attach_mode.rs` cover (a) and (c).

2. **Metrics export must use `PerCpuArray`, never iterate the
   LRU.** (kernel selftest `test_lru_map.c`.) Iterating
   `CONNTRACK` from userspace doesn't set the ref-bit, so the
   iteration races with eviction. We already do per-CPU
   stats (`STATS: PerCpuArray<u64>`, 32 entries) — good. But
   `stats_export.rs` must not, ever, also expose conntrack
   contents as a Prometheus gauge by iteration; that's a
   correctness landmine. Verify.

3. **BTF load failure in containers must be non-fatal.**
   (aya issue #1349.) Many deployments run inside
   containers without `/sys/kernel/btf/vmlinux`. Aya's
   `EbpfLoader` already treats BTF-load failure as
   non-fatal for XDP, but we should make sure our wrapper
   doesn't promote it to fatal. Add a test that explicitly
   removes the vmlinux BTF path and asserts the loader
   still works (and logs a single WARN, not a flood).

4. **Pin-path lifecycle: mount-check + reload-unpin.**
   (xdp-tutorial lessons 6 + 7.) Loader prerequisites:
   (a) `/sys/fs/bpf/expressgateway` exists and has fstype
   `bpf` (mountpoint, not just dir); (b) reload paths
   unpin the old maps before re-pin, or stale fds linger.
   `xdp_pin_paths.rs` should cover both. Mount-type check
   is documented as a precondition in
   `crates/lb/src/xdp.rs` according to the
   `DEFAULT_PIN_DIR` comment — verify it's enforced at
   runtime, not just documented.

5. **Kernel-version errno corruption.** (aya issue #1331.)
   On kernels older than ~6.8, `bpf_map_update_elem` returns
   `int` not `long`; aya reads it as `long` and produces
   bogus large-positive values where errno should be
   negative. If our code reads errno from map ops (e.g. to
   distinguish EEXIST from EAGAIN), the comparison will
   silently always-false on older kernels. Audit
   `loader.rs` for `MapError::SyscallError`-pattern
   matching. If we ever care which errno, gate by kernel
   version or use a probe.

## What changed in our actual code (Round-7 baseline)

For context, the loader/program are not green-field; here's the
shape of the state we're auditing:

- `CONNTRACK`: 1,000,000 LRU entries (v4) / 512,000 (v6).
  No state machine. (`ebpf/src/main.rs:236-241`)
- `L7_PORTS`: 256 HashMap. (`main.rs:244`)
- `ACL_DENY_TRIE`: 100,000 LpmTrie. (`main.rs:250`)
- `STATS`: 32-slot PerCpuArray (10 counters used, slots 0-9).
  (`main.rs:253` + 212-221)
- `XdpMode`: enum (Skb, Drv, Hw) — type-safe; converts to
  `XdpFlags` only at `xdp.attach()` boundary.
  (`loader.rs:289-323`)
- Attach ladder: `XdpModeChoice` enum permits
  DrvWithSkbFallback / DrvOnly / SkbOnly / HwOnly.
  (`loader.rs:458-470`)
- Userspace GPL-license assertion via secondary ELF parse
  (`object` crate) — runs in production not just tests.
  (`loader.rs:26-31`)
- Pin names are stable consts: `CONNTRACK_PIN_NAME`,
  `CONNTRACK_V6_PIN_NAME`, `L7_PORTS_PIN_NAME`,
  `ACL_DENY_TRIE_PIN_NAME`, `STATS_PIN_NAME`.
  (`loader.rs:43-56`)
- `DEFAULT_PIN_DIR = "/sys/fs/bpf/expressgateway"` —
  comment says directory ownership is set by the systemd
  unit; verify mount-type is checked.
  (`loader.rs:62`)

The previous Round 7 audit didn't (or didn't sufficiently) attack:

- Backend-index zero sentinel (#1 above).
- Conntrack as a SYN-flood eviction surface (#3).
- The attach-mode-but-no-redirect silent-drop class (#5).
- The libbpf-incompatible BTF map definitions (aya #1455) — if
  ops uses `bpftool` it sees broken introspection.
- LRO/GRO interaction with our parser (#8).

These are the most promising adversarial threads for Round 8.
