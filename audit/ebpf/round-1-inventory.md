# Round 1 — eBPF / XDP inventory

Owner: `ebpf`
Scope: `crates/lb-l4-xdp/` (userspace loader + simulation) and
`crates/lb-l4-xdp/ebpf/` (in-kernel XDP program). Plus the wiring in
`crates/lb/src/xdp.rs` that drives the optional startup attach.

This is a **discovery** document. No findings yet — open questions
are captured for the round-2 finding rounds and for the other
teammates (`sec`, `code`, `rel`, `proto`).

Toolchain note: `cargo check -p lb-l4-xdp` could not run in this
sandbox — no host `cc` linker is available, so `quote` / `proc-macro2`
build scripts fail before reaching our crate. All claims below are
sourced from static reads of the tree plus `readelf` / `bpftool` on
the committed `lb_xdp.bin`.

---

## 1. Packet path

Single XDP program, no tail calls, no AF_XDP, no perf/ringbuf
userspace channel. Userspace touches BPF maps only.

```
NIC ingress
   │
   ▼
[XDP hook, SEC("xdp"), prog name "lb_xdp"]            crates/lb-l4-xdp/ebpf/src/main.rs:324
   │
   ├─ parse Ethernet (offset 0, 14 B)                 main.rs:337
   │     dst:6  src:6  ether_type:2
   │
   ├─ if ether_type == 0x8100 (802.1Q):
   │     strip 4 B tag, inc STAT_VLAN                 main.rs:343-350
   │     (QinQ explicitly out of scope; Pillar 4b-3)
   │
   ├─ match ether_type
   │     0x0800 → IPv4 path                           main.rs:355
   │     0x86DD → IPv6 path                           main.rs:357
   │     else   → STAT_PASS, return XDP_PASS          main.rs:358-361
   │
   ├─ IPv4 (handle_ipv4)                              main.rs:369
   │     parse Ipv4Hdr; IHL ≥ 5 else Err              main.rs:370-376
   │     LpmKey::<u32>::new(32, src_addr)
   │       ACL_DENY_TRIE.get(&key)
   │         Some → STAT_DROP, return XDP_DROP        main.rs:386-390
   │     parse TCP or UDP header for ports            main.rs:393-420
   │     L7_PORTS.get(&dst_port)
   │       Some → STAT_L7, return XDP_PASS            main.rs:422-425
   │     CONNTRACK.get(&FlowKey{…})
   │       None → STAT_PASS, return XDP_PASS          main.rs:438-444  (userspace then picks backend on its own path)
   │       Some(entry) → rewrite + STAT_TX_V4 +
   │                     return XDP_TX                main.rs:445-450
   │       rewrite_v4 (rewrite MAC, dst IP, dst port,
   │         RFC 1624 incremental checksum)           main.rs:453-548
   │
   ├─ IPv6 (handle_ipv6)                              main.rs:569
   │     parse Ipv6Hdr; skip ≤ 2 ext headers
   │       (Hop-by-Hop, Routing only)                 main.rs:577-602
   │       more / unsupported → STAT_V6_EXT_UNSUPPORTED
   │                            return XDP_PASS
   │     parse TCP or UDP for ports                   main.rs:604-631
   │     L7_PORTS check (same as v4)                  main.rs:633-636
   │     CONNTRACK_V6.get(&FlowKeyV6{…})
   │       None → STAT_PASS, return XDP_PASS
   │       Some(entry) → rewrite + STAT_TX_V6 +
   │                     return XDP_TX
   │       rewrite_v6 (MAC, dst IPv6, dst port, L4
   │         checksum incl. 128-bit pseudo-header)    main.rs:660-740
   │
   └─ on any bounds-check failure (Result::Err) →
         STAT_PARSE_FAIL, return XDP_PASS             main.rs:325-332
         (never XDP_DROP on parse — intentional)

Userspace ingestion paths (no kernel→user data plane channel here):

  - lb-l4-xdp/src/loader.rs — typed accessors over CONNTRACK,
    CONNTRACK_V6, ACL_DENY_TRIE, L7_PORTS via aya 0.13. Reads/writes
    are syscall-driven (bpf_map_update_elem / lookup).
  - STATS is per-CPU; no batching/aggregation hook is exposed yet.
```

Notable absences (could be findings or could be deferred — flagged
to team-lead):

