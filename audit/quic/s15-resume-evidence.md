# S15 resume — post-disk-loss recovery + disk-blocked gates closed

Context: a prior S15 session lost in-flight work to ENOSPC. This resume
protected the work, stood up a build-safe disk-guard, and ran the gates
disk pressure had previously blocked.

## Recovery

- In-flight work (the `mint_retry` config knob — trusted-network escape
  for CF-S15-PASSTHROUGH-RETRY-ODCID) WIP-committed `a8668db6`, pushed to
  `origin/feature/quic-proxy-s15`. Compiles clean (`cargo check
  --workspace --all-features` exit 0).
- S14 (CFBW) confirmed intact on main `36ee1227` (local + remote).

## `mint_retry` knob (recovered work)

- `lb-config::PassthroughConfig::mint_retry: bool` (default `true`).
- `lb-quic::PassthroughParams::mint_retry`, threaded via
  `lb/main.rs::spawn_passthrough`.
- `handle_initial`: when `mint_retry=false` AND no token → forward the
  Initial verbatim (backend owns §6.5 Initial-flood defence); when a
  token is present → verify as before. Default `true` preserves the
  LB-mints-Retry production behaviour.

## Disk-blocked gates — now GREEN

- **Gate (v) R3 no-regression ×3** (`CARGO_INCREMENTAL=0`):
  - RUN1 1330/0/18 exit 0; RUN2 1330/0/18 exit 0; RUN3 1330/0/18 exit 0.
  - Deterministic, identical across runs. The previously-flaky
    `h2h3_fcap1_over_cap_upload` (S13 under-saturation test) passed all 3.
  - Logs: `audit/quic/s15-resume-gate-run{1,2,3}.log`.
- **CF-S15-A2-LLVMCOV — MEASURED** (was DEFERRED):
  - `cargo llvm-cov --workspace --all-features --lcov`
    ([[llvm-cov-workspace-for-depcrate-lines]]).
  - `crates/lb-quic/src/passthrough.rs`: **498/656 lines = 75.91%**.
  - **UNDER the 80% session bar.** Whole-file metric, but passthrough.rs
    is overwhelmingly session-authored (A2 deliverable +497/-55), so this
    is a genuine gap, not [[llvm-cov-session-scope-method]] dilution.
  - Uncovered clusters (lcov DA==0): **599-674** (the `handle_initial`
    Retry-mint + verify region — the new `mint_retry` branch is the
    largest single gap: the e2e drives one path; bounded-state/migration
    tests never mint Retry), plus 787-848 and 994-1037 (error/eviction
    edges).
  - lcov: `audit/quic/s15-resume-cov.lcov`.

## Disk-guard (build-safe, llvm-cov-aware)

`/home/ubuntu/Code/eg-disk-cleanup.sh` — 5-min detached loop. NEVER
prunes while `rustc`/`cargo` is active (v1 raced a live compile and
corrupted dep-graph.bin → false RUN1 failure). Below the 25G floor it
escalates to prune `llvm-cov-target` (17G regenerable, observed) +
profraw + doc + stale `.d`. Never touches source/git/registry/lcov
reports. See [[disk-cleanup-loop-must-not-race-builds]].

## A2 verify-bar status after resume

| Gate | Status |
|---|---|
| (i) real-QUIC wire e2e | PASS (un-ignored @ 5fc39592, in the ×3 count) |
| (ii) NEVER-DECRYPTED LINKAGE | PARTIAL → CF-S15-PASSTHROUGH-FEATURE-GATING-DEEP (dedicated multi-crate refactor) |
| (iii) CID-migration / NAT-rebind | PASS |
| (iv) bounded-state R13 a/b/c | PASS |
| (v) R3 no-regression | **PASS ×3** (1330/0/18, recovered this resume) |
| (vi) strict_source_binding BOTH | PASS |
| llvm-cov ≥80% | **MEASURED 75.91% — UNDER bar; gap = mint_retry branch (A3 test work)** |

## Remaining (carry-forward / A3-A4)

- **CF-S15-A2-LLVMCOV → now a coverage GAP, not deferred:** add table-
  driven unit tests for the `mint_retry` branch (both `true` Retry-mint
  and `false` forward-verbatim no-token paths) + the eviction/error
  edges (787-848, 994-1037) to clear 80%. This is squarely A3's
  "unit tests on each defence" scope.
- **CF-S15-PASSTHROUGH-FEATURE-GATING-DEEP** — gate (ii); dedicated session.
- A3 remainder: Prometheus `quic_passthrough_*` gauges/counters,
  audit-throttle saturation test, spoofed-source e2e.
- A4: promote + s15-final-report + s16-handoff (NOT done — S15 not at 100%).
