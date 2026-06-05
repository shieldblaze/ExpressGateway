# S34 — CI reconciliation report

**Goal:** main's own CI (Build / CI / prod-readiness-gates) was RED on `0d2bd3e9`
despite 18 sessions of green session-gates. Reconcile CI to **honest** green so
the durable signal matches reality — diagnose every red, fix at root, weaken
nothing.

Branch: `feature/ci-reconcile-s34` → PR #225. Base: `main @ 0d2bd3e9`.

**Final branch CI (all green, completed runs):**
- CI: run `27043665467` — success (machete is the only red; `continue-on-error`, non-blocking by design).
- prod-readiness-gates: run `27043665464` — success (D2/D4/D6/D3a/D5/gate-07/build-and-lint).
- Security Audit (CodeQL): run `27043665472` — success.

**Independent honesty audit (author≠verifier):** verdict **"NO GATE WEAKENED —
all green is honest."** Every red→green is a real fix, a CI invocation aligned
to the project's real config, a true de-duplication, an explicitly-named+justified
waiver/carve-out (where a NEW/unlisted failure still turns the gate red), or an
honestly-escalated+documented scope reduction (F-ESC-1).

---

## The reds at 0d2bd3e9, by category

Full per-job diagnosis with log evidence is in `audit/ci/s34-reds.md`. The
toolchain was never at fault — every job already runs 1.88 via the
`rust-toolchain.toml` override.

