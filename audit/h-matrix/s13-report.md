# SESSION 13 REPORT — H2→H3 (closes the H-to-H matrix 9/9) + CF-BODY-WALLCLOCK

Branch: `feature/h-matrix-s13` (base `main` @ `85017edc`).
Roles: lead (this report + verdict, no feature code) / builder-1 (H2→H3 build + CF-BODY-WALLCLOCK plan + F-CAP-1 fix) / verifier (independent verification, strict author≠verifier).

**VERDICT: SESSION 13 COMPLETE — matrix complete 9/9; production-ready pending 5-6 post-matrix sessions: native QUIC proxy, WS-over-H2/H3, gRPC-over-H3 verification, h3spec conformance, chaos/soak suite.**

---

## Phase 0 — baseline + environment hygiene

- Base tip confirmed `85017edc`; clean tree; no S12 strays (`ps aux`).
- **Disk (CF-DISK-1/R9):** started 20 GB (below the 25 GB floor; warm `eg-target` 32 GB). Surgical reclaim of `debug/incremental` (disposable compile cache) → 25 GB, deps cache preserved. Re-cleaned between heavy steps throughout (regenerates each build).
- `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` exported on every cargo call (unset in env / not in `.cargo/config.toml`).
- **R1 baseline GREEN:** `cargo test --workspace --all-features` ×3 deterministic — **211 binaries, 1259 passed, 0 failed, 16 ignored** (CF-IGN-1) identical each run. `clippy --all-targets --all-features -- -D warnings` 0 warnings. `fmt --check` clean.

## Phase 1 — H2→H3 (the net-new cell)

### §3 connector-contract — RESOLVED (lead, first action)
The plan's binary question ("dropped `body_tx` == Reset or FIN?") is answered STATE-DEPENDENT by `lb-quic/src/h3_bridge.rs`: first-peek-None (`:3309-3313`) → bodyless-COMPLETE HEADERS+FIN (the smuggle-risk arm); AwaitNext-None (`j2_req_event_action :4128`) → RESET-without-FIN. **Therefore Hazard (a)'s detached-pump + always-emit-terminal mitigation is LOAD-BEARING, not "defended by construction."** Plan APPROVED with this amendment.

