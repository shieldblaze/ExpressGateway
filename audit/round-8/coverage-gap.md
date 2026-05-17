# Round 8 Phase C — Coverage Gap Analysis (verify)

## Headline counts

| Tag | Count | % of 39 |
|---|---:|---:|
| NEW       | 24 | 61.5% |
| MISSED    | 14 | 35.9% |
| DISPUTE   |  1 |  2.6% |
| **Total** | **39** | **100%** |

**MISSED rate is 35.9% — 3.6× the 10% threshold for "systematic blind spot."**
Escalation per task brief: declare systematic blind spots and theme them.

## Per-finding tag (one-liner each)

### L7

| ID | Tag | Prior-coverage pointer |
|---|---|---|
| ROUND8-L7-01 | NEW    | no prior look at `handle_ws_upgrade` |
| ROUND8-L7-02 | NEW    | `lb-h1::chunked::try_read_size` not audited |
| ROUND8-L7-03 | NEW    | empty-header-name primitive never in smuggle matrix |
| ROUND8-L7-04 | NEW    | XFF / list-rule iteration never audited |
| ROUND8-L7-05 | NEW    | `_` ↔ `-` header normalisation never audited |
| ROUND8-L7-06 | NEW    | per-connection request-count cap never audited |
| ROUND8-L7-07 | NEW    | H2 frame-level recv timeout never audited |
| ROUND8-L7-08 | NEW    | RST_STREAM(CANCEL) on app-level timeout never audited |
| ROUND8-L7-09 | MISSED | PROTO-2-15/-18 audited SNI/Host agreement; comma/control-char *value sanitisation* missed |
| ROUND8-L7-10 | MISSED | CODE-2-09 ported `acquire_async`; H1 take-and-discard pattern undocumented + `set_reusable` API unused |
| ROUND8-L7-11 | NEW    | `lb-h2::frame::decode_frame_low` PADDED flag never audited |
| ROUND8-L7-12 | NEW    | operator-UX of N H2 abuse counters never audited |
| ROUND8-L7-13 | NEW    | URI / path normalisation never audited |
| ROUND8-L7-14 | MISSED | CODE-2-11 shipped proptest harnesses; "rejects CVE-class inputs" assertions absent |
| ROUND8-L7-15 | NEW    | canonical Envoy+nginx edge-defaults table never audited |

### L4

| ID | Tag | Prior-coverage pointer |
|---|---|---|
| ROUND8-L4-01 | NEW    | backend-index-0 sentinel (Katran lesson 10) never audited |
| ROUND8-L4-02 | MISSED | SEC-2-02 / EBPF-2-03 swapped HASH→LRU; Cilium TCP-state-aware pruning never audited |
| ROUND8-L4-03 | NEW    | new-flow-rate cap (Katran lesson 4) never audited |
| ROUND8-L4-04 | NEW    | atomic backend-table publication (Unimog daisy-chain) never audited |
| ROUND8-L4-05 | MISSED | EBPF-2-04 ladders Drv→Skb on attach-syscall error; silent-drop probe missing |
| ROUND8-L4-06 | NEW    | `insert_acl_deny` `prefix_len=0` admission gate never audited |
| ROUND8-L4-07 | NEW    | `BackendEntry::flags` dead-code + doc lie never audited |
| ROUND8-L4-08 | NEW    | IPv4 / IPv6 fragment handling never audited |
| ROUND8-L4-09 | MISSED | EBPF-2-07 was supposed to capture verifier logs that would surface aya #1562 reordering; gate is dormant |
| ROUND8-L4-10 | DISPUTE| EBPF-2-07 marked `Verified-Fixed(ffde98c)` but no `.log.committed` baselines exist; gate is a permanent no-op |
| ROUND8-L4-11 | MISSED | EBPF-2-05 pinning landed; mount-type guard at runtime missing |
| ROUND8-L4-12 | MISSED | EBPF-2-04 + CODE-2-10 audited attach/drop; `BPF_F_REPLACE` + `bpf_xdp_query` coexistence missing |

### OPS