| Red job | Category | Root cause | Fix |
|---|---|---|---|
| Check / Clippy / Test (×2 workflows) | 1 config-lag | CI omitted `--all-features` → `test-gauges` integration tests didn't compile (E0432) | add `--all-features` |
| MSRV (1.85) | 1 config-lag | stale pin; a no-op (rust-toolchain.toml forced 1.88) | bump → 1.88 |
| Security Audit (cargo-audit) | 1 config-lag | `.cargo/audit.toml` drifted from deny.toml | mirror the 2 already-justified advisories |
| D6-coverage | 1 + 3 | `--package lb-h3` deleted (cat 1); whole-package aggregate never matched the per-module charter, and loader.rs has a structural root-only gap (cat 3) | per-module charter gate + named loader carve-out |
| D3a-chaos | 1 config-lag | `cargo nextest` used but never installed | install nextest |
| D4-conformance | 1/2 | TODO config never matched lb-config schema → gateway never bound | real config + h2spec --strict + h3spec named-waiver gate |
| D2-verifier-matrix | 2 env | `vmtest-action@main` dead + no KVM on hosted runners | runner-kernel real-aya verifier-load; full matrix escalated F-ESC-1 |
| Panic Freedom Audit | **3 real** | lb-soak missing the deny lint | add the lint (code) |
| dependabot.yml | 1 config-lag | actions group swept rust-toolchain (#214) | exclude it |

## Reds that surfaced DURING iteration (masked on red main)

Once the upstream reds cleared, jobs that had been *skipped* on red main ran for
the first time and surfaced latent issues — each fixed honestly:

- **Release Build** (was always skipped; `needs:` had failures): release-only
  dead-code `field 'backend' is never read` in lb-quic `passthrough.rs` under
  `-D warnings`. The CF-S15 no-key-material field-audit was `#[cfg(debug_assertions)]`,
  so in release `backend` was read nowhere. Fix: make the audit unconditional →
  the invariant is now enforced in release too, and every field is read in every
  profile. **Real code fix, not an `#[allow]` on the field.**
- **Test `fcap1`** (CF-FCAP1-FLAKE / CF-SATURATION-1): the over-cap test must
  push 66 MiB past the 64 MiB const cap to assert 413; on the 4-core hosted
  runner, CPU contention with its ~14 sibling tests starved the upload (timed
  out at 44 MiB, or the connection broke at 36 MiB). Fix: extend the
  CF-SATURATION-1 timeout (300s) AND isolate fcap1 to its own serial CI step
  (full CPU, uncontended → reliably reaches the cap). **Same `assert_eq!(413)` —
  serialization, not weakening.**
- **D2** (round 2): the committed XDP ELF uses aya LEGACY map definitions, which
  libbpf v1.0+ tools (bpftool, ip) refuse to load. Fix: run the existing
  `round8_verifier_baseline_70` test under sudo — a genuine `BPF_PROG_LOAD` via
  aya (the gateway's own loader) asserting real kernel verifier facts.

## Honesty notes (no gate weakened)

- **D6** enforces the charter's real metric — **per-module ≥80%** on the named
  hot-path modules (`scripts/ci/coverage-check.sh`), run against the FULL
  workspace suite so integration tests exercise the hot paths (lb-quic::listener
  is 87% with the full suite vs 71% subset). The old whole-package aggregate
  (76.6%) was both the wrong metric and below 80%. Every hot-path module passes
  EXCEPT `lb-l4-xdp/src/loader.rs` (50.7%), carved out **by name** (XDP load
  needs root; smoke-validated by D2; charter defers it). A new hot-path module
  <80% turns D6 red (verified with synthetic LCOV). Charter drift on
  `lb-config::validate` / `lb-observability::metrics` (no longer standalone
  files) is documented in the gate + `audit/round-7/deferred-to-ci.md`.
- **D4 h3spec** waives the 12 documented quiche-0.29 limitations
  (CF-QUICHE-UPGRADE) **individually by name**; a new/un-waived failure (or a
  suite that fails to run, MIN_EXAMPLES guard) turns D4 red (verified with a
  13th-failure stub). h2spec `--strict` = 147/147, 0 fail.
- **D2** is the honest subset hosted CI *can* verify (real aya verifier-load on
  the runner's 6.x kernel); the 5.15/6.1/6.6 matrix is escalated **F-ESC-1**
  (self-hosted; `audit/round-7/deferred-to-ci.md`), named not dropped.
- **Security Audit** advisories were already accepted-with-justification in
  deny.toml; mirroring them into audit.toml is a documented sync. `-D warnings`
  intact.

## CI simplification (safe dedup only — "nothing dropped" map)

| Removed | Where it still runs (same form) |
|---|---|
| `rust.yml` Check/Clippy/Test/Format (4 jobs) | ci.yml Check/Clippy/Test/Format (identical, + `--all-features`; PR + main triggers) |
| ci.yml `deny` (cargo-deny) | prod-readiness `gate-07-cargo-deny` (now explicit `check licenses advisories bans sources`) |
| ci.yml `docker` (image build) | prod-readiness `D5-image-scan` (builds the same Dockerfile AND Trivy-scans it) |

Net ~ -6 PR jobs, **zero distinct check lost** (verifier-confirmed). Deeper
restructuring (fold build-and-lint; move geiger/machete to scheduled) deferred
to a follow-up PR to keep this reconciliation diff verifiable.

## Unused Deps (cargo-machete) — honest disposition

`machete` is `continue-on-error: true` **by the project's own design** (the
ci.yml comment: informational, known to false-positive on feature-gated
re-exports). It flags 18 deps across 12 crates — the classic macro-/derive-/
feature-gate false-positive profile (e.g. `tracing`, `serde`, `bytes`, `http`).
It does NOT block the CI workflow (run conclusion = success). Determining
real-vs-false-positive per dep and adding `[package.metadata.cargo-machete]`
ignores (or removing genuinely-unused deps) is a separate hygiene task that
overlaps the carried PR #224 dep work; doing it inside this trust-rebuilding PR
would muddy the "nothing dropped" diff. **Escalated as carry-forward**, not
weakened (it was already non-blocking before this session).

## S32 evidence landed

`audit/soak/s32-report.md` + `s32-quiche-collected-fix.diff` + the 43-file soak
dataset are now on this branch (audit-only, no production source) so main is the
durable home of the CF-GRPC-H3-CHURN-RSS evidence.

## Verification

- Session-local R1 on 1.88: `cargo test --workspace --all-features --no-fail-fast`
  ×3 all-pass (0 failed, 247 ok-result lines/run); `clippy --all-targets
  --all-features -D warnings` clean; `fmt` clean; `cargo build --workspace
  --release -D warnings` clean.
- Branch CI green: CI `27043665467`, prod-readiness `27043665464`, CodeQL
  `27043665472` — all completed-success.
- Independent honesty audit: NO GATE WEAKENED.

## Verdict

**SESSION 34 COMPLETE — main CI reconciled to honest green.**
Config-lag fixes: Check/Clippy/Test/MSRV `--all-features`+1.88, audit.toml sync,
D6 lb-h3 ref, D3a nextest install, dependabot grouping. Env-artifact fixes: D4
conformance harness, D2 runner-kernel verifier-load (+F-ESC-1 escalation), fcap1
isolation. Real-code fixes: lb-soak panic-freedom lint, lb-quic release dead-code.
CI simplified (−6 duplicate jobs). S32 evidence on main. No gate weakened
(verifier-confirmed). machete carried (non-blocking; PR #224 dep work).
Promoting to main per R11; the post-merge main CI run is the final confirmation.
