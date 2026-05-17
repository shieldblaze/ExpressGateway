# Plan for ROUND8-L4-07 — Resolve `BackendEntry::flags` doc-vs-code drift

Finding-ref:     ROUND8-L4-07 (medium, Open)
Reference:       Cilium D2 (service-map sentinel for wildcard); handoff item 1 (related — backend-idx-0 vs flag-bit-0 confusion).
Coverage-gap:    Theme 3 (doc-vs-code claim drift; in-source doc comments on struct fields never audited by REL-2-01). The userspace doc on `BackendEntry.flags` says "bit 0 means rewrite and transmit" but the BPF program never reads the field.

Files touched:
  - `crates/lb-l4-xdp/ebpf/src/main.rs`             (decision point: drop the field, OR use it)
  - `crates/lb-l4-xdp/src/loader.rs`                (mirror struct + doc-comment fix)
  - `crates/lb-l4-xdp/src/lib.rs`                   (`HotSwapManager` mirrors `BackendEntry`)
  - `crates/lb-l4-xdp/tests/round8_backend_flags.rs` (NEW — proof of chosen path)

Approach (decision required at plan-approval; the lead picks one):

### Option A (honest path — recommended): drop the field

1. Remove `flags: u32` and the trailing `_pad: u16` from
   `BackendEntry` and `BackendEntryV6` (BPF side and userspace
   mirror). New layout:
   ```rust
   #[repr(C)]
   #[derive(Clone, Copy)]
   pub struct BackendEntry {
       pub backend_idx: u32,
       pub backend_ip: u32,
       pub backend_port: u16,
       pub _pad: u16,
       pub backend_mac: [u8; 6],
       pub src_mac: [u8; 6],
   }
   ```
   - Saves 4 bytes per CT entry × 1M entries = 4 MB BPF map memory.
   - **Breaking change** for the wire layout. `audit/STATE` / docs
     calling out wire-stability must be updated; the L4 BPF surface
     was already explicitly *not* wire-stable across major versions
     (this is internal kernel-side state, not a network format), so
     the breakage is contained to the single
     `crates/lb-l4-xdp/{ebpf,src}` + `lb-balancer` consumer.

2. Update `BackendEntry::new` signature (drop the `flags` arg). All
   in-tree callers in `loader.rs`, `lib.rs`, `tests/*` need the
   trivial fix (`new(idx, 0, ip, port, mac, mac)` -> `new(idx, ip, port, mac, mac)`).

3. Userspace doc comment becomes the truth: remove the
   "Reserved flag bits; bit 0 means 'rewrite and transmit'" line.

### Option B: actually use the field

1. Declare `pub const FLAG_REWRITE_TX: u32 = 1;` in `loader.rs` (and
   mirror in `ebpf/src/main.rs`).

2. In `handle_ipv4` / `handle_ipv6`, after the CT hit and AFTER the
   L4-01 sentinel check, insert:
   ```rust
   let flags = entry.flags;
   if (flags & FLAG_REWRITE_TX) == 0 {
       incr_stat(STAT_FLAG_DROPPED);
       return Ok(xdp_action::XDP_PASS);
   }
   ```
   - `STAT_FLAG_DROPPED` is a new slot (15).
   - Existing CT entries inserted via `BackendEntry::new(_, 0, _, _, _, _)`
     suddenly become "PASS instead of TX" — a migration hazard.
     Mitigation: `BackendEntry::new` defaults `flags = FLAG_REWRITE_TX`;
     existing callers explicitly passing `0` are caught at compile
     time once the constructor enforces the rule.

3. Document the field truthfully:
   "bit 0 (`FLAG_REWRITE_TX`) = rewrite L2 + emit XDP_TX; all other
    bits reserved for future use (drop, mark-for-l7, etc)."

### Option C (placeholder — not recommended)

1. Rename `flags` to `_flags` with comment `// reserved for Pillar
   4b-N`. Keeps wire-stable layout but doesn't fix the lie — only
   delays it. Reject this option unless an external consumer needs
   the slot reserved.

**Recommended path: Option A.** Wire-layout stability for this struct
is internal-only; the simplification removes a documented-but-unused
feature and cuts BPF memory.

Proof tests (`crates/lb-l4-xdp/tests/round8_backend_flags.rs`, NEW):

Under Option A:
- `backend_entry_layout_no_flags_field`: compile-time
  `mem::size_of::<BackendEntry>() == OLD_SIZE - 4`; layout offsets
  match the BPF side.
- `backend_entry_new_arity_compile_test`:
  doctest / proptest that `BackendEntry::new(1, ip, port, mac, mac)`
  type-checks; passing a stray u32 doesn't.

Under Option B:
- `flag_rewrite_tx_zero_yields_xdp_pass`: sim test, CT entry with
  `flags = 0`, packet matches; result `XDP_PASS`, `STAT_FLAG_DROPPED` ticks.
- `flag_rewrite_tx_one_yields_xdp_tx`: same with `flags = 1`,
  result `XDP_TX`.

Proof:

- `cargo test -p lb-l4-xdp --test round8_backend_flags`.
- Re-capture verifier-log baseline (L4-10).

Risk / blast radius:

- Option A: breaks any out-of-tree consumer reading the BPF map by
  raw size. No such consumer exists today; document the version
  break.
- Option B: introduces a default-deny-by-flags footgun (CT entry
  with `flags = 0` silently passes traffic instead of rewriting).
  Mitigation in step 2 (constructor default).
- Both options touch BPF struct layout → verifier-log re-capture
  required (L4-10 cross-ref).

Cross-ref:
- ROUND8-L4-01: sentinel guard is orthogonal; it gates on
  `backend_ip == 0`, not on `flags`. Both checks coexist under
  Option B; only sentinel remains under Option A.
- ROUND8-L4-04: `BackendTable.entries` layout depends on
  `BackendEntry` size. Sequence the patches so L4-04 lands after
  L4-07's chosen option.
- ROUND8-L4-10: verifier-log baseline must be re-captured per
  kernel after this patch (`scripts/verify-xdp.sh` 5.15 / 6.1 / 6.6).

Owner:           div-l4
Lead-approval: approved 2026-05-14 team-lead-r8