- **No XDP_ABORTED**, no XDP_REDIRECT, no AF_XDP (XSK), no
  ringbuf/perfbuf to userspace. All decisions are local.
- **No tail-call prog array / jump table**, so `BPF_MAP_TYPE_PROG_ARRAY`
  reload races are not a concern.
- **No frags / multibuf handling** — the program assumes single-frag
  packets. Jumbo frames or multi-frag XDP will hit the bounds check
  on the first byte beyond `data_end` and fall through to
  STAT_PARSE_FAIL → XDP_PASS. Documented as deliberate.
- **No `bpf_xdp_adjust_head` / `_tail` / `_meta`**. Rewrite is
  in-place and same-length; VLAN is consumed only by the offset
  arithmetic, never popped from the frame, so the egress packet
  still carries any inbound VLAN tag on XDP_TX. **Open question**
  for `proto`: is the L2 next-hop expected to accept a tagged frame
  back at us, or is single-tag VLAN strip implicit on the wire? The
  rewrite path does not strip the VLAN.

---

## 2. BPF object inventory

### Programs

| SEC                        | Symbol   | Attach type | License section | BTF section | Source                                        |
|----------------------------|----------|-------------|-----------------|-------------|-----------------------------------------------|
| `xdp` (legacy `xdp/` form) | `lb_xdp` | `BPF_PROG_TYPE_XDP` | **MISSING** | **MISSING** | `crates/lb-l4-xdp/ebpf/src/main.rs:324`       |

`readelf -h` on `crates/lb-l4-xdp/src/lb_xdp.bin` (9864 bytes):

- `Type: REL`, `Machine: Linux BPF`, 7 sections.
- Sections: `.strtab`, `.text`, `xdp`, `.relxdp`, `maps`, `.symtab`.
- **No `.BTF`** section. **No `.BTF.ext`**. **No `license`** section.
  `aya-ebpf 0.1` does not emit either automatically — the source has no
  `license!()` invocation, no `#[link_section = "license"] static
  _LICENSE: [u8;…] = *b"GPL\0";`, and no `#[link_section = ".BTF"]`.
  See open questions §7.
- Section `maps` is the **legacy `BPF_MAP_TYPE_*`-by-symbol layout**
  (not the libbpf 1.0+ `.maps` BTF layout). Confirmed empirically:
  `bpftool gen skeleton lb_xdp.bin` fails with
  `legacy map definitions in 'maps' section are not supported by
  libbpf v1.0+`. aya 0.13 still consumes this format, but cross-tool
  inspection (bpftool, BCC) is broken.
- `.text` is 0x120 bytes of compiler helpers (`memcpy`, `memset`,
  `memmove` — emitted HIDDEN). These may pull `bpf_probe_read*`-style
  kfuncs at JIT time on certain LLVM versions.

### Maps (declared in `crates/lb-l4-xdp/ebpf/src/main.rs:209–227`)

| Name             | aya type                  | Key                                | Value             | max_entries | Pinned | Eviction          |
|------------------|---------------------------|------------------------------------|-------------------|-------------|--------|-------------------|
| `CONNTRACK`      | `HashMap`                 | `FlowKey` (16 B)                   | `BackendEntry` (28 B) | 1 000 000   | No     | None (insert fails when full) |
| `CONNTRACK_V6`   | `HashMap`                 | `FlowKeyV6` (40 B)                 | `BackendEntryV6` (40 B) | 512 000     | No     | None (insert fails when full) |
| `L7_PORTS`       | `HashMap`                 | `u16` (dst port, net byte order)   | `u8` (flags)      | 256         | No     | None              |
| `ACL_DENY_TRIE`  | `LpmTrie<u32, u32>`       | `LpmKey<u32>` (CIDR prefix + IPv4) | `u32` (rule id)   | 100 000     | No     | None              |
| `STATS`          | `PerCpuArray<u64>`        | `u32` (slot index 0..32)           | `u64` (counter)   | 32          | No     | N/A (array)       |

Confirmed `readelf -s lb_xdp.bin` symbol sizes for every map entry
(each is a 28-byte aya-ebpf map descriptor; the kernel parses them
from the `maps` section).

ADR-0005 promises LRU upgrade for both conntrack maps in Pillar 4b-3
— **today both are plain HASH**, so adversarial flooding fills the
table and starves legitimate flows once 1 M (resp 512 k) is reached.
Cross-ref to `sec` round-2.

