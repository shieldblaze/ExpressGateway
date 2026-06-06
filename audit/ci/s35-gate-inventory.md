# S35 CI Gate Inventory — the "BEFORE" set

Snapshot of **every distinct CI job/gate** across all workflow files at the S35
baseline (`main` @ `93d011c7`, the S34 honest-green tip). This is the authoritative
"before" half of the gate-map (`s35-gate-map.md`). **R3: nothing in this list may
vanish after consolidation without being named-as-redundant in the gate-map.**

Workflow files present (5): `ci.yml`, `prod-readiness-gates.yml`, `codeql.yml`,
`dependabot.yml`, `release.yml`. Plus `.github/dependabot.yml` (Dependabot *config*,
not a workflow — keep untouched).

> Note: the mission brief's "10+ workflow files" describes the pre-S34 state. S34
> already removed `rust.yml` (dup of ci.yml) and 2 dup jobs (ci `deny`→gate-07,
> ci `docker`→D5). The current baseline is 5 files / 24 jobs.

---

## ci.yml — `name: CI`
Triggers: `push: [main, rust]`, `pull_request`, `workflow_dispatch`. `env.RUSTFLAGS=-D warnings`.
Concurrency: cancel-in-progress per ref.

| # | Job (name) | What it runs | Toolchain | Blocking on PR? |
|---|---|---|---|---|
| G1 | `check` (Check) | `cargo check --workspace --all-targets --all-features` | stable | yes |
| G2 | `clippy` (Clippy) | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | stable | yes |
| G3a | `test` (Test) step a | `cargo test --workspace --all-features --no-fail-fast -- --skip fcap1_h2_over_cap_upload_yields_413` (45 min) | stable | yes |
| G3b | `test` (Test) step b | fcap1 over-cap isolated, **3× retry** (`-p lb-integration-tests --test h2h1_md_streaming_verify -- --exact fcap1_h2_over_cap_upload_yields_413`) (25 min) — CF-FCAP1-FLAKE / CF-SATURATION-1 | stable | yes |
| G4 | `fmt` (Format) | `cargo fmt --all -- --check` | stable | yes |
| G5 | `doc-lint` (Doc Lint) | `bash scripts/ci/doc-lint.sh` (fetch-depth 0; tier-1 stale-pattern + tier-2 audit-of-audit `Verified-Fixed(<sha>)` resolution) | n/a | yes |
| G6 | `panic-freedom` (Panic Freedom Audit) | grep every `crates/*/src/lib.rs` for `#![deny(...clippy::unwrap_used...)]` | n/a | yes |
| G7 | `audit` (Security Audit) | `cargo audit -D warnings` (fail on ANY advisory; ignores live in `.cargo/audit.toml`) | stable | yes |
| G8 | `geiger` (Unsafe Inventory cargo-geiger) | `cargo geiger --all-features --output-format Json` + upload artifact | stable | **no** (`continue-on-error: true`, informational) |
| G9 | `machete` (Unused Deps cargo-machete) | `cargo machete` | stable | **no** (`continue-on-error: true`, informational) — **currently surfaces 17 unused deps across 12 crates → the RED to fix** |
| G10 | `msrv` (MSRV 1.88) | `cargo check --workspace --all-targets --all-features` | **1.88** | yes |
| G11 | `fuzz-smoke` (Fuzz Smoke Test) | `cargo +nightly fuzz run <each target> -- -max_total_time=10` | nightly | yes |
| G12 | `release-build` (Release Build) | `cargo build --workspace --release`; `needs: [check, clippy, test, fmt, panic-freedom, doc-lint]` | stable | yes |

## prod-readiness-gates.yml — `name: prod-readiness-gates`
Triggers: `push: [prod-readiness/round-4, main]`, `pull_request`, `workflow_dispatch`. `env.RUST_MSRV=1.88`.

| # | Job | What it runs | Notes |
|---|---|---|---|
| G13 | `gate-07-cargo-deny` | `cargo-deny check licenses advisories bans sources` (prebuilt cargo-deny 0.19.6) | single source of cargo-deny (ci `deny` removed at S34) |
| G14 | `build-and-lint` | `cargo fmt --all --check` + `cargo clippy --all-targets --all-features -- -D warnings` + `cargo build --workspace`; system deps cmake/clang/libclang-dev/llvm | **1.88**; overlaps G2/G4 (different toolchain) — S34-flagged "fold" target |
| G15 | `D6-coverage` | `cargo llvm-cov nextest --workspace --all-features --ignore-run-fail --lcov` + `bash scripts/ci/coverage-check.sh coverage.lcov` (per-module hot-path ≥80%, `loader.rs` carved by name) + upload lcov | `needs: build-and-lint`; frees runner disk (~28 GB instrumented) |
| G16 | `D4-conformance` | boots REAL gateway (h1s :8443 + quic :8444 → stub :3000); `h2spec -t -k --strict` (147/147) + `bash scripts/ci/h3spec-check.sh` named-waiver (12 documented quiche-0.29 limitations, CF-QUICHE-UPGRADE) | `needs: build-and-lint`; pins H2SPEC v2.6.0 / H3SPEC v0.1.13 |
| G17 | `D3a-chaos-attacks` | `cargo nextest run --all-features --package lb-h2 --package lb-l7 -E 'test(/chaos\|rapid_reset\|continuation\|hpack\|slowloris/)'` | `needs: build-and-lint` |
| G18 | `D5-image-scan` | `docker build -f docker/Dockerfile -t expressgateway:ci .` + Trivy image scan (HIGH,CRITICAL, exit-code 1, ignore-unfixed) | single source of Docker image build (ci `docker` removed at S34); **builds but does NOT run/smoke the image** |
| G19 | `D2-xdp-verifier-smoke` | build `round8_verifier_baseline_70 --no-run`, then `sudo cargo test -p lb-l4-xdp --test round8_verifier_baseline_70 -- --ignored` (genuine BPF_PROG_LOAD; assert verifier ACCEPT on runner kernel) | honest single-kernel subset; full 5.15/6.1/6.6 matrix = F-ESC-1 (self-hosted) |

