# R1 Baseline Result — first pass (pre-fix)

Box: c6a.2xlarge, 8 cores, --test-threads=8. Branch audit/foundation-pass @ fdb1ef9f.

| Gate | Result |
|---|---|
| `cargo fmt --check` | **CLEAN** (FMT_EXIT=0) |
| `cargo clippy --all-targets --all-features -- -D warnings` | **CLEAN** (CLIPPY_EXIT=0) |
| `cargo test --workspace --all-features` run 1 | **FAILED** (exit 101) |
| run 2 | PASS (exit 0) |
| run 3 | PASS (exit 0) |

**NOT baseline-green per R1** (R1 requires all 3 of 3 test runs pass deterministically).

## Baseline failure BL-1 (non-deterministic, 1/3)

Binary: `lb-integration-tests --test balancer_counter_sync`
Test: `test_no_divergence_under_load` (tests/balancer_counter_sync.rs:83)

Verbatim (run 1):
```
thread 'test_no_divergence_under_load' panicked at tests/balancer_counter_sync.rs:83:9:
snapshot diverged from atomic at sample 114: snapshot=7095, bracket=[7096, 7099]
```

R2 classification: WRONG VALUE (snapshot lagged the atomic below the allowed
bracket lower bound). This is NOT a scheduling-starvation timeout — it is an
affirmatively-wrong observed value. Per R2 this is a **REAL DEFECT** candidate
(snapshot/atomic memory-ordering or a non-atomic dual-read in the test model).
Mechanism MUST be proven before classification — assigned to auditor-1.

NOTE: the two carry-in H2 defects (rapid_reset_goaway, h2spec 8.1.2.1#3) did
NOT reproduce in these 3 runs. Per R3/R4 they remain in scope — they are
documented with prior verbatim evidence and are load/timing sensitive;
3 clean-ish runs do NOT clear them. auditor-2 reproduces them under
controlled load deliberately. A 1/3 reproduction rate also means 3 runs is
insufficient confidence for the final R1 gate — Phase 3 re-baseline must be
robust to this.
