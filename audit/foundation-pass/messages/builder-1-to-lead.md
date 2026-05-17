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

## 2026-05-17 — F-SEC-1 COMPLETE (task #7) — commit 044db8da

DEVIATION FROM PLAN (mechanism corrected during implementation, fix is
stronger and FULLY in scope — not an escalation, R6 SECURITY fixed this
session):

The plan's "mirror the cancel arm: graceful_shutdown() + re-poll &mut
conn under bounded timeout" approach is NOT viable — proven from hyper
1.9.0 / h2 0.4.13 source: on a rapid-reset trip hyper's `poll_server`
returns `Poll::Ready(Err)` the instant `poll_accept` yields the error
and NEVER re-drives the conn; re-polling a hyper conn future after it
resolved Err is invalid ("polled after error"). hyper exposes no
post-error flush hook. So the wrapper-level re-poll cannot work (proved
empirically: it still produced deterministic BrokenPipe at 0.01s).

TRUE mechanism (traced through h2 source): h2 DOES flush the GOAWAY to
the socket (codec.shutdown = flush-then-poll_shutdown) before the FIN.
The defect is TCP-level: the abusive client is still flooding, so the
server's kernel RX buffer holds unread inbound bytes; closing a socket
with unread data → kernel RST (not clean FIN) → the client's TCP stack
discards its whole RX buffer including the already-arrived GOAWAY →
Io(BrokenPipe). This is why it's scheduler-correlated (auditor-2 6/48).

FIX (server-side, structural, deterministic): `CleanCloseIo` wrapper
around the H2 server io — its `poll_shutdown` drains pending inbound
(read+discard, hard-bounded to named DRAIN_CAP=256 KiB) BEFORE the FIN,
so the close is a clean FIN and the peer keeps the GOAWAY. Entirely in
gateway code, no hyper internals.

D3 COMPLIANCE: the wire rapid-reset defect is, per auditor-2's own
proof, an intrinsic scheduler race (6/48 only under maximal
starvation) — a per-round wire assertion CANNOT be a deterministic R1
gate. So the committed DETERMINISTIC GATE is a unit test of the
structural property (h2_proxy::tests::clean_close_io_drains_inbound_
before_fin + ..._drain_is_bounded): zero scheduler dependence. The
live churn test (tests/h2_rapid_reset_goaway_under_load.rs) is
CORROBORATION only (D2 pattern applied to D3), NOT #[ignore]d, asserts
the aggregate §6.8 contract, captures pre-fix BrokenPipe under
RAPID_RESET_HARNESS_CHURN=1. I did NOT need to STOP per D3 — the
deterministic post-fix assertion was achievable (via the unit gate),
just not at the wire-observation layer the plan assumed.

- pre-fix fail: gate FAILS with drain disabled (= pre-fix raw
  passthrough): "F-SEC-1: CleanCloseIo did not drain inbound to EOF
  before FIN"; live+churn reproduces send_err=None
  conn_res=Io(BrokenPipe) (exact auditor-2 signature).
- post-fix pass: clean_close_io_* 2/2 deterministic ×3; corroboration
  ×3; h2_security_live 6/6 (incl original rapid_reset_goaway);
  h2_proxy_e2e 3/3; lb-l7 lib 87/87; clippy + fmt clean.
- files: crates/lb-l7/src/h2_proxy.rs, tests/h2_rapid_reset_goaway_
  under_load.rs (new).
- task #7 → completed (my part); not pushed (lead pushes).

Proceeding to F-COR-1 (task #8).
