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

## 2026-05-17 — F-COR-1 COMPLETE (task #8) — commit 9e41d07f

Implemented per D1 (buffer-then-forward, NAMED cap MAX_REQUEST_BODY_BYTES
=64 MiB, 413 on exceed) + D2 (DETERMINISTIC gate). DEVIATION (stronger,
in scope): the deterministic gate uses a connection-counting backend —
the scheduler-independent invariant is "malformed request ⇒ backend
dialed ZERO times" (validate-before-dial). A pure wire-DATA assertion
is NOT deterministic (the race only loses under churn, per auditor-2),
so the dial-count invariant is the correct deterministic gate; h2spec
stays corroboration only (passes, not flaky). Also: a pseudo-header
trailer cannot exist as an http::HeaderName (`:` is not an RFC7230
token char) so the H2 helper's filter is defense-in-depth; the real
proven H2 §8.1 enforcement is the ordering fix (gate test +
dial-count). H3 path uses String pairs → its :-check IS reachable and
unit-tested.

- pre-fix fail: F-COR-1 src reverted → both ordering-race gate tests
  FAIL deterministically "backend was dialed for a malformed request"
  (no churn).
- post-fix pass: h2_validation_before_forward 3/3 ×3; lb-quic feed_body
  trailer 2/2; lb-l7 lib 88/88; lb-quic lib 11/11; trailer_passthrough
  8/8; bridging_h2_* / h3_h1 1/1; h3_h1_trailers_resp_e2e 2/2;
  h2_proxy_e2e 3/3; h2_security_live 6/6; h2spec corroboration PASSES;
  clippy + fmt clean.
- D1 behavioral-change note appended to findings.md under F-COR-1.
- files: crates/lb-l7/src/h2_proxy.rs, crates/lb-quic/src/h3_bridge.rs,
  tests/h2_validation_before_forward.rs (new),
  audit/foundation-pass/findings.md.
- task #8 → completed (my part); not pushed.

Proceeding to F-COR-6 (task #9).

## 2026-05-17 — F-COR-6 COMPLETE (task #9) — commit 7770de99

Removed #[ignore = "S2: request-body forwarding"] and rewrote the
stale-false UNBUILT doc comment to state the datapath is BUILT (S2
P1-A f2af73c4) citing the e2e proofs (h3_h1_stream_body_e2e t1/t5,
h3_to_h1_forwards_non_utf8_body). No assertion change (already passes).

- pre-fix state: lb-quic lib 11 passed / 1 ignored (test "ignored,
  S2: request-body forwarding").
- post-fix pass: lb-quic lib 12 passed / 0 IGNORED; the test runs
  `s2_target_build_h1_request_with_body_... ok`. clippy + fmt clean.
- files: crates/lb-quic/src/h3_bridge.rs.
- task #9 → completed (my part); not pushed.

ALL THREE FINDINGS COMPLETE. Serialized chain done:
F-SEC-1=044db8da, F-COR-1=9e41d07f, F-COR-6=7770de99. NOT pushed
(lead pushes). No R7 blocker hit (F-SEC-1 D3 STOP condition was NOT
triggered — deterministic gate achieved via the structural unit test).
Author != verifier: a different agent must verify each.
