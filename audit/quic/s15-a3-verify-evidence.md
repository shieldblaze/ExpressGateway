# S15 A3 — verify-bar evidence

**Increment:** A3 — threat-model defences + observability.
**Integration tip:** `feature/quic-proxy-s15 @ c88266f3` (builder-1's
A3 impl `b8499ea2` + verifier gates 1-3 + cov, merged by lead).
**Verifier:** `quic-s15-a3-verify` (author≠verifier — builder-1 wrote
the impl; this verification is independent).
**Resume cov baseline:** passthrough.rs 75.91% (A2 resume,
`s15-resume-evidence.md`).

## Verify gates (design §A3)

| Gate | Status | Evidence |
| ---- | ------ | -------- |
| **1 — spoofed-source-IP e2e** | **PASS** | `crates/lb/tests/quic_passthrough_spoofed_source_e2e.rs`. In-process socket-pair fixture: flow installed from peer A (Retry-validated synthetic Initial); spoofed short-header from peer B on the SAME DCID under `strict_source_binding=true` is DROPPED (backend datagram count unchanged) AND emits exactly ONE `audit/source_binding_violation` line (counted via a thread-local `tracing` capture `Layer`). Control: legit short from peer A forwards + emits NO audit line. Negative control `nonstrict_forwards_no_audit` (`strict=false`): peer-B short FORWARDED, zero audit lines — proves the matcher is non-vacuous. 2/2 PASS. |
| **2 — audit-throttle saturation** | **PASS** | `crates/lb/tests/quic_passthrough_audit_throttle_saturation.rs`. `ten_thousand_cap_hits_throttled_to_one_line`: 10_000 distinct-DCID Retry-validated Initials at cap=256 drive thousands of `evict_oldest` cap-hits; `flows_len ≤ 2*cap` throughout (cap path reachable + bounded, no panic), and exactly ONE `audit/quic_passthrough_cap_hit` line is emitted within the single 60s window — NOT 10_000. `short_window_releases_per_window`: a sub-µs window across two flood bursts releases a NEW line in the second window (window-keyed, not a permanent one-shot), still « the ~4000 cap-hits. 2/2 PASS. |
| **3 — metrics** | **PASS** | `crates/lb/tests/quic_passthrough_metrics.rs` — INDEPENDENT integration check driving the REAL `PassthroughListener` with `params.metrics = Some(PassthroughMetrics::register(&registry))`, reading handles back. `flows_gauge_and_retry_counters`: no-token Initial → `retry_minted_total ≥ 1`, `flows == 0`; Retry-validated Initial → `flows == flows_len()` (Q2 `.set(table.len())` semantics, NOT `==1`); bad-token Initial → `retry_rejected_total += 1`, no new flow. `header_parse_errors_counter`: 3 truncated long headers → `header_parse_errors_total == 3` exactly, no flow. `flows_evicted_counter_and_gauge_bounded`: 200 sends at cap=32 → `flows_evicted_total ≥ 1`, `flows == flows_len() ≤ 2*cap`. 3/3 PASS. (`backend_socket_errors_total` is not loopback-triggerable deterministically; covered by builder-1's in-module unit suite.) |
| **4 — llvm-cov ≥80% on passthrough.rs** | **PASS — 91.87%** | `CARGO_INCREMENTAL=0 cargo llvm-cov --workspace --all-features --lcov --output-path /tmp/s15a3-cov.lcov` ([[llvm-cov-workspace-for-depcrate-lines]] — `-p lb-quic`-only undercounts at 64.68% because reverse_pump + spawn paths are integration-test-reachable, not unit-reachable; `--workspace` is the binding measure). DA-per-line over `crates/lb-quic/src/passthrough.rs` ([[llvm-cov-session-scope-method]]): **972/1058 = 91.87%**, independently recomputed by the verifier from the lcov. Lifts the 75.91% resume baseline well over the 80% bar. |
| **5 — ×3 workspace gate + clippy + fmt** | **clippy/fmt PASS; ×3 pending lead** | clippy: `cargo clippy --workspace --all-features --all-targets -- -D warnings` → exit 0 (1m37s, integrated tip c88266f3). fmt: `cargo fmt --all -- --check` → exit 0. ×3 `CARGO_INCREMENTAL=0 cargo test --workspace --all-features` is being run by the lead on the integrated tip (to avoid stacking two workspace builds on the shared target dir); totals to be filled here as `<pass>/<fail>/<ignored>` ×3 when that run lands. NOTE: the full instrumented cov run already executed the entire workspace test suite green under coverage (see /tmp/s15a3-cov.log) — every QUIC passthrough test + the H→H matrix + grpc/h1/h2/h3 suites passed. |

## Coverage detail (gate 4)

* File: `crates/lb-quic/src/passthrough.rs`
* Method: lcov `DA:` per-line, hit = execution-count > 0, over the
  whole file's instrumented lines under `--workspace --all-features`.
* Result: **972 / 1058 = 91.87 %**.
* Driver tests reaching passthrough.rs: the A2 suite
  (`quic_passthrough_e2e`, `_bounded_state`, `_cid_migration`,
  `_strict_source_binding`) + the A3 verify suite (`_spoofed_source_e2e`,
  `_audit_throttle_saturation`, `_metrics`) + builder-1's in-module
  unit suite (retry mint/verify, eviction, `audit_allow`, parse-error,
  backend-socket-error).

## Mechanism notes

* **Audit-line capture (gates 1+2).** A thread-local
  `tracing_subscriber::Registry` + a custom `Layer` counts events whose
  target / name / any field value contains the audit token
  (`source_binding_violation`, `cap_hit`). The landed wiring emits
  `tracing::warn!(event = "audit/…", …)` gated by `audit_allow(&AtomicU64,
  now_ms, window)` (AUDIT_NEVER sentinel → first event always emits,
  then 1/window via CAS). The matcher tokens are substrings of the real
  event strings; the per-event `tracing::trace!("strict_source_binding
  drop")` carries NO token, so it is not double-counted. Non-vacuity is
  proven by the `strict=false` negative control (zero hits).
* **Gauge semantics (gate 3, lead Q2 ruling).** `quic_passthrough_flows`
  is `.set(table.len())` — it tracks DISPATCH-TABLE size, so a migrated
  flow holding two CID keys reads 2. The test asserts `flows ==
  flows_len()`, NOT `== 1`.
* **Layered timing verification** ([[gate-saturation-test-fragility]],
  [[parallel-gate-masks-smuggle]]): gate 2 runs `current_thread`, drives
  10_000 events (saturation), and pairs the long-window one-line
  assertion with a short-window per-window-release assertion as a
  non-vacuous bound against a permanent-one-shot throttle.

## Files added (verifier, all merged into c88266f3)

* `crates/lb/tests/quic_passthrough_spoofed_source_e2e.rs` — gate 1.
* `crates/lb/tests/quic_passthrough_audit_throttle_saturation.rs` — gate 2.
* `crates/lb/tests/quic_passthrough_metrics.rs` — gate 3.

## Verdict

Gates 1-4 PASS (cov 91.87%). Gate 5 clippy + fmt PASS; ×3 totals
pending the lead's integrated-tip run. No defects found in builder-1's
A3 impl — the audit-line + throttle + metrics wiring matches design §A3
and the lead's Q2 gauge ruling exactly.
