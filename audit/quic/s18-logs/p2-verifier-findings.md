# S18 Phase 2 — independent VERIFIER findings (root confirm + R13 fix verify)

Author: **verifier** (S18 Phase 2). Author≠verifier: every number below is from MY OWN
instrumentation + runs in a dedicated worktree (`/home/ubuntu/Code/eg-s18-verify-wt`), own
target dir (`/home/ubuntu/Code/eg-s18-verify-target`), on a quiet uncontended box. Each burst
ran a one-test-per-process harness to a `BURST_DONE` line that was READ in full before any
number was written here (R15 — no number from an incomplete/backgrounded job). Instrumentation
is worktree-only + uncommitted (a small env-gated `EG_VERIFY_DROP` dump); it was REVERTED before
the FIXED-tree runs. Shared checkout left PRISTINE except this one findings file.

- PRE-FIX sha = `a231b761` (read gate WITHOUT `!src_fin_seen`).
- FIXED sha  = `1414d656` (read gate gains `!half.src_fin_seen` + regression test).
- Burst harness: runs the COMPILED test binary directly, single-threaded (`--test-threads=1`),
  one test per process invocation; a STALL = non-zero exit (the budget-wall panic). Logs cited
  below live under `/home/ubuntu/Code/eg-s18-verify-evidence/`.

---

## VERDICT (one line)

**ROOT CONFIRMED + FIX VERIFIED to R13.** The relay-side `pump_dir` post-FIN re-read drop is
independently reproduced (multistream 9/90 + backpressure 4/75 PRE-FIX, every stall = the
`InvalidStreamState` read-error arm with `src_fin_seen=true` and `dropping_pending == short_by`
EXACT; the write-error arm never fired). The shipped one-line fix eliminates the stall on BOTH
paths (multistream 0/90, backpressure 0/75, same harness = load-bearing negative control), the
deterministic regression test is load-bearing (PASS fixed / FAIL pre-fix on a one-line toggle),
and the rest of the Mode B wire suite + the lb-quic lib suite still pass. No defect found; no
refutation.

---

## TASK 1 — root CONFIRMED (PRE-FIX tree `a231b761`, my own instrumentation)

