# lead → builder-1 : PLAN APPROVED (with 3 binding directives)

approved

Plan is sound; mechanisms confirmed. Proceed F-SEC-1 → F-COR-1 → F-COR-6,
one commit each, no push. Comms: I (lead) own SendMessage/TaskUpdate;
keep using messages/builder-1-to-lead.md for checkpoints — I mirror to
the task system. I am resuming you via SendMessage; reply through it.

## D1 — F-COR-1(a) buffer-vs-stream is NOT an owner escalation
Standing rules already resolve it. R6 = fix the correctness defect
tractably this session; the buffer-then-forward approach is CONSISTENT
with the existing already-shipped H2→H2 / H2→H3 sibling paths (they
collect()). The streaming-preserving variant is the GENUINELY-LARGER
option and is explicitly out of scope this session. Proceed with
buffer-then-forward. MANDATORY: an explicit NAMED bounded cap with
413-on-exceed, and a regression test that exceeds the cap and asserts
413 (prove no unbounded-buffer regression). DOCUMENT the behavioral
change (H2→H1 request body now buffered+capped, consistent with the H2
siblings) explicitly in the commit message AND append a note to
audit/foundation-pass/findings.md under F-COR-1. This is a FULL fix of
the finding (not an asterisk, R4); the streaming-preserving variant is
recorded as optional future work only.

## D2 — F-COR-1 must have a DETERMINISTIC ordering-race regression
A flaky/churn-only gate test is itself an R1 violation (R1 = passes
deterministically ×N). Keep the h2spec-under-churn test as corroborating
evidence, but the gate-facing regression for the ordering race MUST be
deterministic: send a malformed H2 request (e.g. pseudo-header in
trailers AND a content-length≠ΣDATA case) and assert the client gets
RST_STREAM/GOAWAY (PROTOCOL_ERROR-class) and NEVER a backend 200
DATA(2) body — no induced churn needed post-fix because the fix closes
the window structurally (validate before dial). Capture the pre-fix
failing run under churn as evidence; the committed regression that the
gate runs must be the deterministic one.

## D3 — F-SEC-1 post-fix assertion must be deterministic
Pre-fix BrokenPipe capture under induced contention = fine as evidence.
But the committed regression must assert that POST-fix a rapid-reset
trip ALWAYS yields a real server GOAWAY without depending on the
scheduler — i.e. deterministic green, no flake. If you cannot make the
post-fix assertion scheduler-independent, stop and message lead before
committing (do not commit a flaky gate test).

All three are within the standing rules — no owner decision pending.
Proceed now.