### Helpers / kfuncs used

Static read of the source — aya-ebpf wraps these as method calls;
the underlying helper IDs are listed.

- `bpf_map_lookup_elem` (CONNTRACK.get, CONNTRACK_V6.get, L7_PORTS.get,
  ACL_DENY_TRIE.get, STATS.get_ptr_mut). Available since kernel
  3.18.
- `bpf_map_update_elem` — NOT called from the BPF program. All map
  mutation is from userspace.
- **No kfuncs** (`bpf_kfunc_*`). No `bpf_dynptr_*`. No CO-RE relocations
  emitted (no `CO_RE_READ` in source). No BTF dependencies in the
  hot path.
- **No `bpf_xdp_adjust_*`** family. Rewrite is byte-write in-place.

### Verifier-relevant program shape

- One entry point, no tail calls, no function calls into program-array.
- Bounded loop: `while extensions_consumed < 2 && …` in `handle_ipv6`
  (main.rs:582–597). Hard bound 2 — verifier will unroll or accept.
- Inline functions (`#[inline(always)]`) on accessors and checksum
  helpers — keeps the call graph shallow.
- `#![deny(clippy::indexing_slicing, …)]` plus manual bounds-checked
  `ptr_at` accessor (main.rs:233–255).
- Manual `core::ptr::read_unaligned` everywhere — packed structs are
  read via `addr_of!` to avoid unaligned-reference UB.
- **8168-byte program** (`readelf -s`: symbol `lb_xdp`, size 8168).
  The 4096-insn complexity limit on pre-5.2 kernels would reject
  this — but the project's stated floor is 5.15 (1M-insn limit), so
  it should fit. Reload-time verifier logs not captured in
  `crates/lb-l4-xdp/ebpf/verifier-logs/` (the directory is empty).

---

## 3. Kernel feature matrix vs. declared minimum

Declared in `DEPLOYMENT.md:27` (single sentence):

> Linux 5.15 LTS or 6.1 LTS for the XDP data plane (when Pillar 4b
> lands the loader integration). The aya-ebpf program's verifier
> constraints are tuned for those LTS kernels.

Comparing assumed features vs. what 5.15 LTS gives you:

| Assumed feature                                | Required since | Status on 5.15 / 6.1 | Notes |
|------------------------------------------------|---------------|----------------------|-------|
| `BPF_PROG_TYPE_XDP`                            | 4.8           | OK                   |       |
| `BPF_MAP_TYPE_HASH`                            | 3.19          | OK                   |       |
| `BPF_MAP_TYPE_LPM_TRIE`                        | 4.11          | OK                   |       |
| `BPF_MAP_TYPE_PERCPU_ARRAY`                    | 4.6           | OK                   |       |
| Legacy `maps` section parsing                  | always        | OK in aya 0.13       | libbpf 1.0+ refuses — see §7 |
| `CAP_BPF` (vs. `CAP_SYS_ADMIN`)                | 5.8           | OK                   | Probe lives at `crates/lb/src/xdp.rs:40` |
| 1M-insn complexity / 32 subprog limits         | 5.2 / 5.10    | OK                   | Program is 8168 bytes |
| BTF / CO-RE                                    | n/a here      | Not used             | Source has no `CORE_READ`, ELF has no `.BTF` |
| Ring buffer (`BPF_MAP_TYPE_RINGBUF`)           | n/a           | Not used             |       |
| `bpf_xdp_adjust_*`                             | n/a           | Not used             |       |
| XDP multibuf / fragments                       | n/a           | Not used             | Program is single-frag |

Conclusion: the **5.15 floor is conservative**. Nothing in the
current program shape requires anything past 5.8. The README floor
could be lowered to **5.10 LTS** without code change. Cross-ref
`rel` round-2 for the support-matrix decision; this is a docs delta,
not a correctness issue.

Open: **no `license` section in the ELF.** The kernel requires one
to permit GPL-only helpers; without it, `bpf_map_lookup_elem` and
the rest of the non-GPL helpers used here still work (their gating
is per-helper). But aya 0.13 calls `BPF_PROG_LOAD` with the license
string from `EbpfLoader::set_license(default = "GPL")` — the loader
field, not the ELF section, is what the kernel reads. Provided aya's
default is `"GPL"`, the kernel accepts; provided it isn't, every
real-NIC load fails. **Must verify** — `loader.rs:212` just calls
`EbpfLoader::new().load(elf)` without setting a license. (Aya 0.13
source check pending — not cached in this sandbox.)

