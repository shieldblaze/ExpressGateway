# S47 — XDP / eBPF L4 datapath review

Scope: `crates/lb-l4-xdp/**` (loader, lib, nic_compat, sim, stats_export,
netlink_xdp, bpffs, the out-of-workspace eBPF crate, and every test),
`crates/lb/src/xdp.rs`, plus the production wiring in `crates/lb/src/main.rs`
and the observability surface in `crates/lb-observability/src/xdp_metrics.rs`.

Method: read-only. No `cargo` command was run and no BPF program was loaded
(2 vCPU / 7 GB box, no CAP_BPF, five other agents' work live on the tree).
Ground truth for the map ABI was taken from the committed object itself by
decoding its ELF section headers, symbol table and legacy `maps` section with
`readelf` + a Python struct decoder — not from either side's source.

---

## 0. What BPF artefacts actually exist, and what that makes verifiable

| Artefact | Path | State |
| --- | --- | --- |
| eBPF source (Rust / aya-ebpf 0.1) | `crates/lb-l4-xdp/ebpf/src/main.rs` (948 L) | present; reviewed line by line |
| Compiled object | `crates/lb-l4-xdp/src/lb_xdp.bin` (35 864 B) | ELF64 LSB relocatable, eBPF, not stripped |
| Build script | `scripts/build-xdp.sh` | present; **never invoked by CI** |
| Verifier baselines | `audit/ebpf/verifier-logs/{5.15,6.1,6.6}.log.committed` | **placeholders** (`HARNESS-CAPTURED-PENDING-CI-RERUN`) |
| Verifier baseline | `audit/ebpf/verifier-logs/7.0.log.committed` | real capture, kernel 7.0.0-1004-aws |

Object anatomy (`readelf -S` / `-sW`): 12 sections — `.text` 0x120,
`xdp` 0x2990, `.relxdp` (43 relocs), `license` 4 B, **`maps` 0xe0 = 8 × 28 B
(legacy `bpf_map_def`, not BTF `.maps`)**, `.BTF` 0x1c32, `.BTF.ext` 0x1dd8.
Symbols: `lb_xdp` FUNC 10 640 B (= 1330 insns), 8 map OBJECTs, `LICENSE`,
and `memcpy`/`memset`/`memmove` as hidden FUNCs in `.text` (BPF-to-BPF calls).

**Verifiable from source, and verified here:** the complete map ABI (both
planes plus the object's own declarations), endianness of every packet field,
bounds-check structure, checksum math, per-CPU aggregation, netlink framing,
the attach/detach state machine, and the full call graph of who writes which
map in production.

**NOT verifiable from source, and therefore reported as gaps rather than
assertions:** per-branch runtime behaviour of the BPF program (nothing in the
tree executes it against a packet — see EBPF-S47-09), and verifier acceptance
on 5.15 / 6.1 / 6.6 (see EBPF-S47-08). Verifier acceptance on the CI runner
kernel *is* genuinely covered by a real `BPF_PROG_LOAD`.

### Relationship to `rt-unsafe.md` (already committed)

I **confirm** every soundness verdict in `rt-unsafe.md` §1.3–§1.6. Its
hand-computed Pod layouts (`FlowKey` 16, `BackendEntry` 24, `FlowKeyV6` 40,
`BackendEntryV6` 36, `BackendTable` 3088) match, byte for byte, what I
independently decoded out of the object's `maps` section — two derivations
from different sources agreeing is the strongest evidence available without a
kernel. No contradiction on any of the 73 sites.

Two scoping notes, neither a contradiction:

1. §1.6 rules the 60 eBPF sites "SOUND, verifier-backed". That is correct for
   memory safety and I agree. The verifier does not adjudicate *ordering*:
   EBPF-S47-06 below is a case where a soundly-`unsafe` block leaves the
   packet mutated on an error path the verifier is perfectly happy with. This
   is exactly the "routing logic as opposed to memory safety" that §6 of
   `rt-unsafe.md` hands to this lane.
2. §1.4 says of `recv`: "cannot exceed the buffer, so the truncate is
   in-range" — correct, and I confirm there is no memory-safety issue. The
   *datagram* can still exceed 32 KiB and be silently truncated by the kernel,
   which is an availability defect, not an unsafety one (EBPF-S47-12).
3. §1.6's supporting sentence "`scripts/verify-xdp.sh` diffs verifier logs
   across 5.15/6.1/6.6" describes the script accurately but overstates the
   live posture: the script is not wired into CI and those three baselines are
   placeholders (EBPF-S47-08). This does not affect the soundness verdict.

---

## 1. Findings

### EBPF-S47-01 [HIGH] — the data plane has no control plane in the production binary

`crates/lb/src/xdp.rs:188-200` is the only production wiring:

```rust
let mut loader = match XdpLoader::load_from_bytes(lb_l4_xdp::LB_XDP_ELF) { ... };
if let Err(e) = loader.kernel_load("lb_xdp") { ... }
match loader.attach_with_fallback("lb_xdp", iface, requested) { ... }
```

Load, kernel-load, attach. Nothing else. Every map-writing API on `XdpLoader`
has **no caller anywhere outside `loader.rs` itself and tests**, verified
per symbol across the workspace:

| API | `loader.rs` | production | tests |
| --- | --- | --- | --- |
| `conntrack_map` (`:892`) | defined | **none** | none |
| `conntrack_v6_map` (`:904`) | defined | **none** | none |
| `acl_trie` (`:916`) / `insert_acl_deny` (`:932`) | defined | **none** | error-type only (`round8_acl_admission.rs:45`) |
| `publish_backends_v4` (`:711`) | defined | **none** | `round8_atomic_backends.rs` |
| `set_new_flow_cap` (`:678`) | defined | **none** | none |
| `install_stats_export` (`:669`) | defined | **none** | `xdp_attach_mode.rs:270` (`#[ignore]`d) |

Only `lb` and `lb-observability` depend on the crate at all (`lb-quic`'s
mention is a comment). Consequence with `[runtime].xdp_enabled = true`:
`CONNTRACK` is permanently empty, so `handle_ipv4` always takes the miss arm at
`ebpf/src/main.rs:590-603` and returns `XDP_PASS`. Zero `XDP_TX`, zero ACL
drops, zero L7 diverts, forever. The accelerator costs a per-packet parse and
delivers nothing.

`docs/features.md:194-196` advertises: *"**L4 XDP/eBPF** data plane —
single-kernel; bounds-checked packet parse + per-CPU new-flow rate cap;
validated live on Linux 7.0."* The parse is real; the rate cap is not
(EBPF-S47-04); the steering is not.

**Failure scenario:** operator enables `xdp_enabled` on a validated kernel
expecting L4 acceleration. `ip link show` reports the program attached and
`STATS` slot 0 climbs, so the deployment looks healthy. Throughput is
unchanged because every packet is `XDP_PASS`ed to the kernel stack.

**Would a test catch it?** No. No test asserts a production map write. This is
a wiring gap, and the only tests that touch wiring (`xdp_attach_mode.rs`) are
`#[ignore]`d and not run by CI.

**Scoping:** `audit/deferred.md:229-239` defers the in-kernel *selection* logic
(Pillar 4b-3) and says backend selection is "still control-plane-driven via
CONNTRACK inserts". That documents the deferral of the kernel-side read path;
it does not document that the control-plane inserter it names does not exist.
Reported on that basis.

---

### EBPF-S47-02 [HIGH] — EBPF-2-05 (map pinning) is unreachable from the binary despite being closed "Verified-Fixed"

`crates/lb-l4-xdp/src/loader.rs:632-634`:

```rust
pub fn load_from_bytes(elf: &[u8]) -> Result<Self, XdpLoaderError> {
    Self::load_from_bytes_pinned(elf, None)
}
```

`crates/lb/src/xdp.rs:188` calls `load_from_bytes`, so `pin_path` is always
`None`, `EbpfLoader::map_pin_path` is never called, and `DEFAULT_PIN_DIR`
(`loader.rs:62`), all seven `*_PIN_NAME` constants and the whole
`bpffs::assert_bpffs` gate are dead in the shipped binary.

`audit/ebpf/round-2-review.md:320-323` records EBPF-2-05 as severity **high**,
`Status: Verified-Fixed(37c513c)`. Its stated impact — *"On bare metal with
native XDP attached, every established TCP connection is broken on restart"* —
is unmitigated in production. The fix landed the library capability and a
bpffs pre-check; it was never wired into `crates/lb/src/xdp.rs`.

Corollary: `stats_export::record_pin_reused` (`:114`) has **zero callers**
anywhere, so the `xdp_pinned_map_reused` gauge that EBPF-2-05 recommendation 5
asked for is permanently false.

**Pin versioning (the crash-recovery question in the brief):** pins are **not
versioned**. Reuse is gated only by aya's `map_type`/`key_size`/`value_size`
comparison, which is a *size* check, not a *layout* check. A future
`BackendEntry` that keeps 24 bytes but reorders fields — e.g. swapping
`backend_ip: u32` (offset 4) with `backend_port: u16` + `_pad: u16` (offsets
8, 10) — would silently reuse a stale pin from the previous version and
rewrite every packet to a wrong dst IP built from port bytes. Nothing in the
tree stamps a schema version into bpffs or validates one. This is latent only
because pinning is never enabled in production; it becomes live the moment
EBPF-S47-02 is fixed, so the two must be fixed together.

**Would a test catch it?** `xdp_pin_paths.rs` asserts the pin *name* constants
and `round8_bpffs_check.rs` asserts the bpffs magic check. Neither exercises
reuse, and no test loads twice against one bpffs directory. The kernel A/B
test EBPF-2-05's sign-off mentions is `#[ignore]`d.

---

### EBPF-S47-03 [MEDIUM] — every XDP metric is inert, and the shipped default configuration fires a RUNBOOK alert forever

`crates/lb/src/main.rs:2475` opens a bare block with **no `xdp_enabled`
guard**, spawning the sampler in every deployment:

```rust
{
    let xdp_metrics = xdp_metrics.clone();
    ...
        match lb_l4_xdp::stats_export::read_stats() {
            Ok(snap) => { ... apply_packet_deltas(&xdp_metrics, &deltas); }
            Err(e) => {
                xdp_metrics.sampler_errors_total.inc();      // :2497
```

`read_stats` (`stats_export.rs:234-235`) starts with
`STATS_HANDLE.get().ok_or(StatsExportError::HandleMissing)?`, and
`install_stats_handle` is never called in production (EBPF-S47-01). So the
`Err` arm runs once per second, forever, in **the default config too** — where
XDP is off and the metric is meaningless.

`docs/guide/RUNBOOK.md:445` defines the alert
`rate(xdp_sampler_errors_total[5m]) > 0`. That is a guaranteed permanent page
on every deployment of the shipped defaults.

The other three XDP metrics are equally inert, by a different mechanism —
their setters have no production caller:

| Metric | Setter | Production callers |
| --- | --- | --- |
| `xdp_attached_mode` | `set_attached_mode` (`xdp_metrics.rs:130`) | **none** |
| `xdp_conntrack_full_total` | `record_conntrack_full` (`:138`) | tests only |
| `xdp_packets_total` | `apply_packet_deltas` (`:117`) | `main.rs:2493`, unreachable per above |

`loader.rs:847` does call `stats_export::record_attach_mode(label)` on a
successful attach, but that writes a process-local `AtomicU8`
(`stats_export.rs:63`) that nothing bridges to Prometheus.
`docs/guide/RUNBOOK.md:426` states **"Wired: yes (xdp_metrics.rs)"** for
`LbXdpAttachMode`, whose trigger is
`xdp_attached_mode{mode="drv"} == 0 AND xdp_attached_mode{mode="skb"} == 1`.
The gauge never receives a sample, so the alert can never evaluate true. The
neighbouring `LbXdpConntrackFull` honestly says "Wired: pending", which makes
the "yes" a specific, checkable doc error rather than general drift.

**Would a test catch it?** No. `metrics_xdp_slots.rs` and
`metrics_xdp_conntrack.rs` call the setters directly from the test, proving
the metric objects work; nothing asserts that production calls them.

---

### EBPF-S47-04 [MEDIUM] — the SYN-flood new-flow rate cap can never fire; its documented fallback is dead code

`crates/lb-l4-xdp/ebpf/src/main.rs:335-343`:

```rust
let cap = match NEW_FLOW_CAP_CFG.get_ptr(0) {
    // SAFETY: aya returned a non-null pointer for this CPU's slot.
    Some(p) => unsafe { *p },
    None => DEFAULT_NEW_FLOW_CAP_PER_CPU,
};
// Cap of 0 = operator disabled the rate limiter entirely.
if cap == 0 {
    return false;
}
```

The comment at `:321-324` claims: *"Since 0 in the cfg map means 'operator
disabled', not-yet-written is distinguished from disabled by consulting this
fallback ONLY when the slot is unreadable."*

That distinction does not exist. `NEW_FLOW_CAP_CFG` is
`PerCpuArray<u32>::with_max_entries(1, 0)` — confirmed in the object as
type 6 (`BPF_MAP_TYPE_PERCPU_ARRAY`), key 4, value 4, max_entries 1. A
per-CPU **array** lookup at a valid index always returns a non-null pointer,
so `get_ptr(0)` is always `Some`, and BPF array maps are zero-initialised at
creation. Therefore `cap == 0` on every CPU until userspace writes the map —
and `set_new_flow_cap` has no production caller (EBPF-S47-01). The `None` arm
and `DEFAULT_NEW_FLOW_CAP_PER_CPU = 125_000` (`:325`) are unreachable.

**Failure scenario:** attacker sprays unique 5-tuples at line rate. Katran's
`is_under_flood()` equivalent, which `docs/features.md:195` lists as a shipped
feature, returns `false` on every packet and `STAT_NEW_FLOW_RATE_CAP` (slot
15) never increments — so the operator's only signal that the mitigation
exists is also silent.

Today the behavioural impact is masked: both the flood arm (`:597-600`) and
the normal miss arm (`:601-602`) return `XDP_PASS`, differing only in which
counter moves. The mechanism is nonetheless broken by construction and would
stay broken when the CT-populate loop lands, which is precisely when it
matters.

**Would a test catch it?** No. `round8_synflood_cap.rs` tests
`loader::CtInsertGate`, a *userspace* token bucket (`loader.rs:1085-1163`)
that is itself never used in production. It never touches
`NEW_FLOW_CAP_CFG`, `is_under_flood`, or the BPF program. This is a `sim`-class
test standing in for kernel behaviour.

---

### EBPF-S47-05 [MEDIUM] — the deny ACL is bypassable by IP fragmentation, and absent entirely on IPv6

Order of operations in `handle_ipv4` (`ebpf/src/main.rs:509-523`):

```rust
    let frag_off = u16::from_be(frag_off_be);
    if (frag_off & 0x3FFF) != 0 {
        incr_stat(STAT_V4_FRAGMENT);
        return Ok(xdp_action::XDP_PASS);          // :514 — returns BEFORE the ACL
    }

    let lpm_key = LpmKey::<u32>::new(32, src_addr);
    if ACL_DENY_TRIE.get(&lpm_key).is_some() {    // :520 — never reached for fragments
        incr_stat(STAT_DROP);
        return Ok(xdp_action::XDP_DROP);
    }
```

**Failure scenario:** a source inside a denied CIDR sets MF=1 (or any non-zero
fragment offset) on its packets. The XDP program returns `XDP_PASS` before the
deny lookup; the kernel reassembles and delivers normally. The deny-list is
bypassed with a one-line change to the attacker's sender.

`handle_ipv6` (`:736-862`) performs **no ACL lookup at all** — there is no
`acl_deny_trie_v6` map in the source or in the object's 8 map definitions. A
peer denied over IPv4 reaches the gateway unimpeded over IPv6.
`loader.rs:931` acknowledges the second half:
`TODO(L4-06): mirror this guard with 1..=128 when an IPv6 ACL trie ships (absent today).`

`audit/deferred.md:195-213` (ROUND8-L4-08) documents *pass-to-kernel for
fragments* as a deliberate Katran/Cilium-parity design choice. It does not
mention that the chosen ordering also disables the security control, and no
`docs/` page notes the IPv4-only scope of the ACL. Latent today because the
ACL is never populated (EBPF-S47-01); live the moment it is.

**Would a test catch it?** No. `round8_fragments.rs:7-9` declares its own
`fn is_fragment_v4(frag_off_be: u16) -> bool` inside the test file and tests
that mirror — it cannot observe ordering relative to the ACL, and it never
runs the BPF program.

---

### EBPF-S47-06 [MEDIUM] — partial rewrite on the TCP error path: L3 is mutated before a bounds check that can fail

Two different TCP header shapes are used for the same packet:

```rust
#[repr(C, packed(2))]
struct TcpHdr {                 // :114-125 — parse path
    src_port: u16, dst_port: u16, _seq: u32, _ack: u32,
    _data_offset_ns: u8, flags: u8, _window: u16,
}                               // 2+2+4+4+1+1+2 = 16 bytes

#[repr(C, packed(2))]
struct TcpHdrRW {               // :723-733 — rewrite path
    src_port: u16, dst_port: u16, _seq: u32, _ack: u32,
    _offset_flags: u16, _window: u16, check: u16, _urg_ptr: u16,
}                               // 2+2+4+4+2+2+2+2 = 20 bytes
```

`rewrite_v4` (`:639-665`) writes both MACs, then the IPv4 dst and its
checksum, and only then bounds-checks the L4 header:

```rust
    let eth_m = unsafe { ptr_at_mut::<EthHdr>(ctx, 0).ok_or(())? };
    unsafe {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*eth_m).dst), entry.backend_mac);
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*eth_m).src), entry.src_mac);
    }
    ...
    unsafe {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*ip_m).dst), entry.backend_ip);
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*ip_m).check), new_check);
    }
    let l4_offset = l3_offset + ip_hdr_len;
    match protocol {
        IPPROTO_TCP => {
            let tcp_m = unsafe { ptr_at_mut::<TcpHdrRW>(ctx, l4_offset).ok_or(())? };   // :665 — can fail HERE
```

`try_lb_xdp`'s caller maps that `Err` to a pass (`:456-459`):

```rust
        Err(()) => {
            incr_stat(STAT_PARSE_FAIL);
            xdp_action::XDP_PASS
        }
```

**Concrete packet:** 90–93-byte Ethernet frame, IPv4 with IHL=15 (40 bytes of
options), TCP header truncated to 16–19 bytes, 5-tuple matching a live
`CONNTRACK` entry. `l4_offset` = 14 + 60 = 74. The parse path needs
74 + 16 = 90 bytes → succeeds. The rewrite path needs 74 + 20 = 94 bytes →
fails. Ethernet padding pads *up* to 60 and cannot shrink a 90-byte frame, so
this is constructible on the wire.

**Result:** the frame is handed to the local kernel stack with its L2
destination set to the backend's MAC, its L3 destination set to the backend's
IP, a *valid* IPv4 checksum for that rewritten address, and a stale TCP
checksum — while being counted as `STAT_PARSE_FAIL`, i.e. as a packet that was
never touched. If the host has IP forwarding enabled the kernel will route it
onward. UDP is unaffected: `UdpHdr` is 8 bytes on both paths.

Requires a `CONNTRACK` hit, so latent behind EBPF-S47-01.

**Would a test catch it?** No. Nothing exercises `rewrite_v4`; `sim.rs` has no
rewrite model at all (EBPF-S47-18).

**Relation to `rt-unsafe.md`:** not a contradiction. Every `unsafe` block above
is sound — `ptr_at_mut` validates before each write and the failure is a clean
`Err`. The verifier and the unsafe audit both pass a program that emits a
mangled packet.

---

### EBPF-S47-07 [MEDIUM] — nothing rebuilds or validates the committed object against its source

`crates/lb-l4-xdp/build.rs:16-28` checks exactly one property — size:

```rust
    if let Ok(meta) = std::fs::metadata(&elf_path) {
        let size = meta.len();
        if size > MAX_ELF_BYTES { panic!( ... ); }
        println!("cargo:rustc-cfg=lb_xdp_elf");
    }
```

`scripts/build-xdp.sh` is not referenced by any workflow
(`grep -rn 'build-xdp\|verify-xdp' .github/` → one *comment* in `ci.yml:466`),
and it is written to swallow its own failure:

```sh
    say "bpf-linker install failed (common causes: missing LLVM dev headers,"
    say "Skipping ELF build. Rerun this script once bpf-linker is installed."
    exit 0
```

**Proof that drift is live, not hypothetical:** I decoded the object's own
`.BTF.ext` `line_info` records, which store each instruction's source line
number and the source text as of build time. 65 records reference
`ebpf/src/main.rs`; **0 of 65 match the current file's line numbering**, with
offsets drifting from −127 to −202 lines. Git corroborates: the object was
last built 2026-05-16 (`7af84128`, "fix(round8): D-1/D-2 eBPF ELF build") and
the source was last changed 2026-08-11 (`8c3b651d`, an S45A comment re-wrap).

**I then checked whether that drift is behavioural, and it is not.** Diffing
the comment-stripped source between `7af84128` and HEAD yields 684 code lines
on both sides and **zero differences**. Today's shipped object does implement
today's source. That is the honest result, and it is why this is MEDIUM and
not CRITICAL.

What is missing is any enforcement of that correspondence. The near-miss is on
record: commit `93155f8b`, *"S45A FIX: restore the
`#[map(name = "new_flow_cap_cfg")]` eBPF attribute"* —

> The comment sweep deleted the doc block above NEW_FLOW_CAP_CFG and took the
> map attribute with it. Without it the static is not emitted as a named BPF
> map, so the pin userspace writes (NEW_FLOW_CAP_CFG_PIN_NAME) would not
> exist [...]

An ABI-bearing attribute was deleted by a comment pass and caught by a human
reading a diff. No test could have caught it, because every test runs the
prebuilt object, which still contained the map.

**Would a test catch it?** No. `real_elf.rs` asserts the object parses and
declares one `lb_xdp` program; `elf_sections.rs` asserts `license`, non-empty
BTF and the size ceiling. All are properties of the *artefact*; none relates it
to the *source*.

---

### EBPF-S47-08 [MEDIUM] — the claimed 5.15 / 6.1 / 6.6 verifier matrix does not exist

`audit/ebpf/verifier-logs/5.15.log.committed` (6.1 and 6.6 identical in kind):

```
verify-xdp.sh: kernel 5.15; loading lb_xdp.bin via lvh
HARNESS-CAPTURED-PENDING-CI-RERUN
This file is a placeholder baseline emitted by ROUND8-L4-10's
implementation pass on a sandbox without `bpf-linker` available [...]
The first green CI run on kernel 5.15 MUST refresh this file via:
    scripts/verify-xdp.sh --kernel 5.15 --update-baseline
```

`scripts/verify-xdp.sh` is never invoked by any workflow, so that refresh
cannot occur. Only `7.0.log.committed` is a real capture (aya
`BPF_PROG_LOAD` + `ProgramInfo` + `bpftool prog show`, kernel 7.0.0-1004-aws).

Downstream claims that rest on it:

- `docs/known-limitations.md:233-235`: *"validated against a specific
  kernel/verifier window (5.15 / 6.1 / 6.6 LTS, plus live-validated on 7.0)"*.
- `docs/guide/DEPLOYMENT.md:37`: *"5.15 LTS / 6.1 LTS / 6.6 are the
  officially-validated LTS window (see `audit/ebpf/verifier-logs/`)"* — a
  citation pointing at the placeholders.

**On the feature floor itself** (the brief's kernel-matrix question), the
program's requirements are genuinely modest and 5.15 is plausible: no BPF ring
buffer, no kfuncs, no CO-RE relocations (`.BTF.ext` carries `line_info` and
`func_info`; there is no `core_relo` consumer, and the program reads no kernel
structs). Feature floors used: `BPF_MAP_TYPE_LRU_HASH` 4.10,
`BPF_MAP_TYPE_LPM_TRIE` 4.11 (with `BPF_F_NO_PREALLOC`, which the object
correctly sets — `flags=1` on `acl_deny_trie`), `BPF_MAP_TYPE_PERCPU_ARRAY`
4.6, BPF-to-BPF calls 4.16 (the `memcpy`/`memmove`/`memset` symbols in
`.text`), `bpf_ktime_get_ns` pre-4.x, and 1330 instructions — far under even
the pre-5.2 4096 limit. aya's XDP attach prefers `bpf_link` (5.9+) and falls
back to netlink below that. So the floor claim is *plausible but unevidenced*;
what is missing is the evidence, not the capability.

**Would a test catch it?** `round8_verify_xdp_gate.rs` asserts the opposite —
it asserts the `HARNESS-CAPTURED-PENDING-CI-RERUN` marker **is present**,
codifying the placeholder state as the expected posture.

---

### EBPF-S47-09 [MEDIUM] — no behavioural coverage of the real BPF program; the "proof" tests re-implement the logic they claim to prove

The brief asked specifically whether `sim.rs` stands in for real datapath
coverage. It is worse than that: several tests do not even use `sim.rs`, they
re-declare the logic inside the test file.

`crates/lb-l4-xdp/tests/round8_ptr_at_bounds.rs:4-15` —
*"ROUND8-L4-09 proof: the `ptr_at` checked-arithmetic bounds check rejects
wrap-around offsets"*:

```rust
/// Userspace mirror of `crates/lb-l4-xdp/ebpf/src/main.rs` `ptr_at` arithmetic.
fn ptr_at_in_bounds(start: usize, offset: usize, len: usize, end: usize) -> bool {
    let needed = match start.checked_add(offset).and_then(|s| s.checked_add(len)) { ... };
```

Deleting `checked_add` from the real `ptr_at` in `ebpf/src/main.rs:372-385`
would not fail this test. The same shape appears in `round8_fragments.rs:7-9`
(`is_fragment_v4`) and `round8_conntrack_state.rs` (`sim_tcp_path`, whose own
header admits *"The eBPF path is unreachable in CI, so this models the BPF
state machine over the userspace table"*).

The three workspace-level tests named in the brief total 75 lines and contain
no XDP:

- `tests/l4_xdp_hotswap.rs` — header *"XDP program hot-swap tests"*; body
  builds a `HotSwapManager`, calls `route_flow` twice and compares two
  `usize`s. No program, no map, no swap of anything in the kernel sense.
- `tests/l4_xdp_maglev.rs` — `MaglevTable::lookup` distribution over 3000 keys.
- `tests/l4_xdp_conntrack.rs` — `HashMap` insert/lookup.

What CI actually runs (`ci.yml:448-474`, job `xdp-smoke`): one real
`BPF_PROG_LOAD` of the committed object on the runner kernel, asserting
`prog_id`/`tag`/`verified_insns > 0`. That is genuine and valuable — **verifier
acceptance is covered** — but it performs no attach and pushes no packet.
`grep -n -- '--ignored' .github/workflows/*.yml` returns exactly that one job.

Consequently `xdp_attach_mode.rs:24-25` is false:

```rust
/// EBPF-2-04: with `Auto`, a NIC rejecting Drv with `EOPNOTSUPP` must WARN [...]
/// `#[ignore]`d — CI runs it under `--ignored` in the privileged stage.
```

CI runs no such stage, and the test body (`:28-35`) is an `eprintln!` stub
that asserts nothing.

The one real datapath validation in the repo is manual and narrow:
`audit/foundation-pass/d1-native-attach-result.md` records a live native
`xdpdrv` attach to ENA `ens5` with *"D1 data-path: STATS aggregate 0 -> 2
(delta 2 > 0)"*. Two packets entered the program. That proves it executes; it
says nothing about which branch, the rewrite, the checksum, the ACL or the CT.

Net position: **verifier acceptance covered (runner kernel only); datapath
behaviour uncovered everywhere.** `BPF_PROG_TEST_RUN` (kernel ≥ 4.12, needs no
NIC and no attach) is the standard tool for this;
`nic_compat.rs:297-307` documents it as blocked on aya 0.13.1 exposing no
public wrapper on `Xdp`, which is accurate for the safe API — the syscall
itself is reachable directly.

---

### EBPF-S47-10 [LOW] — `attach_replacing` is a detach-then-attach with a bare-interface window

`loader.rs:995-1008`:

```rust
                // Detach our previous link first (a real `Xdp::detach`), then re-attach: a fresh
                // attach over our own still-attached program returns EBUSY, and aya 0.13.1 exposes
                // no BPF_F_REPLACE wrapper.
                if let Some(link_id) = self.attached_links.remove(prog_name) {
                    ...
                    xdp.detach(link_id)?;
                }
                self.attach(prog_name, iface, mode)?;
```

Between the two calls the interface carries no XDP program: any ACL denies are
unenforced and any `XDP_TX` flows fall back to the kernel stack. If the
re-attach fails the function returns `Err` with the NIC left bare and
`attached_links` already cleared. The ownership pre-check (`query_xdp` at
`:992`) is also TOCTOU relative to the attach, though under `bpf_link` a
racing third party yields `EBUSY` rather than a clobber.

The kernel does support atomic replace (`XDP_FLAGS_REPLACE` with an expected
fd, or `BPF_LINK_UPDATE`); aya 0.13.1's surface is the limiting factor, and the
comment says so. No production caller today (`attach_replacing` and
`detach_verifying` are called only from tests), which is why this is LOW — but
`loader.rs:959-960` states a drain contract with `crates/lb/src/main.rs` that
does not exist:

```rust
    // ROUND8-L4-12: the drain contract with OPS-04 (`crates/lb/src/main.rs`) is ORDERED — cancel
    // accept loops, drain in-flight tasks, THEN `detach_verifying(prog, iface, our_prog_id)` last.
```

`main.rs` performs no `detach_verifying`; teardown relies on `drop(_xdp_loader)`
at `main.rs:2671`. That is adequate for a clean exit (aya detaches on drop) but
means an abnormal exit leaves the program attached with no cleanup path — the
"leaked XDP program keeps blackholing traffic" case in the brief. With the
current program that degrades to `XDP_PASS`-everything rather than a
blackhole, so the practical harm is bounded today.

**Would a test catch it?** `round8_attach_replace.rs` asserts function
*signatures* and error variants; it never attaches.

---

### EBPF-S47-11 [LOW] — `ethtool` spawned from `$PATH`, without a timeout, while holding ambient CAP_BPF

`nic_compat.rs:233-241`:

```rust
pub fn firmware_of(iface: &str) -> Result<String, NicCompatError> {
    let out = std::process::Command::new("ethtool")
        .arg("-i")
        .arg(iface)
        .output()
```

Reached from `drv_supported` → `attach_with_fallback` → `try_attach_xdp` at
startup. `packaging/expressgateway.service:45` grants
`AmbientCapabilities=CAP_NET_BIND_SERVICE CAP_NET_ADMIN CAP_BPF`, and ambient
capabilities survive `execve`, so the child inherits them.

Two issues, in order of realism:

1. **No timeout.** `Command::output()` blocks until the child exits. `ethtool
   -i` issues a driver ioctl that can hang on wedged firmware; the gateway then
   hangs in startup with no watchdog on this path.
2. **Resolved via `$PATH`, not an absolute path.** Under the shipped systemd
   unit this is largely theoretical (systemd's default PATH is root-owned
   directories, and the unit sets `NoNewPrivileges=true` and
   `ProtectSystem=strict`), but the binary is also run from containers and dev
   shells where PATH is attacker-influenceable, and the inherited caps make the
   payoff high.

**Would a test catch it?** No. `round8_attach_probe.rs` tests `classify` and
`parse_ethtool_firmware` on static strings; it never spawns a process.

---

### EBPF-S47-12 [LOW] — netlink `recv`: fixed 32 KiB, no `MSG_TRUNC`, no timeout, no seq/pid validation

`netlink_xdp.rs:323-331`:

```rust
    // 32 KiB is the conventional netlink buffer and more than enough for one link's attributes.
    let mut reply = vec![0u8; 32 * 1024];
    // SAFETY: reply is a valid writable buffer of reply.len() bytes.
    let n = unsafe { libc::recv(fd, reply.as_mut_ptr().cast(), reply.len(), 0) };
```

Memory-safe (I confirm `rt-unsafe.md` §1.4), but:

- A netlink datagram larger than the buffer is **silently truncated** by the
  kernel; without `MSG_TRUNC` the caller cannot even detect it. On an SR-IOV
  NIC whose `RTM_GETLINK` reply carries `IFLA_VFINFO_LIST` for many VFs
  (~250-300 B each, plus `IFLA_STATS64`/`IFLA_AF_SPEC`) 32 KiB is reachable.
  `parse_getlink_response:151` then returns
  `"truncated netlink message: len=… off=… buf=…"`, which surfaces as
  `XdpQueryFailed` → `detach_verifying` returns `Err` → **the program is left
  attached at shutdown**.
- The `recv` is blocking with no `SO_RCVTIMEO`, sitting in the drain path.
- The socket is never `bind`(2)ed and the reply is not filtered on
  `nlmsg_seq`/`nlmsg_pid` (the request hardcodes `seq = 1` at `:298`), so the
  parser accepts any datagram delivered to the auto-bound address. Exploiting
  it requires a local process that can guess the netlink port-id and win a
  race, so this is defence-in-depth, not a live vector.

The standard remedy for the first point is `recv(..., MSG_PEEK|MSG_TRUNC)` to
size the buffer, or a loop.

**Would a test catch it?** `round8_netlink_xdp_query.rs` parses a captured
well-formed blob. It cannot reach the truncation, timeout or filtering paths.

---

### EBPF-S47-13 [LOW] — a rewritten UDP checksum of zero is emitted as "no checksum"

`ebpf/src/main.rs:701-710` (IPv4) and `:919-931` (IPv6):

```rust
                if old_check != 0 {
                    let mut c = u16::from_be(old_check);
                    c = csum16_update_u32(c, ...);
                    c = csum16_update(c, u16::from_be(old_dst_port), entry.backend_port.swap_bytes());
                    core::ptr::write_unaligned(core::ptr::addr_of_mut!((*udp_m).check), c.to_be());
                }
```

The `old_check != 0` guard correctly honours the IPv4 "checksum not computed"
convention. The *output* is unguarded: if the incremental update yields
`0x0000`, RFC 768 requires transmitting `0xFFFF` instead, because zero means
"no checksum". For IPv6 this is worse than a convention — RFC 8200 §8.1 makes
a zero UDP checksum illegal and the receiver **must** discard the datagram.

**Failure scenario:** roughly 1 in 65 536 rewritten UDP datagrams to an IPv6
backend is silently dropped by the backend's stack, with no counter anywhere
attributing it. Latent behind EBPF-S47-01.

**Would a test catch it?** No. `sim.rs` mirrors `csum16_update`/`fold32`
verbatim but has no UDP-emission model, and no test asserts the `0x0000` case.

---

### EBPF-S47-14 [LOW] — no TTL / hop-limit decrement on the DNAT-and-forward path

`rewrite_v4` (`:630-720`) rewrites both MACs, the IPv4 destination address and
the IPv4 header checksum, then returns `XDP_TX`. `ttl` (`Ipv4Hdr:89`) is never
touched; `rewrite_v6` likewise leaves `hop_limit` (`Ipv6Hdr:101`) alone.

The device is changing the L3 destination and re-transmitting to a different
L2 next hop, which is forwarding behaviour; RFC 1812 §5.3.1 requires the TTL
decrement, whose purpose is to terminate routing loops. A misconfiguration
that loops a flow back to this NIC would not self-terminate.

Note this is a deliberate-looking omission — decrementing would also require a
second incremental checksum update — and Katran sidesteps it by encapsulating
rather than DNAT-ing. Flagged as a documented-decision gap rather than an
outright defect; it is not mentioned in ADR-0004, ADR-0005 or
`known-limitations.md`.

---

### EBPF-S47-15 [LOW] — `L7_PORTS` byte-order contract: the program looks up host order, the ADR specifies network order

`ebpf/src/main.rs:528-562` reads the port into host order and looks up with it:

```rust
            let dp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).dst_port))
            });
    ...
    if unsafe { L7_PORTS.get(&dst_port) }.is_some() {      // :559 (and :808 for v6)
```

`docs/decisions/ADR-0005-bpf-map-schema.md` specifies the key as
*"`u16` (dst port, **net order**)"*.

There is no live mismatch, because nothing populates the map
(EBPF-S47-01). The trap is for whoever wires it: following the ADR and
inserting `443u16.to_be()` (`0xBB01` = 47873 as a native `u16`) produces a
lookup that never matches, so the L7 bypass silently fails open into the L4
path with no error anywhere.

For contrast, the conntrack path gets this right and is internally consistent:
`FlowKey` is built at `:564-571` with `src_port: src_port.to_be()` /
`dst_port: dst_port.to_be()`, which round-trips the `from_be` back to the
wire bytes, matching ADR-0005's "key is stored in network byte order" and the
userspace `FlowKey` doc comments (`loader.rs:75-78`). The ACL is also correct:
the BPF side passes the raw packet `src_addr` (`:519`) and
`insert_acl_deny` (`:942`) inserts `u32::from(ipv4).to_be()`, so both sides
present network-order bytes — which is required for an LPM trie, since the
kernel matches prefix bits from byte 0 upward.

---

### EBPF-S47-16 [LOW] — conntrack eviction on an unvalidated RST, and `tot_len` never bounds the L4 parse

Two small hardening gaps in `handle_ipv4`, both attacker-reachable once
conntrack is populated.

**(a)** `:577-583`:

```rust
    if protocol == IPPROTO_TCP && (tcp_flags & TCP_FLAG_RST) != 0 {
        // The Result is discarded: "no such key" is the steady state for unrelated RST sprays.
        let _ = CONNTRACK.remove(&key);
```

Any packet carrying RST for a known 5-tuple evicts the entry, with no sequence
number or state check. An attacker who knows or can observe the 5-tuple evicts
the flow's pin; the next packet takes the miss path and can be re-pinned to a
different backend mid-connection if the backend set has changed. Cilium's
equivalent also closes on RST, so this is a defensible parity choice — but the
in-window check is what makes it safe there, and the comment does not
acknowledge the trade-off.

**(b)** `tot_len` (`Ipv4Hdr:86`) is read by no one. Bounds come only from
`data_end`, so on a 60-byte padded Ethernet frame carrying an IPv4 packet with
`tot_len = 20` and `protocol = 6`, `ptr_at::<TcpHdr>(ctx, 34)` needs 54 ≤ 60
and **succeeds** — 20 bytes of Ethernet padding are parsed as a TCP header,
yielding attacker-chosen ports and flags. Impact is bounded (the resulting
5-tuple must still hit `CONNTRACK`, whose key includes `src_addr`), but it is
free to fix and it is how (a) becomes reachable without a real TCP header.

---

### EBPF-S47-17 [INFO] — ADR-0005, the "frozen schema" document, no longer describes the schema

`docs/decisions/ADR-0005-bpf-map-schema.md` is cited by ADR-0004 and by the
loader as the single source of truth. Divergences from both the source and the
compiled object:

| ADR-0005 says | Reality (source + object) |
| --- | --- |
| `BackendEntry { backend_idx, **flags: u32**, backend_ip, backend_port, _pad, backend_mac, src_mac }`, "grew from 8 → 28 bytes" | No `flags` field; **24 bytes** (object: `conntrack` value_size 24) |
| `BackendEntryV6` "// 40 bytes" | **36 bytes** (object: `conntrack_v6` value_size 36) |
| `CONNTRACK` / `CONNTRACK_V6` are `BPF_MAP_TYPE_HASH` | `LruHashMap` in source; **type 9 = `BPF_MAP_TYPE_LRU_HASH`** in the object |
| "full-capacity inserts are rejected by the verifier" | Map-full is a **runtime** `-E2BIG` from `bpf_map_update_elem`; the verifier has no role. Moot for LRU, which evicts |
| `L7_PORTS` key in net order | Program looks up host order (EBPF-S47-15) |
| Stats slot table ends at index 9 | Source defines slots 0–15 (`STAT_*` through `STAT_NEW_FLOW_RATE_CAP`) |
| "Pillar 4b will upgrade to `BPF_MAP_TYPE_LRU_HASH`" | Already done (EBPF-2-03) |

The code is self-consistent and correctly cross-asserted — `loader.rs:300-303`
even explains the `flags` removal (*"ROUND8-L4-07 dropped 4 B of flags from
both entries"*) and `loader.rs:314-317`/`:378` pin all five sizes with
`const _: () = assert!(...)`. It is the ADR that drifted.

---

### EBPF-S47-18 [INFO] — `sim.rs` fidelity: independently re-derived, models a strict subset

`crates/lb-l4-xdp/src/lib.rs:45-47` calls `sim` *"the CI-safe functional spec
those routines must satisfy"*. Measured against the program:

| Concern | eBPF program | `sim.rs` |
| --- | --- | --- |
| `fold32`, `csum16_update` | `main.rs:416-430` | **verbatim copy** (`sim.rs:192-204`) — these do agree |
| ACL | `LpmTrie` lookup, kernel LPM semantics | `AclTrie` = linear `Vec<(u32,u8)>` scan with `.max()` (`:161-174`) — independently re-derived |
| VLAN | strip one tag, discard TCI, `Err`→`PASS` | `strip_vlan` returns `vlan_id`, `None` on short frame (`:35-65`) |
| Conntrack decision | L7 check → RST prune → lookup → sentinel guard → rewrite → FIN prune | `decide()` = one `BTreeMap` lookup → `Tx`/`Pass` (`:124-128`) |
| Rewrite / checksums on packets | `rewrite_v4` / `rewrite_v6` | **absent** |
| Fragments, ext headers, flood gate, IHL parse | present | **absent** |

So a `sim.rs` test asserting `decide(&flow) == Tx { backend_idx }` skips four
gates and the entire mutation path. The `AclTrie` divergence is the sharper
one: a linear scan taking the longest match happens to agree with an LPM trie
for these inputs, but it is a different algorithm, so it cannot detect a
key-encoding or prefix-length bug in the real trie — which is exactly the
class of bug EBPF-S47-15 describes for the neighbouring map.

`sim.rs`'s own header is honest about this (*"This does not replace the
in-kernel program — the BPF source is authoritative"*). The problem is the
citation chain above it: ADR-0005 lists these as the tests backing the schema,
and `lib.rs` calls them the functional spec.

---

### EBPF-S47-19 [INFO] — `HotSwapManager` pins flows by backend *index*; a count-preserving reorder silently re-pins live flows

`lib.rs:361-372`:

```rust
    pub fn route_flow(&mut self, flow: FlowKey, hash_key: u64) -> Result<usize, XdpError> {
        if let Some(idx) = self.conntrack.lookup(&flow) {
            if idx < self.current_table.backend_count() {
                return Ok(idx);
            }
            self.conntrack.remove(&flow);
        }
```

Staleness is detected only by range. After `swap_backends(["c","b","a"])` over
`["a","b","c"]` every index is still in range, so a live flow pinned to index
0 now resolves to a different backend — the mid-connection misroute the whole
conntrack exists to prevent. ADR-0005's "Stale-entry recovery" describes only
the shrink case, so the model matches its spec; the spec is the gap.

The kernel path is immune today because `BackendEntry` stores full identity
(`backend_ip`, `backend_port`, `backend_mac`) rather than an index. But
`BackendTable.entries[]` (`main.rs:210-222`) is index-addressed and the
deferred Pillar 4b-3 selection is specified as `entries[hash % count]`, so the
hazard transfers the moment that lands. `generation`/`previous_entries` handle
*when* a table changed, not *whether index i still means the same backend*.

`tests/l4_xdp_hotswap.rs` cannot catch it: it grows `["b1","b2","b3"]` to
`["b1","b2","b3","b4"]`, preserving every index.

---

## 2. Checked and found correct

Recorded so the negatives are auditable.

**Map ABI — userspace ↔ eBPF ↔ compiled object, byte for byte.** Decoded from
the object's legacy `maps` section (8 × 28-byte `bpf_map_def`):

| Map | Type | key | value | max_entries | flags |
| --- | --- | --- | --- | --- | --- |
| `conntrack` | LRU_HASH (9) | 16 | 24 | 1 000 000 | 0 |
| `conntrack_v6` | LRU_HASH (9) | 40 | 36 | 512 000 | 0 |
| `backends_v4` | HASH (1) | 4 | 3088 | 1024 | 0 |
| `l7_ports` | HASH (1) | 2 | 1 | 256 | 0 |
| `acl_deny_trie` | LPM_TRIE (11) | 8 | 4 | 100 000 | 1 (`BPF_F_NO_PREALLOC`, required) |
| `stats` | PERCPU_ARRAY (6) | 4 | 8 | 32 | 0 |
| `new_flow_rate` | PERCPU_ARRAY (6) | 4 | 16 | 1 | 0 |
| `new_flow_cap_cfg` | PERCPU_ARRAY (6) | 4 | 4 | 1 | 0 |

Offsets, both planes (`ebpf/src/main.rs:145-190` vs `loader.rs:68-298`):

```
FlowKey (16)         src_addr@0 dst_addr@4 src_port@8 dst_port@10 protocol@12 pad@13
FlowKeyV6 (40)       src_addr@0 dst_addr@16 src_port@32 dst_port@34 protocol@36 pad@37
BackendEntry (24)    backend_idx@0 backend_ip@4 backend_port@8 _pad@10 backend_mac@12 src_mac@18
BackendEntryV6 (36)  backend_idx@0 backend_ip@4 backend_port@20 _pad@22 backend_mac@24 src_mac@30
BackendTable (3088)  generation@0 count@4 entries@8 previous_count@1544 _pad@1548 previous_entries@1552
RateWindow (16)      window_start_ns@0 flows_this_window@8 _pad@12
```

Field order, widths and explicit padding agree on both sides; every struct is
`#[repr(C)]` with a named `_pad`/`pad` member so there is no implicit padding
(the `u16`-before-`u64` hazard in the brief does not occur). Sizes are pinned
by `const _: () = assert!(...)` at `loader.rs:314-317` and `:378` and match the
object's declarations above. `pod_padding.rs` proves the constructors zero the
pad bytes. **No ABI mismatch found.**

**Packet-access bounds.** Every access goes through `ptr_at`/`ptr_at_mut`
(`main.rs:371-399`), which re-read `ctx.data()`/`ctx.data_end()` on each call
and use `checked_add` for both the offset and the length — so no check is
reused across a pointer advance, and the CVE-2022-23222-class wrap elision is
explicitly guarded. `ihl_words` is masked to `0x0F` and floored at 5
(`:497-500`), so `ip_hdr_len ∈ [20,60]` and the derived `l4_offset` is
re-validated by the next `ptr_at`. The IPv6 extension-header walk (`:749-764`)
is bounded to two iterations and re-validates after each `off +=
(len + 1) * 8`; a hostile `hdr_ext_len` can only push `off` past `data_end`,
which fails the next `ptr_at` and returns `XDP_PASS`. Confirms
`rt-unsafe.md` §1.6.

**Endianness on packet fields.** Ports are `u16::from_be` on read and
`.to_be()` when placed in a key — a round trip that stores wire bytes, matching
the userspace side. Checksum inputs are transformed consistently
(`u16::from_be(old_check)`, `u16::from_be(old_dst_port)`,
`entry.backend_port.swap_bytes()`), which is valid because a one's-complement
sum is byte-order agnostic when all terms share an order (RFC 1071 §1.2.B).
`csum16_update_u32` splits the host-order address so the high half is the first
wire word; `csum16_update_v6` builds `(a[i] << 8) | a[i+1]`, i.e. wire-order
16-bit words. No double- or missing-swap found on any path.

**Checksums.** RFC 1624 eq. 3 (`HC' = ~(~HC + ~m + m')`) — the formulation that
avoids the eq. 1/eq. 2 "−0" defect — with `fold32` folding twice, which is
sufficient for any `u32` input. The IPv4 header checksum is updated for the dst
change; the L4 checksum is updated for both the pseudo-header dst and the port
on TCP and UDP, v4 and v6. The IPv4 UDP zero-checksum special case is handled
on input (see EBPF-S47-13 for the output side).

**Per-CPU aggregation — the "online vs possible CPUs" bug is NOT present.**
`read_stats` (`stats_export.rs:234-249`) reads through aya's
`PerCpuArray::get`, whose `PerCpuValues` length comes from
`aya::util::nr_cpus()` = `/sys/devices/system/cpu/possible`. Summation is
`fold(0, u64::wrapping_add)` over the full slice. Counters are `u64` written by
a single 8-byte store on the BPF side (`main.rs:402-409` uses a raw
`*mut u64`, never a `&mut`), so cross-plane reads are not torn on 64-bit.
`NUM_SLOTS = 16` matches `STAT_*` 0–15 and is guarded by
`stat_slot_indices_are_wire_stable` and `num_slots_matches_enum`; the map is
sized 32 for headroom.

**Netlink framing and parsing.** Request layout is correct
(`nlmsghdr` len@0/type@4/flags@6/seq@8/pid@12; `ifinfomsg` family@16,
ifindex@20 — `netlink_xdp.rs:295-302`), and `attr_start = align(16, 4)` matches
`IFLA_RTA`. The attribute walk uses `.get()` and `checked_add` throughout, and
carries both a `rta_len < RTATTR_HDR_LEN` guard (`:223`) and a
non-advancing-position guard (`:232`), so it cannot loop forever or read out of
bounds. `NLMSG_ERROR` with a non-zero errno becomes `Err`; a `prog_id` of 0 is
normalised to `None` (`:123`) so a phantom prog 0 cannot look like a foreign
owner. Confirms `rt-unsafe.md` §1.4.

**Capability handling.** `crates/lb/src/xdp.rs:78-119` probes `CAP_BPF` then
requires `CAP_NET_ADMIN`, falling back to `CAP_SYS_ADMIN` for pre-5.8 kernels
where the `caps` crate reports `Ok(false)` for an unknown bit. Every failure
branch logs a specific, actionable reason and returns `None`, so the gateway
starts with L4 acceleration off rather than refusing to boot or panicking — and
the fallback *is* announced (`tracing::warn!` with `xdp_enabled = false` and a
`reason`). No privileged operation occurs before the probe.

**Atomic backend publication.** `publish_backends_v4` (`loader.rs:711-743`)
bounds `count` before any write (`TooManyBackends` returned pre-write), shifts
current→previous for the Unimog daisy-chain, zeroes the tail so a shrink leaves
no addressable stale backend, and publishes the whole 3088-byte value in a
single `map.insert` — a lone `bpf_map_update_elem`, so a concurrent data-plane
lookup sees all-old or all-new. The VIP key is `u32::from(vip).to_be()`,
matching the raw network-order `dst_addr` the program passes to
`backend_table_published` (`main.rs:593`). Correct as written; unreached in
production (EBPF-S47-01).

**Conntrack map-full behaviour under a SYN flood** (explicit brief question).
Both conntrack maps are `BPF_MAP_TYPE_LRU_HASH` (confirmed as type 9 in the
object, not just in source). At capacity the kernel evicts an LRU victim rather
than failing the insert, so the answer to "what happens when the map is full"
is: **oldest-first eviction, never a cross-flow overwrite and never a hard
insert failure**. That is the correct choice and it closes the
`ENOMEM`-starves-new-flows hole EBPF-2-03 identified. Two residual notes,
already captured above rather than duplicated here: eviction pressure can be
steered by an attacker via unvalidated RSTs (EBPF-S47-16a), and the per-CPU
admission cap that is supposed to keep flood traffic out of the table never
engages (EBPF-S47-04). Neither can be reached today because nothing inserts.

---

## 3. ALREADY-KNOWN — verified as documented, not re-reported

| Item | Reference |
| --- | --- |
| Maglev consistent-hash selection in the XDP data plane is deferred | `audit/deferred.md:215-244` (ROUND8-L4-04) |
| Fragments pass to the kernel; no in-XDP reassembly | `audit/deferred.md:195-213` (ROUND8-L4-08) — *the ACL-ordering consequence, EBPF-S47-05, is not covered there* |
| `bpftool` / libbpf ≥ 1.0 cannot load the legacy-`maps` object; aya only | `audit/deferred.md:287-290` (D-2) — this is why `bpftool prog load` was not a usable verification path here |
| Native DRV attach on ENA fails through aya's `bpf_link`; netlink attach fallback missing | `audit/deferred.md:278-285` (D-1); `netlink_xdp.rs` implements query only, so the gap stands |
| ENA native XDP needs MTU ≤ 3498 and combined channels ≤ max/2 | `audit/deferred.md:270-276`, `docs/guide/DEPLOYMENT.md:198-227` |
| XDP off by default; single-kernel; not CO-RE-portable | `docs/known-limitations.md:228-250` |
| QinQ / stacked 802.1Q unsupported | ADR-0005 follow-ups |
| `probe_xdp_silent_drop` is a scaffold returning `ProbeUnavailable` | `nic_compat.rs:297-307`, disclosed in the module header |
| 7.x outside the official verifier matrix (open product decision) | `audit/ebpf/verifier-logs/README.md` |

---

## 4. Summary

| ID | Sev | Location | Claim |
| --- | --- | --- | --- |
| EBPF-S47-01 | HIGH | `crates/lb/src/xdp.rs:188-200` | Program is attached but no map is ever populated — every packet takes the CT-miss `XDP_PASS` branch; the L4 accelerator is a no-op |
| EBPF-S47-02 | HIGH | `crates/lb-l4-xdp/src/loader.rs:632-634` | EBPF-2-05 (pinning), closed "Verified-Fixed", is unreachable in production; pins are also unversioned |
| EBPF-S47-03 | MED | `crates/lb/src/main.rs:2475-2500` | All 4 XDP metrics inert; `xdp_sampler_errors_total` climbs 1/s in the default config → a RUNBOOK alert fires forever |
| EBPF-S47-04 | MED | `crates/lb-l4-xdp/ebpf/src/main.rs:335-343` | `is_under_flood()` can never fire — the cfg slot always reads 0, which the code treats as "disabled"; the 125 k fallback is dead |
| EBPF-S47-05 | MED | `crates/lb-l4-xdp/ebpf/src/main.rs:512-523`, `:736` | Deny ACL is bypassable by IP fragmentation (fragment `PASS` precedes the lookup) and absent on IPv6 |
| EBPF-S47-06 | MED | `crates/lb-l4-xdp/ebpf/src/main.rs:639-665` | Partial rewrite: `TcpHdr` 16 B parsed vs `TcpHdrRW` 20 B rewritten — L2/L3 mutated before an L4 bounds check that can fail |
| EBPF-S47-07 | MED | `crates/lb-l4-xdp/build.rs:16-28` | Nothing rebuilds or validates the committed object against its source; drift proven via its own `.BTF.ext` line info |
| EBPF-S47-08 | MED | `audit/ebpf/verifier-logs/5.15.log.committed` | The claimed 5.15/6.1/6.6 verifier matrix does not exist — placeholders, and CI never runs `verify-xdp.sh` |
| EBPF-S47-09 | MED | `crates/lb-l4-xdp/tests/round8_ptr_at_bounds.rs:5-15` | No behavioural coverage of the real BPF program; "proof" tests re-implement the logic inside the test file |
| EBPF-S47-10 | LOW | `crates/lb-l4-xdp/src/loader.rs:995-1008` | `attach_replacing` is detach-then-attach: bare-interface window; failed re-attach leaves the NIC unprotected |
| EBPF-S47-11 | LOW | `crates/lb-l4-xdp/src/nic_compat.rs:233-241` | `ethtool` spawned from `$PATH` with no timeout while holding ambient `CAP_BPF`/`CAP_NET_ADMIN` |
| EBPF-S47-12 | LOW | `crates/lb-l4-xdp/src/netlink_xdp.rs:323-331` | Netlink `recv`: 32 KiB fixed, no `MSG_TRUNC`/timeout/seq check → oversized reply leaves the program attached at shutdown |
| EBPF-S47-13 | LOW | `crates/lb-l4-xdp/ebpf/src/main.rs:706-710`, `:927-931` | A rewritten UDP checksum of `0x0000` is emitted as "no checksum"; illegal for IPv6, receiver discards |
| EBPF-S47-14 | LOW | `crates/lb-l4-xdp/ebpf/src/main.rs:630-720`, `:864-941` | No TTL / hop-limit decrement on the DNAT-and-forward path |
| EBPF-S47-15 | LOW | `crates/lb-l4-xdp/ebpf/src/main.rs:559`, `:808` | `L7_PORTS` looked up in host order; ADR-0005 specifies network order — latent silent-miss trap |
| EBPF-S47-16 | LOW | `crates/lb-l4-xdp/ebpf/src/main.rs:577-583` | Unvalidated TCP RST evicts a conntrack entry; `tot_len` never bounds the L4 parse, so Ethernet padding parses as TCP |
| EBPF-S47-17 | INFO | `docs/decisions/ADR-0005-bpf-map-schema.md` | The frozen-schema ADR no longer describes the schema (phantom `flags`, wrong sizes, wrong map type) |
| EBPF-S47-18 | INFO | `crates/lb-l4-xdp/src/sim.rs:124-181` | Simulator is independently re-derived and models a strict subset; cannot detect eBPF drift |
| EBPF-S47-19 | INFO | `crates/lb-l4-xdp/src/lib.rs:361-372` | `HotSwapManager` pins by backend index; a count-preserving reorder silently re-pins live flows |

Cross-agent: confirms `rt-unsafe.md` §1.3-§1.6 in full (no soundness
contradiction); EBPF-S47-06 and EBPF-S47-12 are the logic/availability
counterparts of sites that agent correctly ruled memory-safe.
