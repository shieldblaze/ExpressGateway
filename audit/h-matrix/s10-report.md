# SESSION 10 — H-to-H MATRIX — REPORT (in progress)

- Branch: `feature/h-matrix-s10` (base `main` @ `cce5a8ed`, the S9 promote).
- Team (Opus, strict author≠verifier): `lead` (this), `builder-1` (general-purpose),
  `verifier` (general-purpose). Worktree strategy: see "Process note" below.
- Goal: H2→H2 to the R8 BUILT bar (→ 6 of 9), then H1→H2 honest-stop-gated.

---

## VERDICT: (pending)

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

**Verification (verifier, independent):** (pending — full R8 proof battery both
legs + F-MD-4 + F-CAP-1 + coverage re-measure + R3 sweep)

## Phase 2 — H1→H2 (honest-stop gated)
(decision pending H2→H2 completion + budget)

## Phase 3 — gates + regression
(pending)

## Carry-forwards
- CF-RESP-1 (NEW, surfaced at plan time): response-leg `collect()` buffering also
  in H2→H3 (`h3_response_to_h2`) and likely H3-front response paths — track for
  remaining unbuilt cells; relates to the H3 "response-egress headline" note.
- Carried: CF-DEDUP-1 (unify H1/H2/H2H2 pumps), CF-DEP-1 (2 dependabot advisories,
  owner), CF-IGN-1 (16 inherited #[ignore]), F-ESC-1, N-1, S4-NUANCE-1,
  CF-COV-1/2, CF-COV-S7, CF-DISK-1 (encoded in R10).

## S11 handoff
(to be written at COMPLETE)
