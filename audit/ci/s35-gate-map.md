# S35 CI Gate-Map — BEFORE → AFTER (R3: nothing dropped)

Maps every distinct gate from the "before" inventory (`s35-gate-inventory.md`,
G1–G24) to where it runs AFTER the S35 consolidation. **Rule (R3/R4):** every
before-gate must be either (a) PRESENT after (same/stricter check), or (b)
explicitly NAMED-as-redundant/cruft. A gate that is neither = STOP. No gate was
weakened; two were *strengthened*.

## File-level change
`5 workflow files → 4` (+ one single-sourced composite action):

| Before (5) | After (4) |
|---|---|
| `ci.yml` | `ci.yml` (composite setup; geiger/machete moved out) |
| `prod-readiness-gates.yml` | `prod-readiness-gates.yml` (build-and-lint folded; D5 + smoke) |
| `codeql.yml` (misnamed weekly `cargo audit`) | **deleted** → folded into `ci.yml` audit + `scheduled-scans.yml` |
| `dependabot.yml` (no-op `notify`) | **deleted** (cruft) |
| `release.yml` | `release.yml` (unchanged) |
| — | **`scheduled-scans.yml`** (new: weekly audit + geiger + machete) |
| — | **`.github/actions/rust-setup/action.yml`** (new: single-sourced checkout-adjacent toolchain+cache+system-deps, R12) |

`.github/dependabot.yml` (Dependabot *config*) is untouched.

## Gate-by-gate accounting

