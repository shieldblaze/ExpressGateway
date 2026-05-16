# Plan for ROUND8-L4-09 — Overflow-safe `ptr_at` bounds check + property test

Finding-ref:     ROUND8-L4-09 (medium, Open)
Reference:       aya issue #1562 (Rust LLVM scalar/pointer reordering); xdp-tutorial network-byte-order lesson; CVE-2022-23222 (pointer-arith bounds-check elision class); handoff item 6.
Coverage-gap:    Theme 1 (EBPF-2-07 verifier-log gate dormant — would have surfaced any aya #1562-class reordering). Theme 4 (multi-validator: ebpf audited verifier-log capture; nobody walked the arithmetic-overflow class).

Files touched:
  - `crates/lb-l4-xdp/ebpf/src/main.rs`             (`ptr_at` / `ptr_at_mut`: checked-add bounds)
  - `crates/lb-l4-xdp/tests/round8_ptr_at_bounds.rs` (NEW — proptest)
  - `audit/ebpf/verifier-logs/{5.15,6.1,6.6}.log.committed`  (regenerate after patch; coordinated with L4-10)

Approach:

1. **Checked-add bounds in `ptr_at`** (`crates/lb-l4-xdp/ebpf/src/main.rs:259-281`):
   - Today:
     ```rust
     let start = ctx.data();
     let end = ctx.data_end();
     let len = mem::size_of::<T>();
     if start + offset + len > end {
         return None;
     }
     Some((start + offset) as *const T)
     ```
   - Replace with:
     ```rust
     let start = ctx.data();
     let end = ctx.data_end();
     let len = mem::size_of::<T>();
     // Defence against CVE-2022-23222-class bounds-check elision:
     // any future caller with runtime-controlled `offset` or `len`
     // could wrap usize.
     let end_offset = offset.checked_add(len)?;
     let needed = start.checked_add(end_offset)?;
     if needed > end {
         return None;
     }
     // SAFETY: bounds validated, no wrap.
     Some((start.checked_add(offset)?) as *const T)
     ```
   - Same for `ptr_at_mut`.
   - `checked_add` lowers to `llvm.uadd.with.overflow.i64` which the
     BPF verifier handles cleanly (per aya issue #1562 follow-up
     discussion).

2. **Property test** (`crates/lb-l4-xdp/tests/round8_ptr_at_bounds.rs`,
   NEW; runs as a normal Rust test, no kernel needed because it
   exercises the same arithmetic via a stub `XdpContext`):
   - Test scaffolding: a `FakeXdpContext { data: usize, data_end: usize }`
     plus a `ptr_at_fake<T>(...)` that mirrors the arithmetic of
     `ptr_at` exactly. Re-implement, do not export — keeps the BPF
     crate `no_std`.
   - Properties:
     - `ptr_at_overflow_offset_returns_none`: for any
       `(start, offset, len, end)` quadruple where
       `offset + len > usize::MAX - start`, the function returns
       `None`. Use `proptest!`.
     - `ptr_at_in_bounds_returns_some`:
       `start <= start + offset + len <= end` implies `Some`.
     - `ptr_at_out_of_bounds_returns_none`:
       `start + offset + len > end` (no wrap) implies `None`.
   - At minimum 1024 cases per property; seeded corpus includes the
     boundary values `usize::MAX`, `usize::MAX - 1`, `0`,
     `mem::size_of::<Ipv4Hdr>()` (20),
     `mem::size_of::<Ipv6Hdr>()` (40), and the existing v6 ext-header
     `off` ceiling (~4136) from `handle_ipv6`.

3. **Verifier-log capture** (cross-ref ROUND8-L4-10):
   - After this patch, run `scripts/verify-xdp.sh 5.15 && scripts/verify-xdp.sh 6.1 && scripts/verify-xdp.sh 6.6`.
   - The verifier log will show `llvm.uadd.with.overflow` lowered
     instructions; diff against pre-patch baseline.
   - Commit `audit/ebpf/verifier-logs/{5.15,6.1,6.6}.log.committed`
     as the new baseline.
   - CI command (already in plan-template for L4-10):
     `scripts/verify-xdp.sh --kernel 5.15` etc. — that flag form is
     proposed by L4-10's plan (the current script takes a positional
     arg; L4-10 also normalises to the `--kernel` form).

Proof:

- `cargo test -p lb-l4-xdp --test round8_ptr_at_bounds`. Property
  cases >= 1024.
- Verifier-log capture per kernel matrix; diffs land in
  `audit/ebpf/verifier-logs/`.
- Inspection step: the verifier-log baseline must contain at least
  one `llvm.uadd.with.overflow` lowering reference for the
  `ptr_at` instantiations of `EthHdr`, `Ipv4Hdr`, `Ipv6Hdr`, `TcpHdr`,
  `UdpHdr`, `VlanHdr`.

Risk / blast radius:

- The checked arithmetic compiles to ~3 more BPF insns per `ptr_at`
  call. Hot path is dominated by packet parsing; benchmark target
  < 1% throughput impact.
- `?` operator inside `ptr_at` returns `Option<*const T>` — same
  type as today. No call-site change at the existing 12 `ptr_at::<T>`
  call sites in `main.rs`.
- The BPF verifier on 5.15 has known weaknesses with `llvm.uadd`
  intrinsics; if the new code fails to verify on 5.15, the
  fallback is hand-rolled saturating arithmetic
  (`if offset.checked_add(len).is_none() { return None; }`). L4-10's
  CI gate catches this immediately.

Cross-ref:
- ROUND8-L4-10 (verifier-log baseline): is the gate that catches
  this finding's class of regression on every patch. Land L4-10
  first; this finding's verifier-log commit is part of the
  baseline-population step.
- ROUND8-L4-04 (atomic backend-table) adds the only currently-planned
  runtime-controlled offset in the hot path (per-VIP backend index
  modulo count). That patch must use `checked_add` per this
  finding's pattern.

Owner:           div-l4
Lead-approval: approved 2026-05-14 team-lead-r8
