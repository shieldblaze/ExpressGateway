# Plan for CODE-2-04 — Atomic ordering audit (workspace-wide)
Finding-ref:     CODE-2-04 (high, Open) — input list from SEC-2-16
Files touched:
  - `crates/lb-core/src/backend.rs`                 (G sites)
  - `crates/lb-io/src/pool.rs`                      (G sites incl. `:372` capacity gate)
  - `crates/lb-io/src/quic_pool.rs`                 (R sites pending sec input)
  - `crates/lb-h2/src/security.rs`                  (R sites pending sec input)
  - `crates/lb-security/src/zero_rtt.rs`            (R sites pending sec input)
  - `docs/decisions/atomics.md`                     (NEW — policy doc)
  - `.github/workflows/ci.yml`                      (grep guard step)
  - `scripts/ci/atomic-lint.sh`                     (NEW — Relaxed-in-gate scan)

Approach:
This is one workspace-wide plan with an appendix mapping each of the
~50 sites to a category. Sec hands the per-detector list (SEC-2-16);
the round-2-review §CODE-2-04 Appendix A is the source classification.

Step 1 — Policy doc. `docs/decisions/atomics.md` defines three
categories and the rule:
- **S (stats-only)**: metric gauges. `Relaxed` permitted.
- **G (enforcement gate)**: counter read drives an admit/reject
  decision. Load = `Acquire`, matching mutation = `Release`, RMW that
  participates in both = `AcqRel`. CAS success = `Release`, failure =
  `Relaxed`.
- **L (lifecycle flag)**: shutdown/ready flags. Store = `SeqCst`,
  Load = `Acquire`. (Today empty — CODE-2-03 uses `CancellationToken`.)

Step 2 — Site-by-site edits per appendix table. For each G-classified
load, replace `Relaxed` with `Acquire`; for each matching `fetch_add`
/ `fetch_sub`, replace with `Release` or `AcqRel`. For CAS loops,
`compare_exchange_weak(_, _, Release, Relaxed)`.

Step 3 — Lint. `scripts/ci/atomic-lint.sh`:
```bash
#!/usr/bin/env bash
# Fail CI if a G-marked file uses Ordering::Relaxed on a load whose
# return value flows into a comparison.
set -euo pipefail
FILES=(
  crates/lb-io/src/pool.rs
  crates/lb-io/src/quic_pool.rs
  crates/lb-core/src/backend.rs
  crates/lb-h2/src/security.rs
  crates/lb-security/src/zero_rtt.rs
)
PATTERN='\.load\(Ordering::Relaxed\)\s*(>=|>|<=|<|==|!=)'
if rg -n -P "$PATTERN" "${FILES[@]}"; then
  echo "ERROR: gate load uses Relaxed; see docs/decisions/atomics.md"
  exit 1
fi
```
Hooked into CI as a new step after fmt+clippy. Documented as a
"clippy-lite" check.

Step 4 — Loom proofs (paired with CODE-2-11). Three loom tests cover
the highest-risk gates:
- `lb-core/tests/loom_backend.rs::saturating_decrement_no_underflow`
- `lb-io/tests/loom_pool.rs::capacity_gate_no_overshoot`
- `lb-h2/tests/loom_rapid_reset.rs::counter_vs_threshold_observable`

Each test models 2 threads, runs under `loom::model(|| { ... })`.

Sec input absorption (SEC-2-16, when delivered):
- Per-detector classifications drop into Appendix B of
  `docs/decisions/atomics.md`. Sites in `quic_pool.rs:184,217,230,236,
  271,283,403,405,499,516,518,566,587` currently R-classified will be
  finalised by sec; default position: capacity-gate sites become G,
  evict-tracking sites become G (Release on `fetch_sub` paired with
  queue removal).

Appendix A — per-site classifications (inherited from round-2-review §CODE-2-04):

| File:line | Op | Current | Target | Reasoning |
|---|---|---|---|---|
| `lb-core/backend.rs:87` `active_connections fetch_add` | Relaxed | **Release** | scheduler reads it |
| `lb-core/backend.rs:92,98,104,105` CAS loop | (Relaxed,Relaxed) | **(Release,Relaxed)** | saturating-CAS publication |
| `lb-core/backend.rs:116,121,126,132,133` `active_requests` | Relaxed | **Release / Acquire** | same |
| `lb-core/backend.rs:144,149,162,163,164` EWMA | Relaxed | Relaxed (S) | metrics-only |
| `lb/main.rs:1127, 1212` `active_connections` | Relaxed | Relaxed (S) | gauge — verify CODE-2-05 semaphore not repointing |
| `lb-io/pool.rs:113,141,185` `idle_count` | Relaxed | Relaxed (S) | gauge |
| `lb-io/pool.rs:372` `total.load` | Relaxed | **Acquire** | capacity gate |
| `lb-io/pool.rs:389` `total.fetch_add` | Relaxed | **Release** | gate publication |
| `lb-io/pool.rs:395` per-peer fetch_add | Relaxed | **Release** | scoped gate |
| `lb-io/pool.rs:455` `total.fetch_sub` on evict | Relaxed | **Release** | pairs with queue remove |
| `lb-io/quic_pool.rs:184,217,230,236,271,283` capacity-gate sites | Relaxed | **Acquire/Release** (pending sec) | TOCTOU equivalent |
| `lb-io/quic_pool.rs:403,405,499,516,518,566,587` evict-replace | Relaxed | **AcqRel** (pending sec) | RMW participates both sides |
| `lb-io/dns.rs:396,416,434,438,451,467,475,489,508,530,532` | Relaxed | Relaxed (S) | metrics-only cache counters |
| `lb-h2::security::RapidResetDetector` counter | Relaxed | **Release / Acquire** (sec to confirm) | gate |
| `lb-h2::security::ContinuationFloodDetector` | Relaxed | **Release / Acquire** | gate |
| `lb-h2::security::HpackBombDetector` byte-accumulator | Relaxed | **Release / Acquire** | gate |
| `lb-security::ZeroRttReplayGuard` write-idx | Relaxed | **SeqCst on CAS** | replay correctness |
| Per-IP rate bucket (added per SEC-2-04 / CODE-2-01) | n/a | **Acquire on load** | new code |

Proof:
- `bash scripts/ci/atomic-lint.sh` exits 0 on a clean tree, exits 1 on
  a planted regression (test: `tests/ci_atomic_lint.rs` plants
  `Relaxed` then asserts script fails).
- Three loom tests above run under `RUSTFLAGS="--cfg loom" cargo test
  --release` — bounded to ≤2 threads, ≤6 ops per thread to fit in CI.
- Each gate site has a comment `// G: load=Acquire (atomics.md)` for
  reviewability.

Risk / blast radius:
- Memory-ordering relaxation is *more* permissive; tightening to
  Acquire/Release/AcqRel/SeqCst is monotonically safer. No correctness
  regression possible.
- Performance: per-op cost rises by ~1–3 ns on x86 (no real impact
  outside hot loops; the affected loops are connection-pool gates and
  per-request detectors, not per-byte).
- aarch64 / ARMv8 builds may *gain* observable correctness fixes that
  x86 hid (per Round 2 review §CODE-2-04 reproduction note).

Cross-ref:    SEC-2-16 (input list owner), CODE-2-11 (loom tests),
              CODE-2-05 (semaphore on `active_connections`)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
