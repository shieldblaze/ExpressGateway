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

---

## 2026-05-17 — F-SEC-1 REOPENED → v2 fix (builder-1)

Task #7 (F-SEC-1) reopened. Prior commit 044db8da (CleanCloseIo
break-on-Pending drain) was INSUFFICIENT — phase3-final/RESULT.md
proved the real wire test `tests/h2_security_live.rs::rapid_reset_goaway`
still FAILS ~1/3 under full-workspace 8-core load with the original
signature `rapid_reset: send_err=None
conn_res=Ok(Ok(Err(Error { kind: Io(Kind(BrokenPipe)) })))` (client
never received a server GOAWAY).

Plan written: audit/foundation-pass/plans/builder-1-fsec1-v2.md.
Pre-authorized SECURITY scope (R6 unconditional) — proceeded without
waiting. No R7 fork (lingering-close is the unambiguous RFC-correct
behaviour).

### ROOT CAUSE (proven from hyper-1.9.0 + h2-0.4.13 source)

h2 DOES flush the GOAWAY before the FIN: rapid-reset trip ->
`handle_go_away` queues GOAWAY + sets `State::Closing` ->
`State::Closing` arm runs `ready!(self.codec.shutdown(cx))?` =
`framed_write::shutdown` = `flush()` (writes GOAWAY bytes via
`poll_write_buf`+`poll_flush`) THEN `inner.poll_shutdown` (FIN), THEN
`State::Closed` -> conn future resolves Err -> hyper surfaces it -> our
`res = &mut conn` arm returns Err -> `conn`/io dropped.

The real defect is the ABORTIVE RST CLOSE. The prior CleanCloseIo
drained inbound only until the first `Poll::Pending`, then BROKE AND
FINed anyway. During a rapid-reset flood the abusive client is
continuously streaming RST_STREAM frames, so the kernel recv buffer is
essentially never durably empty and the client still has bytes in
flight. Closing/dropping a TCP socket while the peer is still actively
sending makes Linux emit an RST (RFC 1122 §4.2.2.13 / tcp_close); the
client's TCP stack discards its ENTIRE RX buffer — incl. the
already-arrived GOAWAY — on the RST, surfacing only Io(BrokenPipe),
send_err=None. Nondeterministic (~1/3) = the RST-vs-clean-close race
under 8-core contention. The prior unit proxy
`clean_close_io_drains_inbound_before_fin` false-verified because it
only modelled "finite data then clean EOF" — never "peer keeps
sending past the FIN", the actual condition.

### FIX (server-side, structural — bounded graceful lingering close)

`CleanCloseIo::poll_shutdown` now performs a proper lingering close
(nginx-`lingering_close` pattern): GOAWAY already flushed by h2's
codec.shutdown (verified in source) BEFORE poll_shutdown; we drain
inbound until the peer closes its write half (EOF — the normal
reaction to a GOAWAY), and on `Poll::Pending` we RETURN
`Poll::Pending` (yield, wait for the post-GOAWAY FIN) instead of
racing the peer with our own RST-causing close. Hard-bounded by BOTH
the existing 256 KiB DRAIN_CAP and a new 5 s LINGER_DEADLINE
(tokio::time::Sleep, named const, strictly inside the 60 s
HttpTimeouts::total) so a silent/wedged client cannot pin the worker.
Client now receives ...GOAWAY...FIN in order on a cleanly closed
socket -> decodes GOAWAY -> conn future resolves
Err(GoAway(_,_,Remote)) (is_go_away()/is_remote()). DoS mitigation
unchanged (connection still dies, bounded). No protocol behaviour
change for conformant peers.

Diff: crates/lb-l7/src/h2_proxy.rs only (CleanCloseIo doc/struct/
poll_shutdown + new const + Duration import + 1 new unit test).
Prior unit tests KEPT unchanged (R5) as additional coverage; new unit
test `clean_close_io_does_not_fin_while_peer_still_open` asserts the
core property (Pending => no FIN; FIN only after peer EOF). NOT the
gate.

### EVIDENCE (verbatim)

PRE-FIX (cited phase3-final/RESULT.md, the FINAL R1 gate, verbatim):
  rapid_reset: send_err=None conn_res=Ok(Ok(Err(Error { kind: Io(Kind(BrokenPipe)) })))
  thread 'rapid_reset_goaway' panicked at tests/h2_security_live.rs:342:5
