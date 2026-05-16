# Round 8 — Lead Decisions

Plan-approval and arbitration log for Phase D and beyond.

## R8-L-001 — Bulk plan approval (Phase D entry)

All 35 Phase-D plan files under `audit/round-8/fixes/` were
content-checked: each names the finding ID, the files-to-touch, a
proof test, and cites at least one of the 4 coverage-gap themes
from `audit/round-8/coverage-gap.md`. 25 plan files used the
Round-3 field-marker template (`Finding-ref:` / `Files touched:` /
`Approach:` / `Proof:` / `Lead-approval:`); the 10 div-ops plans
used a more flowing prose layout (`# Fix plan — <ID>` heading +
named sections `## A. Design` / `## B. Proof` etc). Both shapes
are accepted on substance.

**Decision: approve all 35 plans.** Field-marker plans have their
`Lead-approval:` line updated to `approved 2026-05-14 team-lead-r8`
(25 files). Prose-style plans are approved by this paragraph
en bloc.

## R8-L-002 — L7-08 hyper-1.x `send_reset` fallback

div-l7's plan flagged that hyper-1.x does not expose explicit
`send_reset` on the H2 connection wrapper. The plan relies on
hyper's drop-emits-CANCEL semantics. If, during implementation,
that assumption proves false on the pinned hyper version, the
finding is downgraded to **deferred-with-rationale** in
`audit/deferred.md` and tracked for the hyper-2.x upgrade. **Lead
pre-acknowledgement**: yes.

## R8-L-003 — L7-12 weight-table sanity check

div-l7's L7-07+L7-12 bundle proposes a HAProxy-style composite
"glitches counter" with default threshold 200 and per-kind weights
1–10. Before code lands, the implementer should sanity-check
against HAProxy's published cost table (referenced in `ref-l7`'s
research file). **Lead pre-acknowledgement**: yes — implementer
adjusts in commit message if HAProxy's defaults differ; not a
re-plan trigger.

## R8-L-004 — OPS-01 zero-downtime README claim deletion

The README claim "Zero-downtime reload via SO_REUSEPORT and FD
passing" is aspirational. The bundle plan `OPS-01-L4-12-L4-04.md`
both deletes the claim and adds the corrected behaviour
(bounded-outage detach). **Lead pre-acknowledgement**: yes —
this is the lead-pre-approved disposition from `cross-review.md`
§D.3.

## R8-L-005 — Drain coordinator ownership

`OPS-04-L4-12.md` is the single drain-coordinator plan. Every
listener (TCP / TLS / H2 / H3 / admin) consumes the coordinator's
phase signal. div-l4 contributes the XDP detach signature;
div-l7 contributes the per-protocol close call. **Lead
pre-acknowledgement**: yes.

## R8-L-006 — Audit-of-audit doc-lint gate (L4-10+OPS-09)

The new `scripts/ci/doc-lint.sh` extension parses every
`Verified-Fixed(<sha>)` claim in `audit/<area>/round-2-*.md`
and verifies the SHA's diff content matches the recommendation
(metric names, file paths, function idents). This would have
caught EBPF-2-07's no-op last round. **Lead pre-acknowledgement**:
yes — must run on every PR.
