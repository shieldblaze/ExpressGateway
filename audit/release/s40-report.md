# S40 — Repo Release Hygiene (Session A) — Report

**Base:** `main @ c4b445cd` (S39 promoted, pre-prod validation done, CI honest-green).
**Branch:** `feature/repo-release-hygiene-s40` · **PR:** #230.
**Mandate:** make the project maintainable + release-friendly **without touching
production protocol source.** No gate dropped; no load-bearing/evidence doc
deleted without owner approval.

---

## 1. Branch cleanup

Classified every local + remote branch by **ancestry to `origin/main`** (true
proven-merge; squash-aware — `--merged` alone misses squash merges, per S35).

- **Deleted — provably merged (ancestor of main, commits already in main):**
  - Local (7): `feature/ops-bcd-s37`, `feature/ops-layer-s36`,
    `feature/security-audit-s38`, `s37-b-config`, `s37-c-h2cov`, `s37-c-reload`,
    `s37-d-deps`.
  - Remote (7): `feature/ops-bcd-s37`, `feature/ops-layer-s36`,
    `feature/perf-burnin-s39`, `feature/security-audit-s38`, `s37-b-config`,
    `s37-c-reload`, `s37-d-deps`.
- **Kept:** `main`, `origin/old-main` (per mandate).
- **Surfaced, NOT deleted** (R4/R7 — never delete unmerged):
  - `feature/perf-burnin-s39` (local) — `c4fd0149` IS in main's history (merged
    to HEAD), but `git -d` refused on the just-deleted upstream; left per R9
    (no force-delete). Safe to `-D` if desired.
  - Squash-merge candidates (non-ancestor, not auto-proven): `feature/repo-hygiene-s35`,
    `s11-verify`, the `s6/*`, `s7/*`, `s8/*` session verify/gate branches,
    `s37-verify*`.
  - Genuinely unmerged / divergent: `feature/grpc-h3-churn-rss-s32` (S32 partial/
    escalation), `feature/lib-usage-investigation`, `feature/cfbw-s14-verify`,
    and the long-divergent `origin/gg` (64), `origin/restructcure` (207),
    `origin/rust` (212).
  - `origin/dependabot/*` — left untouched (dependabot-managed open PRs).

## 2. Doc reorg + deletions (owner-approved at the Phase-0 checkpoint)

- **Relocated** the 4 operator docs `CONFIG/DEPLOYMENT/METRICS/RUNBOOK` → `docs/guide/`.
  Updated **in the same commit**: the load-bearing `scripts/ci/doc-lint.sh` FILES
  array (so the gate still scans them — a non-resolving path would be a SILENT
  weakening), README references, the one SECURITY.md reference. **doc-lint re-run
  GREEN** (tier-1 + tier-2/52 claims).
- **Deleted (3, owner-approved):**
  - `docs/conformance/coverage.md` — stale generated snapshot (frozen HEAD
    `de5c6dbf`, 75.45%), superseded by the live D6/coverage gate; no script reads it.
  - `scripts/ci/atomic-lint.sh` — orphan: no workflow runs it; cited a
    non-existent `docs/decisions/atomics.md`.
  - ~4 MB untracked session scratch (never committed; real evidence already
    tracked under `audit/soak`/`audit/perf`).
- **Archived (7)** clean one-off scripts `s26-*` / `s31-*` → `scripts/archive/`.
- **Scaffolded** `docs/guide/README.md` (user index) + `docs/arch/README.md`
  (developer index) for Session B.
- **R3-driven boundaries (zero `crates/`/`tests/` in the diff — independently
  verified):** developer docs (`architecture.md`, `decisions/`, `research/`,
  `edge-defaults.md`, `features.md`, `known-limitations.md`) were KEPT at their
  established paths — they are cited **by path** from production crate source +
  tests + `manifest/required-artifacts.txt`; moving them would strand prod
  references or edit production source. `docs/arch/README.md` indexes them in
  place. Likewise `never_decrypted_proof.sh` (prod-cited linkage gate) +
  `halting-gate.sh` (manifest-required) were kept in `scripts/`, not archived.
- Inventories: `s40-doc-inventory.md` (classification) — author≠verifier confirmed
  no load-bearing/CI-referenced doc deleted.

## 3. GitHub Actions rewrite (gate-map: 0 dropped)

- **4 workflows → 3 + 1 composite:** merged the two per-PR BLOCKING workflows
  (`ci.yml` + `prod-readiness-gates.yml`) into one sectioned `ci.yml` (16 jobs);
  renamed `scheduled-scans.yml` → `scheduled.yml`; `release.yml` carries the 3
  publish jobs + the new soak gate. The `rust-setup` composite + `dependabot.yml`
  unchanged. R12 maintained (shared setup single-sourced).
- **Every gate preserved with byte-equivalent commands** — all 22 distinct gates
  (G1–G22) + I1–I3 infra + the 8 flake-protection behaviors (fcap1 3-retry,
  `--no-fail-fast`, `--all-features`, coverage `--ignore-run-fail` + free-disk,
  pinned h2spec/h3spec, doc-lint `fetch-depth:0`, SUITE_SERIAL). Full mapping +
  the 6-entry branch-protection required-check **rename list** (owner action) in
  `s40-gate-map.md`.
- **Validation:** PyYAML parse clean; **actionlint v1.7.7 PASS** on all 3;
  **independent verifier** diffed every BEFORE→AFTER gate body (comments
  stripped) — **22/22 byte-equivalent, 0 dropped**, "Security Audit" name
  unchanged, publish jobs correctly gated to tags, permissions tightened to
  per-job least-privilege.
