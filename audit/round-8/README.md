# Round 8 — Adversarial Re-Audit

Independent re-audit of `prod-readiness/round-4` (Round-7 verdict was
CONDITIONAL GO). Round 8's stance is **adversarial**: the prior team
graded its own work; this round explicitly challenges that.

## Phases

- **A — RESEARCH** (`STATE=ROUND_8_RESEARCH`): `ref-l7` + `ref-l4` study
  production references and write `audit/round-8/research/<ref>.md`
  with ≥10 distinct lessons + 5 defensive patterns + non-goals each.
- **B — DIVERGENCE** (`STATE=ROUND_8_DIVERGENCE`): `div-l7`, `div-l4`,
  `div-ops` walk our code vs the research. Findings under
  `audit/round-8/findings/`. Resolved-as-fine items briefly in
  `audit/round-8/divergence/<area>-resolved.md`. Cross-review notes
  in `audit/round-8/cross-review.md`.
- **C — RECONCILE** (`STATE=ROUND_8_RECONCILE`): each finding tagged
  NEW / MISSED / DISPUTE relative to the Round-1..-7 audit register.
  Summary in `audit/round-8/coverage-gap.md`. Disputes in
  `audit/round-8/disputes.md`. MISSED >10% triggers a deeper redo.
- **D — FIX** (`STATE=ROUND_8_FIXES`): plan-approve + fix every
  medium+. `verify` (author ≠ verifier) signs off.
- **E — REGRESSION** (`STATE=ROUND_8_REGRESSION`): re-run Round-7 gate
  matrix + any new gates the references suggest.
- **F — FINAL** (`STATE=ROUND_8_FINAL`): `audit/round-8/FINAL.md` with
  GO/NO-GO (no "conditional").

## Layout

```
round-8/
  README.md
  research/
    <reference>.md           # e.g. pingora.md, envoy.md, katran.md
  findings/
    <ID>.md                  # ROUND8-<AREA>-<NN> per finding
  divergence/
    l7-resolved.md           # "different but fine" notes
    l4-resolved.md
    ops-resolved.md
  cross-review.md            # div-* peer review of each other's findings
  reconcile/
    <ID>.md or single        # NEW/MISSED/DISPUTE tags
  coverage-gap.md            # MISSED bucket summary
  disputes.md                # DISPUTE arbitrations
  recheck.md                 # spot-check of >=20% of Round-1..-7 Verified-Fixed items
  fixes/                     # Phase D plans
  verify/                    # Phase D verification reports
  FINAL.md                   # Phase F verdict
```

## Stance refresher

- Do NOT trust Round-1..-7 "Verified-Fixed" without re-checking. Spot
  check ≥20% (see `recheck.md`).
- Do NOT defer to CI the way the previous audit did. If the gate cannot
  run here, FINAL.md must say so with the exact command + exact env.
- Do NOT pad. If a search returned nothing, say so with the search
  expression.
