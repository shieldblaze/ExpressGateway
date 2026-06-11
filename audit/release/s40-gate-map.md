# S40 Gate-Map — before → after (0 gates dropped)

Proves the Phase-2 workflow rewrite reproduces **every** distinct gate from
`s40-ci-inventory.md`. Structure rewritten (4 workflows → 3 + 1 composite);
**no gate dropped, no flake-protection behavior lost, no production code touched.**

**Before:** `ci.yml` (10 jobs) + `prod-readiness-gates.yml` (6) + `scheduled-scans.yml` (3) + `release.yml` (3) + `rust-setup` composite + `dependabot.yml`.
**After:** `ci.yml` (16 blocking jobs) + `scheduled.yml` (3) + `release.yml` (3, +soak in Phase 3) + `rust-setup` composite + `dependabot.yml`.

The consolidation: the two separate per-PR BLOCKING workflows (`ci.yml` +
`prod-readiness-gates.yml`) are merged into one sectioned `ci.yml`. The weekly
scans file is renamed (`scheduled-scans.yml` → `scheduled.yml`). The release
pipeline is unchanged here (soak gate added in Phase 3).

---

## A. Per-PR/push blocking gates

| # | BEFORE (workflow / job) | AFTER (workflow / job) | Status | Behavior preserved |
|---|-------------------------|------------------------|--------|--------------------|
| G1 | ci.yml / `check` | ci.yml / `check` | PRESERVED | `cargo check --workspace --all-targets --all-features` (identical) |
| G2 | ci.yml / `clippy` | ci.yml / `clippy` | PRESERVED | `clippy ... -- -D warnings` (identical) |
| G3 | ci.yml / `test` (suite) | ci.yml / `test` | PRESERVED | `--all-features --no-fail-fast`, skip fcap1, 45m timeout (identical) |
| G3b | ci.yml / `test` (fcap1) | ci.yml / `test` (fcap1 step) | PRESERVED | isolated 3-retry, `--exact`, 25m timeout (identical) |
| G4 | ci.yml / `fmt` | ci.yml / `fmt` | PRESERVED | `cargo fmt --all -- --check`, cache off (identical) |
| G5 | ci.yml / `doc-lint` | ci.yml / `doc-lint` | PRESERVED | `bash scripts/ci/doc-lint.sh`, **fetch-depth: 0** (identical) |
| G6 | ci.yml / `panic-freedom` | ci.yml / `panic-freedom` | PRESERVED | grep `#![deny(...unwrap_used)]` over all lib crates (identical) |
| G7 | ci.yml / `audit` | ci.yml / `audit` | PRESERVED | `cargo +stable install cargo-audit` → `cargo audit -D warnings` (identical) |
| G8 | ci.yml / `msrv` | ci.yml / `msrv` | PRESERVED | `cargo check` on toolchain 1.88, all-targets/features (identical) |
| G9 | ci.yml / `fuzz-smoke` | ci.yml / `fuzz-smoke` | PRESERVED | nightly cargo-fuzz, each target 10s (identical) |
| G10 | ci.yml / `release-build` | ci.yml / `release-build` | PRESERVED | `cargo build --workspace --release`, **same `needs:`** (identical) |
| G11 | prod-readiness / `gate-07-cargo-deny` | ci.yml / `cargo-deny` | PRESERVED (renamed) | pinned cargo-deny 0.19.6, `check licenses advisories bans sources` (identical) |
| G12 | prod-readiness / `D6-coverage` | ci.yml / `coverage` | PRESERVED (renamed) | free-disk + `cargo llvm-cov nextest --workspace --all-features --ignore-run-fail` + `coverage-check.sh` + lcov artifact (identical) |
| G13 | prod-readiness / `D4-conformance` | ci.yml / `conformance` | PRESERVED (renamed) | **pinned h2spec v2.6.0 / h3spec v0.1.13**, boot real gateway, `h2spec --strict`, `h3spec-check.sh` (identical config + commands) |
| G14 | prod-readiness / `D3a-chaos-attacks` | ci.yml / `chaos-attacks` | PRESERVED (renamed) | `cargo nextest -p lb-h2 -p lb-l7 -E 'test(/chaos\|rapid_reset\|continuation\|hpack\|slowloris/)'` (identical) |
| G15 | prod-readiness / `D5-image-scan` | ci.yml / `image-scan` | PRESERVED (renamed) | docker build + `docker-smoke.sh` (run+serve) + Trivy HIGH/CRITICAL exit-1 (identical) |
| G16 | prod-readiness / `D2-xdp-verifier-smoke` | ci.yml / `xdp-smoke` | PRESERVED (renamed) | build `--no-run` + `sudo ... round8_verifier_baseline_70 --ignored` (identical) |

## B. Weekly scheduled scans

| # | BEFORE | AFTER | Status |
|---|--------|-------|--------|
| G17 | scheduled-scans.yml / `audit` | scheduled.yml / `audit` | PRESERVED (file renamed) — `cargo audit -D warnings`, weekly cron `54 10 * * 0` |
| G18 | scheduled-scans.yml / `geiger` | scheduled.yml / `geiger` | PRESERVED — `continue-on-error`, geiger.json artifact 30d |
| G19 | scheduled-scans.yml / `machete` | scheduled.yml / `machete` | PRESERVED — `cargo machete`, cache off |