- **Green CI run on the branch (R15):** PR #230 — _<run id, recorded at promote>_.
- Deliberate (NOT gate drops): legacy push triggers (`rust`,
  `prod-readiness/round-4`) dropped — `main` + all-PR coverage unchanged.
- **Real gap surfaced + fixed (R1):** the first full run was green on 15/16 jobs;
  `fuzz-smoke` failed at `cargo install cargo-fuzz` — a recent `cargo-platform@0.3.3`
  (a cargo-fuzz dep) raised its MSRV to 1.91, and the install lacked an explicit
  `+toolchain` so the repo's `rust-toolchain.toml` (1.88) overrode it. Fixed to
  `cargo +nightly install cargo-fuzz --locked` (nightly is this job's installed
  toolchain + new enough; matches the `+stable install` convention every other
  tool job already uses to dodge the 1.88 pin). The gate's ASSERTION is unchanged
  (each fuzz target still runs 10 s under nightly) — only the broken tool install
  is corrected. **This is byte-identical to main's fuzz-smoke job, so main is
  latently broken by the same upstream release** (it was green at f5934400 before
  cargo-platform 0.3.3 shipped) — this fix repairs main too.

## 4. Soak release gate (dedicated EC2)

On-demand provision → run → report → **teardown** model (owner Q4). Three
single-sourced scripts:
- `scripts/release-soak.sh` (controller) — GitHub OIDC → scoped IAM role (no
  long-lived keys) → provision **c6a.2xlarge** (8-core + ENA) → poll S3 for the
  verdict → **ALWAYS teardown** (trap on EXIT/INT/TERM). `--dry-run` stubs all AWS.
- `scripts/soak/release-soak-onbox.sh` — build → 12-scenario soak (reuses
  `run-soak.sh`) → verdict → S3 upload → self-terminate. Drops raw CSVs + caps
  logs (S39 disk lesson).
- `scripts/soak/soak-verdict.sh` — the gate: every scenario `overall=BOUNDED` +
  panic=0 or FAIL; compact summary, not 100M CSVs.
- Wired as the `release.yml` `soak-gate` job (workflow_dispatch; OIDC perms).
- **Validated this session** (no soak box available — owner Q4): shellcheck CLEAN;
  `--dry-run` renders provision/user-data/teardown with no AWS resources created;
  `soak-verdict.sh` proven against REAL markers — S37 12-scenario set → **PASS
  12/12 panic=0**, an S31 DRIFT dir → **FAIL** (DRIFT, panics=0; a `panic_total 0`
  false-positive was caught + fixed with precise awk). Independent verifier
  reconfirmed all three.
- First REAL run is the first release (owner configures the repo vars/secrets per
  the `release-soak.sh` header).

## 5. Dev-setup cleanup + doc

- `docs/arch/DEV-SETUP.md` — clone/build/test, run-the-gates-locally, run-a-soak,
  **box requirements per task** (small for dev; c6a.2xlarge+ENA for soak/perf),
  toolchain pins (1.88 MSRV hard + nightly-for-fuzz + the separate ebpf
  nightly+bpf-linker), **disk management** (shared `eg-target`, CF-DISK-1 ~40 GB,
  the soak-CSV/gateway-log hazards + reclaim order), worktree + shared-tree commit
  discipline, the release-soak flow.
- `.gitignore`: added `release-soak-out/` so on-box soak scratch can't be
  committed (prevents the untracked-scratch class cleaned in §2).
- Dev-env assessment: **no git worktree sprawl remains** (only the main checkout;
  prune = no-op). Loose `~/Code/eg-s*/` + `s9-gate` evidence dirs (~3.5 MB) left
  in place + documented as reclaimable (R4: tiny, evidence-adjacent, shared box).

## 6. Verification (author≠verifier, R15)

- **Independent verifier** (separate agent): gate-map **PASS** (22/22 byte-
  equivalent, 0 dropped); doc classification **PASS** (only the 3 approved
  deletions, none load-bearing; doc-lint exit 0; dev docs intact; 0 crates/tests
  touched); soak scripts **PASS** (dry-run safe, verdict accurate).
- **Green CI run:** PR #230 final run — cited at promote (R15).
- **Post-merge main CI green on the new workflows:** confirmed at promote (R11).

## 7. Carry-forward / observations

- **Branch-protection required-check rename** (owner admin action) — 6 D-gate
  checks moved into the `CI` workflow; see `s40-gate-map.md` §F.
- **Soak repo vars/secrets** to configure before the first release soak (listed
  in `release-soak.sh` header + DEV-SETUP §"Release soak gate").
- **Dependabot** reports 2 vulnerabilities (1 moderate, 1 low) on the default
  branch (GitHub Advisory DB, broader than RUSTSEC; the cargo-audit/cargo-deny
  gates are green). Not session-introduced (no deps changed) — flag for a deps
  session.
- Pre-existing carries unchanged: CF-S27-2 (WS-H2/hyper#4050),
  CF-S37-D-TOKIO-1.52-RELAY, CF-S39-H3-REJECT-LOG-SPAM, CF-S38-QUIC-MAXCONN, F-ESC-1.
- **SESSION B** (handoff): write the public-facing docs (what-is-this /
  how-it-works / architecture / getting-started / config-reference / positioning)
  into the `docs/guide` + `docs/arch` scaffolding.

---

**VERDICT:** _finalized at promote_ — branches cleaned (7 local + 7 remote merged
deleted, unmerged surfaced); docs reorganized (3 deleted-approved, 4 operator docs
relocated, guide/arch scaffolded); CI rewritten (gate-map: 0 dropped, actionlint +
independent-verifier confirmed, green run cited); soak release gate scripted +
validated; dev-setup documented. No production protocol source touched.
