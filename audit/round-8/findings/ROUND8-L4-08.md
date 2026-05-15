### ROUND8-L4-08 — IPv4 fragments and IPv6 Fragment Header are silently forwarded as if they were complete L4 packets (Katran lessons 2-3)

Reference: `audit/round-8/research/katran.md` lessons 2-3 (no fragment handling — drop or pass to kernel); `audit/round-8/research/cilium.md` "Non-goals: No fragment reassembly"
Our equivalent: `crates/lb-l4-xdp/ebpf/src/main.rs:395-477` (IPv4 path), `:595-684` (IPv6 path)

Severity: medium
Status:   Proposed-Fix (div-l4, task#73, 2026-05-15, commit d4d81e40 `ROUND8-L4-02/08 — repair NUM_SLOTS sibling assertions`) — the push-back is addressed: `round8_fragments.rs:59` no longer hardcodes `assert_eq!(NUM_SLOTS,13)`. Re-anchored to the enum: both fragment slots must be `< NUM_SLOTS` AND `NewFlowRateCap+1 == NUM_SLOTS`. Proof: `cargo test -p lb-l4-xdp --test round8_fragments` 6/6 PASS (was 5/1-FAIL); `stats_export` 3 pass + 1 ignored. L4-03 BPF source already changed under 43d250ee; verifier-log re-capture is the ROUND8-L4-10 diff-gate's job at first privileged CI run (cross-ref L4-10; tests-only repair). Verify re-checks.
          [Prior: Push-back (verify, task#70, 2026-05-15) — eBPF frag detection correct, but proof `round8_fragments.rs:59` FAILED: hardcoded `assert_eq!(NUM_SLOTS,13)` while L4-03 grew it to 16. See audit/round-8/verify/l4.md.]

Divergence:
- Katran and Cilium are explicit non-goals: fragments are dropped or kernel-passed, because non-first fragments lack L4 ports.
- Us: the IPv4 path reads `frag_off` in the `Ipv4Hdr` struct (line 108) but `handle_ipv4` *never inspects it*. If the packet is a second-or-later fragment of a flow whose first fragment we already saw, we read random bytes from offsets where TCP/UDP ports would be, build a `FlowKey` with garbage `src_port`/`dst_port`, do a conntrack lookup on garbage, miss, and `XDP_PASS`. That's the *benign* case. The *malicious* case: the garbage happens to collide with a real flow's 5-tuple, and we rewrite the fragment's payload (which is mid-stream data, not a TCP header) thinking it's a new flow. The rewritten payload then leaves via XDP_TX.
- For IPv6, the `next_hdr` extension-header loop handles `Hop-by-Hop` and `Routing` only. The Fragment header (protocol 44) is *not* handled — if the packet is a fragmented IPv6 datagram, `next_hdr == 44`, we fall into the `_ => XDP_PASS` branch and pass to kernel. **OK in benign case but**: the fragment header is in the *first* fragment too, so even our normal-case IPv6 traffic gets passed to kernel if any extension header chain ends with `44`. We give up on a class of legitimate traffic without any counter.

Impact:
- IPv4: a non-first-fragment packet whose offset bytes coincide with port numbers of an established TCP flow gets rewritten. Mid-stream payload corruption.
- IPv6: any fragmented packet gets passed to kernel without an `XDP_FRAGMENT_UNSUPPORTED` counter. Operators have no signal.
- The Pillar 4a/4b boundary should explicitly say "fragments are dropped" or "fragments are passed". Today the behaviour is undefined.

Reproduction:
- IPv4: Send a 5000-byte UDP datagram with `DF=0`; the kernel will fragment. The second fragment has `frag_off > 0` and no UDP header. Our `handle_ipv4` reads bytes 0..4 of what used to be the L4 payload as `src_port` / `dst_port`. If those bytes happen to match a CONNTRACK entry's 5-tuple, the fragment gets rewritten.
- IPv6: Send a fragmented IPv6 packet; observe `STAT_PASS` increment but no diagnostic.

Recommendation:
1. Top of `handle_ipv4`, after reading `frag_off`:
   ```
   let frag_off = u16::from_be(unsafe { core::ptr::read_unaligned(addr_of!((*ip).frag_off)) });
   // RFC 791: bit 14 = MF (more fragments), bits 0-12 = fragment offset.
   // If either is non-zero, this is not the first complete fragment.
   if (frag_off & 0x3FFF) != 0 {
       incr_stat(STAT_FRAGMENT);
       return Ok(xdp_action::XDP_PASS); // let the kernel reassemble / handle
   }
   ```
2. For IPv6: add `IPPROTO_FRAGMENT = 44` to the extension-header skip list (or treat it as "give up, pass to kernel" with a dedicated counter). Add `STAT_V6_FRAGMENT` counter.
3. Document in `crates/lb-l4-xdp/ebpf/src/main.rs` module header that fragmented packets are passed to the kernel by design.