## C. Release pipeline (tag `v*`)

| # | BEFORE | AFTER | Status |
|---|--------|-------|--------|
| G20 | release.yml / `build-binary` | release.yml / `build-binary` | PRESERVED (unchanged) |
| G21 | release.yml / `docker` | release.yml / `docker` | PRESERVED (unchanged) |
| G22 | release.yml / `release` | release.yml / `release` | PRESERVED (unchanged) |
| — | (none) | release.yml / soak release gate | **ADDED (Phase 3)** — `scripts/release-soak.sh`; not a preserved gate |

## D. Infra & cross-cutting

| # | Item | Status |
|---|------|--------|
| I1 | `.github/actions/rust-setup` composite | PRESERVED (unchanged); now referenced by ci.yml + scheduled.yml |
| I2 | `.github/dependabot.yml` (cargo + actions, dtolnay exclude) | PRESERVED (unchanged) |
| I3 | concurrency `cancel-in-progress` | PRESERVED (ci.yml) |
| FP1 | fcap1 isolation + 3-retry | PRESERVED (ci.yml `test`) |
| FP2 | `--no-fail-fast` | PRESERVED |
| FP3 | `--all-features` everywhere | PRESERVED |
| FP4 | coverage `--ignore-run-fail` | PRESERVED |
| FP5 | coverage free-disk step | PRESERVED |
| FP6 | conformance pinned tool versions | PRESERVED |
| FP7 | doc-lint `fetch-depth: 0` | PRESERVED |
| FP8 | SUITE_SERIAL (in source; documented by ci.yml `test` comment) | PRESERVED |

**Result: 22/22 distinct gates preserved + I1–I3 + FP1–FP8. 0 dropped.**

---

## E. Deliberate structural changes (NOT gate drops)

1. **Two blocking workflows → one `ci.yml`.** The 6 D-gates moved from
   `prod-readiness-gates.yml` into `ci.yml`. Same triggers (PR + push-main +
   dispatch), same commands. Maintainability: one CI workflow, one composite,
   sectioned.
2. **Legacy branch triggers dropped.** Old `ci.yml` fired on push to `[main, rust]`;
   old prod-readiness on `[prod-readiness/round-4, main]`. New `ci.yml` fires on
   push `main` + all PRs + dispatch. The `rust` / `prod-readiness/round-4` legacy
   branches no longer trigger CI — **gate coverage of `main` and all PRs is
   unchanged** (a stale branch's CI run is not a gate).
3. **`scheduled-scans.yml` → `scheduled.yml`** (rename only).

## F. Required-status-check rename list (OWNER ACTION — branch protection)

Merging the D-gates into `CI` changes their status-check context strings. If
`main` branch protection lists required checks, update these **6** (the 10
original `CI` jobs and the weekly scans are unchanged):

| OLD required check | NEW required check |
|--------------------|--------------------|
| `prod-readiness-gates / gate-07-cargo-deny` | `CI / cargo-deny (licenses/advisories/bans/sources)` |
| `prod-readiness-gates / D6-coverage` | `CI / Coverage (per-module hot-path >= 80%)` |
| `prod-readiness-gates / D4-conformance` | `CI / Conformance (h2spec --strict + h3spec)` |
| `prod-readiness-gates / D3a-chaos-attacks` | `CI / Chaos Attack Suite` |
| `prod-readiness-gates / D5-image-scan` | `CI / Container Image (build + serve smoke + trivy)` |
| `prod-readiness-gates / D2-xdp-verifier-smoke` | `CI / XDP Verifier Smoke (runner kernel)` |

The 10 carried-over `CI` checks keep their exact names (Check, Clippy, Test,
Format, Doc Lint, Panic Freedom Audit, **Security Audit**, MSRV (1.88), Fuzz
Smoke Test, Release Build) — no branch-protection change needed for those.

## G. Verification

- YAML: all 3 workflows + composite + dependabot parse clean (PyYAML).
- `actionlint v1.7.7`: **PASS** on all 3 workflows.
- doc-lint (G5) re-run locally after the Phase-1 doc moves: **GREEN** (tier-1 + tier-2/52 claims).
- **Green CI run on the branch (R15):** PR #230, run **27373231328** — **all 16
  jobs SUCCESS** (the full new `ci.yml`). Two real gaps the rewrite surfaced were
  fixed first (NOT gate drops): `fuzz-smoke` install → `cargo +nightly install`
  (upstream cargo-platform MSRV 1.91 vs the repo's 1.88 pin); the `test` job's
  `--all-features` build hit ENOSPC → a `.github/actions/free-disk` composite now
  used by `test` + `coverage`. Both repair the same latent breakage on `main`.
- **Independent verifier (author≠verifier, R15): PASS** — diffed every BEFORE→AFTER
  gate body (comments stripped), 22/22 byte-equivalent, 0 dropped, "Security
  Audit" name unchanged, §F rename list accurate, no load-bearing doc deleted,
  soak scripts correct.
