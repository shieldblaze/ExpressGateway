# S40 Doc Inventory — classified

**Base:** `main @ c4b445cd`. **Purpose:** classify every doc before any action (R4:
no deletion without classification + proof; for evidence/load-bearing, owner approval).

**Classification key**
- **PUBLIC** — public/user-facing (front door, ops, config reference). Keep; goes to `docs/guide/` or stays at root per GitHub convention.
- **DEV** — developer/technical (architecture, internals, ADRs, research). Keep; goes to `docs/arch/`.
- **LOAD-BEARING** — a CI gate / script reads it, OR it justifies a documented limitation/waiver, OR it is the committed evidence trail. **KEEP**; relocate only with CI-reference update.
- **ARCHIVAL** — one-time session report / gate output / evidence. Keep under `audit/` (optionally `audit/archive/`). **Never delete.**
- **REDUNDANT** — superseded/duplicate/orphan. Delete-candidate **with proof** → owner checkpoint.

---

## 1. Root markdown + LICENSE (8 files)

| File | Class | Disposition | Notes |
|------|-------|-------------|-------|
| `README.md` | PUBLIC + LOAD-BEARING | **KEEP @ root** | project front door; in `doc-lint.sh` FILES; GitHub convention |
| `LICENSE` | LOAD-BEARING | **KEEP @ root** | legal (35 KB full text) |
| `CHANGELOG.md` | PUBLIC + LOAD-BEARING | **KEEP @ root** | in `doc-lint.sh` FILES |
| `SECURITY.md` | PUBLIC + LOAD-BEARING | **KEEP @ root** | GitHub surfaces `/SECURITY.md`; in `doc-lint.sh` FILES |
| `CONFIG.md` | PUBLIC (config ref) + LOAD-BEARING | **RELOCATE→`docs/guide/` (proposed)** | in `doc-lint.sh` FILES → must update FILES array if moved |
| `DEPLOYMENT.md` | PUBLIC (ops) + LOAD-BEARING | **RELOCATE→`docs/guide/` (proposed)** | in `doc-lint.sh` FILES |
| `METRICS.md` | PUBLIC (ops) + LOAD-BEARING | **RELOCATE→`docs/guide/` (proposed)** | in `doc-lint.sh` FILES |
| `RUNBOOK.md` | PUBLIC (ops) + LOAD-BEARING | **RELOCATE→`docs/guide/` (proposed)** | in `doc-lint.sh` FILES |

> **doc-lint coupling (HARD):** `scripts/ci/doc-lint.sh` FILES = README, RUNBOOK,
> DEPLOYMENT, METRICS, CHANGELOG, CONFIG, SECURITY, docs/features.md. Relocating ANY
> of these **requires** updating the FILES array in the same commit (R3). README /
> CHANGELOG / SECURITY / LICENSE stay at root (GitHub conventions); the 4 operator
> docs are relocation candidates. **Relocation is pre-authorized (R6); surfaced here
> as a structural choice, not a deletion.**

## 2. `docs/**` (40 files)

| File(s) | Class | Disposition |
|---------|-------|-------------|
| `docs/architecture.md` (400 L) | DEV | →`docs/arch/architecture.md` |
| `docs/features.md` (86 L) | DEV + LOAD-BEARING (doc-lint FILES + README link) | →`docs/arch/` **iff** doc-lint FILES + README updated; else keep path |
| `docs/known-limitations.md` (136 L) | DEV/PUBLIC (README links) | →`docs/arch/` (keep README link working) |
| `docs/edge-defaults.md` (63 L) | DEV | →`docs/arch/` |
| `docs/decisions/ADR-0001..0010` + `ebpf-toolchain-separation.md` + `quinn-to-quiche-migration.md` (12) | DEV (ADRs) | →`docs/arch/decisions/` |
| `docs/research/*.md` (15: envoy, nginx, haproxy, pingora, katran, cloudflare-lb, aws-alb-nlb, aya-ebpf, grpc, hpack-qpack, dos-catalog, compression-rfcs, cross-cutting, tokio-io-uring, rfc9000/9112/9113/9114) | DEV (background research) | →`docs/arch/research/` |
| `docs/*/.gitkeep` | infra | keep / drop as dirs gain content |
| **`docs/conformance/coverage.md` (68 L)** | **REDUNDANT** | **DELETE-candidate → checkpoint** (see §5) |

## 3. `audit/**` — the evidence trail