Local reproduce attempt (3x full-workspace 8-core): all green this
round (the ~1/3 rate did not surface in 3; mechanism already proven +
captured in phase3-final, per directive "cite + reproduce once").

POST-FIX isolated sanity:
  rapid_reset: send_err=None conn_res=Ok(Ok(Err(Error { kind: GoAway(b"", PROTOCOL_ERROR, Remote) })))
  test rapid_reset_goaway ... ok

POST-FIX GATE (>=15 consecutive `rapid_reset_goaway` GREEN, each UNDER
full `cargo test --workspace --all-features -- --test-threads=8`
8-core contention): see appended block below once the 16-run harness
completes.

---

## 2026-05-17 — F-SEC-1 v2 ACCEPTANCE EVIDENCE (builder-1)

FINAL fix design (refined from the plan after discovering the
lingering-only variant regressed `zero_window_stall_stream_reset` —
that close must stay prompt): **FIN-FIRST then bounded post-FIN
drain**. A TCP FIN never causes an RST; only DROPPING a socket with
unread inbound does, and h2 drops the io the instant our poll_shutdown
resolves. So poll_shutdown (1) delegates the inner FIN promptly (zero
teardown-latency regression — keep-alive-stall close still immediate),
then (2) reads+discards inbound until the peer's reciprocal FIN (EOF),
hard-bounded by 256 KiB DRAIN_CAP AND a 1 s LINGER_DEADLINE, before
letting the drop proceed. Client receives ...GOAWAY...FIN on a cleanly
closed socket -> decodes the GOAWAY.

### PRE-FIX (cited verbatim, phase3-final/RESULT.md — the FINAL R1 gate)
```
rapid_reset: send_err=None conn_res=Ok(Ok(Err(Error { kind: Io(Kind(BrokenPipe)) })))
thread 'rapid_reset_goaway' panicked at tests/h2_security_live.rs:342:5:
expected server-initiated GOAWAY after rapid-reset flood; send_err=None, conn_res=Ok(Ok(Err(Error { kind: Io(Kind(BrokenPipe)) })))
```
(Real R1 condition: `cargo test --workspace --all-features --
--test-threads=8` ×3 8-core, FAILED 1/3. Mechanism proven in
RESULT.md §R2; reproduced-once locally — the ~1/3 rate did not
resurface in 3 but the captured FINAL-gate failure stands as the
pre-fix proof per directive.)

### POST-FIX GATE — REAL wire test, >=15 consecutive GREEN UNDER full
### `cargo test --workspace --all-features -- --test-threads=8` 8-core
### contention (--nocapture so the wire line is captured every run)

Command per run: `cargo test --workspace --all-features --
--test-threads=8 --nocapture`  (the literal R1 contended condition;
--nocapture only adds stdout capture, does not change parallelism).

Result: **16 / 16 consecutive GREEN, BEST_CONSEC=16** (bar is >=15).
Every one of the 16 runs, for the real `tests/h2_security_live.rs::
rapid_reset_goaway`:
```
rapid_reset: send_err=None conn_res=Ok(Ok(Err(Error { kind: GoAway(b"", PROTOCOL_ERROR, Remote) })))
test rapid_reset_goaway ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s   (whole h2_security_live binary)
```
- GoAway(_, PROTOCOL_ERROR, **Remote**) => client h2 conn observed a
  server-initiated GOAWAY: `is_go_away()` AND `is_remote()` both true
  (the exact RFC 9113 §6.8 / CVE-2023-44487 contract the test asserts;
  the assertion was NOT modified).
- 16/16 runs contain that exact proof line; 0/16 runs have ANY
  `BrokenPipe` / non-GOAWAY teardown in the rapid_reset path;
  any_FAILED=0 every run; NO other test regressed (h2_security_live
  6/6 incl. `zero_window_stall_stream_reset` every run).

Gate harness: /tmp/fsec1_gate3.sh ; per-run logs /tmp/fsec1c_run_*.log
(verbatim), summary /tmp/fsec1_gate3_out.log.

