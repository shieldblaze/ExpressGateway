# SESSION 11 REPORT — H-matrix: H1→H2 BUILT (7 of 9)

**Branch:** `feature/h-matrix-s11` (base main @ 2f9f13bb). **Cell built:** **H1→H2**
(HTTP/1.1 front ⇒ HTTP/2 backend), both legs converted from buffering to R8
bounded-incremental streaming, F-MD-4 graceful-drop fix inherited + the H1-side
per-stream guard proven load-bearing. **H1→H3 honest-stopped** with R8 plan
authored (`s12-h1h3-plan.md`).

**Verdict: SESSION 11 COMPLETE** — H1→H2 BUILT & verified (7 of 9 cells); H1→H3
honest-stopped with R8 plan authored; Phase 3 GREEN (R1 ×3 1241/0 deterministic,
scoped llvm-cov session sub-metric 81.02% ≥80%, clippy/fmt clean, R3 intact, R13
(a)+(b)+(c) recorded). Two pre-existing test-harness flakes the heavier gate
exposed were fixed (not asterisked). Promote to main per R11.

Team: lead (coordination, gates, verdict — no feature code), builder (feature
code, agentId adf822521ab4dd07f), verifier(s) (independent real-wire proofs +
coverage; agentIds aec7a7ff935429a71 [I1/I2], a6287b52e8ea1b3d9 [I3]). Strict
author≠verifier on every increment.

## Phase 0 — baseline (GREEN)
- Base tip confirmed main @ 2f9f13bb; feature/h-matrix-s11 created+pushed.
- No stray S10 processes; disk 27 GB free (≥25 GB floor); eg-target ubuntu-owned.
- R1 ×3: run1/2/3 all exit 0, **1224 passed / 0 failed** each.
- clippy `--all-targets --all-features -D warnings` clean; `fmt --check` clean.

## Increments (author≠verifier each)

### I1 — extract `drive_h2_upstream_send` (CF-DEDUP-1) + re-wire H2→H2 [71cbc01d]
- Builder: extracted the S10 F-MD-4 detached-send block (h2_proxy.rs ~2300–2413)
  into one `pub(crate) async fn drive_h2_upstream_send(pool, backend_addr,
  upstream_req: Request<H2ReqBody>, verdict_rx, pump, body_timeout) ->
  Result<Response<IncomingBody>, ProxyErr>` (h2_proxy.rs:2582). Logic verbatim;
  only mechanical deltas (`pool.clone()`/`body_timeout` params; `ProxyErr`
  widened pub(crate)). H2→H2 Branch B re-wired to call it. NET BEHAVIOUR
  UNCHANGED. (Surfaced: two independent `ProxyErr` enums — h1_proxy:1797 +
  h2_proxy — the helper speaks the h2_proxy one.)
- Verifier (INDEPENDENT): H2→H2 suite (`tests/h2h2_md_streaming_verify.rs`)
  **21/21** under parallel gate. R13(b) isolation-burst on `fmd4_client_rst_mid_
  body_never_complete_at_h2_upstream`: **50/50, 0 smuggles**. R13(c) load-bearing
  negative control: defeating the detached-task structure → **8/8 smuggle**,
  restored → pass. → I1 VERIFIED, helper load-bearing. (Mechanism note: the
  per-stream `inject_abort!` lives in the caller's pump; the helper's uniquely
  load-bearing contribution is detached-task body ownership — BOTH needed.)

### I2 — H1→H2 streaming REQUEST leg [9cc16b61] + verifier suite [5e1c2357]
- Builder: `proxy_h1_to_h2_request` (h1_proxy.rs:1586) mirroring
  `proxy_h2_to_h2_request` with H1 M-D-lite ingress framing (None=clean,
  Some(Err)=truncation + `inject_abort!`(H1PumpAbort), NO `is_end_stream`),
  head-only preamble `build_h1_to_h2_upstream_parts`, `record_retained_h1`
  gauge, Branch A (validate-before-dial, zero pool contact) / Branch B
  (bounded pump → `drive_h2_upstream_send`). Dispatch `proxy_h1_to_h2` maps
  ProxyErr → 502/504/413/400. Dead `translate_h1_request_to_h2` removed.
  `H2_ABORT_OBSERVE_TIMEOUT` widened pub(crate).