- **1000 git-tracked files** (652 md/txt/toml among them), **114 MB**. Class: **ARCHIVAL / LOAD-BEARING**. **KEEP wholesale.**
- Hard CI couplings inside `audit/`:
  - `audit/coverage-scope.md` — the coverage charter (referenced by `coverage-check.sh`; the module list is hardcoded in the script, so moving the doc won't break the gate, but it is the LOAD-BEARING charter → keep).
  - `audit/**/round-*-review.md` + `round-*-findings.md` — walked by `doc-lint.sh` tier-2 (audit-of-audit). `find audit -name 'round-*-review.md'` must keep finding them → **must stay under `audit/`** (relocation within audit/ is fine; moving out is not).
  - `audit/round-7/deferred-to-ci.md`, `audit/unsafe-justifications.md`, `audit/ci/s34-report.md`, `audit/ci/s35-gate-map.md` — cited by scripts/workflow comments.
- **Disposition:** KEEP all tracked audit content. Optional `audit/archive/` reorg of pre-S30 session dirs is RELOCATION (autonomous), only if it preserves the tier-2 `find` path — **deferred / low value; recommend leaving audit/ as-is** to avoid any tier-2 path risk.

## 4. Scattered / component docs (keep)

| File | Class | Disposition |
|------|-------|-------------|
| `bench/README.md`, `fuzz/README.md` | DEV (component) | KEEP in place |

---

## 5. REDUNDANT delete-candidates (OWNER CHECKPOINT — proof for each)

The repo is overwhelmingly load-bearing docs + an intentional evidence trail; genuine
deletion candidates are **few**. Each below has a proof; none is deleted without owner OK.

| # | Path | Proof of redundancy | Risk if deleted |
|---|------|---------------------|-----------------|
| **D-1** | `docs/conformance/coverage.md` | **Stale generated snapshot**: header says "Generated by `cargo llvm-cov ... --summary-only` at Pillar-4a-plus-Step-5 baseline (HEAD `de5c6dbf`)", reports a frozen 75.45% workspace total. **Superseded by gate G12** (`D6-coverage`, the live per-module ≥80% charter gate). NOT read by any script (`coverage-check.sh` hardcodes the module list + parses live LCOV). | None — no CI/script/README reference; pure stale artifact. (Alternative: move to `audit/archive/` instead of delete.) |
| **D-2** | `scripts/ci/atomic-lint.sh` | **Orphan**: `grep -rn atomic-lint .github/` = 0 hits (no workflow runs it). It references `docs/decisions/atomics.md` which **does not exist on disk**. | None — provably unwired; not a gate. (It's a script, not a doc; flagged here for completeness.) |
| **D-3** | Untracked scratch (~4 MB): `audit/deps/s31-gate-{baseline-0.28,029-1.88}/`, `audit/deps/s33-gate-{baseline,phase1..4}/`, `audit/deps/s31-h3spec-029.toml`, `audit/perf/s39-sc3-isolated/sc3_slowloris-work/`, `audit/perf/s39-sc9-recheck/sc9_grpc_h3-work/`, `audit/h3spec/s26-verify/s26-h3spec-preview.cases.txt`, `audit/ops/s36-baseline/{baseline.marker,x3-postfix.sh}`, `audit/soak/s33-run-resoak.sh`, `audit/soak/s36-soak-data/sc9-prefix-reference/sc9-reference.marker` | **Never committed**: `git status ??` + `git check-ignore` = untracked, not ignored. Session gate-run working dirs; the committed soak/perf evidence already lives under `audit/soak/**` and `audit/perf/s39-*` (tracked). | None — untracked working output; removing them is local hygiene, not a git change. |

## 6. AMBIGUOUS (surface — owner decides; default = conservative)

| Path | Question | Default if no decision |
|------|----------|------------------------|
| `scripts/s26-*.sh`, `scripts/s31-*.sh`, `scripts/halting-gate.sh`, `scripts/never_decrypted_proof.sh` | Historical one-off session/repro scripts; some cite now-deleted files (`docs/manifest-drift-proposal.md`, `audit/deps/s31-cov-summary.txt`). Archive vs keep vs delete? | **Relocate→`scripts/archive/`** (relocation, not deletion) — keep the repro provenance, off the active scripts path |
| Operator docs relocation (CONFIG/DEPLOYMENT/METRICS/RUNBOOK → `docs/guide/`) | Move into the new guide tree, or keep at root? | Proposed: **relocate** + update `doc-lint.sh` FILES + README links (pre-authorized relocation) |
| `audit/archive/` reorg of pre-S30 sessions | Worth reorganizing the evidence trail? | **No** — leave audit/ as-is (avoid tier-2 `find`-path risk); low value |

---

## Summary for the checkpoint

- **Nothing in the committed evidence trail (`audit/**`, 1000 files) is a deletion candidate.**
- **Genuine REDUNDANT delete-candidates: 3** (D-1 stale coverage snapshot, D-2 orphan script, D-3 untracked scratch — D-3 is untracked so not even a git change).
- **Relocations** (pre-authorized): docs/** → docs/arch/, operator docs → docs/guide/ (with doc-lint FILES update).
- **Scaffold** docs/guide/ + docs/arch/ with index READMEs for Session B.
- **No load-bearing or CI-referenced doc is proposed for deletion.**
