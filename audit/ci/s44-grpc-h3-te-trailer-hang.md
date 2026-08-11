# CF-S44-GRPC-H3-TE-HANG — `grpc_h3_without_te_header_still_delivers_trailer` hangs indefinitely

**Date:** 2026-08-02 · **Status:** OPEN — found, attributed, **not fixed**
**Severity:** CI-infrastructure. Burns a full 6-hour runner job; does **not** fail loudly.
**First observed:** run `30755744744`, job `91517507600` (Coverage), branch `s44-coverage-metric`.

---

## 1. What happened

The Coverage job ran for **5+ hours** with no progress and had to be cancelled manually. It
would otherwise have sat until GitHub's 6-hour job timeout.

nextest's own escalating SLOW markers name the test:

```
SLOW [>17460.000s] (─────────) lb-quic::grpc_h3_e2e grpc_h3_without_te_header_still_delivers_trailer
SLOW [>17520.000s] ...
SLOW [>17940.000s] (─────────) lb-quic::grpc_h3_e2e grpc_h3_without_te_header_still_delivers_trailer
```

and the orphan-process sweep on cancellation confirms what was still alive:

```
Terminate orphan process: pid (3056)  (cargo-llvm-cov)
Terminate orphan process: pid (8445)  (cargo-nextest)
Terminate orphan process: pid (12606) (grpc_h3_e2e-ee1b7dc6f6c6ee7e)
```

**1564 of 1565 tests completed.** This one test never returned — ~17,940 s and counting when
killed. `Starting 1565 tests across 234 binaries` at 16:09:19Z; cancelled 21:11:34Z.

## 2. This is a HANG, not slowness

`crates/lb-quic/tests/grpc_h3_e2e.rs:960` sets the test's own budget:

```rust
const OVERALL: Duration = Duration::from_secs(20);
```

The test ran **~900× that budget**. Whatever the mechanism, the 20 s deadline did not bound it.

## 3. Not caused by the S44 coverage change

- The coverage-metric change is in `scripts/ci/coverage-check.sh`, which runs in the **next**
  workflow step (`Enforce per-module hot-path threshold`) — still `pending` when the job was
  cancelled. It never executed.
- The change is a pure LCOV text parser. It cannot affect a QUIC test binary.
- The same test **passed** on runs `30749813681` and `30751410142`, which completed all 1565
  tests in ~343 s and ~372 s respectively.

Therefore: **pre-existing and intermittent**, surfaced here for the first time.

## 4. Two candidate mechanisms — not yet distinguished

Both are consistent with the evidence; the log alone cannot separate them, and **neither has
been proven**. Do not treat either as the cause without measurement.

**(a) Blocked on the suite serial guard.** Every test in this binary opens with

```rust
macro_rules! serial_guard {
    () => { let _suite_serial = SUITE_SERIAL.lock().await; };   // line 75-79
}
```

An `await` on an async mutex with **no timeout**. If an earlier test in the binary leaked a
task or aborted while holding the guard, every later test blocks here forever — and nextest
would still show the test as "started", exactly as observed.

**(b) The deadline is not enforced on every await path.** `drive_grpc_h3_core` computes

```rust
let deadline = tokio::time::Instant::now() + overall;
```

A deadline checked at loop boundaries does not bound an inner await that never resolves
(QUIC handshake, backend accept, response read). Setup that runs *before* the driver —
`spawn_h2_grpc_backend`, `start_h3_listener_h2` — is outside the budget entirely.

Next step to distinguish: reproduce under `--test-threads=1` with a `RUST_LOG` trace, or attach
to the wedged binary and dump task backtraces. Whichever it is, **the structural problem stands
on its own**: an untimed `lock().await` in a shared test harness means one stuck test wedges
the entire binary for the full 6-hour job limit.

## 4b. Second observation — the hang is intermittent, and the suite is broadly fragile

The job was re-run on the identical SHA (`6c81222f`). Result: **16/16 green in 986 s**, and the
hanging test passed in **0.100 s**:

