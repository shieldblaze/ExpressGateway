# Round 8 — Phase B Cross-Review Synthesis (team-lead)

3 cross-review files landed under `audit/round-8/cross-review-by-{l7,l4,ops}.md`
(887 lines total). 39 findings examined (15 L7 + 12 L4 + 12 OPS).
This file applies lead arbitrations and pre-stages Phase C.

## A. Severity arbitrations applied (concurred by ≥2 reviewers)

| Finding | From | To | Concurrence | Rationale |
|---|---|---|---|---|
| **ROUND8-L7-01** WS premature 101 | high | **critical** | div-ops + div-l7 | Same shape as Pingora GHSA-xq2h-p299-vjwv / Envoy GHSA-rj35-4m94-77jh (both CVSS 9.3). Combined with OPS-06 (no tracing callsites), an on-call engineer has no span to correlate the upgrade failure to the upstream dial. Operationally blind + remotely-triggerable. |
| **ROUND8-L7-04** XFF clobber | medium | **high** | div-ops | Envoy GHSA-ghc4-35x6-crw5 exact shape. Combined with no audit log, a successful RBAC bypass downstream of us would be invisible to our telemetry. |
| **ROUND8-L7-08** | medium | **high** | div-ops | Per cross-review-by-ops.md (escalation rationale therein). |
| **ROUND8-L4-05** | medium | **high** | div-ops | Per cross-review-by-ops.md. |
| **ROUND8-OPS-04** TCP listener `JoinHandle::abort()` | medium | **high** | div-l7 | Monotonic per-IP-counter drift across drains; default-config bite. |
| **ROUND8-L7-12** | low | **medium** | div-ops | Ops-grounds reframe (consolidated H2 glitches counter rationale). |

No CHALLENGEs survived the cross-review; all 39 findings stand.

## B. Cross-cuts to bundle for Phase D plan-ownership

Bundles consolidate cross-cutting findings under a single Phase-D plan
owner; each constituent finding retains its ID but the plan is shared.

1. **L7-01 + OPS-06** — premature 101 + zero tracing callsites. Single
   plan: defer-101-until-upstream-completes AND wire `traceparent`
   propagation through the upgrade path. **Owner: div-l7 (primary)
   with div-ops as second author on the tracing side.**
2. **L4-10 + OPS-09** — verifier-log no-op + doc-lint stub. Audit-of-audit
   gate: `doc-lint.sh` must reject any `Verified-Fixed` claim that
   doesn't have a matching committed artefact (commit content match,
   not just commit existence). **Owner: div-ops.**
3. **L7-07 + L7-12** — consolidated H2 "glitches counter" composite-knob
   per HAProxy pattern. Replace N independent counters with one
   tunable. **Owner: div-l7.**
4. **L4-11 + OPS-07** — bpffs mount + systemd hardening
   (`RequiresMountsFor=/sys/fs/bpf`, `SystemCallFilter`,
   `RestrictAddressFamilies`, `ProtectKernel*`). **Owner: div-ops.**
5. **L4-12 + OPS-04** — XDP clean detach + accept-loop cancel arm.
   The "drain" needs to be a single coordinator, not two
   independent code paths. **Owner: div-ops (drain coordinator);
   div-l4 contributes the XDP detach signature.**
6. **L7-14 proptest seeds** — extend the proptest scaffolding to
   `lb-l4-xdp/tests/` per div-l4's cross-cut. **Owner: div-l7.**
7. **OPS-01 + L4-12 + L4-04** — `SO_REUSEPORT + FD passing`
   zero-downtime claim is fictional on BOTH L4 (no `BPF_F_REPLACE`)
   AND L7 (no actual FD-pass code). Either implement OR delete the
   claim from README. **Owner: div-ops.**

## C. Severity table after arbitration

| Severity | L7 | L4 | OPS | Total |
|---|---|---|---|---|
| **Critical** | 1 (L7-01) | 0 | 0 | **1** |
| **High** | 4 (L7-02, -04, -07, -08) | 5 (L4-01, -02, -03, -05, -06, -10) — wait, that's 6 | 3 (OPS-01, -04, -06) | varies — see below |

Let me re-tally honestly from finding files post-arbitration:

| Severity | Findings |
|---|---|
| critical | L7-01 |
| high     | L7-02, L7-04, L7-07, L7-08, L4-01, L4-02, L4-03, L4-05, L4-06, L4-10, OPS-01, OPS-04, OPS-06 |
| medium   | L7-03, L7-05, L7-06, L7-09, L7-11, L7-12, L7-13, L4-04, L4-07, L4-08, L4-09, L4-11, L4-12, OPS-02, OPS-03, OPS-05, OPS-07, OPS-09, OPS-10, OPS-11 |
| low      | L7-10, L7-14, L7-15, OPS-08, OPS-12 |
| info     | (none — all findings are at least low) |

**Headline counts post-arbitration:**
- Critical: **1**
- High: **13**
- Medium: **20**
- Low: **5**
- **Total medium+: 34** out of 39 findings → 34 plans owed in Phase D.

## D. Decisions reserved for the lead (no further reviewer input needed)

1. **Apply the bundles in §B** — bundles do not change finding IDs;
   they reduce Phase-D plan count from 34 to ~28.
2. **L4-10 audit-of-audit gate** — `doc-lint.sh` already exists per
   the prior audit's REL-2-01; div-ops's OPS-09 says it's too thin.
   Phase D plan must extend it to verify the *content* of every
   `Verified-Fixed(<sha>)` claim matches the SHA's actual diff.
3. **OPS-01 README zero-downtime claim** — given the cross-cut with
   L4-12 + L4-04, the realistic choice is **delete the claim** for
   v1 and re-add later when actual handoff is implemented. Lead
   pre-approves this disposition; div-ops plan should reflect.
4. **Phase B is closed.** Move to Phase C.