### Build (builder-1) — commits `e1073f4b`, `00f69622`, `11ed191a`
Replaced the buffering `proxy_h2_to_h3` (`collect→request_h3_upstream→h3_response_to_h2`) with a both-legs streaming relay on the shared `lb_quic::stream_request_to_h3_upstream` + `H3RespOut::Decoded` connector — an exact mirror of the proven `proxy_h1_to_h3`. New: `build_h2_to_h3_fieldlist` (head-only), `h2_decoded_resp_head_builder` (H2→H2 head transform), `H2RespAbort` (constructible response-leg truncation error). `crates/lb-l7/src/h2_proxy.rs` +357/-88; new suite `tests/h2h3_md_streaming_verify.rs` +1921 (13 tests, mirror of h1h3). Connector UNCHANGED.
- **Hazard (a):** detached `tokio::spawn` pump owning the inbound body; emits `Reset` on `None && !is_end_stream()` / `Some(Err)`, `End{trailers}` on clean/validated-trailer end. Survives downstream H2 RST cancel.
- **Hazard (b):** `H3RespEvent::Reset → Err(H2RespAbort)` into the `StreamBody`; load-bearing arm is no-CL chunked (avoids the #13b CL-masking trap).
- **§(c)1:** the missing hop-by-hop strip is CORRECT — hyper's H2 server encoder omits `connection:` (RFC 9113 §8.2.2). NO R12.
- **Trailer-mandate WIN:** `grpc-status` reaches the H2 client as native `Frame::trailers` — POSITIVELY asserted. H2→H3 is the gRPC-capable →H3 cell.

### Independent verification (verifier) — report `audit/h-matrix/s13-h2h3-verify.md`, tree `git diff 11ed191a` EMPTY (all mutations reverted)
- **A** suite 13/13 (245s), 5 MiB byte-identity both dirs, `grpc-status="0"`.
- **B** mutation (a) line 2467 `Reset→End` → smuggle `complete_after=1` FAIL-as-designed; mutation (b) line 2580 delete `Err(H2RespAbort)` → chunked-arm `Ok(88684)` leak FAIL, CL arm still passed (confirms no-CL is the load-bearing arm). Both reverted.
- **C** R13(b) bursts current_thread: REQ 60/60 dialed=60 zero-complete; RESP-chunked 60/60 no-leak. Deterministic — the historically-flaky single-threaded smuggle did NOT reproduce.
- **D** gauge body-INDEPENDENT (4 MiB→gauge 8192); mutating away the per-chunk `fetch_sub` → memory test FAILs (`in_situ=4194304`) ⇒ non-vacuous; the harness `req_body_bytes_live` addition feeds only the non-vacuity tally, appears in no security assertion ⇒ STRENGTHENS the proof.
- **E** scoped session llvm-cov (DA per-line over exact fn ranges, `--workspace` lcov): first pass 216/260 = 83.1% ≥80%.
- **G** trailer mandate = real hard `assert_eq!(grpc_status, Some("0"))`.
- **H** ×3 `--workspace --all-features` gate: 1272 passed, 0 failed, 16 ignored, deterministic; clippy/fmt clean workspace-wide.

### Finding F-CAP-1 (verifier; LOW; R4 fixed-in-session, NOT asterisked)
The over-cap test passed via backpressure-stall, not the cap-RST it claimed — the H3 echo backend's `initial_max_data(16 MiB)` stalled forwarding before the 64 MiB cap, so the mid-body over-cap Reset branch (`h2_proxy.rs:2494-2496`) was never crossed (coverage-dead). **Fix (builder-1, test-harness ONLY, final `db61cb8d`):** new `spawn_h3_echo_backend_with_flow(…, max_data, max_stream_data)` lets the fcap1 backend use an 80 MiB window on BOTH `initial_max_data` and `initial_max_stream_data_bidi_remote` (other 8 call sites delegate with the old 16/8 MiB defaults — byte-identical). The first attempt (`6c7cacd0`) was itself FLAKY — the H2 client send loop broke on a transient zero-capacity poll, so some runs sent only 32/46 MiB and never reached the cap (would have flaked the ×3 gate, an R1 risk); `db61cb8d` makes only `Err(RST)`/closed/timeout terminal, so 3/3 runs forward the full 66 MiB → exactly the 64 MiB cap → Reset. Strengthened assertions: `backend_body_live ≥ 64 MiB − chunk` (non-vacuity: proves the over-cap arm, not a stall, aborted), `complete=0` (no smuggle), and `client_clean_200 == false` (downstream sees 502 + errored body — the cap-abort propagates, never a smuggled-complete). Cell source UNCHANGED (`git diff 11ed191a db61cb8d -- crates/` EMPTY). **Verifier re-confirmation on `db61cb8d`:** (1) diff test-only; (2) wire-determinism 5/5 plain runs — all forward the full 66 MiB, the over-cap arm fires, `backend_body_live` ∈ [67043328, 67075836] (all ≥ CAP−chunk = 67042816, just under CAP = 67108864), `complete=0`, `client_clean_200=false` (502 + errored body); the 38 MiB sub-cap pass seen at `6c7cacd0` is ELIMINATED; (3) coverage: the determinism fix (retry-on-zero-capacity) lets fcap1 survive instrumentation, so WITH fcap1 the over-cap Reset branch `h2_proxy.rs:2495-2496` is HIT and the session sub-metric reaches 224/260 = 86.2%; the CONSERVATIVE no-fcap floor is 216/260 = 83.1% (≥80% even WITHOUT the 2 over-cap lines). Security invariant (`complete=0`, no smuggle) holds in every run.

**Residual robustness note (verifier, honest):** the load-bearing `≥CAP−chunk` non-vacuity assert cleared by only ~512 B in the worst of 5 runs — a 64 MiB volume-dependent test under 8-core saturation is the CF-SATURATION-1 flake shape. **Ruling rule (verifier-recommended, lead-adopted):** the final ×3 `--workspace` SATURATION gate is the decider — fcap1 clean 3/3 ⇒ accept (over-cap arm deterministically wire-exercised + ≥80% conservative floor + identical pump-cap arm covered in H2→H1/H2→H2), with the volume-fragility recorded as CF-FCAP-MARGIN (a future fast pump-cap unit test would remove the volume dependency — hardening, not a blocker); fcap1 flakes even once ⇒ builder-1 adds that unit test + de-brittles the e2e BEFORE promote (R1/R2 — a flaky gate test is not acceptable). **Gate result: fcap1 PASSED cleanly 3/3 under the 8-core saturation gate** (log lines 954/3548/6142) ⇒ **RULING (a) — ACCEPT.** The thin-margin/volume concern did NOT manifest under saturation; R1's ×3-deterministic bar is met. F-CAP-1 RESOLVED (over-cap arm deterministically wire-exercised 5/5 plain + 3/3 saturated). Volume-fragility → CF-FCAP-MARGIN (future fast pump-cap unit test = hardening; NOT a blocker, NOT an asterisk — the finding is resolved and the gate is deterministic).

## Phase 2 — CF-BODY-WALLCLOCK — HONEST-STOPPED (build deferred to S14; plan authored)
builder-1's plan (`audit/h-matrix/s14-cf-body-wallclock-plan.md`, `0eb0a818`) established it is GENUINELY LARGE (R6) — **two mechanism classes**, one CROSS-CRATE:
- **Class A (H1→H1, H2→H1):** wall-clock wraps the L7's own hyper-H1-client send — `h1_proxy.rs:1508`, `h2_proxy.rs:1866`.
- **Class B (H1→H2, H2→H2):** the wall-clock is in **`lb-io::http2_pool` (`http2_pool.rs:232`, default 30 s)** — NOT lb-l7. Cross-crate fix.
- Fix = F-S7-6 idle-deadline; progress signal = the pump's `tx.send`-success (co-located with the R8 gauge, backpressure-park never counts ⇒ R8 preserved); genuinely-new **TWO-PHASE timer** (idle deadline for upload + a separate fixed head-timeout for the post-upload head-wait, since an opaque hyper send can't be idle-tracked once the pump is done). Single-source `idle_bounded_send`; Class B needs a new `Http2Pool::send_request_idle`.
- **Owner escalations (R7):** R-CFBW-1 (lb-io pool-API semantic change — pool-owner sign-off) and R-CFBW-2 (the fixed head-timeout value — product decision).
- Estimate **12-17 h**. 5th same-class site `CF-S7-RHU` (`request_h3_upstream`'s own fixed 30 s) to fold into the S14 sweep.
- Decision: rushing this cross-crate fix into the S13 tail forfeits a clean matrix close for a poorly-bounded change — honest-stop with a ready plan is the higher-leverage outcome (mission-anticipated).

## Phase 3 — gates + regression
- Final R1 ×3 `--workspace --all-features` on the post-fix tree (`db61cb8d`): **GREEN — 212 binaries, 1272 passed, 0 failed, 16 ignored, identical ×3** (deterministic at full 8-core parallelism). fcap1 clean 3/3 under saturation.
- clippy `-D warnings` + fmt `--check` workspace-wide: **clean (exit 0)**.
- R3 no-regression: S1-S12 + the 8 prior BUILT cells intact (verifier first-pass ×3 showed 1272/0/16 with all sibling cells running).
- R13 (a)+(b)+(c) evidence both F-MD-4 legs: §Phase-1 B/C above.
- Scoped session coverage ≥80%: conservative no-fcap floor **216/260 = 83.1%** (clears ≥80% WITHOUT the 2 over-cap lines); full measure incl. fcap1-under-instrumentation **224/260 = 86.2%** (over-cap Reset arm covered). Binding session sub-metric, verifier-measured independently (lcov DA per-line over exact fn ranges, `--workspace` for dep-crate lines).

## Matrix status — 9/9 (BUILT bar; R11)
H1→H1, H2→H1, H1→H2, H2→H2, H3→H3, H3→H1, H3→H2, H1→H3 (prior) + **H2→H3 (this session)** = **9 of 9 streaming cells**.
**Matrix complete ≠ production-ready (R11).** Remaining program-level work (S14+, est. 5-6 sessions): native QUIC proxy, WebSockets-over-H2 (RFC 8441), WebSockets-over-H3 (RFC 9220), gRPC-over-H3 conformance verification, full h3spec conformance pass, chaos/soak suite.

## S14 handoff
1. **CF-BODY-WALLCLOCK build** — plan `0eb0a818` ready; two classes, cross-crate (lb-io), two-phase timer; owner sign-off R-CFBW-1 (pool API) + R-CFBW-2 (head-timeout value) needed FIRST; fold in CF-S7-RHU. Est 12-17 h.
2. Post-matrix road to production: native QUIC proxy; WS-over-H2/H3; gRPC-over-H3 conformance; h3spec; chaos/soak.

## Carry-forwards (not in S13 scope)
F-ESC-1 (multi-kernel CI lane), N-1 (jumbo-MTU doc), S4-NUANCE-1, CF-COV-1/2/S7/S11, **CF-DEP-1 (2 Dependabot advisories — owner, now SIX sessions old; reconfirmed on push)**, CF-IGN-1 (16 inherited `#[ignore]`), CF-DISK-1 (recurring 25 GB squeeze — reclaim `debug/incremental`), CF-RESETPEER-1, CF-RESP-1 (closed for →H3; H1-front trailer CASE-ii carried), CF-S7-RHU (→H3 request-leg fixed 30 s, fold into CF-BODY-WALLCLOCK sweep), **CF-FCAP-MARGIN (NEW — the H2→H3 fcap1 e2e proves the over-cap arm via a 64 MiB upload with a ~512 B non-vacuity margin; saturation-stable 3/3 but a future fast pump-cap unit test would remove the volume dependency for instrumentable, margin-free coverage — hardening, low priority).**