---

## 4. Driver mode

`crates/lb-l4-xdp/src/loader.rs:122–146` — `enum XdpMode { Skb, Drv, Hw }`
mapping to `XdpFlags::{SKB_MODE, DRV_MODE, HW_MODE}`.

Selected mode in startup wiring: **`XdpMode::Skb` (generic/SKB)**,
hard-coded at `crates/lb/src/xdp.rs:115`:

```rust
match loader.attach("lb_xdp", iface, XdpMode::Skb) {
```

Config surface (`lb-config::RuntimeConfig`): only `xdp_enabled: bool`
and `xdp_interface: Option<String>`. There is **no config knob for
driver mode**, **no fallback** Drv → Skb, **no probe** of NIC
support. SKB mode runs on every NIC but loses the entire performance
benefit of native XDP. Cross-ref `rel` round-2: a production L4 LB
without native-mode attach is not "Katran-class".

Disable-on-failure logic (`crates/lb/src/xdp.rs:124–131`):

- Cap probe failure → log + continue without XDP.
- ELF parse failure → log + continue without XDP.
- `kernel_load` failure (verifier reject) → log + continue.
- `attach` failure (driver refuses, iface missing) → log + continue.

This is the right shape — the L4 fast path is always optional and
the L7 stack is the ground truth. Continuity over correctness.

---

## 5. Loader / userspace

### aya 0.13 usage (`crates/lb-l4-xdp/src/loader.rs`)

- `EbpfLoader::new().load(elf)` (loader.rs:212) — parse + map create.
  Does NOT call `set_license`, `set_global`, or `btf_object`. License
  defaults; BTF feature-detection is whatever aya 0.13 does (auto-detect
  /sys/kernel/btf/vmlinux if absent in the ELF).
- `xdp.load()` (loader.rs:246) — kernel `BPF_PROG_LOAD`.
- `xdp.attach(ifname, mode.to_flags())` (loader.rs:275) — netlink
  attach. The `XdpLinkId` returned is **dropped**. Comment claims aya
  keeps the link alive via `self.ebpf`. Verification needed — if aya
  detaches on `XdpLinkId` drop, our program loses its attach the
  instant `attach()` returns. **Cross-ref `code` round-2: confirm
  aya 0.13's `Xdp::attach` keeps the link in the `Xdp` handle.**
- Typed map accessors: `conntrack_map`, `conntrack_v6_map`, `acl_trie`,
  `take_map` (loader.rs:286–342). `Pod` impls are claimed safe
  (loader.rs:51–53, 75–76, 96–97, 118–119). These compare byte-for-byte
  against the ELF — drift between userspace and ebpf struct layouts
  is caught at accessor construction.

### CAP_BPF probe (`crates/lb/src/xdp.rs:39–55`)

```rust
caps::has_cap(None, CapSet::Effective, Capability::CAP_BPF)
caps::has_cap(None, CapSet::Effective, Capability::CAP_NET_ADMIN)
```

- Probes the **effective** set on `None` (current thread).
- Missing CAP_BPF or CAP_NET_ADMIN → tracing warn + return None.
- ProbeError (e.g. `/proc/self/status` unreadable) → warn + return None.
- Does NOT check `CAP_SYS_ADMIN` for kernels < 5.8 where CAP_BPF
  alone is insufficient. **Cross-ref `sec` round-2.**

### Pinned-map filesystem paths and modes

- **None.** There is no call to `Map::pin`, `Program::pin`,
  `Link::pin`. No `bpffs` mount expected. No pinned paths declared
  in any config or constant.
- Consequence: every reload of the binary creates fresh maps. Existing
  conntrack state is lost on restart. **This is mentioned nowhere in
  the reload docs.** Cross-ref `rel` round-2: reload-zero-drop test
  must account for cold conntrack on restart.

### Lifecycle on shutdown

`crates/lb/src/main.rs:977–982`:

```rust
let _xdp_loader = if let Some(rt) = config.runtime.as_ref() {
    xdp::try_attach_xdp(rt)
} else { None };
```

Held in a local, never reassigned, never explicitly dropped. Drop
runs at the end of `async_main` along with every other local. Aya's
`Ebpf` drop should detach the XDP program — provided the link is
kept inside `self.ebpf` (see §5 above). No timeout, no graceful
detach, no flush of in-flight maps. Cross-ref `rel`.

