# SESSION 10 — H-to-H MATRIX — REPORT

- Branch: `feature/h-matrix-s10` (base `main` @ `cce5a8ed`, the S9 promote).
- Team (Opus, strict author≠verifier): `lead` (this), `builder-1` (general-purpose),
  `verifier` (general-purpose). Worktree strategy: see "Process note" below.
- Goal: H2→H2 to the R8 BUILT bar (→ 6 of 9), then H1→H2 honest-stop-gated.

---

## VERDICT: SESSION 10 COMPLETE
**H2→H2 BUILT (M-D pump → M-B egress, both legs streamed) — 6 of 9 cells.** Found
+ fixed a real F-MD-4 request-smuggling defect (intermittent, gate-masked) during
verification; re-verified to the BUILT bar with a load-bearing hardened
regression now living inside the ×3 gate. Phase 2 (H1→H2) honest-stopped per the
mission rule, its R8 plan authored as the S11 head-start. Phase 3 ×3
`--all-features` green deterministic; fmt/clippy clean. `proxy_request` (promoted
H2→H1) byte-identical throughout (R12). Promoted to `main` per R11.

The →H1 column and the H3-front row were already complete; **the →H2 column now
has H2→H2 and H3→H2** (H1→H2 remains). Cells BUILT: H3→H1, H3→H2, H3→H3, H2→H1,
H1→H1, **H2→H2**. Remaining (3): H1→H2, H1→H3, H2→H3.

---