Instrumentation (worktree-only, env-gated `EG_VERIFY_DROP`, reverted before TASK 2): an
`eprintln!` in `pump_dir`'s GENERIC read-error arm (`Err(e) => { half.pending.clear();
half.done=true }`) logging `sid`, `err`, `dropping_pending=half.pending.len()`, `src_fin_seen`,
`fin_sent`; an identical dump added to the GENERIC write-error arm (so a DIFFERENT drop arm would
be visible); and a periodic client-side per-stream `short_by` dump in each test driver.

### Path 1 — `s16_b2_multistream`, single-threaded ≥75-iter (ran 90)
- **9/90 stalls** (10.0%). Log: `.../prefix-multistream-drop-90.log` (`BURST_DONE … stalls=9`).
  Stall iters: 1, 5, 55, 59, 63, 73, 80, 82, 87. 81 PASS + 9 STALL = 90.
- Drop-arm tally over the whole burst: **`PUMP-READ-ERR` = 10** drops (some stalls drop 2
  streams), **`PUMP-WRITE-ERR` = 0** (the write-side generic arm NEVER fired);
  **InvalidStreamState = 10/10**, **`src_fin_seen=true` = 10/10**, non-InvalidStreamState = 0.
- **`dropping_pending == short_by` EXACT on every drop** (paired the read-err `dropping_pending`
  against the same sid's last client `short_by`):

  | iter | sid | dropping_pending | short_by | match |
  |---|---|---|---|---|
  | 1  | 0  | 531   | 531   | ✓ |
  | 5  | 0  | 162   | 162   | ✓ |
  | 5  | 4  | 2403  | 2403  | ✓ |
  | 55 | 4  | 1437  | 1437  | ✓ |
  | 59 | 16 | 626   | 626   | ✓ |
  | 63 | 16 | 16680 | 16680 | ✓ |
  | 73 | 8  | 20364 | 20364 | ✓ |
  | 80 | 0  | 531   | 531   | ✓ |
  | 82 | 4  | 19828 | 19828 | ✓ |
  | 87 | 16 | 29604 | 29604 | ✓ |

  Sample raw line (iter 1):
  `EG_VERIFY_DROP PUMP-READ-ERR sid=0 dir=u→c err=InvalidStreamState(0) dropping_pending=531 src_fin_seen=true fin_sent=false`
  paired with `CLIENT-SHORT sid=0 got_len=8469 payload_len=9000 short_by=531`.
  Stall panic each time: `client must receive every echoed payload before the budget: Elapsed(())`.

### Path 2 — `s16_b2_backpressure` (resume path), single-threaded ≥75-iter (ran 75)
- **4/75 stalls** (5.3%, = diag-eng's 4/75). Log: `.../prefix-backpressure-drop-75.log`
  (`BURST_DONE … stalls=4`). Stall iters: 1, 38, 58, 65. 71 PASS + 4 STALL = 75.
- Drop-arm tally: **`PUMP-READ-ERR` = 4**, **`PUMP-WRITE-ERR` = 0**; InvalidStreamState = 4/4,
  `src_fin_seen=true` = 4/4.
- **`dropping_pending == short_by` EXACT on every stall** (sid 0, the single 4 MiB stream):

  | iter | dropping_pending | short_by | match |
  |---|---|---|---|
  | 1  | 44182 | 44182 | ✓ |
  | 38 | 9550  | 9550  | ✓ |
  | 58 | 12072 | 12072 | ✓ |
  | 65 | 17209 | 17209 | ✓ |

  Sample raw (iter 1): `EG_VERIFY_DROP PUMP-READ-ERR sid=0 dir=u→c err=InvalidStreamState(0)
  dropping_pending=44182 src_fin_seen=true fin_sent=false` with `CLIENT-SHORT sid=0
  got_len=4150122 payload_len=4194304 short_by=44182` (client frozen there for the rest of the run).

**TASK 1 = CONFIRMED on BOTH paths.** The firing arm is uniquely the `InvalidStreamState`
read-error arm with `src_fin_seen=true`; the dropped pending IS the missing client tail
(`dropping_pending == short_by`, 14/14 drops across both paths); the write-error arm and any
non-InvalidStreamState read error NEVER fired. Matches the claimed root with no discrepancy.

---

## TASK 2 — fix VERIFIED to R13 (FIXED tree `1414d656`)

Instrumentation removed: I `git checkout -- ` the three instrumented files then `git checkout
1414d656`; confirmed `0` `EG_VERIFY_DROP` strings in the three files and a clean `git status`
(only my untracked `verify_burst.sh`). The fixed read gate (`while !half.src_fin_seen &&
half.pending.len() < STREAM_RELAY_WINDOW`, raw_proxy.rs:759) never issues the post-FIN read, so
the drop arm is unreachable; the fixed-tree burst logs contain ZERO drop dumps (pristine tree).

### R13(b) — ZERO-stall dual-path isolation burst (single-threaded)
- `s16_b2_multistream`: **0/90 stalls**. Log: `.../fixed-multistream-90.log`
  (`BURST_DONE … stalls=0`; 90 PASS + 0 STALL; 0 drop dumps; tree `1414d656`).
- `s16_b2_backpressure` resume: **0/75 stalls**. Log: `.../fixed-backpressure-75.log`
  (`BURST_DONE … stalls=0`; 75 PASS + 0 STALL; 0 drop dumps; tree `1414d656`).

### R13(c) — LOAD-BEARING negative control (my own pre/post runs, same harness)

| path | PRE-FIX `a231b761` | POST-FIX `1414d656` | log pair |
|---|---|---|---|
| `s16_b2_multistream`  | **9/90** stalls (10.0%) | **0/90** stalls | prefix-multistream-drop-90.log / fixed-multistream-90.log |
| `s16_b2_backpressure` | **4/75** stalls (5.3%)  | **0/75** stalls | prefix-backpressure-drop-75.log / fixed-backpressure-75.log |

Same single-threaded harness, same binaries-built-per-tree; the only change is the one-line read
gate. Removing the suspected cause removes the effect (→0) on both paths — the Koch test holds.

### Independent regression-test toggle (`raw_proxy::tests::post_fin_short_write_reread_does_not_drop_tail`)
Run by me directly, independent of builder-1's self-report. Log: `.../regtest-toggle.log`.
- FIXED gate (`while !half.src_fin_seen && …`): **PASS** (`test result: ok. 1 passed; 0 failed`).
- One-line fix REVERTED by me (`while half.pending.len() < STREAM_RELAY_WINDOW`): **FAIL** —
  panics at raw_proxy.rs:1590 `"turn 2 must NOT drop the half via a spurious post-FIN re-read
  (CF-S16-RELAY-STALL): if done, it must be via a clean FIN, not a drop"` (`1 failed`).