Tests: NONE weakened/skipped/#[ignore]d/deleted (R5). The prior unit
`clean_close_io_drains_inbound_before_fin` (which asserted the
INSUFFICIENT design's invariant and false-verified) was REWRITTEN to
assert the corrected, STRONGER invariant
(`clean_close_io_drains_inbound_to_eof_before_resolving`: poll_shutdown
must not resolve until inbound drained to EOF so the drop is clean) +
a NEW core-property unit test
(`clean_close_io_does_not_resolve_while_peer_still_open`) + the bound
test kept. These unit tests + the untouched corroboration test
(`tests/h2_rapid_reset_goaway_under_load.rs`) are ADDITIONAL coverage,
explicitly NOT the gate — the gate is the real wire test above.

R1 triple (`cargo test --workspace --all-features` ×3 8-core) + fmt +
clippy result: appended below once /tmp/r1_triple.sh completes.
Commit follows; NOT pushed. Author != verifier — a different agent
re-verifies.

---

## 2026-05-17 — R1 triple + clippy/fmt result, and a SEPARATE flake found (builder-1)

`/tmp/r1_triple.sh` (8-core, real R1 commands):
```
cargo fmt --check                                   -> FMT_EXIT=0    (CLEAN)
cargo clippy --all-targets --all-features -Dwarnings -> CLIPPY_EXIT=0 (0 warn/err lines, CLEAN)
R1 TEST RUN 1: exit=0   any_FAILED=0  h2_security_live: 6 passed 0 failed
R1 TEST RUN 2: exit=0   any_FAILED=0  h2_security_live: 6 passed 0 failed
R1 TEST RUN 3: exit=101 any_FAILED=2  h2_security_live: 6 passed 0 failed
```

F-SEC-1 itself: GREEN in ALL 3 R1-triple runs AND all 16 gate3 runs
(h2_security_live 6/6 incl. rapid_reset_goaway every time). The fix
holds unconditionally.

R1-triple RUN 3's failure is a DIFFERENT, pre-existing test and is NOT
caused by the F-SEC-1 change. Captured mechanism (R2 — from output,
not hand-waved):
```
---- drain_tests::test_sigterm_drains_h2_with_goaway stdout ----
thread '...' panicked at tests/reload_zero_drop.rs:1045:14:
h2 TLS handshake must succeed (cert harness wiring): Custom { kind: InvalidData, error: InvalidMessage(InvalidContentType) }
test result: FAILED. 5 passed; 1 failed; ... finished in 20.74s
error: test failed, to rerun pass -p lb-integration-tests --test reload_zero_drop
```
Why it is independent of F-SEC-1:
- It panics at the **rustls TLS handshake** (`InvalidMessage(
  InvalidContentType)`) at reload_zero_drop.rs:1045 — a stage that
  runs ENTIRELY BEFORE any h2/serve_connection/GOAWAY/poll_shutdown
  logic. The F-SEC-1 change is confined to
  `CleanCloseIo::poll_shutdown` (post-connection-establishment
  teardown); that code is never reached when the handshake itself
  fails.
- Coverage proof: across the 16 gate3 full-workspace 8-core runs +
  R1-triple runs 1&2, `reload_zero_drop` passed 6/6 EIGHTEEN times;
  it failed exactly ONCE (R1-triple run 3) = ~1/19 under identical
  contention. Provenance: last touched by F-COR-4 `05d801c1`
  (pre-F-SEC-1); `git log` shows the F-SEC-1 commit does not touch
  tests/reload_zero_drop.rs.
- Isolated repro: `test_sigterm_drains_h2_with_goaway` (which DOES
  exercise the SIGTERM->graceful_shutdown->CleanCloseIo::poll_shutdown
  path) passes 5/5 in isolation, including its real GOAWAY-drain
  assertion — so the F-SEC-1 teardown path is healthy; the failure is
  purely the pre-handshake TLS flake.

Classification: this is a SEPARATE pre-existing low-rate TLS-handshake
flake in `tests/reload_zero_drop.rs::test_sigterm_drains_h2_with_
goaway`, OUTSIDE F-SEC-1 (task #7) scope, NOT introduced by this fix.
Flagging for the lead as a distinct finding for the R1 baseline
workstream (it is a real defect per R2 — a TLS handshake should not
intermittently fail; likely a cert-harness/listener-readiness race in
that test's setup — but it is not F-SEC-1 and not mine to fix under
this task's SECURITY-scoped directive; recorded, not asterisked).

F-SEC-1 (task #7) acceptance bar — REAL wire test
`tests/h2_security_live.rs::rapid_reset_goaway` GREEN >=15 consecutive
(achieved 16/16) under full-workspace 8-core contention with the
verbatim server GOAWAY and zero BrokenPipe — IS MET. Committing the
F-SEC-1 fix now (NOT pushed). Author != verifier.
