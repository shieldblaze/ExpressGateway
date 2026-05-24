# SESSION 9 — H-to-H MATRIX — REPORT

- Branch: `feature/h-matrix-s9` (base `main` @ `8723c205`, the S8 promote).
- Team (agent teams, Opus, own worktrees): `lead`, `protocol-eng` (protocol-expert),
  `builder-1` (general-purpose), `verifier` (general-purpose). Strict author≠verifier.
- **Outcome: H1→H1 BUILT (M-D-lite) — 5 of 9 cells. F-CAP-1 conformance fix landed on
  H1→H1 AND the previously-promoted H2→H1. Phase 2 honest-stopped (H2→H2 plan authored).**

---

## VERDICT: SESSION 9 COMPLETE
H1→H1 built and independently verified to the BUILT bar; the F-MD-4 cross-cell
sweep closed (all 4 prior cells PASS); the F-CAP-1 conformance defect fixed and
re-verified in both affected cells; Phase 3 ×3 gate green; promoted per R11.
Phase 2 honest-stopped per owner ruling (verification quality over cell count).

---

## Phase 0 — green baseline (R1 ×3)
Base tip confirmed `8723c205`. S8 stray disk-watcher (PID 159289 + child) killed;
3 stale S8 worktrees removed; `feature/h-matrix-s9` branched.
- `cargo test --workspace --all-features` ×3: **1182 passed / 0 failed / 16 ignored**,
  deterministic. `cargo fmt --check` clean. `cargo clippy --all-targets
  --all-features -- -D warnings` clean. (The 16 ignored are inherited from the S8
  baseline — not introduced this session; see CF-IGN-1.)

## Phase 0a — F-MD-4 (RST≠EOF) cross-cell sweep — ALL 4 PASS
`audit/h-matrix/s9-fmd4-sweep.md` (commit `04f04661`). protocol-eng audited; lead
independently re-confirmed by mechanism.
| Cell | Verdict | Mechanism (lead-confirmed) |
|---|---|---|
| H2→H1 | PASS | `is_end_stream()` ×5 gate + `PumpAbort`-as-Err on every abort arm |
| H3→H1 | PASS | `conn_actor.rs` `decode_into_pending`: `End` emitted only inside `if fin{}`; RESET→`Reset` (no terminator written) |
| H3→H2 | PASS | same producer gate → `H3ReqAbort` Err ⇒ hyper RST_STREAMs the H2 upstream |
| H3→H3 | PASS | `End`→genuine QUIC FIN; RESET/None→`AbortNoFin`→shutdown(Write, CANCELLED) no FIN |
The H3 row gates the positive stream-end at the SHARED producer (`conn_actor.rs`),
not the consumer. Each cell has a non-vacuous, non-`#[ignore]`'d real-wire
`complete=0` regression test, all confirmed running inside the R1 ×3 gate. No
defect, no fix. `StreamStopped` documented as a fail-safe arm (shares the catch-all
route to `Reset`); a forced recv-side STOP_SENDING test would be vacuous.

