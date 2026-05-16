### ROUND8-L4-07 — `BackendEntry::flags` is dead code on the BPF side; "bit 0 means rewrite and transmit" doc is a lie

Reference: `audit/round-8/research/cilium.md` D2 (service-map sentinel for wildcard) + handoff item 1 (related to backend-idx-0 question)
Our equivalent: BPF side `crates/lb-l4-xdp/ebpf/src/main.rs:186` (`flags: u32` declared, never read); loader side `crates/lb-l4-xdp/src/loader.rs:136` (`flags: u32` with comment "bit 0 means 'rewrite and transmit'")

Severity: medium
Status:   Verified-Fixed (verify, task#70, 2026-05-15) — `flags` field dropped from BPF + loader BackendEntry structs (wire layout consistent across both sides); the "bit 0" lie removed. Proof 5/5. See audit/round-8/verify/l4.md.

Divergence:
- Cilium / Katran reserve flag bits in their backend entries for things like "wildcard service", "drop", "deferred", etc. The flag bits are read in the BPF program and gate behaviour.
- Us: `BackendEntry { ..., flags: u32, ... }` is present in both the BPF struct (`ebpf/src/main.rs:186`) and the userspace mirror (`loader.rs:136`). The userspace mirror's doc says: "Reserved flag bits; bit 0 means 'rewrite and transmit'". *The BPF program never reads `flags`*. `grep -n "flags" ebpf/src/main.rs` returns only the struct-field declarations and an unrelated `_offset_flags` in the TCP rewrite struct.
- Every conntrack hit unconditionally executes `rewrite_v4` / `rewrite_v6` and emits `XDP_TX`, regardless of `flags`.

Impact:
- Wire-stable struct field promised in the API but unused. A future operator who adds a conntrack entry with `flags = 0` *expecting* "this means drop" or "this means hold-for-cnxn" gets `XDP_TX` instead — the opposite of the doc.
- More concretely: this is a half-built feature gate. The doc lies about the semantics, and a reader of `loader.rs` cannot tell whether `flags = 0` means "don't transmit" (per doc bit-0-set) or "transmit" (per actual behaviour).

Reproduction:
- `grep -nE "flags\b" crates/lb-l4-xdp/ebpf/src/main.rs` — shows only struct declarations.
- Construct `BackendEntry::new(..., flags = 0, ...)` and insert; the BPF program emits XDP_TX.
- Construct `BackendEntry::new(..., flags = 0xDEADBEEF, ...)` and insert; the BPF program ALSO emits XDP_TX. The flags field is read-only-by-userspace storage.

Recommendation:
Pick one:
1. **Honest path**: drop the `flags` field entirely. Remove from both structs. Update `BackendEntry::new` signature. Bump the wire-size const (`BACKEND_ENTRY_SIZE`).
2. **Use the field**: add `let flags = entry.flags;` in `handle_ipv4`/`handle_ipv6` after the conntrack hit. Define `const FLAG_REWRITE_TX: u32 = 1;` and gate the `rewrite_v4` call on `if flags & FLAG_REWRITE_TX != 0`. Add `STAT_FLAG_DROPPED` counter for the else branch.
3. **Rename to `_flags`** with a `// reserved for Pillar 4b-N (future feature TBD)` comment if we want to keep wire-stability for a future use.

Option 1 is the cleanest. Whichever we pick, the userspace doc must match reality.