---

## 6. Reload semantics

Hot reload story today:

1. The **userspace** Maglev / conntrack hot-swap (`HotSwapManager`,
   `src/lib.rs:349–409`) is exercised by tests — it preserves
   conntrack entries across backend-set changes.
2. The **BPF program** itself is not hot-reloaded by any code path in
   the tree. SIGHUP / config reload (covered by `crates/lb`) updates
   userspace state but never re-attaches the XDP program. The aya
   `Xdp::attach` happens exactly once at process start.
3. There is **no `BPF_LINK_UPDATE`** call. There is **no
   detach-then-attach** call. No traffic-loss window is documented
   because the program is never replaced live.
4. Map updates *during* reload are atomic per-entry through aya's
   `HashMap::insert` (single `bpf_map_update_elem` syscall each).
   Batch update is not used.

Open: if the BPF program source itself changes (new feature, bugfix),
the only path to deploy is a process restart, which loses every
conntrack entry (see §5: no pinning). **Cross-ref `rel`.**

---

## 7. Open questions for teammates

For **`sec`**:

1. `crates/lb-l4-xdp/ebpf/src/main.rs` has **no `license` ELF section**
   and the userspace loader does not call `EbpfLoader::set_license`.
   Confirm whether aya 0.13's default license string is `"GPL"` (the
   only string the kernel accepts as GPL-compatible). If not, the
   real-NIC load will fail and the warning path
   (`crates/lb/src/xdp.rs:111`) will fire silently in production.
2. `caps::has_cap(None, CapSet::Effective, …)` reads `/proc/self/status`.
   On kernels < 5.8 the program needs CAP_SYS_ADMIN, which is not
   probed — the attach will fail with EPERM and we'll log
   `xdp disabled — attach failed`. Should the probe also try
   CAP_SYS_ADMIN as a fallback?
3. **bpffs perms**: nothing is pinned. If Pillar 4b-3 lands map
   pinning, the bpffs mount, mode, and uid/gid are not yet specified
   anywhere. Standard Cilium-style pinning at `/sys/fs/bpf/expressgateway/`
   needs design.
4. Verifier-bypass surface: every `unsafe` block in
   `crates/lb-l4-xdp/ebpf/src/main.rs` is a `ptr_at` read or
   `read_unaligned` of a packed field after a bounds check. No raw
   pointer arithmetic outside `ptr_at` / `ptr_at_mut`. No `usize`
   casts of `data_end` that bypass the bounds check.

For **`code`**:

1. `loader.rs:275` drops `XdpLinkId`. Comment claims aya keeps the
   link alive — please verify against aya 0.13 source. If wrong, the
   attach detaches the moment `attach()` returns.
2. `loader.rs:286–342` returns aya `Map`/`LpmTrie` wrappers borrowed
   from `&mut self.ebpf`. This serialises all map access through the
   `XdpLoader` handle. For a control plane that runs as a Tokio task,
   this borrow is a per-task lock. Confirm whether anything in
   `crates/lb` mutates the maps concurrently from multiple tasks —
   if so, `XdpLoader` needs `Arc<Mutex<…>>` or per-map ownership via
   `take_map`.
3. The `Pod` impls for `FlowKey`, `BackendEntry`, `FlowKeyV6`,
   `BackendEntryV6` (loader.rs:53, 76, 97, 120) are `unsafe impl`s
   with comments — please cross-check against the ebpf side struct
   layouts (`ebpf/src/main.rs:143–195`) for byte-by-byte equality,
   including padding bytes. The userspace `pad` fields are public but
   never zeroed by the constructors I can find — uninitialised
   padding bytes will land in BPF map keys and break hash collisions.
4. The committed `lb_xdp.bin` is 9864 bytes — larger than ADR-0004's
   stated "~3 KB" and ADR-0005's stated "3 kB". Either ADR text is
   stale, or the committed blob has accreted debug info. `readelf`
   reports the `.text` is 0x120 (288) and the `xdp` section is 0x1fe8
   (8168); the rest is symbols + strings + relocs. Worth a refresh
   pass on the ADRs.

For **`rel`**:

1. **No map pinning** — restart drops every conntrack entry. The
   reload-zero-drop test claims zero drops, but BPF state is cold
   after restart. Confirm whether the test exercises the XDP path or
   only the userspace L7 path.