## Phase 1a — Q-H1..Q-H4 resolved (`s9-h1h1-plan-RESOLVED.md`, lead-approved)
- **Q-H1**: MIRROR the Branch-B pump in `h1_proxy` (protect the BUILT M-D cell; H1
  is Branch-B-only so sharing wouldn't be clean DRY). CF-DEDUP-1 later.
- **Q-H2**: pump confined to upstream body delivery; keep-alive cap (connection-
  driver state) + single-use `take_stream` (ROUND8-L7-10) preserved, regression-
  locked by `round8_body_overread.rs`.
- **Q-H3**: forward trailers faithfully (R3); hyper-1.9.0 `decode_trailers` does NOT
  reject forbidden framing fields → added `validate_h1_request_trailers` → 400.
- **Q-H4**: 64 MiB cap → 413 parity (documented cross-cell constraint, NOT a fork).

## Phase 1b/1c — H1→H1 built (builder-1) + independently verified (verifier)
Net-new = M-D-lite ingress pump in `h1_proxy.rs::proxy_request` + M-E proof harness.
Branch-B-only, bounded mpsc depth 8 × 8 KiB = 64 KiB fixed in-flight window →
in-flight-counting `StreamBody` → hyper http1 sender. Carried forward: F-MD-1
(force HTTP/1.1, strip CL/TE), F-MD-2 (receiver-drop→drain-and-validate≠413),
F-MD-3 (non-vacuous `H1_REQ_MAX_RETAINED_BODY_BYTES` gauge), **F-MD-4 mirror-image
on H1** (inbound body is `Kind::Chan`: `frame()==None` IS the clean end;
`Some(Err(IncompleteBody))` is the truncation signal — the inverse of H2's
ambiguous-`None`; `is_end_stream` deliberately not used). Local `H1PumpAbort`
(h2_proxy.rs byte-unchanged; `MAX_REQUEST_BODY_BYTES` imported).

Independent verification (`s9-h1h1-verify.md`; src diff-empty vs builder tip):
1. Real-wire both legs byte-identical (1000 B, 5 MiB, 8 MiB → 200).
2. Memory body-size-INDEPENDENT: in-situ retained = 1 window ≤ 256 KiB ≪ 4 MiB;
   inverted probe load-bearing (trips at 4 MiB); live-occupancy peak 64 KiB.
3. Backpressure both legs: 48 MiB body, pause ~8 MiB ≪ 48 MiB, resume to full+200.
4. **F-MD-4 both framings**: CL and chunked premature close → `complete=0`,
   `dials=1` (non-vacuous); pre-pump CL/TE → 400 `dials=0`.
5. 64 MiB cap → 413; Q-H3 legit trailer forwarded / forbidden trailer rejected.
6. Coverage (canonical, ×3 deterministic): H1 session sub-metric initially 95.54%.

## F-CAP-1 (escalated to owner → fix both cells) — fixed + re-verified
**Finding:** a streaming over-cap (>64 MiB) upload — or a forbidden-framing trailer
— returned **502** instead of **413/400** when the backend hadn't sent a response
head, because the caller mapped the `send_request` error before consulting the
pump's verdict. Identical in H1→H1 and the shipped H2→H1. Security/memory/smuggling
invariants held throughout (never relayed complete, memory bounded, upstream
aborted) — only the client status differed. S8 had left this arm UNTESTED (not a
blessed precedent), so I escalated per R7 (real product-behavior + re-opens shipped
code). **Owner ruling: fix both cells** (sibling divergence is the R4 anti-pattern;
body-too-large is a client error → 413 not retry-storm-inducing 5xx).
**Fix (caller-side only, both cells; SHA `23b45d6f`):** on `send_request` error,
await the pump verdict (bounded by `timeouts.body`) and prefer a classified
`BodyTooLarge`→413 / `BadRequest`→400 over the generic 502; genuine upstream
failures still → 502/504. FIFO Err-before-close / `is_end_stream` / `take_stream`
byte-unchanged in H2→H1.
**Re-verification (both cells, `s9-h1h1-verify.md` ROUND 2):** diff-empty proof on
both src files; H1 over-cap CL+chunked→413, forbidden trailer→400, genuine→502;
H2→H1 over-cap→413, genuine→502; deterministic (×8 / ×6, no flap). H2→H1 F-MD-4
`complete=0` + full S8 suite green (NO regression). H1 sub-metric **97.80%**;
F-CAP-1 caller arms **100% covered in BOTH cells**. Negative control (revert →
test fails 502≠413) proves the test is load-bearing.

## Phase 2 — HONEST-STOPPED (owner-ratified)
Per owner ruling, the F-CAP-1 both-cells fix + re-verify took the Phase-2 budget;
one (now two) correctly-verified cells beat two with known divergence. **H2→H2 R8
plan authored as the S10 head-start** (`s9-h2h2-plan.md`): composes BUILT M-D H2
ingress + M-B H2 upstream connector; net-new = the M-D→M-B egress seam; open
questions Q-HH-1..3 for build-time.

## Phase 3 — gates + promote
- `cargo fmt --check` clean; `cargo clippy --all-targets --all-features -- -D
  warnings` clean.
- `cargo test --workspace --all-features` ×3: **1203 passed / 0 failed / 16 ignored**,
  deterministic (3× identical) at full 8-core parallelism (= 1182 baseline + 21 new:
  builder e2e 6 + verifier suite 15). The F-MD-4 (all 4 cells) + F-CAP-1 (413/400)
  regression tests all ran INSIDE the ×3 gate (R10 — not isolated). Disk ended 31 GB.
- Promote per R11: `feature/h-matrix-s9` → `main` via `--no-ff` honest-message merge
  (message names the F-CAP-1 conformance fix landing on the previously-promoted H2→H1).

## Environment / disk event (R9)
Shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`. During the verifier's FIRST
coverage run, a bare `cargo llvm-cov --workspace` (no `--test` scoping) instrumented
the ENTIRE workspace into a separate `llvm-cov-target` (~18 GB, NOT reusing `debug`),
driving free disk from 28 GB → 8.4 GB. I surfaced immediately (R9) and took the
SURGICAL action (vs the literal `cargo clean`, which would have destroyed the active
coverage run): deleted the IDLE non-instrumented `eg-target/debug` (26 GB, no live
writer), restoring 34 GB while the verifier's active target was untouched. The
re-verify coverage used the SCOPED `--test` command (~3.5 GB). Phase 3 rebuilt
`debug` from scratch. See CF-DISK-1.

## Carry-forwards
- **CF-DISK-1 (NEW):** on the 67 GB box, scope `cargo llvm-cov` to the relevant
  `--test` binaries (S8 precedent); a bare `--workspace` instrumented run can't reuse
  `debug` and nearly ENOSPC'd. For S10, scope coverage or provision more disk.
- **CF-DEP-1 (NEW):** GitHub dependabot flags 2 advisories (1 moderate, 1 low) on the
  default branch — pre-existing, dependabot branches exist, out of S9 cell scope.
  Surface to owner; not introduced this session.
- **CF-IGN-1 (NEW):** 16 `#[ignore]`'d tests inherited from the S8 baseline; consistent
  across S6-S8. Characterize legitimacy in a future session (likely root/XDP-gated).
- **CF-DEDUP-1:** unify the H1 and H2 PumpAbort + pump once both cells are BUILT and
  both have independent regression locks.
- Carried from S8: F-ESC-1 (multi-kernel verifier CI lane), N-1 (jumbo-MTU
  xdp.frags doc), S4-NUANCE-1, CF-COV-1/2, CF-COV-S7 (H3→H3 no coverage slack).

## S10 handoff (dependency-ordered remaining cells — 4 of 9 unbuilt)
1. **H2→H2** (plan ready, `s9-h2h2-plan.md`): M-D ingress + M-B egress seam. Easiest next.
2. **H1→H2**: H1 M-D-lite ingress (now BUILT) + M-B egress.
3. **H1→H3 / H2→H3**: need M-C (H3 upstream via QUIC) — heaviest.
Then: chaos/soak suite, native QUIC proxy, WS/gRPC-over-H3, full h3spec conformance.