- Verifier (INDEPENDENT, 13-test real-wire suite H1 client→h1_proxy(h2_upstream)
  →real H2 backend):
  - Byte-identity Branch A (1 KiB) + Branch B 5 MiB + 8 MiB — byte-for-byte both
    directions, status 200, backend `complete=true`.
  - Memory gauge non-vacuous + LOAD-BEARING: in-situ retained **122,864 B** (≤
    4×window 262,144; ≪ 4 MiB body); inverted probe `record_retained_h1(4 MiB)`
    → gauge 4,194,304 > 262,144, proving the ≤4×window assert catches a buffering
    regression. Live-occupancy peak 81,920 B for a 4 MiB stream.
  - Backpressure: paused at 5,636,096 B of 48 MiB (≤8 MiB bound); resume → 200,
    full 50,331,648 B delivered.
  - **F-MD-4 H1 mirror-image**: R13(a) in-gate (2 current_thread tests in the
    13/13 parallel suite); R13(b) isolation-burst **CL-short 50/50 + chunked
    25/25, 0 smuggles** (1200 + 600 internal reps); R13(c) load-bearing negative
    control — broke the pump `Some(Err)` `inject_abort!` → **12/12 smuggle**
    (truncated 262,144 B relayed as COMPLETE), restored → pass.
  - **F-CAP-1**: over-cap (66 MiB) → **413**; forbidden trailer (>window) →
    **400** no leak; dead backend → **502**. Matches promoted H2→H2 (R12 OK;
    forbidden-trailer 400 nuance is H1-side classifier emitting clean 400 vs
    H2's RST — security-equivalent, no leak).
  → I2 VERIFIED — request leg BUILT-bar (request side) met.

### I3 — H1←H2 streaming RESPONSE leg [1fa40e26] + verifier suite [eaca7129]
- Builder: `upstream_response_to_h1` (h1_proxy.rs:2579) converted from
  `body.collect().await` to streaming — H2→H1 head transform (status +
  `crate::h2_to_h1::RESPONSE_HOP_BY_HOP` strip [widened pub(crate)] + lowercase +
  alt-svc) then `body.boxed()`. async→sync. `build_h1_response_with_trailers`
  kept intact (still used by `h3_response_to_h1`). No error adaptation (H2
  Incoming body error is already hyper::Error).
- Verifier (INDEPENDENT): R3 request suite still **13/13**; large response
  byte-identity 5/8 MiB; **streaming head-before-full-body** (head at 42 ms while
  backend stalls mid-body; remainder released later byte-identical) with a
  load-bearing negative control (revert to collect() → head wait times out →
  FAILS; restored). **Trailer wire determination (D3/CF-RESP-1) = CASE (ii)**:
  raw H1 wire shows chunked body + `0\r\n\r\n`, NO `x-checksum`, NO `Trailer:`
  header → hyper's H1 encoder drops H2 response trailers absent a head
  declaration. Confirmed no existing test asserts proxy-level H1←H2 wire trailer
  relay → bounded documented behaviour (matches nginx default), NOT a regression.
  Relaying trailers while streaming is impossible without re-buffering (R8) → the
  correct tradeoff; tracked as CF-RESP-1.
  → I3 VERIFIED — response leg BUILT-bar met (trailer behaviour = case ii).

### R3 gate-blocker #1 — drain-test ephemeral-port TOCTOU flake [2cd61cd8]
- Phase-3 gate run 2/3 flaked: `test_sigterm_drains_h2_with_goaway` panicked
  `backend bind: AddrInUse` (reload_zero_drop.rs:420). MECHANISM (R2, proven):
  `ephemeral_port()`'s bind-port-0 → read → DROP → consumer-rebinds TOCTOU; my
  +17 concurrent H1→H2 test binaries churned ephemeral ports and exposed the
  foundation harness's self-documented race. NOT product code; my tests use the
  correct bind-and-hold pattern. Per R2 (port collision = real defect, not
  environmental) / R3 (foundation flake = blocker) / R4 (fix-and-verify) / R6
  (tractable) → FIXED this session.
- Builder fix (test harness only, no product/SIGTERM/GOAWAY assertion or timing
  changed — diff-verified): PART A bind-and-hold in-process backends
  (`spawn_blocking_h1_backend`/`spawn_slow_h1_backend` → return real addr,
  config written after); PART B `try_spawn_gateway` retry-with-fresh-port for the
  subprocess gateway listener (h1/h2 drain). H3 drain unchanged (no in-process
  backend; sound). Builder 30× isolation burst: 30/30 ok, 0 AddrInUse.
- Lead-run authoritative confirmation: R1 ×3 on 2cd61cd8 → 3/3 green (1241/0
  each) under full-workspace concurrency (the cross-binary collision condition).

### I4 — fmt fix [18bd9ec6]
- `cargo fmt` — formatting-only (line wrap/collapse) in the S11 new code that
  build+clippy did not flag; semantics-preserving. fmt --check clean.

### R3 gate-blocker #2 — F-CAP-1 H2→H1 backend timeout under saturation [705916d1]
- A SECOND authoritative ×3 run flaked (run 3): `fcap1_h2_over_cap_upload_yields_
  413` (h2h1_md_streaming_verify.rs) → expected 413, got **502**. MECHANISM
  (R2, proven): `status=502 written=131072 backend_body_bytes=131215` — only
  128 KiB forwarded (≪ 64 MiB cap), so NOT a 413-vs-502 verdict race. Root cause:
  `spawn_body_counting_backend`'s **3 s per-read timeout** (`Err(_) => break`)
  fired under full `--workspace --all-features` 8-core CPU saturation (>3 s
  scheduling gap), closed the upstream mid-upload; the gateway CORRECTLY returned
  502 for a genuinely-dropped upstream — **zero server-side misbehaviour** (the
  R2 environmental exception, mechanism shown), i.e. a test-harness fragility.
  Isolation burst: 30/30 (manifests only under parallel saturation).
- Fix (test harness only, no assertion weakened): bumped that 3 s + the two
  same-class h1h1 drain-loop timeouts (5 s) to **30 s** (~10× starvation margin,
  bounded under the tests' 60 s client budgets). The product's 502-on-genuine-
  upstream-drop behaviour is correct and unchanged.

## Phase 3 — gates (final commit 705916d1)
- **R1 ×3** (`cargo test --workspace --all-features`): **3/3 green, 1241 passed /
  0 failed each, deterministic** (after both flake fixes). The first two ×3
  attempts each exposed one pre-existing flake (drain TOCTOU; F-CAP-1 backend
  timeout) — both fixed, then a clean ×3.
- **clippy** `--all-targets --all-features -D warnings`: clean (exit 0).
- **fmt --check**: clean (exit 0).
- **Scoped llvm-cov session sub-metric (verifier, INDEPENDENT, two-step
  `--no-report` then `report --package lb-l7`)**: **81.02% (303/374 DA lines) ≥
  80%**. Per-fn: proxy_h1_to_h2_request 86.4%, build_h1_to_h2_upstream_parts
  96.2%, concat_h1_chunks 80.0%, proxy_h1_to_h2 dispatch 77.3%,
  upstream_response_to_h1 75.7%, h1_to_h2_proxy_err 57.1% (completeness arms),
  drive_h2_upstream_send 62.9% (benign head-wins-race relay path under-exercised
  — the SECURITY-CRITICAL abort/reset path IS covered by R13c). Method:
  `audit/h-matrix/s11-h1h2-cov.awk`. (Whole-file h1_proxy 43.3% / h2_proxy 44.0%
  confirm the dilution the scoped metric avoids.)
- **R3 no-regression**: S1–S10 + 6 prior BUILT cells intact — 1241/0 (= 1224
  baseline + 13 h1h2_md + 4 h1h2_resp), no foundation test removed/ignored/weakened.
- **R13 layered smuggling (F-MD-4)**: (a) in-gate ×3; (b) isolation-burst CL
  50/50 + chunked 25/25, 0 smuggles; (c) load-bearing negative control 12/12
  smuggle-when-broken → restored. All recorded (I1 H2→H2 + I2 H1→H2).

## Honest-stop — H1→H3 (Phase 2)
Budget consumed by the H1→H2 build + the deep independent verification (the
verifier's R13 bursts alone ran ~2 h) + the R3 drain-flake fix. Per the exit rule
("one fully-verified cell beats two half-done"), H1→H3 was NOT built. Its R8 plan
is authored: `audit/h-matrix/s12-h1h3-plan.md`. Key findings recorded there: the
→H3 egress (`request_h3_upstream`) buffers BOTH legs (collected Bytes request +
whole-body `H3UpstreamResponse`); the BUILT H3→H3 cell streams via a separate
conn_actor connector; **CF-DEDUP-2** (new) — extract a shared streaming
H3-upstream connector serving all three →H3 cells (H1→H3, H2→H3, H3→H3); F-MD-4
on the H3 upstream is RESET-without-FIN; the h3 decode_frame R8 trap applies.

## Carry-forwards
- **CF-RESP-1** (H1←H2 leg): streamed H1←H2 responses do not forward H2 response
  trailers (hyper needs a pre-declared head `Trailer:`; impossible while
  streaming). Bounded, documented (case ii), matches nginx. Also still open for
  H3 upstream response (`request_h3_upstream` whole-body read; multi-GB ceiling).
- **CF-DEDUP-2** (new): shared streaming H3-upstream connector across →H3 cells.
- **CF-DEDUP-1** (S11 partial-resolve): `drive_h2_upstream_send` now single-sources
  the H2-upstream egress for H2→H2 + H1→H2; H3→H2 still has its own egress —
  review for convergence before program end.
- **CF-SATURATION-1** (new): the full `--workspace --all-features` gate at 8-core
  saturation exposes tight (3–5 s) per-read/idle timeouts in real-wire test
  backends as load-induced flakes (drain TOCTOU + F-CAP-1 backend both this
  session). Two instances fixed; a sweep of remaining short backend timeouts in
  the streaming-verify suites is warranted before the matrix grows further.
- **CF-COV-S11** (new): `drive_h2_upstream_send` head-wins-race relay path
  (h2_proxy ~2666/2697/2703–2716) is under-exercised — in all 38 scoped tests the
  pump verdict resolves before the upstream head, so the benign relay branch is
  never taken. Aggregate ≥80% met; the security-critical abort path is covered.
  Add a head-wins-race test (slow-verdict / fast-upstream-head) to close it.
- **CF-RESETPEER-1**, **CF-COV-*, CF-IGN-1, CF-DEP-1** (2 Dependabot advisories,
  owner — still surfaced on every push), **F-ESC-1, N-1, S4-NUANCE-1**: carried.

## S12 handoff
- **2 cells remain**: H1→H3, H2→H3 (both →H3; both buffering). Build per
  `s12-h1h3-plan.md` — extract the shared streaming H3-upstream connector first
  (CF-DEDUP-2), wire H1→H3, then H2→H3 → **9 of 9** matrix complete.
- Promote feature/h-matrix-s11 to main (`--no-ff`) per R11 before S12 branches.
- Program-level still unbuilt: chaos/soak suite, native QUIC proxy, WS/gRPC-over-
  H3, full h3spec conformance.