| ID | Tag | Prior-coverage pointer |
|---|---|---|
| ROUND8-OPS-01 | MISSED | REL-2-01 doc rewrite missed the README FD-passing claim |
| ROUND8-OPS-02 | NEW    | drain jitter / probabilistic close (Envoy `drain_manager_impl.cc`) never audited |
| ROUND8-OPS-03 | MISSED | REL-2-02 spec'd histogram + counter; only counter shipped; status overclaimed Verified-Fixed |
| ROUND8-OPS-04 | MISSED | CODE-2-03 migrated spawns to tracker; accept-loop *body* still uses `JoinHandle::abort` |
| ROUND8-OPS-05 | MISSED | REL-2-08 startup-time cardinality check landed; per-emission gate never audited |
| ROUND8-OPS-06 | MISSED | REL-2-07 already disclosed Partial; L7 wire-in still missing after 3 rounds |
| ROUND8-OPS-07 | MISSED | SEC-2-11 closed CAP_SYS_ADMIN fallback; `systemd-analyze security` scoring never performed |
| ROUND8-OPS-08 | NEW    | SBOM provenance (`manual-fallback` tool) deferred to CI, never closed |
| ROUND8-OPS-09 | MISSED | REL-2-01 / REL-2-14 added narrow doc-lint patterns; broader claim-drift philosophy never adopted |
| ROUND8-OPS-10 | NEW    | drain budget tuning vs Pingora `EXIT_TIMEOUT = 300s` for streaming workloads never audited |
| ROUND8-OPS-11 | MISSED | REL-2-04 shipped probe split; `readiness_settle_ms=1000` vs kubelet 10s probe period never audited |
| ROUND8-OPS-12 | NEW    | container image OCI labels / healthcheck / RO rootfs never audited |

## Systematic blind-spot themes

14 MISSED items cluster into four themes:

### Theme 1 — "Verified-Fixed" snapshot of script existence, not capability

The audit register treated "the script/library/API surface exists"
as equivalent to "the capability is wired into production." Five
findings demonstrate this:

- **EBPF-2-07** (DISPUTE / ROUND8-L4-10): verify-xdp.sh script exists;
  `.log.committed` baselines do not; gate is dormant.
- **REL-2-07** (MISSED / ROUND8-OPS-06): W3C codec library exists; zero
  L7 callsites use it. Status was already `Verified-Fixed-Partial`
  with the gap disclosed; subsequent rounds did not close.
- **REL-2-02** (MISSED / ROUND8-OPS-03): drain ordering wired; one of
  two required metrics did not ship; status was wrongly upgraded to
  `Verified-Fixed`.
- **CODE-2-03** (MISSED / ROUND8-OPS-04): spawn-tracker migration
  shipped; per-listener accept-loop cancel arm did not; status note
  overclaims scope.
- **REL-2-08** (MISSED / ROUND8-OPS-05): startup-time cardinality
  check landed; runtime per-emission guard did not; status was already
  `Verified-Fixed-Partial` with disclosure.

**Action**: the audit-of-audit gate Round-8 lead arbitrated under
bundle B-2 (L4-10 + OPS-09) — `doc-lint.sh` must verify that every
`Verified-Fixed(<sha>)` claim's referenced artefact actually closes
the recommendation. The current `doc-lint` cannot tell the difference
between "script committed" and "script + baseline committed".

### Theme 2 — Operational-vs-laboratory test posture

Five findings would only bite at scale or in multi-replica deployments:

- **ROUND8-OPS-02** (drain jitter / thundering herd): only visible
  with >2-3 replicas + stateful upstream LB.
- **ROUND8-OPS-11** (readiness_settle vs kubelet probe period): only
  visible against a real K8s control plane.
- **ROUND8-L4-02** (replay-old-RST evicts live flows): only visible
  at >100k flows/sec.
- **ROUND8-L4-03** (SYN-flood new-flow-rate cap): only visible at
  >125k flows/sec/core.
- **ROUND8-L4-04** (atomic backend-table updates): only visible at
  >>10s of backends + high rebalance frequency.

