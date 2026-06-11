# S40 CI Inventory — the "before" gate set (every distinct gate)

**Base:** `main @ c4b445cd` (S39 promoted, CI honest-green).
**Purpose:** the authoritative list of EVERY distinct gate that runs today, so the
Phase-2 workflow rewrite can prove **0 gates dropped** (the S35 gate-map rule). Each
gate below MUST map to an "after" home in `s40-gate-map.md`.

Source files inventoried (`.github/`):
- `.github/workflows/ci.yml` (9.3 KB) — per-PR/push core gates
- `.github/workflows/prod-readiness-gates.yml` (14.8 KB) — D-gates (real-CI-env)
- `.github/workflows/scheduled-scans.yml` (3.2 KB) — weekly informational scans
- `.github/workflows/release.yml` (3.1 KB) — tag-triggered build/publish
- `.github/actions/rust-setup/action.yml` — single-sourced composite setup (R12)
- `.github/dependabot.yml` — dependency update config

Wired support scripts (load-bearing — a gate executes them):
- `scripts/ci/doc-lint.sh` — G5
- `scripts/ci/coverage-check.sh` — G12 (hardcodes the per-module hot-path list)
- `scripts/ci/h3spec-check.sh` — G13 (inline 12-waiver list)
- `scripts/ci/docker-smoke.sh` — G15
- `scripts/build-xdp.sh` — produces the committed ELF that G16 loads (not run in CI)

NOT wired (orphan — see doc-inventory): `scripts/ci/atomic-lint.sh` (no workflow
references it; references the non-existent `docs/decisions/atomics.md`).

---

## A. Per-PR / per-push BLOCKING gates

### From `ci.yml` (triggers: push main/rust, all PRs, dispatch; `RUSTFLAGS: -D warnings`; concurrency cancel-in-progress)

| ID | Job | What it asserts | How (exact command) |
|----|-----|-----------------|---------------------|
| **G1** | `check` | Workspace compiles, all targets + all features | `cargo check --workspace --all-targets --all-features` |
| **G2** | `clippy` | No clippy warnings (all targets/features) | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| **G3** | `test` (main) | Full test suite passes (no-fail-fast) | `cargo test --workspace --all-features --no-fail-fast -- --skip fcap1_h2_over_cap_upload_yields_413` (timeout 45m) |
| **G3b** | `test` (fcap1) | 64 MiB upload cap → 413, isolated to avoid CPU-starvation flake | `cargo test -p lb-integration-tests --test h2h1_md_streaming_verify --all-features -- --exact fcap1_h2_over_cap_upload_yields_413`, **3-retry** loop, fail only if all 3 fail (CF-SATURATION-1) (timeout 25m) |
| **G4** | `fmt` | Formatting clean | `cargo fmt --all -- --check` |
| **G5** | `doc-lint` | Operator-doc stale-pattern guard + audit-of-audit SHA validation | `bash scripts/ci/doc-lint.sh` with **`fetch-depth: 0`** (tier-2 walks history) |
| **G6** | `panic-freedom` | Every `crates/*/src/lib.rs` carries `#![deny(...clippy::unwrap_used)]` | inline grep `-Pzoq` over all lib crates |
| **G7** | `audit` | No RUSTSEC advisory (per-PR strict) | `cargo +stable install cargo-audit --locked` → `cargo audit -D warnings` |
| **G8** | `msrv` | Compiles on pinned MSRV 1.88 | `cargo check --workspace --all-targets --all-features` on toolchain `1.88` |
| **G9** | `fuzz-smoke` | Every fuzz target runs 10 s with no crash | nightly `cargo fuzz run <t> -- -max_total_time=10` for each `cargo fuzz list` target |
| **G10** | `release-build` | Release codegen succeeds | `cargo build --workspace --release` (timeout 30m) — `needs: [check, clippy, test, fmt, panic-freedom, doc-lint]` |

### From `prod-readiness-gates.yml` (triggers: push prod-readiness/round-4 + main, all PRs, dispatch; `RUST_MSRV: 1.88`)

