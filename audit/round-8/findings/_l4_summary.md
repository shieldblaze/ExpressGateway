# L4 / XDP / eBPF — Round 8 divergence-analyst summary (`div-l4`)

## Scope

Crates audited: `crates/lb-l4-xdp/` (BPF program + aya loader + stats export
+ userspace sim), `crates/lb/src/xdp.rs` (capability probe + attach
helper), `scripts/build-xdp.sh`, `scripts/verify-xdp.sh`.

Bar: examine ≥10 candidate divergences against the production
references in `audit/round-8/research/`. Bar met: 12 open findings +
10 examined-and-resolved.

## Open findings (file: ROUND8-L4-NN.md)

| ID            | Sev    | One-liner                                                                            |
|---------------|--------|--------------------------------------------------------------------------------------|
| ROUND8-L4-01  | high   | Backend-index 0 / zero-IP is a valid lookup result; no sentinel (Katran lesson 10)   |
| ROUND8-L4-02  | high   | Pure LRU conntrack, no TCP state awareness — replay-old-RST evicts live flows        |
| ROUND8-L4-03  | high   | No SYN-flood / new-flow-rate cap; LRU thrashes under flood                           |
| ROUND8-L4-04  | medium | Backend-table updates are non-atomic; mid-update flows see partial state             |
| ROUND8-L4-05  | medium | Drv-mode attach never runtime-probes XDP_TX; MLX5/CX6 silent-drop class undetectable |
| ROUND8-L4-06  | high   | `insert_acl_deny(prefix_len=0)` accepted → default-deny footgun                       |
| ROUND8-L4-07  | medium | `BackendEntry::flags` is dead code on BPF side; userspace doc lies                   |
| ROUND8-L4-08  | medium | IPv4 fragments rewritten as if first-fragment; IPv6 Fragment header passed silently  |
| ROUND8-L4-09  | medium | `ptr_at` has no overflow guard on `start + offset + len`; aya #1562 class            |
| ROUND8-L4-10  | high   | EBPF-2-07 graded Verified-Fixed but no verifier-log snapshots committed              |
| ROUND8-L4-11  | medium | Loader doesn't enforce bpffs mount-type before pinning; opaque failure mode          |
| ROUND8-L4-12  | medium | XDP attach doesn't use `BPF_F_REPLACE`; can't gracefully re-attach over prior LB     |

## Examined-and-resolved (file: ../divergence/l4-resolved.md)

10 patterns where reference projects diverge from us but we made a
deliberate or unaffected choice. Captured to avoid re-asking next round.

## Notable audit-of-audit observations

- **ROUND8-L4-10** is the most pointed: Round-7's EBPF-2-07 row says
  `Verified-Fixed(ffde98c)`. That commit added a script and a README,
  no actual verifier-log snapshots. The diff-gate is dormant. The
  `audit/unsafe-justifications.md:109` claim that the verifier-log
  diff is a CI gate is currently false.
- **Round-7 EBPF-2-04** (attach-mode fallback ladder) correctly fixed
  the literal complaint ("attach is hard-coded to SKB"). It did not
  cover the **silent-drop on Drv mode** class (ROUND8-L4-05) or the
  **co-tenant clobber** class (ROUND8-L4-12). Both classes were named
  in handoff and missed in Round 7.
- **Round-7 EBPF-2-03** (LRU swap) correctly fixed the ENOMEM-on-fill
  problem. The Cilium-flavoured TCP-state-aware pruning (ROUND8-L4-02)
  and the Katran flood cap (ROUND8-L4-03) are separate problems the
  swap doesn't solve; the Round-7 review treated EBPF-2-03 as closing
  *all* conntrack concerns. Not so.
- **`BackendEntry::flags`** (ROUND8-L4-07) is a half-built feature:
  documented behaviour ("bit 0 = rewrite-and-TX") doesn't match
  runtime (always rewrite-and-TX). The previous audit's EBPF-2-09
  Pod-padding review touched these structs and did not catch the
  dead-field mismatch.

## Spot-check on prior Verified-Fixed grades

Sampled 4 of the 8 EBPF-2-NN grades from `audit/ebpf/round-2-review.md`:

| ID         | Round-2 grade           | Round-8 spot-check                                          |
|------------|-------------------------|-------------------------------------------------------------|
| EBPF-2-01  | Verified-Fixed(67117a5) | `link_section = "license"` present in `ebpf/src/main.rs:45` — OK |
| EBPF-2-03  | Verified-Fixed(c009219) | `LruHashMap` in place, sized 1M/512k — OK as far as ENOMEM goes |
| EBPF-2-04  | Verified-Fixed(75d4740) | Ladder present, label recorded — but see ROUND8-L4-05/12 for incomplete fix |
| EBPF-2-07  | Verified-Fixed(ffde98c) | **FAIL** — no committed logs; see ROUND8-L4-10  |

EBPF-2-07 is the one that didn't land. The other three landed the
code change but each closes a narrower problem than the audit prose
implies.

## Recommended fix-pack priority

1. ROUND8-L4-06 (ACL `/0` deny) — one-line userspace guard; production
   safety, no design work.
2. ROUND8-L4-10 (verifier log snapshots) — unblocks ROUND8-L4-09
   confirmation; needs a privileged CI run.
3. ROUND8-L4-01 + ROUND8-L4-07 (backend-idx-0 sentinel + dead `flags`)
   — bundle; one commit can address both cleanly.
4. ROUND8-L4-08 (fragment handling) — adds 2 stat slots, ~20 LoC BPF.
5. ROUND8-L4-12 (attach replace semantics) — graceful redeploy story.
6. ROUND8-L4-02 + ROUND8-L4-03 (TCP-state + flood cap) — defer to
   Pillar 4b-3 if necessary, but document the deferral and the
   consequences in an ADR.
7. ROUND8-L4-11 (bpffs mount check) — small but high-signal-to-noise.
8. ROUND8-L4-05 (silent-drop probe) — needs hardware to test; can
   defer to a later round if no MLX5/CX6 deploy is planned soon.
9. ROUND8-L4-04 (atomic backend-table swap) — biggest design lift;
   ADR-track.
10. ROUND8-L4-09 (ptr_at overflow guard) — small, cheap, do alongside
    the verifier-log work in #2.

No code changes proposed/applied in this round.
