# S18 Phase 2 — verifier brief (independent root confirmation + R13 fix verification)

You are the **verifier**. Author≠verifier: you trust NO other agent's prose or build. The
diagnosis (diag-eng) and the fix (builder-1) are both claims you must independently confirm
from YOUR OWN instrumentation + runs, citing COMPLETED logs (R15: never a number from an
incomplete/backgrounded job — wait for process exit, read the full log). One focused pass.

## Context (claims to confirm/refute — do not take on faith)
- CLAIMED ROOT (diag-eng): `raw_proxy.rs::pump_dir` re-reads a quiche-collected source AFTER
  its FIN, hits `Err(InvalidStreamState)`, and the generic read-error arm (`half.pending.clear();
  half.done=true`) DROPS the still-pending u2c tail + never forwards the FIN. Signature:
  `dropping_pending == short_by`, only the `InvalidStreamState` arm fires, `src_fin_seen=true`.
- CLAIMED FIX (builder-1, shipped on `feature/quic-proxy-s18`): the read gate gains
  `!half.src_fin_seen`. Proven by diag-eng's env-gated experiment to give 0/90 multistream +
  0/75 backpressure.
- Full diagnosis: `audit/quic/s18-logs/p1-diag-findings.md`. Fix design: `audit/quic/s18-fix-design.md`.

## ENVIRONMENT
- You will be given a dedicated worktree path (created by the lead off the FIXED tip). Use
  `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-s18-verify-target` per command (your OWN target dir).
- A git worktree may check out ANY commit detached (`git checkout <sha>`), even though the
  branch is checked out elsewhere. You will toggle between the PRE-FIX commit and the FIXED tip.
- The lead will tell you the PRE-FIX sha (the commit before builder-1's fix) and the FIXED sha.
- 8 cores. Your timing-sensitive bursts run on a QUIET box (the lead serializes — no other heavy
  job runs concurrently). Do not launch parallel compiles during a burst.

## TASK 1 — independently CONFIRM the root (PRE-FIX tree, your own light instrumentation)
You do NOT need diag-eng's full quiche patch. On the PRE-FIX tree, add a SMALL env-gated
(e.g. `EG_VERIFY_DROP`) `eprintln!` in `pump_dir`'s GENERIC read-error arm (the `Err(e)` arm
that does `half.pending.clear(); half.done=true`) logging: `sid`, `err` (the quiche error
variant), `dropping_pending = half.pending.len()`, `src_fin_seen`. Also log the client-side
`short_by` per stream near the budget (or reuse the test's existing short dump). Run a
≥75-iter isolation burst of `s16_b2_multistream` (single-threaded amplifies the race) AND a
≥75-iter burst of `s16_b2_backpressure` (resume path). CONFIRM at ≥3 stalls across both paths:
the firing arm is `InvalidStreamState` with `src_fin_seen=true` and `dropping_pending == short_by`.
If you instead find a DIFFERENT drop arm or the numbers don't match, REFUTE and report — do not
rubber-stamp. Cite completed logs + stall counts.

## TASK 2 — VERIFY the shipped fix to the R13 bar (FIXED tip)
Check out the FIXED tip. Remove your TASK-1 instrumentation (or keep it env-OFF — confirm it
can't perturb). Then:
- (R13 b) ≥75-iter isolation burst of `s16_b2_multistream` AND ≥75-iter of `s16_b2_backpressure`
  resume → expect **ZERO stalls on BOTH paths**. Use `--features test-gauges`, single-threaded
  (to keep amplifying the race — a clean single-threaded ≥75-iter burst is strong). Cite the
  completed `BURST_DONE`/result lines + the 0 counts.
- (R13 c) LOAD-BEARING NEGATIVE CONTROL: you have it from TASK 1 (same harness shows the stall
  PRE-FIX) vs TASK 2 (ZERO post-fix). State both rates side by side from your OWN runs.
- Run builder-1's deterministic `pump_dir` regression test independently and confirm it passes
  on the fixed tree AND fails on the pre-fix tree (toggle the one-line gate) — independent of
  builder-1's self-report.

## TASK 3 — no-regression spot check
On the FIXED tip, run the rest of the Mode B wire suite once (`s16_b1_two_connections`,
`s16_b2_reset_not_fin`, `s16_b2_stream_relay_smoke`, `s16_b3_reset_propagation_smoke`,
`s16_b3_reset_propagation_verify`) under `--features test-gauges` and confirm all pass — the fix
must not regress the B3 reset/smuggling paths. (The full ×3 workspace gate is the lead's Phase 3.)

## OUTPUT
Write `audit/quic/s18-logs/p2-verifier-findings.md`: TASK 1 confirm/refute with the drop-arm
evidence at ≥3 stalls both paths (cited logs); TASK 2 the ZERO-stall dual-path burst (cited
logs + iter counts) and the pre/post negative-control rates; TASK 3 the Mode B suite result;
and a one-line VERDICT (root CONFIRMED + fix VERIFIED to R13, or REFUTED with the discrepancy).
Keep the shared checkout pristine except that one findings file; instrumentation stays in your
worktree, uncommitted. Report log paths + the verdict back to the lead.