| # | Before gate | After location | Verdict |
|---|---|---|---|
| G1 | `ci` check (stable, `cargo check --workspace --all-targets --all-features`) | `ci.yml` **check** (composite stable) | ✅ PRESENT — identical command |
| G2 | `ci` clippy (stable, `-D warnings`, all-targets/all-features) | `ci.yml` **clippy** (composite stable, clippy) | ✅ PRESENT — identical |
| G3a | `ci` test — suite minus fcap1 | `ci.yml` **test** step a | ✅ PRESENT — identical |
| G3b | `ci` test — fcap1 isolated, 3× retry (CF-FCAP1/SATURATION) | `ci.yml` **test** step b | ✅ PRESENT — identical (R2 flake protocol intact) |
| G4 | `ci` fmt (stable, `cargo fmt --all -- --check`) | `ci.yml` **fmt** (composite stable, rustfmt, cache off) | ✅ PRESENT — identical |
| G5 | `ci` doc-lint (`scripts/ci/doc-lint.sh`, fetch-depth 0) | `ci.yml` **doc-lint** | ✅ PRESENT — identical |
| G6 | `ci` panic-freedom (deny-lint grep) | `ci.yml` **panic-freedom** | ✅ PRESENT — identical |
| G7 | `ci` audit (`cargo audit -D warnings`) | `ci.yml` **audit** (composite stable) | ✅ PRESENT — identical strict audit |
| G8 | `ci` geiger (Unsafe Inventory, informational, `continue-on-error`) | `scheduled-scans.yml` **geiger** (weekly) | ↪️ MOVED PR→weekly — STILL RUNS; was already non-blocking informational (R7 / S34-deferred plan). Named. |
| G9 | `ci` machete (Unused Deps, informational, **was RED**) | `scheduled-scans.yml` **machete** (weekly) + **FIXED** (17 genuinely-unused deps removed, `s35-report.md`) | ↪️ MOVED PR→weekly + 🟢 FIXED. Still runs; honest green. Named. |
| G10 | `ci` msrv (1.88 `cargo check --workspace --all-targets --all-features`) | `ci.yml` **msrv** (composite 1.88) | ✅ PRESENT — identical |
| G11 | `ci` fuzz-smoke (nightly cargo-fuzz, 10s/target) | `ci.yml` **fuzz-smoke** (composite nightly) | ✅ PRESENT — identical |
| G12 | `ci` release-build (`cargo build --workspace --release`) | `ci.yml` **release-build** (composite stable) | ✅ PRESENT — identical (same `needs`) |
| G13 | `prod` gate-07-cargo-deny (`check licenses advisories bans sources`) | `prod-readiness` **gate-07-cargo-deny** | ✅ PRESENT — identical |
| G14 | `prod` build-and-lint = fmt + clippy + `cargo build --workspace` on **1.88** | **FOLDED** → fmt:G4 (stable, fmt is toolchain-invariant); clippy:G2 (stable clippy ⊇ 1.88 clippy lints); "compiles on MSRV 1.88":G10 (`cargo check` all-targets/all-features on 1.88) + codegen:G12 (release build) | 🟰 REMOVED-as-redundant (R4 fold; the S34-deferred plan, `s34-report.md:96`). No distinct check lost — each sub-check is covered same-or-stricter. Named. |
| G15 | `prod` D6-coverage (llvm-cov per-module ≥80%, `coverage-check.sh`) | `prod-readiness` **D6-coverage** (composite; `needs: build-and-lint` edge removed) | ✅ PRESENT — identical check (scheduling-only `needs` dropped, not a gate) |
| G16 | `prod` D4-conformance (h2spec --strict + h3spec named-waiver) | `prod-readiness` **D4-conformance** (composite; `needs` removed) | ✅ PRESENT — identical |
| G17 | `prod` D3a-chaos-attacks (rapid-reset/continuation/hpack/slowloris) | `prod-readiness` **D3a-chaos-attacks** (composite; `needs` removed) | ✅ PRESENT — identical |
| G18 | `prod` D5-image-scan (docker build + Trivy HIGH/CRITICAL) | `prod-readiness` **D5-image-scan** (build + **RUN+SERVE smoke** + Trivy) | ✅ PRESENT + 💪 STRENGTHENED — adds `scripts/ci/docker-smoke.sh` (real request through the live container asserts 200+body) between build and scan. Build+scan preserved. |
| G19 | `prod` D2-xdp-verifier-smoke (sudo XDP BPF_PROG_LOAD, assert verifier ACCEPT) | `prod-readiness` **D2-xdp-verifier-smoke** (composite) | ✅ PRESENT — identical |
| G20 | `codeql.yml` audit — WEEKLY `cargo audit` (no `-D`), also PR-to-main | PR/push → `ci.yml` **audit** G7 (stricter `-D`); weekly → `scheduled-scans.yml` **audit** (now `-D warnings`) | 🟰 FOLDED + 💪 STRENGTHENED — weekly cadence preserved (`54 10 * * 0`), now strict. The weak duplicate is gone, not the coverage. Named. |
| G21 | `dependabot.yml` notify — `echo` a message daily | **deleted** | 🗑️ REMOVED-cruft — zero gate value (no build/test/lint/scan). Dependabot itself runs from `.github/dependabot.yml`, unaffected. Named. |
| G22 | `release` build-binary (amd64+arm64) | `release.yml` **build-binary** | ✅ PRESENT — unchanged |
| G23 | `release` docker (multi-arch GHCR push) | `release.yml` **docker** | ✅ PRESENT — unchanged |
| G24 | `release` create-release | `release.yml` **release** | ✅ PRESENT — unchanged |

## Net result
- **Distinct checks dropped: 0.** 22 present (3 strengthened: G18 smoke, G20 strict-weekly, machete-now-green); G14 folded-redundant (named); G21 removed-cruft (named); G8/G9 moved PR→weekly (named, still run).
- **R12 single-sourcing:** the repeated `checkout-then-toolchain+cache+system-deps` is now `./.github/actions/rust-setup`, referenced by ci.yml (check/clippy/test/fmt/audit/msrv/fuzz/release-build), prod-readiness (D6/D4/D3a/D2), and scheduled-scans (audit/geiger/machete) — defined once.
- **PR critical path is lighter:** geiger+machete no longer run per-PR (weekly instead); build-and-lint's duplicate fmt/clippy/build no longer double-run. No PR-blocking *check* removed.

## Triggers preserved (no coverage-by-trigger lost)
- Strict `cargo audit`: per-PR/push (ci.yml, was G7) **and** weekly (scheduled-scans, folds G20's cadence). Superset of before.
- geiger/machete: weekly + dispatch (were per-PR/push/dispatch — intentionally moved off PR path, R7).
- All `ci.yml` + `prod-readiness` gates: still `pull_request` + `push` + `workflow_dispatch`.
