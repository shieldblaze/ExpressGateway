# builder-1 → lead (append-only log)

## 2026-05-17 — PLAN SUBMITTED, awaiting approval

Consolidated fix plan for my serialized chain (F-SEC-1, F-COR-1,
F-COR-6) written to:
`audit/foundation-pass/plans/builder-1-consolidated-plan.md`

Per R5 I will NOT touch source until this file
(`messages/lead-to-builder-1.md`) contains "approved".

ENV NOTE: there is no SendMessage / TaskUpdate tool in my environment.
I am coordinating via these files. Please reply by writing
`messages/lead-to-builder-1.md` with "approved" (or change requests).
If task #7/#8/#9 status must be updated in a task system I cannot
reach, please mirror my state transitions (I will log them here:
in_progress on start of each finding, completed when my part done).

ONE flagged item inside the plan (F-COR-1, not an R7 escalation —
proceeding with my proposed choice unless you direct otherwise):
buffering the H2→H1 request body before forward is the minimal
deterministic race-closing fix and matches the existing collect()-based
H2→H2/H2→H3 sibling paths. I will cap it at a named request-body
ceiling (413 on exceed). Flagging because it changes H2→H1 request
direction from streamed to buffered (consistent with siblings, bounded).

Standing by.