```
PASS [   0.100s] (1259/1565) lb-quic::grpc_h3_e2e grpc_h3_without_te_header_still_delivers_trailer
```

So the hang is **intermittent**, not deterministic — one occurrence in three observed runs.

But the same re-run failed a *different* test in the *same* binary, also about trailers:

```
FAIL [   0.232s] (1256/1565) lb-quic::grpc_h3_e2e grpc_h3_trailer_survives_all_response_sizes
thread '...' panicked at crates/lb-quic/tests/grpc_h3_e2e.rs:1242:9:
assertion `left == right` failed: sz=262144
  left: Some(502)
 right: Some(200)
```

A **502 instead of 200 at a 256 KiB response**. This is **not** on the known-flake list
(CF-FCAP1-FLAKE, CF-SATURATION-1, CF-S35-T5-FLAKE, CF-S37-D6-H2PROXY-FLAKY,
CF-S38-RELOAD-BOOT-FLAKE) — it is a new observation.

Two different trailer tests in `grpc_h3_e2e` misbehaved in two consecutive instrumented runs
(one hang, one 502). The uninstrumented `Test` job passed both times. The common factor is the
llvm-cov instrumented build, which is substantially slower — consistent with
[[gate-saturation-test-fragility]] ("gate saturation exposes tight test-backend timeouts as
flakes").

**That mechanism is a hypothesis, not a finding.** Per R2 a flake is classified by *proven*
mechanism from captured output, and no one has proven this one. What is established: the
symptom, the exact assertion, the response size, and that both occurrences are
instrumentation-only. Whether the 502 is a tight test-backend timeout or a genuine
gateway-side failure on large H3 responses is **open** — and the second possibility is a
product bug, not a test bug, so it should not be assumed away.

## 5. Why this is worse than a failing test

- **`--ignore-run-fail`.** The Coverage job runs
  `cargo llvm-cov nextest --workspace --all-features --ignore-run-fail` precisely so a flake
  does not lose the coverage measurement. A *hang* is not a failure — the flag cannot help,
  and the job neither fails nor completes. It just burns runner time silently.
- **Instrumented builds are slower**, which is exactly the condition that exposes
  timing-dependent deadlocks. The uninstrumented `Test` job **passed** in the same run — so
  the Test lane is not a reliable detector for this.
- **No job-level timeout.** No `timeout-minutes:` is set on the Coverage job, so the ceiling is
  GitHub's 6-hour default.

## 6. Recommended actions (not taken here)

1. **Cheap and immediate — add `timeout-minutes:` to the CI jobs.** The Coverage job's test
   step has historically finished in ~6 minutes; a 45–60 minute cap turns a silent 6-hour burn
   into a fast, loud red. This is a containment measure, not a fix, and it should not be
   confused with one.
2. **Bound the serial guard.** Wrap `SUITE_SERIAL.lock()` in a `tokio::time::timeout` and panic
   with a clear message on expiry, so the *next* occurrence names the culprit instead of
   hanging.
3. **Bound the test itself** — wrap the whole body in `tokio::time::timeout(OVERALL * 2, …)` so
   `OVERALL` actually means something end-to-end, including setup.
4. **Then** find the real mechanism. Items 1–3 stop the bleeding; they do not explain it.

Do **not** close this by deleting or `#[ignore]`-ing the test — it covers a real protocol
behaviour (trailer delivery without `te: trailers`), and an intermittent hang in a QUIC/H3
path may well indicate a genuine product-side deadlock rather than a test artifact. That
question is open.

## 7. Reproduce / re-observe

```sh
gh api repos/:owner/:repo/actions/jobs/91517507600/logs \
  | sed 's/\x1b\[[0-9;]*m//g' | grep -E "SLOW|Terminate orphan"
cargo nextest run -p lb-quic --test grpc_h3_e2e \
  grpc_h3_without_te_header_still_delivers_trailer --test-threads=1
```