2. **SKB mode only** — `crates/lb/src/xdp.rs:115` hard-codes
   `XdpMode::Skb`. No Drv-mode probe, no fallback chain. For a
   data-plane LB this is a perf cliff; needs a config knob and a
   capability/feature probe.
3. **No verifier matrix** — `crates/lb-l4-xdp/ebpf/verifier-logs/` is
   empty. The promised `xtask xdp-verify` from ADR-0005 §"Follow-ups"
   does not exist. Without it, every kernel-version change is a
   prod-blind risk.
4. Conntrack map type is **HASH not LRU_HASH**. Under adversarial
   flow churn the map fills, new inserts fail (the BPF program never
   inserts — userspace does — so failures land in userspace), and
   STAT_PASS climbs while new flows bypass the fast path. No
   alerting on this.

For **`proto`**:

1. **VLAN strip semantics**: the ingress path consumes one 802.1Q
   tag by offset arithmetic only — the L2 header is never popped, so
   on `XDP_TX` the egress frame still carries the inbound VLAN tag.
   Is this the intended on-the-wire shape, or should the rewrite
   path strip the tag? `rewrite_v4` / `rewrite_v6` start the L2
   write at offset 0 (`ptr_at_mut::<EthHdr>(ctx, 0)`), not
   `l3_offset - 14`, so the VLAN tag is preserved untouched.
2. **IPv6 extension headers**: we accept at most two of
   `IPPROTO_HOPOPTS` or `IPPROTO_ROUTING`. Anything else (Fragment,
   AH, ESP, Destination Options) → STAT_V6_EXT_UNSUPPORTED → XDP_PASS.
   Confirm this matches the project's IPv6 promise (especially:
   Fragment headers are emitted by stacks under MTU pressure).
3. **UDP checksum == 0**: IPv4 path leaves it alone (RFC 768 permits
   UDP/IPv4 with zero checksum); IPv6 path also leaves it alone but
   notes the RFC requires non-zero. The comment says "we only
   rewrite if one was already computed", but the kernel and many NICs
   will drop UDP/IPv6 with zero checksum, so the unchanged zero is
   not a portability hazard — it just means we deliver an
   already-broken packet. Confirm.
4. **TCP options**: not parsed, not rewritten. Pillar 4b deferred
   work. Cross-ref ADR-0004 §"Follow-ups".
5. **Maglev table size must be prime**: enforced at userspace
   construction (`is_prime` in `src/lib.rs:188`). The BPF side does
   not see the Maglev table — userspace pushes pre-resolved
   `(flow → backend)` mappings into CONNTRACK. So the BPF program
   never picks a backend on first-packet; it always XDP_PASS-es to
   userspace. **First-packet latency is therefore one full userspace
   round trip** — confirm this is the documented design and not a
   gap.

---

## Files inventoried

- `crates/lb-l4-xdp/Cargo.toml`
- `crates/lb-l4-xdp/build.rs`
- `crates/lb-l4-xdp/src/lib.rs` (630 lines, sim + types)
- `crates/lb-l4-xdp/src/loader.rs` (428 lines, aya wrapper)
- `crates/lb-l4-xdp/src/sim.rs` (417 lines, deeper sim — not opened in
  detail this round; functional spec only)
- `crates/lb-l4-xdp/src/lb_xdp.bin` (9864 B, committed BPF ELF)
- `crates/lb-l4-xdp/tests/real_elf.rs` (kernel-free ELF parse smoke)
- `crates/lb-l4-xdp/ebpf/Cargo.toml`
- `crates/lb-l4-xdp/ebpf/rust-toolchain.toml` (nightly-2026-01-15)
- `crates/lb-l4-xdp/ebpf/src/main.rs` (747 lines, real aya-ebpf prog)
- `crates/lb/src/xdp.rs` (143 lines, startup attach + cap probe)
- `crates/lb/src/main.rs:977–982` (lifecycle hold)
- `crates/lb-config/src/lib.rs:75, 79, 535` (config surface)
- `scripts/build-xdp.sh` (BPF toolchain driver)
- `docs/decisions/ADR-0004-ebpf-framework.md`
- `docs/decisions/ADR-0005-bpf-map-schema.md`
- `docs/decisions/ebpf-toolchain-separation.md`
- `DEPLOYMENT.md` (kernel + capabilities)
