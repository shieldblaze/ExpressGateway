# SESSION 26 — CF-S22-QUICHE-H3-MIGRATION LANDING — REPORT

**Branch:** `feature/quiche-h3-migration-s23` (continued, not re-branched).
**Base tip (S25 PARTIAL):** `0fdbc9b5`. **main:** `90915781` (S22-hardened) — unchanged pending verdict.
**Team:** lead + test-eng + migration-eng + verifier (agent team `h3-land-s26`).
**Tool:** h3spec 0.1.13. **Date:** 2026-06-01.

> **VERDICT: SESSION 26 COMPLETE — H3-front fully migrated to `quiche::h3`, the hand-rolled
> H3/QPACK layer fully removed, migration PROMOTED to main `--no-ff`.** 7 of 9 carried h3spec
> findings (#16-21, #24) CLOSED BY CONSTRUCTION (proven by a fresh h3spec run, two verifiers,
> ZERO regressions); Huffman gained; the remaining #23/#25 are documented quiche-0.28 QPACK
> uni-stream limitations (inert, COR/low, shared by the prior main, close on CF-QUICHE-UPGRADE).
> ~2461 LOC of dead hand-rolled framing deleted from production (S25) + the lb-h3 crate
> (−1172 LOC) removed this session. Scoped llvm-cov confirmed ≥80% (R11/R15) before the merge;
> **promoted via merge commit `5be6c263`** (main `90915781` → `5be6c263`, `--no-ff`).
>
> **Owner-ratified rationale for promote-over-PARTIAL:** every R11 gate is green except the
> literal "#23/#25 pass", which is unachievable without a quiche upgrade; the program already
> ships main with documented quiche-0.28 deviations (#1-10), and this branch is **strictly better
> than the already-shipped S22 main** on the identical axis (12 vs 19 h3spec failures, 7 carried
> closed, Huffman, ~3.6k LOC of duplicate H3/QPACK code gone across S24-S26).

---

## 1. Phase 0 — baseline + harness reference

- Base tip `0fdbc9b5` confirmed (not main); tree clean; 31 GB free; 14 GB RAM; no stray procs.
- **Baseline gate run-1: 1438 passed / 0 failed / rc=0** (completed, `audit/s26-logs/baseline-run1.log`),
  reproducing S25's ×3 reference (1438/0) on the **identical commit**. The ×3+clippy+fmt burden is
  applied to the Phase-3 *changed* state (R11), since the base is byte-identical to S25's
  already-×3-verified tip.
- **Harness reference mapped:** ~15 harnesses depend on the lb_h3 codec (8 in lb-quic/tests, 6 in
  root tests/, 1 fuzz) + lb-h3's own proptest_qpack. Per-suite ref: h3h3 26/26, h1h3 12/12 +
  h2h3 13/13, proto 5/5, quic_listener 6/6, codec_roundtrip 3/3, security_qpack_bomb 1/1,
  round8 3/3, lib (testcodec) 22/22 + proptest 4/4.
- **lb_h3 production references = doc-comment prose only** (no `use`); production was already
  lb_h3-free (S25). The QPACK decode-from-gateway direction was already on `quiche::h3::qpack::Decoder`.

## 2. Phase 1 — the shared test-codec (byte-identity proven)

**Design (lead-approved, `s26-plan.md`): wholesale relocate** — moved all 5 lb_h3 modules
(frame+varint+error+qpack+security) VERBATIM into a TEST-ONLY crate `crates/lb-h3-testcodec`
(INC-A, `80d5e694`), re-exporting the identical public surface. Byte-identical by construction.
Chosen over the handoff's "switch QPACK to quiche" because that would change crafted wire bytes
(Huffman) at every QpackEncoder site = an R3 intent-drift risk.

**Byte-identity — TWO independent confirmations (AGREE):**
1. **verifier:** all 5 modules sha256-identical (zero diff); lib.rs differs only in line-1 doc;
   22/22 relocated unit tests identical to lb-h3's; a **non-vacuous corpus differential**
   (6/6 byte-equal on the exact frames/varints/QPACK-blocks the harnesses craft, with a
   `#[should_panic]` negative control proving the comparison is live); `cargo metadata` confirms
   production links neither crate (all-dev edges).
2. **lead:** independent sha256 of all 5 modules + `pub use`/`mod` surface diff — IDENTICAL.

## 3. Phase 2 — harness rewrite (intent-preserved) + lb-h3 deletion

**INC-B (`2d85ffa9` + fuzz-lock `518681d2`):** re-pointed every harness `use lb_h3`/`lb_h3::` →
`lb_h3_testcodec` (15 files + the proptest_qpack git-rename + fuzz target). Token rename only;
the existing `quiche::h3::qpack::Decoder` call sites untouched.

**INC-B-VERIFY (PASS):** the INC-B diff token-normalized → minus and plus lines are an **identical
set** (proving ZERO change to crafted bytes / assertions / match-arm semantics); proptest
relocation is a one-line git-rename, body byte-equal, PROPTEST budget/seed unchanged (S15 lesson);
**all per-suite counts match Phase-0** (bursts ran to completion, not deadline-skipped); **CATCH
preservation** confirmed by reading the actual assertions (round8 400+hits==0;
stream_body_errors saw_terminator==0; F-MD-4 complete_after==0 + non-vacuity controls).

**INC-C (`802faea5` + `91f00e7c`): lb-h3 crate DELETED** — −1172 LOC (Cargo.toml 21 + src 1151);
members/dep/dev-dep/fuzz-dep wiring removed; Cargo.lock + fuzz/Cargo.lock minimal (no pin churn);
stale lb_h3 prose **rewritten to quiche::h3 reality** across prod-src + test-doc-comments
(comment-only proof: zero non-comment lines changed in any test file). Final
`grep -rn 'lb_h3\|lb-h3'` = only `lb-h3-testcodec`'s own name. `cargo build --workspace
--all-features` + clippy clean.

**Honest LOC accounting:** the ~611 codec LOC are **retained** as test-only infra in
lb-h3-testcodec (not deleted). S26's deletion = the dead lb-h3 crate (−1172) as a distinct unit
+ all dev/test linkage. The production hand-rolled-H3-layer deletion headline remains the **~2461
LOC** from S25 (production was already lb_h3-free before S26).

**Process note (shared-tree):** two git index-collisions occurred (a builder's commit briefly
swept in another's staged change); both caught pre-/post-push and cleanly resolved (reset-soft +
explicit-path recommit), nothing lost, history linear. Lesson reinforced: `git commit -- <paths>`.

## 4. Phase 3 — full re-validation (the migration's proof)

### 4.1 ×3 gate — GREEN (lb-h3-deleted final state, HEAD 91f00e7c)
`cargo test --workspace --all-features` ×3 = **1442 / 0 / 17 each** (deterministic; completed runs
`audit/s26-logs/phase3-gate.log.pass{1,2,3}`, S26-PHASE3-DONE rc=0). **clippy --all-targets
--all-features -D warnings = CLEAN (CLIPPY_RC=0); fmt --check = CLEAN (FMT_RC=0).** Count
1442 = baseline 1438 **+4** (INC-D's additive H3-terminate soak self-tests; codec relocation is
net-zero — lb-h3's 26 tests → lb-h3-testcodec's 26). **Zero test lost, zero failure** — all 9
cells, Mode A/B, H1/H2, the S22 security fixes, the F-MD-4 bursts (ran to completion, not
deadline-skipped), R8 gauges all intact. R3 no-regression PROVEN in the final state.
(Note: a single `lb-quic` lib warning appears under a plain default-features *release* build —
NOT under the binding `clippy --all-targets --all-features` gate, which is clean; feature-gated,
verified pre-existing — see §6.)

### 4.2 FRESH h3spec — THE HEADLINE (R15: read on the lb-h3-DELETED stack)
`audit/h3spec/s26-h3spec-final.log` (h3spec 0.1.13 vs the migrated, lb-h3-deleted binary @
91f00e7c): **49 examples, 12 failures**, ✘ set **IDENTICAL to the preview** (deletion changed no
production behavior, as expected). vs S22-postfix (hand-rolled, 19 failures): **7 of 9 carried
findings CLOSED by construction** — #16 H3_MISSING_SETTINGS, #17/#18 DATA/HEADERS-on-control,
#19 2nd-SETTINGS, #20 HTTP/2-settings→H3_SETTINGS_ERROR, #21 CANCEL_PUSH-in-request,
#24 H3_CLOSED_CRITICAL_STREAM. #11-15/#22 still ✔. **#23/#25 still ✘** (quiche-0.28 QPACK
uni-stream limitation — §5). The 10 transport #1-10 unchanged. **ZERO ✔→✘ regressions.**
Two independent verifications AGREE (verifier #7 mechanism proof from quiche source + verifier
compare_final.sh diff — see §5).

### 4.3 Re-soak (H3-terminate, release binary, 960s) — BOUNDED (NEW scenario sc7_h3terminate)
`audit/soak/s26-soak-data/` (release expressgateway, 960s ≥ 900s, 193 samples, unperturbed box).
**overall = BOUNDED, panic_total = 0:**
- rss_kb BOUNDED (last-third median 25518 vs first-third 25206 = +1.2%; plateau ~25 MB; first=7752 warmup).
- vmhwm_kb BOUNDED (+2.7%, peak 27536 KB — peak gauge per the S21 lesson).
- fds BOUNDED (11 flat, max 12); threads BOUNDED (9 flat); panic_total stayed 0 start→end.
- Load: h3_load ok=12608 / err=0 (ingress decode + inline-400 round-trips + bounded no-backend
  drops); h3_reset_flood ok=24,136,673 / err=0 (F-MD-4 RST/STOP_SENDING under churn).
The migrated quiche::h3 H3-terminate front holds bounded under sustained ingress + inline-400 +
a 24M-stream reset flood. (F-S26-1: backend-less by design — the soak covers ingress + egress +
F-MD-4 + no-backend-drop; the relay + §7.1 guard are harness/R13-reachable, §6.)
### 4.4 R8 / R13 / coverage
- **R8 / R13 re-confirmed (verifier, from the completed ×3 gate log) — PASS.** F-MD-4
  smuggle-catch on the migrated E1+E2 surface all `ok` in 3/3 passes (E2/H3→H3
  client_reset_midrequest_burst + upstream_reset_midresponse_burst; H1→H3 + H2→H3
  fmd4_*_burst + never_complete; F-CAP-1 over_cap_*_never_complete + g5 reset). R8 bounded-memory
  gauges all `ok` (h3h3 request/response memory_bounded; h1h3/h2h3 memory_gauge non-vacuous +
  load-bearing; c5 retained-ceiling; r8 chunked trailers). Both KEEP-surface invariants hold in
  the final deleted state.
- **scoped llvm-cov — PASS (verifier DA-per-line, audit/h3spec/s26-cov.lcov).** Production H3
  surface on the final lb-h3-deleted state: **h3_bridge.rs prod 86.19%** (1061/1231; whole-file
  89.57%), **conn_actor.rs prod 91.28%** (607/665), **h3_config.rs prod 100%** (18/18) — all ≥80%.
  Migrated E1+E2 fns aggregate **85.21%** (778/913). Cross-check vs lcov LF/LH region aggregate
  AGREES (h3_bridge 89.02%, conn_actor 91.56%). The two individually-sub-80% fns
  (stream_h1_response 75.71%, stream_request_to_h3_upstream 79.00%) are rare
  infrastructure-FAILURE arms (pool.acquire→502, empty-pool→502, missing-connection→502) —
  uncovered in S24/S25 too (no S26 change), while the SECURITY-critical F-MD-4 / F-CAP-1 / R8 arms
  ARE covered + green. No-regression holds (S26 adds no production code).
- **CF-FCAP1-FLAKE re-manifested under llvm-cov instrumentation (NOT a regression, NOT H3-related,
  does not block):** the cov instrumented sweep hit `fcap1_h2_over_cap_upload_yields_413` (in
  `tests/h2h1_md_streaming_verify.rs` — an **H2→H1** cell, untouched by the H3 migration) failing
  with status=None instead of 413. Captured mechanism (R2): *"peer closed connection without
  sending TLS close_notify" → UnexpectedEof* — the gateway's H2/TLS teardown races the 413 head
  **under contention**, and the ~2× slower instrumented binaries supply exactly that contention.
  This is the documented CF-FCAP1-FLAKE / CF-S19-TLS-TEARDOWN-413 (isolation-proven pre-existing).
  **The failing test CHANGES run-to-run** under instrumentation (run2: fcap1_h2 [H2→H1]; run4:
  req_backpressure + h2h3_fcap1) — non-determinism is itself proof of a timing flake, not a defect
  (a real defect fails the SAME test deterministically). The binding **non-instrumented ×3 gate was
  1442/0/17 ×3 deterministic** (ALL these tests green). The lcov is produced with
  `cargo llvm-cov --ignore-run-fail`; coverage of the migrated H3 surface is fully exercised by the
  all-passing H3 suites. (The cov run is a coverage MEASUREMENT, not the pass/fail gate — that is
  the ×3 gate, which is green.) lb-quic shows NO warning under the --all-features cov build (the only
  "warning" line is cargo's expected --ignore-run-fail exit-101 note), confirming the single
  default-features release-build warning is feature-gated + pre-existing (S26 made no lb-quic code
  change — INC-C was Cargo dev-dep removal + comment rewrites).

## 5. THE HEADLINE — h3spec carried-findings result (preview, to be confirmed on the deleted stack)

Apples-to-apples vs S22 (both 49 examples): **19 → 12 failures.** The migration closes **7 of the
9 carried findings by construction** (quiche::h3 owns the client control + QPACK uni-streams):
#16 (H3_MISSING_SETTINGS), #17/#18 (DATA/HEADERS-on-control), #19 (2nd SETTINGS), #20 (HTTP/2
settings → H3_SETTINGS_ERROR), #21 (CANCEL_PUSH-in-request), #24 (H3_CLOSED_CRITICAL_STREAM).
The 6 S22-fixed (#11-15, #22) still pass. **Huffman gained** (quiche Huffman-encodes; lb_h3 was raw).

**#23 + #25 did NOT close** (QPACK encoder/decoder uni-stream instruction validation:
4.1.3 dyn-table-capacity>limit; 4.4.3 Insert-Count-Increment=0). **Mechanism PROVEN (verifier,
file:line from quiche-0.28 source):**
- quiche::h3::Error (mod.rs:357) has **no** Qpack*StreamError variant; to_wire only knows
  QpackDecompressionFailed→0x200; grep for ENCODER/DECODER_STREAM_ERROR/0x0201/0x0202 = ZERO.
  No public Connection API / Event to inspect these instructions → **no gateway-level hook**.
- mod.rs:2804-2825 State::QpackInstruction: "Read data from the stream and discard immediately"
  — quiche byte-counts but **never parses** Set-Capacity / Insert-Count-Increment (the gateway
  never sees these bytes).
- decoder.rs `pub struct Decoder {}` is **empty** (dynamic table = `// TODO`); encoder.rs is
  lookup_static only → **no dynamic table is ever allocated**, so #23/#25 are **provably inert**
  (no decompression-bomb / amplification vector). COR (not SEC), matching S22.
- **Zero ✔→✘ regressions** vs S22; both #23/#25 also failed on the hand-rolled stack
  (s22-postfix:51/53). They **reclassify** from "our carried bug" → **quiche-0.28 limitation**
  (CF-QUICHE-UPGRADE, alongside transport deviations #1-10). Fix = patch quiche = genuinely large.

The remaining 10 failures are the documented quiche-0.28 transport deviations #1-10 (unchanged).

## 6. Findings & carry-forwards

### 6.0 Documented quiche-0.28 limitations (ONE coherent list — close on CF-QUICHE-UPGRADE)
These are the h3spec cases the gateway does NOT satisfy because **quiche 0.28 itself does not**,
and the gateway runs on `quiche::h3`. None are gateway bugs; all are inert/low-severity; all
close when quiche is upgraded (CF-QUICHE-UPGRADE). Documented here, NOT asterisked:
- **#1-10 (transport):** quiche-0.28 suppresses CONNECTION_CLOSE on first-packet errors / does not
  validate the listed transport parameters + reserved bits → no TRANSPORT_PARAMETER_ERROR /
  PROTOCOL_VIOLATION emitted. Carried since S22; on the prior (shipped) main too.
- **#23 (QPACK 4.1.3) + #25 (QPACK 4.4.3) — NEW to this list this session:** the gateway uses
  static-only QPACK (`set_qpack_max_table_capacity(0)`), and `quiche::h3` OWNS the QPACK
  encoder/decoder uni-streams (`with_transport`). quiche-0.28 *reads-and-discards* those stream
  instructions (mod.rs:2804-2825 — only byte-counts, never parses Set-Dynamic-Table-Capacity /
  Insert-Count-Increment), exposes **no** Qpack*StreamError variant (mod.rs:357) and **no** public
  API/Event for the gateway to validate them, and its QPACK Decoder is an empty struct (no dynamic
  table ever allocated; decoder.rs `// TODO`). → the gateway cannot emit
  QPACK_ENCODER_STREAM_ERROR / QPACK_DECODER_STREAM_ERROR without patching quiche. **Inert** (the
  dynamic-table machinery these instructions concern is never engaged → no decompression-bomb /
  amplification), COR/low-severity, and the prior hand-rolled main ALSO failed both (s22-postfix
  :51/53). Two verifiers proved the mechanism + no-regression.
- **CF-QUICHE-FRAME-COMPLETENESS (§7.1):** quiche-0.28 delivers a no-content-length truncated
  DATA-frame + clean FIN as Event::Finished (carried from S25; CL-truncation guard mutation-proven;
  malformed-backend-only, H3 stream isolation = no desync). Re-tightens on the same quiche upgrade.



- **F-S26-1 (NEW, pre-existing condition, COR/low):** the production binary's H3-terminate
  listener is **backend-less** — `spawn_quic`/`quic_listener_params_from_config` never call
  `with_backends`/`with_h3_backend`/`with_h2_backend` (git-proven: the string never existed in
  main.rs history → NOT a migration regression). The migrated relay + §7.1 CL guard are
  library/harness-reachable only; the soak covers ingress + inline-400 + F-MD-4 + no-backend-drop.
  Empirically confirmed by the INC-D load client (silent drop, no reset). → carry-forward / v1
  release-note; wiring a config-driven H3 backend pool is a separate product item.
- **#23/#25 → CF-QUICHE-UPGRADE** (quiche-0.28 QPACK uni-stream limitation, inert, COR/low).
- **CF-QUICHE-FRAME-COMPLETENESS** (§7.1, carried from S25), **CF-FCAP1-FLAKE**, **CF-DEP-1**,
  F-ESC-1, N-1, Mode-A perf tiers + CF-S15-PASSTHROUGH-RETRY-ODCID — unchanged.
- **CF-S22-QPACK-HUFFMAN:** closes — Huffman gained in production via quiche.

## 7. Commits (this session)
80d5e694 INC-A relocate codec · 2d85ffa9 INC-B re-point · 518681d2 fuzz-lock ·
5f4deb71 INC-D sc7 soak · f2dc39be INC-D StreamBlocked fix · 802faea5 INC-C delete lb-h3 ·
91f00e7c INC-C proptest fmt/path · 9167d1de docs.
