### ROUND8-L4-09 — `ptr_at` bounds check uses scalar+pointer order vulnerable to aya #1562 / Rust-LLVM verifier rejection

Reference: `audit/round-8/research/aya.md` lesson 3 / issue #1562 (Rust LLVM emits `scalar += pkt_ptr` instead of `pkt_ptr += scalar`); xdp-tutorial network-byte-order lesson; handoff item 6
Our equivalent: `crates/lb-l4-xdp/ebpf/src/main.rs:259-281` (`ptr_at` / `ptr_at_mut`)

Severity: medium
Status:   Verified-Fixed (verify, task#70, 2026-05-15) — ptr_at/ptr_at_mut use checked_add(offset)?.checked_add(len)? then >end then re-derive addr; wrap-around bounds-check elision (aya#1562/CVE-2022-23222) closed. Proof 7/7. See audit/round-8/verify/l4.md.

Divergence:
- The verifier-safe pattern (clang): `data + offset > data_end` where `data` is the LHS of every addition.
- Aya #1562 documents that the Rust LLVM backend sometimes reorders operands to `scalar + pkt_ptr`, which loses pointer provenance, and the verifier rejects the program.
- Our `ptr_at`:
  ```rust
  if start + offset + len > end {
      return None;
  }
  Some((start + offset) as *const T)
  ```
  uses `start + offset + len`. `start` and `end` come from `XdpContext::data()` / `data_end()` (both `usize`). `offset` and `len` are `usize`. After type unification everything is scalar `usize`, so the verifier sees pure scalar arithmetic — *which is actually safer for the verifier than pkt_ptr arithmetic*. The classic aya #1562 reject doesn't apply *here* (because we don't add to a packet pointer, we add to a u64 representation of it).
- BUT: there is no overflow check on `start + offset + len`. If `len` were e.g. `usize::MAX / 2` and `offset` were the same, the sum wraps below `end` and the bounds check passes spuriously. `len = size_of::<T>()` for in-tree types is always small (<=40), and `offset` is internally controlled — so the practical exploit surface is zero today. But this is the exact pattern that bit the kernel in CVE-2022-23222 (pointer-arith bounds-check elision).
- More concretely the lesson the references paid for: any time we touch packet-pointer arithmetic, we need the verifier-log proof. We have no committed verifier logs (see ROUND8-L4-10).

Impact:
- Today: no exploit because all `offset` and `len` values are compile-time-known small constants.
- Future-bug-shape: anyone who refactors `ptr_at` to take a runtime-controlled offset (e.g. variable IPv6 ext-header walk where `off` grows) needs to add an overflow guard. The IPv6 ext-header walk at `main.rs:603-622` *does* accumulate `off` from untrusted packet bytes (`hdr_ext_len`); `off += (usize::from(len) + 1) * 8`. With `len = 0xFF`, one iteration adds 2048; with the loop bound of 2 iterations and the existing `IPV6_HDR_LEN = 40` start, `off` reaches at most ~4136. That's safe. *But* the bounds-check elision principle applies — any future caller that loops differently can wrap.

Reproduction:
- Build the ELF with `RUSTFLAGS="--emit=llvm-ir"`; grep the IR for the `ptr_at` instantiations; verify the verifier-log shows `start += offset` order, not `offset += start`. We don't have this captured (see ROUND8-L4-10).

Recommendation:
1. Add an overflow-safe bounds check using checked arithmetic:
   ```rust
   let end_offset = offset.checked_add(len).ok_or(())?;
   let needed = start.checked_add(end_offset).ok_or(())?;
   if needed > end {
       return None;
   }
   ```
   The BPF verifier accepts `checked_add` lowered through `llvm.uadd.with.overflow.i64` cleanly.
2. Add a property-style test that exercises `ptr_at` with synthetic `(start, offset, len, end)` quadruples including overflow boundaries — runs in a normal Rust test, no kernel needed.
3. **Block on this**: capture the verifier log per kernel matrix (5.15 / 6.1 / 6.6) per ROUND8-L4-10, then diff against `clang -target bpf -O2`-emitted reference IR for the same pattern.
