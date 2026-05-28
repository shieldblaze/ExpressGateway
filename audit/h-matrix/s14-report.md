# SESSION 14 — CF-BODY-WALLCLOCK (idle-deadline conversion)

> Base tip: `01432589` (S13 promoted, H-matrix 9/9). Branch: `feature/cfbw-s14`.
> Owner-resolved upfront: **R-CFBW-1** = idle/no-progress deadline (single-sourced
> across all 4 cells); **R-CFBW-2** = SEPARATE configurable `head_timeout`, default
> 60 s. Plan reference: [`s14-cf-body-wallclock-plan.md`](s14-cf-body-wallclock-plan.md).

---

## Phase 0 — baseline + hygiene  ✅ COMPLETE

| Probe | Result |
|---|---|
| Base tip | `01432589` confirmed (`Promote S13: H2→H3 BUILT — H-to-H matrix COMPLETE 9/9 (R11)`) |
| Stray processes (`ps aux`) | none |
| Free disk (`df -h /home/ubuntu`) | 22 G after pruning 2.4 G `target/debug/incremental` (CF-DISK-1: under the 25 G R9 floor; shared `CARGO_TARGET_DIR` means parallel worktrees don't duplicate, so 19-22 G is workable — proven by Phase 0 gate × 3 succeeding without ENOSPC) |
| `cargo fmt --check` | clean (exit 0) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean (exit 0) |
| **R1 gate run #1** (`cargo test --workspace --all-features`) | **PASS** (exit 0; log: `s14-phase0-gate-run1.log` — tail-only due to initial pipe, exit code confirms determinism) |
| **R1 gate run #2** | **PASS** — 1272 passed / 0 failed / 16 ignored (log: `s14-phase0-gate-run2.log`) |
| **R1 gate run #3** | **PASS** — 1272 passed / 0 failed / 16 ignored (log: `s14-phase0-gate-run3.log`) |

R1 ×3 deterministic ✅. Counts match S13 promoted baseline (`1272/0/16`).

### Team + worktrees
- Team: `h-matrix-s14` (Opus, stable-names).
- Worktrees (all on `01432589`, shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`):
  - lead → `/home/ubuntu/Code/ExpressGateway` (`feature/cfbw-s14`)
  - pool-eng → `.claude/worktrees/pool-eng` (`feature/cfbw-s14-pool`)
  - builder-1 → `.claude/worktrees/builder-1` (`feature/cfbw-s14-builder`)
  - verifier → `.claude/worktrees/verifier` (`feature/cfbw-s14-verify`)

---

## Phase 1 — lb-io idle-deadline mechanism (R-CFBW-1)  ✅ COMPLETE

Pool-eng deliverables (branch `feature/cfbw-s14-pool` → integrated at `e33343cd` on `feature/cfbw-s14`):
- `crates/lb-io/src/idle_send.rs` (new): `idle_bounded_send<F,T>` two-phase helper + `IdleSendError {IdleTimeout, HeadTimeout}`. Phase A idle watchdog (re-armed on `last_progress` bumps), Phase B `head_timeout` cap once `upload_complete` flips.
- `crates/lb-io/src/http2_pool.rs`: new `Http2Pool::send_request_idle` method. Existing `send_request` byte-identical → zero regression on `h3_bridge.rs:2573` + existing pool tests.
- Race-handling: `biased;` select + post-fire `last_progress` re-check absorbs the bump-at-tick race; arm (iii) is the load-bearing two-phase proof (upload-complete then slow head → `HeadTimeout` at ~5.5 s, NOT `IdleTimeout` at ~0.6 s).
- Plan-approval round-trip: `s14-pool-eng-design.md` reviewed + flagged the §3 race; arm (iii)'s expected timing windowed to 5-6 s confirms the race-handling is exercised in practice.

Gates (pool-eng's report + lead's independent re-run):
- `cargo test -p lb-io --all-features`: 55/55 PASS (verified locally, exit 0).
- llvm-cov scoped: `idle_send.rs` 100.00% (293/293); `Http2Pool::send_request_idle` 85.71% (30/35; 5 uncovered = handshake-fail + Send-class error → require fault-injection infra, Phase-3-shaped).
- `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean, `cargo fmt --check` clean, `cargo check --workspace --all-features` clean.

Memory updated: [[s14-pool-eng-phase1-built]].

---

## Phase 2 — wire 4 cells + head_timeout (R-CFBW-2)  ✅ COMPLETE (lead-integrated, pre-verifier)

Builder-1 deliverables (`feature/cfbw-s14-builder` → integrated at `e83cf47e` on `feature/cfbw-s14`):
- `8159e161` — `HttpTimeouts::head` (Default 60 s) + `HttpTimeoutsConfig::head_timeout_ms` (default 60_000, validated `>0`) + wired through `build_h1_proxy` + `build_h2_proxy` + all `HttpTimeouts { … }` literal sites in tests.
- `97c4e4a7` — H2→H1 Branch A: rename `body → head` timeout (R-CFBW-3, consistency only).
- `13e05447` — **H1→H1**: pump bumps `last_progress` on every `tx.send(Ok)`-success; sets `upload_complete = true` at verdict-Ok terminal arms (clean-EOF + trailer-Ok); replaces `:1508` wall-clock with `idle_bounded_send`. F-CAP-1 inner arm + `:1523` backstop unchanged.
- `8cb3d3c0` — **H2→H1 Branch B**: same shape at `:1866`; `:1881` backstop unchanged.
- `20ef4fac` — **Class B (H1→H2 + H2→H2)**: `drive_h2_upstream_send` signature widens to thread `last_progress`/`upload_complete`/`epoch`/`idle`/`head_timeout` (plus `#[allow(clippy::too_many_arguments)]`); inner detached `send_request` → `send_request_idle`; `body_timeout` retained for the `:3003` F-CAP-1 verdict-rx backstop.

Sanity gates (lead, post-merge):
- `cargo check --workspace --all-features` clean (3.13 s, exit 0).
- Builder-1's pre-push: `cargo test -p lb-l7 --all-features` 200/200 across 17 binaries; `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean; `cargo fmt --check` clean.

R12 equivalence (audit-ready, will be re-verified in Phase 3):
- Class A (H1→H1 + H2→H1 Branch B) → `lb_io::idle_send::idle_bounded_send(send_fut, Arc::clone(&last_progress), Arc::clone(&upload_complete), epoch, self.timeouts.body, self.timeouts.head)`.
- Class B (H1→H2 + H2→H2) → `Http2Pool::send_request_idle(addr, req, last_progress, upload_complete, epoch, idle, head_timeout)` via shared `drive_h2_upstream_send`. The pool method internally calls `idle_bounded_send` (Phase 1). One watchdog body, four cells.
- Branch-A buffered sites (`h1_proxy.rs:1710`, `h2_proxy.rs:2101`) kept on `send_request` — within-window body has no streaming pump (structural divergence, not semantic; documented in builder-1 design).

Owner-decided open questions: all resolved per builder-1 implementation (Branch-A kept on send_request, no defensive flag, backstops keep their existing duration).

---

## Phase 3 — verifier (R13 per cell + R12 equivalence + final gate + promote)  ⏳ IN PROGRESS

Pre-Phase-3 lead measurements (taken over from verifier after their bg llvm-cov died silently):
- **R3 regression gate** (integrated `e83cf47e`): **1283 passed / 0 failed / 16 ignored** (+11 tests vs S13 baseline 1272 — pure additions from `HttpTimeouts::head` test fixture + cell-wiring smoke arms; **no regression on any existing test**). Log: `s14-phase2-gate-run1.log`.
- Phase 1 audit sections 1 + 2 APPROVED by verifier (code-correctness + helper determinism ×3). Sections 3 + 4 (cov + R3 regression on Phase 1 alone) absorbed into Phase 3 since verifier's bg jobs kept dying — Phase 3's ×3 gate + scoped cov measurement supersedes them.

Verifier now dispatched for Phase 3 deliverables (R13 per cell + R12 equivalence + R1 ×3 deterministic gate + scoped cov ≥80% + S15 handoff + promote per R11).

---

## Phase 2 — wire 4 cells + head_timeout (R-CFBW-2)  ⏳ pending

---

## Phase 3 — gates + verification + promote  ⏳ pending