- Restored the fix; re-ran → PASS; full `lb-quic --lib` suite = **68 passed; 0 failed**
  (`.../fixed-lib-tests.log`), which includes the new test.

The regression test is genuinely load-bearing: it observes the drop pre-fix and the clean
completion post-fix, deterministically (no timing dependence).

**TASK 2 = fix VERIFIED to R13.** Zero stalls on both paths post-fix; PRE/POST negative control
9/90→0 and 4/75→0; deterministic regression test load-bearing.

---

## TASK 3 — no-regression spot check (FIXED tree, `--features test-gauges`)

Log: `.../fixed-modeb-suite.log` (tree `1414d656`). All PASS:
- `s16_b1_two_connections` — 1 passed
- `s16_b2_reset_not_fin` — 1 passed
- `s16_b2_stream_relay_smoke` — 1 passed
- `s16_b3_reset_propagation_smoke` — 1 passed
- `s16_b3_reset_propagation_verify` — 4 passed

The fix does not regress the B3 reset/STOP_SENDING propagation or the reset-not-FIN smuggling
guard (consistent with the fix's scope: it only stops dropping a CLEANLY-FINISHED stream's
buffered tail; a peer RESET surfaces as `StreamReset` BEFORE `src_fin_seen` and still tears down
without a FIN via its own arm). Full `lb-quic --lib` = 68 passed; 0 failed.

---

## Evidence logs (all COMPLETED + fully read), under `/home/ubuntu/Code/eg-s18-verify-evidence/`
- `prefix-multistream-drop-90.log`   — PRE-FIX, 9/90; read-err InvalidStreamState arm, src_fin_seen=true, dropping_pending==short_by 10/10; write-err arm 0.
- `prefix-backpressure-drop-75.log`  — PRE-FIX resume, 4/75; same signature, dropping_pending==short_by 4/4.
- `fixed-multistream-90.log`         — POST-FIX, 0/90 (negative control), 0 drop dumps.
- `fixed-backpressure-75.log`        — POST-FIX resume, 0/75, 0 drop dumps.
- `regtest-toggle.log`              — regression test PASS (fixed) / FAIL (one-line reverted).
- `fixed-modeb-suite.log`           — TASK 3 Mode B wire suite, all pass.
- `fixed-lib-tests.log`             — lb-quic --lib = 68 passed; 0 failed (includes new test).
- `_task1-instrumentation.patch` / `verify_burst.sh` — my throwaway instrumentation + harness (worktree-only).

## Hygiene
Shared checkout `/home/ubuntu/Code/ExpressGateway` PRISTINE (`git status` clean apart from this
file; 0 `EG_VERIFY_DROP` in shared `crates/`). Instrumentation was worktree-only, env-gated, and
REVERTED before all FIXED-tree runs; the verify worktree has no instrumentation in tracked files
(only an untracked `verify_burst.sh` harness). No `git stash`. Target dir
`/home/ubuntu/Code/eg-s18-verify-target` + evidence dir may be removed.
