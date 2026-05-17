# Plan for ROUND8-L4-08 — IPv4 / IPv6 fragment handling (drop-or-pass, never rewrite)

Finding-ref:     ROUND8-L4-08 (medium, Open)
Reference:       Katran lessons 2-3 (no fragment reassembly; drop or pass-to-kernel); Cilium non-goal ("No fragment reassembly").
Coverage-gap:    Theme 1 (NEW — fragment handling never audited; Pillar 4a/4b boundary never explicitly declared the policy). Theme 4 (multi-validator handoff: code reviewed parse_hdr depth, proto reviewed L7 framing, neither walked L3 fragment offset).

Files touched:
  - `crates/lb-l4-xdp/ebpf/src/main.rs`             (IPv4: read `frag_off` and short-circuit non-first-fragment to PASS; IPv6: add `IPPROTO_FRAGMENT` to extension-header chain with dedicated counter)
  - `crates/lb-l4-xdp/src/stats_export.rs`          (`StatSlot::{V4Fragment, V6Fragment}`)
  - `crates/lb-l4-xdp/tests/round8_fragments.rs`    (NEW — sim proof)
  - `audit/deferred.md`                             (declare "fragments pass to kernel" as the design)

Approach:

1. **IPv4 first-fragment-only path** (`crates/lb-l4-xdp/ebpf/src/main.rs`,
   inside `handle_ipv4` after the existing `ihl_words` parse):
   ```rust
   // RFC 791: 16-bit field; bit 14 = MF (more fragments),
   // bit 13 = DF, bits 0-12 = fragment offset in 8-byte units.
   // If MF==1 OR offset>0, this is not a complete datagram.
   let frag_off_be = unsafe {
       core::ptr::read_unaligned(core::ptr::addr_of!((*ip).frag_off))
   };
   let frag_off = u16::from_be(frag_off_be);
   if (frag_off & 0x3FFF) != 0 {
       incr_stat(STAT_V4_FRAGMENT);
       return Ok(xdp_action::XDP_PASS); // kernel handles reassembly
   }
   ```
   - `XDP_PASS` (not `XDP_DROP`): the kernel's reassembly path is the
     reference design (Katran "pass to kernel"). Dropping fragments
     breaks legitimate UDP-large/QUIC-Initial flows.
   - Counter `STAT_V4_FRAGMENT` (slot 16) is the operator signal —
     `xdp_packets_total{result="v4_fragment"}` exposes it.

2. **IPv6 Fragment header handling** (in the v6 ext-header walk loop
   at `main.rs:603-622`):
   - Add `IPPROTO_FRAGMENT: u8 = 44;` constant.
   - Add a `match next_hdr` arm:
     ```rust
     IPPROTO_FRAGMENT => {
         incr_stat(STAT_V6_FRAGMENT);
         return Ok(xdp_action::XDP_PASS);
     }
     ```
     The Fragment header is *present in the first fragment too* — so
     we must skip the L4 parse for any v6 packet carrying it, not
     just non-first-fragments. Same "pass to kernel" policy.
   - Counter `STAT_V6_FRAGMENT` (slot 17).

3. **stats_export wiring**: `StatSlot::V4Fragment` -> `"v4_fragment"`,
   `StatSlot::V6Fragment` -> `"v6_fragment"`.

4. **Design declaration** (`audit/deferred.md`):
   - Add: "**Fragmented datagrams**: ExpressGateway XDP path does not
     reassemble fragments. IPv4 non-first fragments and any IPv6
     packet carrying a Fragment Extension Header (44) are passed to
     the kernel's network stack via `XDP_PASS`. Operators relying on
     L4-LB for fragmented flows should set the upstream MTU so
     fragmentation does not occur, or accept the kernel-stack
     latency. Counters: `xdp_packets_total{result="v4_fragment"|"v6_fragment"}`."
   - This matches Katran and Cilium's documented non-goal.

5. **Proof tests** (`crates/lb-l4-xdp/tests/round8_fragments.rs`, NEW):
   - `sim_v4_first_fragment_passes`: build an IPv4 packet with `MF=1`
     and `frag_off=0`; feed to sim; assert action == XDP_PASS and
     `STAT_V4_FRAGMENT` ticks.
   - `sim_v4_later_fragment_passes`: `MF=0` and `frag_off=1480`; same
     assertion. Critical: the L4 ports field is past the end of
     the parser's reach for a later fragment.
   - `sim_v4_unfragmented_unchanged`: `frag_off=0, MF=0`; existing
     paths fire (no `STAT_V4_FRAGMENT` increment).
   - `sim_v6_fragment_header_passes`: build a v6 packet with
     `next_hdr = 44`; assert `STAT_V6_FRAGMENT` ticks; action ==
     XDP_PASS.
   - `sim_v6_hop_by_hop_then_fragment`: stacked ext-headers
     (Hop-by-Hop -> Fragment); assert `STAT_V6_FRAGMENT` ticks (the
     walk reaches the Fragment header).
   - `sim_v4_pre_fix_cve_repro` (regression): the malicious case
     from the finding (a fragment whose bytes happen to match a real
     5-tuple) — assert no CT lookup happens for non-first-fragment,
     and the packet is not rewritten.

Proof:

- `cargo test -p lb-l4-xdp --test round8_fragments`.
- Verifier-log baseline re-capture (L4-10): new packed-field read on
  IPv4 + new ext-header arm on IPv6.

Risk / blast radius:

- Reading `frag_off` is a single packed `u16` read at a known offset
  — verifier-trivial.
- The MF/offset mask `0x3FFF` is correct in network byte order
  (bit 14 of the 16-bit field is MF, low 13 bits are offset).
  Double-check during code review against RFC 791 §3.1.
- IPv6 ext-header loop has a fixed iteration cap (currently 2) for
  verifier reasons; adding `IPPROTO_FRAGMENT` as an additional arm
  inside the existing loop body does not raise iteration count.

Cross-ref:
- ROUND8-L4-10: verifier-log re-capture.
- The `audit/deferred.md` entry coordinates with REL/OPS audit-of-audit
  (Theme 3); the decision is *explicit* not silent.

Owner:           div-l4
Lead-approval: approved 2026-05-14 team-lead-r8