## Phase 0 — green baseline (R1 ×3) — GREEN
Base tip confirmed `cce5a8ed`. No stray processes from S9 (`ps aux` clean). Disk
31 GB free. 8 cores. Shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` (22 GB
on entry). `feature/h-matrix-s10` branched + pushed.

`cargo test --workspace --all-features` ×3 (real cargo exit code captured via
PIPESTATUS, not a truncated-tail false positive):
- RUN 1: cargo_rc=0, 1203 passed / 0 failed / 16 ignored
- RUN 2: cargo_rc=0, 1203 passed / 0 failed / 16 ignored
- RUN 3: cargo_rc=0, 1203 passed / 0 failed / 16 ignored

`cargo fmt --check` clean (rc=0). `cargo clippy --all-targets --all-features
-- -D warnings` clean (rc=0). The 16 ignored are inherited from the S9 baseline
(CF-IGN-1), not introduced this session.

(Method note: the first baseline attempt piped cargo through `tail -60` before
redirect, which captured `tail`'s exit code instead of cargo's and lost the count.
Re-run with `PIPESTATUS[0]` + full-log count to satisfy R1 honestly — single-run
or unverified-exit "green" is not green, S8/F-MD-4 precedent.)

## Phase 1 — H2→H2 (plan resolved + approved)
Plan: `s10-h2h2-plan-RESOLVED.md` (lead-resolved from the S9 head-start
`s9-h2h2-plan.md`). Key finding at plan-against-tree time: the H2→H2 cell on entry
is a **buffering proxy with TWO R8 violations** — request leg
(`translate_h2_request_to_h2` `collect()`) AND response leg
(`upstream_h2_response_to_h2` `collect()`). (Contrast H2→H1, whose response leg
already streams via `finalize_response`→`body.boxed()`.) S10 converts both legs.

Resolved open questions:
- Q-HH-1 (dedup vs mirror): **MIRROR** — do not edit the BUILT/promoted H2→H1
  `proxy_request`; reuse leaf helpers; defer extraction to CF-DEDUP-1. R12
  divergence guard: verifier runs the identical proof battery against H2→H2.
- Q-HH-2 (trailers): forward + reject-pseudo on the request leg (reuse
  `validate_request_trailers`); response trailers ride the boxed Incoming body.
- Q-HH-3 (M-B multiplexing/GOAWAY/send_timeout): no regression to H3→H2;
  pool `send_timeout` is a test-config note for the backpressure probe, not a fork.

### Process note (worktree strategy — lead decision, R9-driven)
Builder edits in the main worktree (no source-path churn against the shared
target dir); verifier runs after the builder commits, authoring its OWN proof
suite and confirming `git diff` empty on the touched src files vs the builder
tip. Author≠verifier is preserved structurally (separate agents +
verifier-authored proofs + src diff-empty check), chosen over per-agent
worktrees to avoid path-keyed target-dir duplication that risks the 25 GB free
floor (CF-DISK-1 lesson). Independent coverage re-measure uses the SCOPED
llvm-cov command (R10).

### Increment evidence
**Build (builder-1, commit `30918809`, pushed):** single file `h2_proxy.rs`
(+434/−104). Lead independently verified:
- `proxy_request` (1333–1879) byte-identical across `cce5a8ed`→`30918809`
  (sha256 `0223264d…` both revs) — promoted H2→H1 pump untouched (R12). H1 +
  H2→H3 paths untouched.
- `translate_h2_request_to_h2` REPLACED by `build_h2_upstream_request_parts`
  (head-only, no body collect); no dead code.
- Request leg (`proxy_h2_to_h2_request`, new ~1950): faithful M-D mirror —
  lookahead validate-before-dial (Branch A zero-pool-contact on within-window
  malformed), Branch B bounded 64 KiB in-flight window, `in_flight_bytes` gauge
  + `record_retained` at the M-D sites, egress `h2_pool.send_request` with
  `H2ReqBody` (PumpAbort→Box<dyn Error>). **F-MD-4**: every abort arm
  (None-without-END_STREAM, forbidden trailer, over-cap, Some(Err)) injects
  `Err(PumpAbort)` BEFORE the verdict; `is_end_stream` gates clean-EOF in both
  loops. **F-CAP-1**: on non-Timeout send error, await `verdict_rx` bounded by
  `timeouts.body`, prefer BodyTooLarge→413 / BadRequest→400 over Upstream→502;
  Timeout→504 without consulting verdict; `pump.abort()` only after the await.
- Response leg (`upstream_h2_response_to_h2`, now sync ~2528): `collect()`
  removed → `body.boxed()` streaming-by-construction + lowercase header
  normalization + alt-svc; trailers ride the terminal frame (R8 violation #2 gone).
- No residual body `collect()` in the H2→H2 paths (only comments + a header-Vec
  `.collect()`).
- Builder smoke: `proxy_h2_listener_h2_backend` ok; `proto_translation_e2e` 5/5;
  bridging_h2_h2 + bridging_h3_h2 ok; clippy (lb-l7/lb-io) clean. Disk 30 GB.
- No plan deviations.

**Verification (verifier, independent; commits `…55b73898`, src diff-empty vs
builder tip `30918809`):** `tests/h2h2_md_streaming_verify.rs` +
`s10-h2h2-cov.awk` + `s10-h2h2-verify.md`. **19/20 conditions PASS; 1 REAL
DEFECT (F-MD-4) found → H2→H2 NOT YET BUILT.**
- Request byte-identity Branch A (1 KiB) + B (5/8 MiB) verbatim at H2 backend. PASS.
- Memory gauge non-vacuous: in-situ 80 KiB (≈1.25×window) for 4 MiB & 48 MiB
  (body-INDEPENDENT); inverted probe trips. PASS.
- Request backpressure: 48 MiB, paused at 1.44 MiB under stall, retained 80 KiB,
  resume → full 50331648 + 200. PASS.
- F-CAP-1: over-cap 66 MiB → **413** (backend drained), forbidden pseudo-trailer
  → RST no-leak, CL-mismatch → no-leak, dead backend → **502** (arm
  discriminates). PASS.
- Response leg (net-new): 8/48 MiB byte-identical (no truncation); slow-client
  bounded-by-construction (honest — no gauge claimed); trailers relayed. PASS.
- Coverage SESSION sub-metric (binding) **84.26%** (257/305) ≥ 80%
  (`proxy_h2_to_h2` 77.27%, `proxy_h2_to_h2_request` 82.39%,
  `build_h2_upstream_request_parts` 90.91%, `upstream_h2_response_to_h2` 83.33%);
  scoped `cargo llvm-cov --workspace --features test-gauges --lcov --test
  h2h2_md_streaming_verify` (R10). Disk 27 GB.
- R3: h2h1 15/15, h1h1 14/14, proto_translation 5/5, h3_h2 10/10 (shared
  Http2Pool un-regressed).
- **DEFECT F-MD-4 (BLOCKER, R2/R6/R8):** client RST mid-body (256 KiB Branch B,
  full-forward + 500 ms settle + RST_STREAM/CANCEL, no END_STREAM) → H2 backend
  records `complete=true` w/ full body in ~25–50% of ISOLATED runs (truncated
  request relayed as complete = request smuggling). MASKED under the parallel
  gate (20/20) — the gate would not catch it; repro needs `--test-threads=1`
  repetition. `complete=true` ⟺ gateway sent a clean upstream END_STREAM ⟺ pump
  dropped `tx` without enqueuing `Err(PumpAbort)`. Dispatched to builder-1 to
  diagnose (instrumented) + fix, with an explicit R12 mandate (the suspect arm is
  byte-identical in the promoted H2→H1 `proxy_request`; default = fix both if
  shared). Verifier to re-verify deterministically + harden the regression so it
  lands robustly inside the ×3 gate (a methodological finding: the parallel gate
  masked a real F-MD-4 smuggle — single-run/parallel "green" is not green, R1/R2).

**Fix (builder-1, `9173bd97`; src files `h2_proxy.rs` + `http2_pool.rs` only;
`proxy_request` byte-unchanged, content-sha `e0161a3e…` identical):** root cause
confirmed by instrumentation = a **graceful-drop race** feeding the H2 upstream:
a downstream client RST cancels the gateway's H2-server stream future, dropping
the in-flight `send_request` future + upstream request body at a clean frame
boundary, so hyper emits a clean END_STREAM upstream BEFORE polling the injected
`PumpAbort`. Fix = (1) DETACH the upstream send + verdict resolution into a task
that owns `send_fut`+body (a downstream cancel no longer drops the upstream body);
(2) `Http2Pool::reset_peer(addr)` on abort verdicts (connection-teardown backstop,
the multiplexed analog of H1's `conn_handle.abort()`); (3) `inject_abort!` holds
the sender + `tx.closed().await` (≤`H2_ABORT_OBSERVE_TIMEOUT`=5 s) so hyper
observes the FIFO Err before any channel close. R12: H2→H2-SPECIFIC — H2→H1 proven
safe (40/40, structural immunity: dedicated conn + chunked-terminator semantics),
H3→H2 safe (30/30, synchronous abort in poll_frame). proxy_request not modified.

**Re-verification round 2 (verifier, `8a7a49a6`; src byte-identical to the fix
tip — verifier added only tests): H2→H2 BUILT bar MET.**
- F-MD-4: **50/50 isolated single-threaded, 0 smuggles** (was 25–50% pre-fix).
- **Hardened regression + load-bearing negative control**: F-MD-4 test refactored
  to `current_thread` flavor (the bug's low-contention condition) + an internal
  24-iteration smuggle loop + a `dialed` non-vacuity guard, non-`#[ignore]`'d.
  NEGATIVE CONTROL: the hardened test FAILS at iter 0 on the pre-fix parent
  (`saw_complete=true`, 262144 B) and PASSES 24/24 on the fix → it now catches a
  regression of this bug INSIDE the parallel gate (closing the R1/R2 gap).