| ID | Job | What it asserts | How |
|----|-----|-----------------|-----|
| **G11** | `gate-07-cargo-deny` | Licenses/advisories/bans/sources clean | prebuilt cargo-deny 0.19.6 → `cargo-deny check licenses advisories bans sources` (explicit subcommand list — single source; the ci.yml duplicate was removed at S34) |
| **G12** | `D6-coverage` | **PER-MODULE** hot-path line coverage ≥ 80% (charter metric, not aggregate) | free-disk step → install cargo-llvm-cov + nextest → `cargo llvm-cov nextest --workspace --all-features --ignore-run-fail --lcov` → `bash scripts/ci/coverage-check.sh coverage.lcov` (loader.rs named carve-out); uploads `coverage.lcov` artifact |
| **G13** | `D4-conformance` | h2spec --strict 147/147 **and** h3spec passes with only the 12 named quiche waivers | build `lb`, boot real gateway (h1s :8443 + quic :8444 in front of python backend), `h2spec -t -k --strict` + `bash scripts/ci/h3spec-check.sh`. **Pinned** H2SPEC v2.6.0 / H3SPEC v0.1.13 (waiver list exactness) |
| **G14** | `D3a-chaos-attacks` | Rapid-Reset / CONTINUATION / HPACK / slowloris detectors hold | `cargo nextest run --all-features -p lb-h2 -p lb-l7 -E 'test(/chaos|rapid_reset|continuation|hpack|slowloris/)'` |
| **G15** | `D5-image-scan` | Image builds **and boots and serves** a real request, then scans clean | `docker build -f docker/Dockerfile` → `bash scripts/ci/docker-smoke.sh` (run+serve 200+body) → Trivy HIGH/CRITICAL `exit-code: 1` `ignore-unfixed` |
| **G16** | `D2-xdp-verifier-smoke` | Committed XDP ELF loads + kernel verifier ACCEPTs on the runner kernel | build `round8_verifier_baseline_70 --no-run` → `sudo ... cargo test -p lb-l4-xdp --test round8_verifier_baseline_70 -- --ignored` (real BPF_PROG_LOAD; aya legacy maps) |

---

## B. Weekly SCHEDULED scans (non-blocking, but must keep running) — `scheduled-scans.yml`

Triggers: `cron '54 10 * * 0'` (Sun 10:54 UTC) + dispatch. `RUSTFLAGS: -D warnings`.

| ID | Job | What it asserts | Notes |
|----|-----|-----------------|-------|
| **G17** | `audit` (weekly) | Strict `cargo audit -D warnings` weekly | catches advisories newly published against **unchanged** deps between PRs (the value of the schedule); folds the old misnamed `codeql.yml` weekly audit, strengthened |
| **G18** | `geiger` | Unsafe-block inventory (cargo-geiger JSON) | `continue-on-error: true` — informational, archives `geiger.json` artifact (30 d) |
| **G19** | `machete` | No unused dependencies (`cargo machete`) | GREEN as of S35; weekly so a future unused dep surfaces loud without blocking PRs |

---

## C. RELEASE pipeline (tag `v*`) — `release.yml`

Triggers: `push tags v*`. `permissions: contents:write, packages:write`.

| ID | Job | What it does |
|----|-----|--------------|
| **G20** | `build-binary` | Matrix release build x86_64 + aarch64 (cross gcc for arm64), upload artifacts |
| **G21** | `docker` | Multi-arch (amd64+arm64) image build + push to GHCR (semver + sha tags, gha cache) |
| **G22** | `release` | Create GitHub Release with both binaries + generated notes — `needs: [build-binary, docker]` |

> **Phase-3 note:** the release pipeline does NOT currently invoke the soak gate. Phase 3
> adds `scripts/release-soak.sh` (dedicated-EC2 12-scenario soak) and wires/document its
> invocation at release. This is an ADDITION, not a preserved gate.

---

## D. Supporting infrastructure (must be preserved in the rewrite)

| ID | Item | Why it must survive |
|----|------|---------------------|
| **I1** | `.github/actions/rust-setup` composite | Single-sources toolchain + cache + system-deps (R12 maintainability). Inputs: `toolchain`, `components`, `cache`, `system-deps`. |
| **I2** | `dependabot.yml` | cargo (daily, limit 1, grouped) + github-actions (daily, limit 1, grouped) with `dtolnay/rust-toolchain` **excluded** (toolchain tag ≠ action version, bogus #214 bump). |
| **I3** | concurrency `cancel-in-progress` on CI | resource hygiene on the PR path. |

### Cross-cutting behaviors that MUST survive (R2 flake protocol)

1. **fcap1 isolation + 3-retry** (G3b) — CF-SATURATION-1 / CF-FCAP1-FLAKE.
2. **`--no-fail-fast`** on the suite (G3) — get the full failure set, not first-fail truncation.
3. **`--all-features`** everywhere (= `test-gauges`; the R8 memory tests read it).
4. **D6 `--ignore-run-fail`** (G12) — coverage measures even if a test flakes; the verdict is `coverage-check.sh`, not pass/fail.
5. **D6 free-disk step** (G12) — instrumented `--workspace` build is ~28 GB.
6. **D4 pinned tool versions** (G13) — h2spec v2.6.0 / h3spec v0.1.13 keep the 12-waiver list exact.
7. **doc-lint `fetch-depth: 0`** (G5) — tier-2 audit-of-audit walks historical SHAs.
8. **SUITE_SERIAL** in-test tokio Mutex (in source, documented by G3's comment) — heavy real-wire e2e binaries self-serialize.

---

## Gate count summary

- **Blocking per-PR/push:** 16 (G1–G16: 10 in ci.yml + 6 in prod-readiness-gates.yml)
- **Weekly scheduled:** 3 (G17–G19)
- **Release (tag):** 3 (G20–G22)
- **Infra/cross-cutting:** I1–I3 + 8 flake-protocol behaviors

**Total distinct gates the rewrite must reproduce: 22 (G1–G22)** + I1–I3 + the 8 flake-protocol behaviors.