**Action**: Round 1-7 ran against single-instance test setups; the
Pillar 4b roadmap must include a load-test / multi-replica gate that
exercises these classes. Phase D plans for L4-02/-03/-04 already
acknowledge "Pillar 4b-3 or later"; we should pin a specific
acceptance criterion.

### Theme 3 — Doc-vs-code claim drift

Three findings expose that the operator-facing claims drifted from
source after REL-2-01:

- **ROUND8-OPS-01** (README FD-passing claim).
- **ROUND8-OPS-09** (doc-lint philosophy too narrow).
- **ROUND8-L4-07** (`BackendEntry::flags` doc lies about behaviour).

The pattern: REL-2-01 audited *operator-facing prose* (README /
DEPLOYMENT / RUNBOOK). It did not audit *in-source doc comments* on
struct fields, or aspirational language ("Zero-downtime via FD
passing") whose implementation was explicitly deferred. The
`doc-lint.sh` gate ratchets the exact bug class it was written for
(binary-name) and nothing else.

**Action**: the bundle B-2 plan (L4-10 + OPS-09) covers this; the
plan needs to extend doc-lint to a positive-assertion model where
defaults documented in RUNBOOK are imported from source consts.

### Theme 4 — Multi-validator audit handoff

Four findings show that one validator examined an adjacent surface
and another should have walked the predicate but didn't:

- **PROTO-2-15/-18 ↔ ROUND8-L7-09** (SNI/Host agreement audited;
  comma/control-char *value sanitisation* missed).
- **CODE-2-09 ↔ ROUND8-L7-10** (`acquire_async` ported; H1
  take-and-discard pattern undocumented + `set_reusable` unused).
- **CODE-2-11 ↔ ROUND8-L7-14** (proptest harnesses shipped; CVE-class
  rejection assertions absent — the "no panic" + "bounded
  consumption" harness was the floor; nobody walked back to seed
  the inputs the references paid for).
- **EBPF-2-04 ↔ ROUND8-L4-12** (attach mode ladder audited; multi-
  program `BPF_F_REPLACE` coexistence not).

**Action**: Phase D plans for these four findings must include a
cross-area sign-off step: when a `code` finding touches L7 hot path,
the `proto` validator should re-walk the predicate; when an `ebpf`
finding touches the attach surface, the `code` validator should walk
multi-program coexistence; when a `code` finding ships a proptest
harness, the `protocol` validator should seed CVE corpus.

## Recommendation to Phase D

Per task brief, MISSED > 10% triggers "systematic blind spot." We are
3.6× the threshold across four distinct themes. Three options:

1. **Proceed to Phase D normally with theme-aware plan ownership.**
   The 14 MISSED items + 1 DISPUTE need plan owners; the lead's
   bundle list in `cross-review.md §B` already groups them sensibly.
   Themes 1-4 above should be cited in each plan as the audit-failure-mode
   that allowed the bug to slip through.

2. **Loop back to a deeper Round 1-7 redo.** Round-8 NEW count is
   24/39 (61.5%) — these are areas the prior audit didn't look at,
   not areas it dismissed. A deeper Round 1-7 redo would re-walk
   those areas at the same depth as the original 70 findings. Cost
   is high (4-6 weeks); marginal value vs. proceeding with Phase D
   is unclear — the new findings are *additional surface*, not
   *re-evaluation of the original surface*.

3. **DISPUTE-only mini-loop first.** ROUND8-L4-10 alone is the
   single DISPUTE; arbitration is straightforward (re-classify
   EBPF-2-07 to Partial; commit baselines as part of Phase D plan).
   This does not require a separate loop.

**Recommendation: Option 1 (proceed normally) with theme citations
in each Phase-D plan and an explicit Phase-D acceptance gate that
re-runs the audit-of-audit doc-lint extension before close.**

The escalation rule (>10% MISSED → blind spot) is satisfied, but the
blind spots are localised to four themes and do not undermine the 53
prior `Verified-Fixed` items we spot-checked at 17/17 STILL-VERIFIED
or PARTIAL with the partial gap disclosed (with one exception:
EBPF-2-07, which is the DISPUTE).