- Full battery 21/21 (no detached-task refactor regression): byte-identity both
  legs, gauge non-vacuous + inverted-probe-load-bearing (in-situ 80 KiB, trips at
  4 MiB), request+response backpressure, F-CAP-1 (413/400/502 discriminated),
  trailers both directions, zero-dial reject.
- R3 incl. the SHARED pool: h3_h2 10/10, h2h1 15/15, h1h1 14/14,
  proto_translation 5/5 — `Http2Pool` + new `reset_peer` un-regressed.
- Coverage SESSION sub-metric (binding) **83.20%** (312/375) ≥ 80% (scoped
  llvm-cov; multi-thread varies 80.53–83.20% via the F-CAP-1 race arm, always
  ≥80%). `reset_peer` 100%.
- `reset_peer` assessment: NOT strictly load-bearing (detached task + inject_abort
  alone fix it — 72 iters clean with reset_peer no-op'd); NO multiplex collateral
  (a concurrent healthy 8 MiB stream to the same backend survived 5/5). Kept as
  defense-in-depth for the wedged-upstream (5 s timeout) case. → CF-RESETPEER-1.
- Determinism: verifier suite ×3 = 21/21 each.

## Phase 2 — H1→H2 — HONEST-STOPPED (lead, R7)
Per the mission honest-stop rule ("one fully-verified cell beats two half-done")
and R7 (lead owns the honest-stop call): with a security defect to fix +
re-verify + the mandatory Phase 3 ×3 gate consuming the remaining budget, I
honest-stopped Phase 2. **H1→H2 is NOT built this session.** Its R8 plan is
authored as the S11 head-start (incorporating the S10 F-MD-4 fix mechanism once
known). Rationale: shipping H1→H2 half-verified while H2→H2 carries a known
smuggling defect would be the opposite of the program's verification-quality bar.

## Phase 2 — H1→H2 (honest-stop gated)
(decision pending H2→H2 completion + budget)

## Phase 3 — gates + regression — GREEN
`cargo test --workspace --all-features` ×3 (real cargo exit via PIPESTATUS):
- RUN 1/2/3: cargo_rc=0, **1224 passed / 0 failed / 16 ignored**, deterministic
  (1203 S9 baseline + 21 new H2→H2 proofs). The hardened F-MD-4 (24-iter loop) +
  F-CAP-1 ran INSIDE the parallel ×3 gate (R10) with no flake.
- `cargo fmt --check` clean (rc=0); `cargo clippy --all-targets --all-features
  -- -D warnings` clean (rc=0). Disk ended 26 GB free (above the 25 GB floor, R9).
- R3: no regression to S1–S9 or the prior 5 BUILT cells (subsumed by the green
  full-workspace ×3 + the verifier's explicit sibling sweep).
- Coverage: the binding H2→H2 session sub-metric is 83.20% (verifier-measured,
  scoped llvm-cov per R10/CF-DISK-1) ≥ 80%.

## Carry-forwards
- **CF-RESETPEER-1 (NEW):** `Http2Pool::reset_peer` tears down the whole
  multiplexed connection on a single-stream abort. Proven this session: NOT
  strictly load-bearing (the detached send task + `inject_abort` FIFO observation
  fix the smuggle alone) and NO multiplex collateral (concurrent stream survived).
  Kept as defense-in-depth for the wedged-upstream (`tx.closed()` 5 s timeout)
  case. Refinement question for CF-DEDUP-1/S11: keep, narrow to per-stream, or
  drop. Carries into H1→H2 (same egress).
- **CF-RESP-1 (NEW, plan time):** response-leg `collect()` buffering also in
  H1→H2 (`upstream_response_to_h1`), H2→H3 (`h3_response_to_h2`), and likely the
  H3-front response paths — convert per remaining unbuilt cell; relates to the H3
  "response-egress headline" note.
- **CF-DEDUP-1 (now compelling):** H2→H2 and H1→H2 share the IDENTICAL
  egress-to-`Http2Pool` streaming machinery (detached send + reset_peer +
  inject_abort + F-CAP-1 arm + verdict gate); only the ingress pump differs (H2
  M-D vs H1 M-D-lite). Extract a shared helper at S11 and re-verify both cells.
- **METHOD note (binding for future cells):** the parallel ×3 gate MASKED a real
  F-MD-4 smuggle (passed 20/20 while the bug reproduced 25–50% single-threaded).
  Every cell's F-MD-4/smuggling regression MUST be hardened (current_thread +
  internal repetition + a load-bearing negative control proving it fails pre-fix)
  so the gate actually catches it. Encoded in the S11 plan's BUILT bar.
- Carried: CF-DEP-1 (2 dependabot advisories on the default branch — owner work,
  surfaced on every push this session), CF-IGN-1 (16 inherited #[ignore]),
  F-ESC-1, N-1, S4-NUANCE-1, CF-COV-1/2, CF-COV-S7, CF-DISK-1 (encoded in R10).

## S11 handoff (dependency-ordered remaining cells — 3 of 9 unbuilt)
1. **H1→H2** (plan ready, `s11-h1h2-plan.md`): H1 M-D-lite ingress (None=clean /
   Some(Err)=truncation F-MD-4 mirror-image) + M-B egress + response-leg stream.
   MUST reuse the S10 detached-send/reset_peer/inject_abort egress hardening from
   the start (same `Http2Pool` graceful-drop hazard) — consider the CF-DEDUP-1
   extraction here and re-verify H2→H2 + H1→H2 together. → 7 of 9.
2. **H2→H3 / H1→H3**: need M-C (H3 upstream via QUIC, heaviest); response leg
   `h3_response_to_h2` also buffers (CF-RESP-1).
Then: chaos/soak suite, native QUIC proxy, WS/gRPC-over-H3, full h3spec
conformance. Owner: CF-DEP-1 (2 advisories).

Base for S11: promote tip of `feature/h-matrix-s10` (this session) on `main`.
Per the standing rule, verify `main`'s tree before branching — do not trust the
prompt's stated base.