## codeql.yml — `name: Security Audit` (misnamed file: it is a cargo-audit job, not CodeQL)
Triggers: `push: [main]`, `pull_request: [main]`, `schedule: '54 10 * * 0'` (weekly Sun), `workflow_dispatch`.

| # | Job | What it runs | Notes |
|---|---|---|---|
| G20 | `audit` (Dependency Audit) | `cargo audit` (**no** `-D warnings`) | WEAKER duplicate of G7. **Distinct value = the weekly `schedule`** (catches a newly-published advisory on an unchanged dep between PRs). Stale comment cites MSRV "1.85". |

## dependabot.yml — `name: Dependabot periodic run`
Triggers: `schedule: '0 0 * * *'` (daily), `workflow_dispatch`.

| # | Job | What it runs | Notes |
|---|---|---|---|
| G21 | `notify` | `echo` a message about the dependabot schedule | **NO-OP / zero gate value.** Does not test, build, lint, or guard anything. Dependabot itself is driven by `.github/dependabot.yml` (config), independent of this workflow. Pure cruft. |

## release.yml — `name: Release`
Triggers: `push: tags: ['v*']` only (does NOT run on PR or push-to-branch).

| # | Job | What it runs | Notes |
|---|---|---|---|
| G22 | `build-binary` | matrix `cargo build --release --target {x86_64,aarch64}-unknown-linux-gnu` + upload artifacts | release-only |
| G23 | `docker` (Docker Multi-Arch) | buildx multi-arch (`linux/amd64,linux/arm64`) build+push to GHCR (`docker/Dockerfile`), `cache-from/to: gha` | release-only; `needs: build-binary` |
| G24 | `release` (Create Release) | `softprops/action-gh-release` with the two binaries | release-only; `needs: [build-binary, docker]` |

---

## Shared CI scripts (single-sourced gate logic — must remain)
- `scripts/ci/doc-lint.sh` — G5 (tier-1 stale patterns + tier-2 audit-of-audit).
- `scripts/ci/coverage-check.sh` — G15 per-module hot-path ≥80% verdict.
- `scripts/ci/h3spec-check.sh` — G16 h3spec named-waiver (12 quiche limitations).
- `scripts/ci/atomic-lint.sh` — **not referenced by any workflow** (orphan; left untouched this session — not a gate).

## Distinct-gate accounting (what consolidation must preserve)
A "distinct gate" = a unique check whose loss would reduce coverage. Counting:
- **Build/lint surface:** check (stable, G1), clippy (stable, G2 + 1.88, G14-clippy), fmt (stable, G4 + 1.88, G14-fmt), build (release, G12 + workspace 1.88, G14-build), MSRV check (G10).
- **Test surface:** test suite + fcap1-isolated (G3a/G3b), chaos attacks (G17), fuzz smoke (G11), D2 XDP verifier (G19).
- **Conformance/coverage:** h2spec+h3spec (G16), per-module coverage (G15).
- **Supply-chain/security:** cargo-audit `-D` (G7) + scheduled audit (G20), cargo-deny (G13), geiger (G8), machete (G9).
- **Repo-hygiene gates:** doc-lint (G5), panic-freedom (G6).
- **Image/release:** Docker build+scan (G18) + release pipeline (G22-G24).
- **Cruft (no gate value):** dependabot `notify` (G21).

**Consolidation targets pre-authorized by R7 / the S34-deferred plan (`audit/ci/s34-report.md` line 96):**
1. Fold `build-and-lint` (G14) — its fmt/clippy overlap G4/G2. Keep the **MSRV-toolchain build** distinct.
2. Move `geiger` (G8) + `machete` (G9) off the PR critical path to a **scheduled** workflow — they STILL RUN, just not per-PR.
3. Single-source the repeated checkout+toolchain+cache+system-deps setup (R12) via a composite action.
4. Fold the weaker `codeql.yml` audit (G20) into the strict G7 audit **while preserving its weekly `schedule`** (strengthen, never weaken).
5. Remove the `dependabot.yml` no-op `notify` (G21) — zero gate value (named cruft).
6. Fix the `machete` RED (G9): 17 genuinely-unused deps → honest removal (analysis: `s35-report.md`).
