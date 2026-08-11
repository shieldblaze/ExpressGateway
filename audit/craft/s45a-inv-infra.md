# scanner-infra — scripts / root / workflows / config

## Summary

files_scanned=225  delete=20  archive=8  keep=178  ambiguous=19

Scope = all tracked files except `crates/**`, `audit/**`, `docs/**` and the root
`*.md` / `LICENSE` (the latter two belong to the docs scanner; flagged at the end
if uncovered). Breakdown: `tests/` 90, `fuzz/` 48, root 29, `scripts/` 28,
`config/` 10, `bench/` 6, `.github/` 6, `docker/` 3, `manifest/` 2, `.cargo/` 2,
`packaging/` 1.

**Method.** Every "no caller" claim below is `git grep -F <basename>` across ALL
tracked files (workflows, other scripts, `docs/**`, `README`/`CONTRIBUTING`,
`Cargo.toml`, `Dockerfile`, `packaging/`, and `audit/**` read as a *reference*
source), with the file itself excluded. Raw evidence is quoted per row.

### Corrections to the lead's pre-established facts (both strengthen the case)

1. **The 20 root scripts landed in FIVE commits, not one.** `2257ca44`
   (run-baseline/cov-full/cov/lint/x3), `832ef859` (watchdog, x3-takeover),
   `f6d64237` (fcap-isolate, x3c-takeover), `937a879c` (the 6 `run-d-*` +
   p4-x3 + p4-x3f + resoak) — all 2026-06-07 — and `73b18602` (ci-watch,
   d6rerun-watch, main-ci-watch) on 2026-06-08. All five are S37 session
   commits; the "one-off lead harness" characterisation holds.
2. **`run-resoak.sh` has ZERO referencing files, not one.** The
   `audit/release/s40-doc-inventory.md:74` hit is a *substring* match on a
   different path — `audit/soak/s33-run-resoak.sh` (an untracked file listed
   as D-3 scratch). An anchored grep `(^|[^/-])run-resoak\.sh` returns nothing.
   So **all 20** root scripts have zero referencing files.

### Proof that all 20 root scripts are dead, not merely unreferenced

Verified on disk, this session:

- `.claude/worktrees/` **is empty** (`ls` → `.` and `..` only); `git worktree
  list` shows only the primary checkout. 14 of the 20 `cd` into
  `.claude/worktrees/{s36-verify,s37-deps,s37-verify}` guarded by `|| exit 99`
  → they abort on line 4–8 before doing anything.
- `/home/ubuntu/Code/eg-target` (the `CARGO_TARGET_DIR` all of them export)
  **does not exist**.
- The 3 `gh`-watchers hardcode completed one-off IDs: PR `#228`, run
  `27145437614`, run `27145437741`.
- `run-watchdog.sh` watches branches `s37-b-config` / `s37-d-deps` in the same
  dead worktree.

### Deletion hazard the sweeper MUST respect (measured)

`scripts/ci/doc-lint.sh` **tier-2 test 2** (lines 271–313) extracts
`(audit|crates|scripts|packaging)/…` paths from the `Recommendation:` block of
every `Verified-Fixed` finding in `audit/**/round-*-review.md` +
`round-*-findings.md`, and fails if the path is absent from the closure SHA's
tree **and** absent at HEAD (`if ! [ -e "$p_clean" ]`, line 307). Deleting or
**moving** such a path can turn `doc-lint` RED.

Enumerating the 17 walked review files, the only `scripts/`+`packaging/` paths
cited are: **`scripts/build-xdp.sh`**, **`scripts/verify-xdp.sh`**
(`audit/ebpf/round-2-review.md:490`, inside a `Recommendation:` block), and
**`packaging/expressgateway.service`**. All three are marked GATE below. A
fourth, `scripts/ci/atomic-lint.sh`, is cited only inside a `Status:` HTML
comment (not a Recommendation) and **already does not exist at HEAD** — that is
fine and must not be "fixed" by re-creating it.

**Baseline established (read-only):** `bash scripts/ci/doc-lint.sh` → tier-1 OK,
tier-2 OK (52 Verified-Fixed claims checked), exit 0. Re-run after the sweep.

---

## CALLER MATRIX (every script/one-off file)

### Root `run-*.sh` — the 20 S37 lead-harness scripts

`run-baseline.sh | 42 | callers: NONE (git grep -F run-baseline.sh across all tracked files → 0 hits outside itself) | DELETE | cd .claude/worktrees/s36-verify (dir absent) || exit 99; CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target (absent). S37 Phase-0 one-off; its output audit/ops/s37-phase0/ is already committed (5 files).`
`run-ci-watch.sh | 10 | callers: NONE | DELETE | Watches PR #228 (long closed) + cd's into the absent s36-verify worktree. Pure session scaffolding.`
`run-cov-full.sh | 35 | callers: NONE | DELETE | cd s36-verify || exit 99. One-off S36-A coverage on SHA 4480fb83; output audit/ops/s36-aimpl-cov/ committed (4 files). Superseded by CI job `coverage`.`
`run-cov.sh | 36 | callers: NONE | DELETE | cd s36-verify || exit 99. Scoped variant of the above, same dead worktree, same committed output.`
`run-d-build.sh | 24 | callers: NONE | DELETE | cd .claude/worktrees/s37-deps (absent) || exit 99. S37-D stage-2 dep compile-confirm; writes into the absent s36-verify worktree. Output dir audit/ops/s37-d-lead is NOT tracked (0 files).`
`run-d-clippy.sh | 16 | callers: NONE | DELETE | cd s37-deps || exit 99. Same dead pair of paths.`
`run-d-gate.sh | 29 | callers: NONE | DELETE | cd s37-deps || exit 99. S37-D final self-gate; pins reqwest 0.12.28 by hand.`
`run-d-reqwest.sh | 24 | callers: NONE | DELETE | cd s37-deps || exit 99. S37-D stage-6 reqwest-0.13 trial — the trial was REJECTED (reqwest held at 0.12, Cargo.toml:195).`
`run-d-tokio-revert.sh | 28 | callers: NONE | DELETE | cd s37-deps || exit 99. The tokio-1.52 attribution repro. Its finding is already durable in Cargo.toml:73-78 (the ">=1.51, <1.52" hold + CF-S37-D-TOKIO-1.52-RELAY rationale) — no provenance is lost.`
`run-d6rerun-watch.sh | 9 | callers: NONE | DELETE | Hardcodes CI run id 27145437614 (a completed 2026-06 run) in 4 places. Confirms lead's example.`
`run-fcap-isolate.sh | 24 | callers: NONE | DELETE | cd .claude/worktrees/s37-verify (absent) || exit 99. Output audit/ops/s37-verifyC-lead/ committed (5 files).`
`run-lint.sh | 13 | callers: NONE | DELETE | cd s36-verify || exit 99. Confirms lead's example. Superseded by CI jobs fmt+clippy.`
`run-main-ci-watch.sh | 13 | callers: NONE | DELETE | Hardcodes run ids 27145437741 + 27145437614 (both completed).`
`run-p4-x3.sh | 32 | callers: NONE | DELETE | cd s36-verify || exit 99. Output dir audit/ops/s37-p4 NOT tracked (0 files).`
`run-p4-x3f.sh | 32 | callers: NONE | DELETE | Byte-identical to run-p4-x3.sh except the OUT path (see DUP-1). Output dir audit/ops/s37-p4f NOT tracked (0 files).`
`run-resoak.sh | 34 | callers: NONE (anchored grep '(^|[^/-])run-resoak\.sh' → 0 hits; the s40-doc-inventory hit is the unrelated audit/soak/s33-run-resoak.sh) | DELETE | cd s36-verify || exit 99. Its 12-scenario batching is superseded and single-sourced by scripts/soak/release-soak-onbox.sh; its output audit/soak/s37-soak-data/ is committed (39 files).`
`run-watchdog.sh | 22 | callers: NONE | DELETE | GIT=.claude/worktrees/s36-verify (absent), TGT=/home/ubuntu/Code/eg-target (absent); polls branches s37-b-config / s37-d-deps. The durable lesson is the memory feedback-watchdog-every-long-job, not this file.`
`run-x3.sh | 29 | callers: NONE | DELETE | cd s36-verify || exit 99. Output audit/ops/s36-aimpl-x3/ committed (2 files).`
`run-x3-takeover.sh | 48 | callers: NONE | DELETE | cd s37-verify || exit 99. Output audit/ops/s37-verifyB-lead/ committed (3 files).`
`run-x3c-takeover.sh | 48 | callers: NONE | DELETE | Differs from run-x3-takeover.sh in exactly 2 lines: the header sentence and the OUT path (see DUP-2).`

### `scripts/ci/**` — CI-invoked, gate-read

`scripts/ci/coverage-check.sh | 173 | callers: .github/workflows/ci.yml:278 `run: bash scripts/ci/coverage-check.sh coverage.lcov`; CONTRIBUTING.md:42; docs/arch/DEV-SETUP.md:87; docs/arch/extending.md:54; run-cov-full.sh:19 | KEEP (GATE) | D-6 verdict. Its hot-path module list + the loader.rs carve-out + the S44 merged-DA metric are hardcoded IN THE SCRIPT — content is gate-read.`
`scripts/ci/doc-lint.sh | 379 | callers: .github/workflows/ci.yml:106 `run: bash scripts/ci/doc-lint.sh`; CONTRIBUTING.md:41 | KEEP (GATE) | Tier-1 FILES=() array (19 entries) + STALE_PATTERNS + the `doc-lint-allow` marker are all gate-read. Tier-2 pins scripts//packaging/ paths at HEAD (see hazard above).`
`scripts/ci/docker-smoke.sh | 164 | callers: .github/workflows/ci.yml:428 `IMAGE=expressgateway:ci bash scripts/ci/docker-smoke.sh`; docs/arch/DEV-SETUP.md:89; docs/arch/extending.md:56; docs/guide/getting-started.md:18; docker/smoke/gateway.toml:3 | KEEP (GATE) | image-scan job; proves the container boots AND serves.`
`scripts/ci/h3spec-check.sh | 116 | callers: .github/workflows/ci.yml:375 `bash scripts/ci/h3spec-check.sh ./h3spec 127.0.0.1 8444`; CONTRIBUTING.md:43; docs/arch/DEV-SETUP.md:88; docs/arch/extending.md:55; docs/arch/security-and-conformance.md:93; docs/features.md:148; docs/known-limitations.md:169 | KEEP (GATE) | The 12 named CF-QUICHE-UPGRADE waiver strings + MIN_EXAMPLES=40 are gate-read. (Header prose has one stale filename — see AMB-10.)`

### `scripts/` top level

`scripts/build-xdp.sh | 110 | callers: .gitattributes:1; .github/workflows/ci.yml:458 (comment: "the committed ELF (built by scripts/build-xdp.sh)"); CHANGELOG.md:185; audit/ebpf/plans/EBPF-2-01.md:5,6,28,44; EBPF-2-03.md:71; EBPF-2-07.md:89; audit/ebpf/verifier-logs/README.md:39; audit/round-8/fixes/ROUND8-L4-10.md:56,97 (+9 more) | KEEP (GATE) | Produces crates/lb-l4-xdp/src/lb_xdp.bin, which the xdp-smoke job loads. Also doc-lint tier-2 HEAD-pinned.`
`scripts/halting-gate.sh | 58 | callers: manifest/required-artifacts.txt:9 (self-listed); SECURITY.md:25; docs/architecture.md:246; docs/arch/extending.md:67,183; ADR-0001:137; ADR-0003:87; ADR-0010:89,111,188,209 | AMBIGUOUS (KEEP + flag) | Heavily documented, therefore user-facing → KEEP. BUT measured RED today: see AMB-1.`
`scripts/never_decrypted_proof.sh | 156 | callers: crates/lb-quic/Cargo.toml:83,108; crates/lb/Cargo.toml:103; crates/lb/src/main.rs:1898; crates/lb-quic/examples/passthrough_linkage_probe.rs:2; audit/quic/s15-a2-verify-evidence.md:13,55 | KEEP | Cited from PRODUCTION source and from a live `examples/` target that exists solely to feed it. S40 proposed archiving it and the S40 report (line 60) records the decision NOT to. Leave in place.`
`scripts/periodic-clean.sh | 37 | callers: NONE as an invocation (audit/h3-program/s1-report.md:123 records a session cron; audit/round-9/verify-proto-2-12.md:33 is a git-status mention) | KEEP | Generic, non-session-scoped maintenance (git worktree prune + cargo clean). It is the executable form of the memory rule disk-cleanup-loop-must-not-race-builds — the pgrep build-in-flight guard (lines 15-20) and the <5-min target/ mtime guard (line 23) ARE that lesson. Reusable; deleting it re-opens the dep-graph-corruption class.`
`scripts/release-soak.sh | 173 | callers: .github/workflows/release.yml:66 `run: bash scripts/release-soak.sh`; release.yml:35,39; ci.yml:385; docs/arch/DEV-SETUP.md:105,151,161,165; docs/arch/extending.md:73 | KEEP (GATE) | The release soak gate controller (OIDC → EC2 → soak → verdict → teardown).`
`scripts/verify-xdp.sh | 221 | callers: audit/ebpf/round-2-review.md:490 (inside a Recommendation: block → doc-lint tier-2 HEAD-pinned); audit/ebpf/plans/EBPF-2-07.md:7,42,46,90; audit/ebpf/verifier-logs/README.md:4,42,43,44,56; audit/ebpf/verifier-logs/{5.15,6.1,6.6}.log.committed:1,9,12; audit/FINAL_REPORT.md:147; audit/foundation-pass/ESCALATION-F-ESC-1.md:9,28 | KEEP (GATE) | Not CI-invoked (hosted runners lack nested virt — F-ESC-1) but doc-lint tier-2 HEAD-pins the path AND the three committed verifier-log baselines name it as their regeneration command. Do not move.`

### `scripts/soak/**`

`scripts/soak/run-soak.sh | 65 | callers: scripts/soak/release-soak-onbox.sh:50; scripts/perf/s39-burnin.sh:27; audit/soak/s36-sc9-reference.sh:28; docs/arch/DEV-SETUP.md:95,100,158; run-resoak.sh:10 | KEEP (GATE) | The single-sourced soak driver behind the release gate + a documented developer entry point.`
`scripts/soak/soak-verdict.sh | 84 | callers: scripts/soak/release-soak-onbox.sh:54; docs/arch/DEV-SETUP.md:101,159 | KEEP (GATE) | The release gate's PASS/FAIL verdict (all-BOUNDED + panic=0).`
`scripts/soak/release-soak-onbox.sh | 79 | callers: scripts/release-soak.sh:97 `bash scripts/soak/release-soak-onbox.sh ...`; release-soak.sh:18; .gitignore:41; docs/arch/DEV-SETUP.md:117,157 | KEEP (GATE) | Runs ON the provisioned soak EC2.`
`scripts/soak/s20-run.sh | 63 | callers: NONE as an invocation; referenced by audit/soak/s21-handoff.md:8 ("use scripts/soak/s20-run.sh with the reduced concurrencies") — a historical handoff note, not a caller | ARCHIVE | S20-specific run definition ("the exact, reproducible run definition") with the S20 Mode-B 4-stream/1-stream split baked in. Provenance for the S20 soak numbers; superseded operationally by run-soak.sh.`
`scripts/soak/s21-run.sh | 73 | callers: NONE as an invocation; audit/soak/s21-report.md:157 cites it as how the S21 batches were run | ARCHIVE | Encodes the S21 sequential-batch rationale (the S20 run1 oversubscription anti-pattern). Provenance for the shippable-v1 soak.`
`scripts/soak/s21-gate.sh | 28 | callers: NONE as an invocation; audit/soak/s21-report.md:230 cites it as the gate that produced the S21 tree verdict | ARCHIVE | S21 Phase-4 regression gate with the debuginfo=0 disk workaround. Provenance for a published gate result.`

### `scripts/perf/**` — S39 measurement drivers

`scripts/perf/s39-sweep.sh | 37 | callers: NONE (no workflow, script, doc or Cargo reference; only its own output header appears, at audit/perf/s39-bench-data/sweep-summary.txt:1 "[s39-sweep ...] protocols=... concs=1 8 32 64 128") | ARCHIVE | Direct provenance: this script wrote the committed sweep-summary.txt whose numbers reach the public docs via audit/perf/s39-report.md → docs/guide/PERFORMANCE.md. Delete it and the published RPS/latency table loses its repro.`
`scripts/perf/s39-burnin.sh | 38 | callers: NONE (its output audit/perf/s39-burnin/burnin-verdicts.txt:13 self-identifies as "[s39-burnin] BOUNDED=11 DRIFT=1 of 12") | ARCHIVE | Produced the 4-hour 12-scenario burn-in cited by docs/arch/backpressure.md:115,125 and docs/guide/PERFORMANCE.md:8,181 (via audit/perf/s39-burnin.md). Highest-value provenance in this group.`
`scripts/perf/s39-oha.sh | 70 | callers: NONE | ARCHIVE | The author≠verifier cross-validation of eg-bench against the external `oha` client — the argument for trusting the H3/QUIC numbers oha cannot measure. Repro of a published credibility claim.`
`scripts/perf/s39-x3.sh | 25 | callers: scripts/perf/s39-gate-feasible.sh:17 (comment: "run the full s39-x3.sh instead") | ARCHIVE | S39 R1 ×3 gate. Move together with s39-gate-feasible.sh so the cross-reference stays valid.`
`scripts/perf/s39-gate-feasible.sh | 48 | callers: NONE | ARCHIVE | Records WHY the S39 gate was reduced (CF-DISK-1: the ×3 needs ~38-40G, the box had ~37G) and what was run instead. That rationale is the provenance for a weakened-looking gate — must not be lost.`

### `scripts/archive/**` — the S40 precedent (already archived)

`scripts/archive/s26-gate.sh | 22 | callers: NONE | KEEP | Already at the archive destination.`
`scripts/archive/s26-h3spec.sh | 67 | callers: NONE | KEEP | Already archived.`
`scripts/archive/s26-phase3-gate.sh | 43 | callers: NONE | KEEP | Already archived.`
`scripts/archive/s31-cov.sh | 28 | callers: NONE | KEEP | Already archived.`
`scripts/archive/s31-gate.sh | 57 | callers: audit/deps/s31-report.md:22,36,218 (cited by its ORIGINAL pre-archive path `scripts/s31-gate.sh`) | KEEP | Already archived; the stale citation path is an audit-file matter, out of scope.`
`scripts/archive/s31-h3spec.sh | 62 | callers: audit/deps/s31-report.md:238 (old path) | KEEP | Already archived.`
`scripts/archive/s31-phase2-reproofs.sh | 72 | callers: audit/deps/s31-report.md:264 (old path) | KEEP | Already archived.`

### `.github/**`

`.github/workflows/ci.yml | 469 | callers: manifest/required-artifacts.txt:11; branch protection (required status checks) | KEEP (GATE) | 16 jobs. No commented-out steps found (see DUP/notes).`
`.github/workflows/release.yml | 174 | callers: manifest/required-artifacts.txt:12; tag push v*; workflow_dispatch | KEEP (GATE) | Soak gate + publish pipeline.`
`.github/workflows/scheduled.yml | 75 | callers: cron '54 10 * * 0'; workflow_dispatch | KEEP (GATE) | Weekly audit/geiger/machete.`
`.github/actions/rust-setup/action.yml | 47 | callers: ci.yml (×9 `uses: ./.github/actions/rust-setup`); scheduled.yml (×3) | AMBIGUOUS (KEEP + flag) | Live composite, R12 single-sourcing. Description names two workflows deleted at S40 — see AMB-10.`
`.github/actions/free-disk/action.yml | 21 | callers: ci.yml:122 (test job), ci.yml:264 (coverage job) | KEEP (GATE) | Prevents ENOSPC on the --all-features + instrumented builds.`
`.github/dependabot.yml | 31 | callers: GitHub Dependabot service | KEEP | The dtolnay/rust-toolchain exclude-patterns block (lines 26-31) is a load-bearing WHY-note (PR #214 bogus @1.100 bump).`

### Root dotfiles / config

`Cargo.toml | 235 | callers: cargo (workspace root, package lb-integration-tests); manifest/required-artifacts.txt:1 | KEEP (GATE) | Contains the load-bearing tokio <1.52 hold rationale (73-78), the reqwest-0.12 hold (189-195), the lb-compression + arc-swap removal notes, and the panic=abort CODE-2-02 note. All LOAD-BEARING under the standard.`
`Cargo.lock | — | callers: cargo; manifest/required-artifacts.txt:2 | KEEP | —`
`rust-toolchain.toml | 4 | callers: rustup; manifest/required-artifacts.txt:3; ci.yml:26-28 (RUST_MSRV mirrors it) | KEEP (GATE) | channel = 1.88.`
`deny.toml | 76 | callers: .github/workflows/ci.yml:242 `cargo-deny check licenses advisories bans sources`; manifest/required-artifacts.txt:4; .cargo/audit.toml:1-3 (declares itself a mirror) | KEEP (GATE) | Every ignore carries a justification; the dropped RUSTSEC-2026-0009 note (26-27) is a settled-decision record.`
`.cargo/audit.toml | 40 | callers: cargo-audit (ci.yml:225, scheduled.yml:36) | KEEP (GATE) | Waiver list with per-entry justification; must stay in sync with deny.toml.`
`.cargo/config.toml | 12 | callers: cargo (every build) | KEEP | BINDGEN_EXTRA_CLANG_ARGS — without it the BoringSSL/quiche build breaks. Behavioral.`
`.halting-gate.sha256 | 1 | callers: scripts/halting-gate.sh:52; manifest/required-artifacts.txt:10 | KEEP (GATE) | VERIFIED THIS SESSION: `cat manifest/required-artifacts.txt manifest/required-tests.txt | sort | sha256sum` = 8b4ef334… — matches. Any edit to either manifest file breaks check 7.`
`.gitattributes | 2 | callers: git; content references scripts/build-xdp.sh | KEEP | Marks the BPF ELF binary/non-diffable.`
`.gitignore | 44 | callers: git | KEEP | The fuzz/corpus re-include pair (lines 8-9) and the release-soak-out/ note (41-44) are load-bearing.`
`.dockerignore | 7 | callers: docker build (ci.yml:422, release.yml docker job) | KEEP | —`
`.trivyignore | 26 | callers: aquasecurity/trivy-action (ci.yml:430-436) | KEEP (GATE) | One CVE waiver with a written RE-REVIEW TRIGGER. Waiver list = gate-read.`

### `manifest/`, `packaging/`, `docker/`, `config/`, `bench/`, `fuzz/`, `tests/`

`manifest/required-artifacts.txt | 141 | callers: scripts/halting-gate.sh:36 (check 4) + :52 (check 7 hash); self-listed at line 7 | AMBIGUOUS (KEEP + flag) | 16 listed artifacts no longer exist — see AMB-2. Content is sha256-pinned: editing it without rehashing .halting-gate.sha256 turns check 7 RED.`
`manifest/required-tests.txt | 59 | callers: scripts/halting-gate.sh:46 (check 5) + :52 (check 7 hash); self-listed at line 8 | AMBIGUOUS (KEEP + flag) | 7 listed test fns no longer exist — see AMB-3. Same sha256 pinning.`
`packaging/expressgateway.service | 113 | callers: doc-lint tier-2 HEAD-pin (cited in a walked review file); docs/guide/DEPLOYMENT.md renders it | KEEP (GATE) | Operator-installable unit. Two false gate-claims in its header — see AMB-9. The commented-out `#WatchdogSec=15s` (line 110) is LOAD-BEARING commented config, not slop: lines 106-109 state the deferred-because reason. Do NOT strip.`
`docker/Dockerfile | 3967 B | callers: .github/workflows/ci.yml:422; release.yml:148; manifest/required-artifacts.txt:13 | KEEP (GATE) | Built + smoke-tested + Trivy-scanned every PR.`
`docker/Dockerfile.test | 4 | callers: manifest/required-artifacts.txt:14 ONLY (grep -F 'Dockerfile.test' → 1 non-audit hit, the manifest) | AMBIGUOUS (KEEP + flag) | See AMB-6: FROM rust:1.85-bookworm vs MSRV 1.88 — it cannot build the workspace. Manifest-pinned, so not freely deletable.`
`docker/smoke/gateway.toml | 1824 B | callers: scripts/ci/docker-smoke.sh (copied to /etc/expressgateway/config.toml; the file's own line 3 documents this) | KEEP (GATE) | image-scan input.`
`config/default.toml | — | callers: CONTRIBUTING.md, README.md, crates/lb-config/src/lib.rs, crates/lb/src/main.rs, docker/Dockerfile, docker/smoke/gateway.toml, docs/arch/DEV-SETUP.md, docs/arch/extending.md, docs/guide/{CONFIG,getting-started,troubleshooting}.md | KEEP | Shipped default + referenced from production source.`
`config/examples/*.toml (9 files) | — | callers: docs/guide/CONFIG.md cites all 9; getting-started.md cites h1/h1s; troubleshooting.md cites h1s-websocket | KEEP | Documented, user-facing.`
`bench/README.md | 70 | callers: NONE (audit/release/s40-doc-inventory.md:61 ruled "KEEP in place") | AMBIGUOUS (KEEP + flag) | See AMB-5: cites 3 paths that do not exist and a package name that does not exist.`
`bench/criterion/{h1_throughput,h2_throughput,h3_throughput,xdp_pps,compression}.rs | 10-11 each | callers: manifest/required-artifacts.txt:106-110 ONLY. NO [[bench]] target in any Cargo.toml (git grep 'bench' over all Cargo.toml → only [profile.bench], lb-soak's eg-bench bin, and fuzz's bench=false). No benches/ dir exists anywhere. | AMBIGUOUS (KEEP + flag) | See AMB-4: 5 unwired `fn main(){ eprintln!("… stub — not yet implemented") }` files. Slop-shaped but manifest+sha256 pinned.`
`fuzz/Cargo.toml + fuzz/fuzz_targets/*.rs (9) + fuzz/corpus/**/*.bin (28) + fuzz/rust-toolchain.toml + fuzz/Cargo.lock | — | callers: .github/workflows/ci.yml:183-195 (fuzz-smoke enumerates via `cargo +nightly fuzz list` and runs EVERY target) | KEEP (GATE) | 9 declared targets = 9 files on disk; corpus seeds are re-included by .gitignore:9 and warm every run. All live.`
`fuzz/findings/*.smoke.txt (5) | 40-109 each | callers: NONE (only fuzz/README.md:89 names the directory, not the files) | AMBIGUOUS (KEEP + flag) | See AMB-8.`
`fuzz/README.md | — | callers: audit/release/s40-doc-inventory.md:61 ("KEEP in place") | AMBIGUOUS (KEEP + flag) | See AMB-7.`
`tests/*.rs (90 files) | — | callers: cargo auto-discovery for package lb-integration-tests → run by .github/workflows/ci.yml:133 `cargo test --workspace --all-features`; ci.yml:145 names tests/h2h1_md_streaming_verify.rs explicitly; 54 of them are pinned by manifest/required-artifacts.txt | KEEP (GATE) | VERIFIED: every one of the 90 contains at least one #[test]/#[tokio::test] — zero vacuous or orphan test files. Session-named files (s14_cfbw_h1h1.rs, round8_drain_15case.rs, h3_s3_inflight_h1_drain_proof.rs, h2h1_md_coverage_driver.rs) all compile and run in CI; they are live tests, not scaffolding. Their CONTENT is the crates-scanners' concern, not mine.`

---

## PROPOSED DELETIONS

All 20 are the S37 lead harness. Every one has zero referencing files anywhere
in the repo, and every one is additionally *inert* — it cannot execute, because
the worktree or the CI object it targets no longer exists. Where a script
produced committed evidence, that evidence stays (it lives under `audit/`); only
the launcher goes.

`DEL | run-baseline.sh | 0 callers. cd .claude/worktrees/s36-verify || exit 99 — worktree dir is empty (verified). Output audit/ops/s37-phase0/ already committed.`
`DEL | run-ci-watch.sh | 0 callers. Watches closed PR #228; cd's into the same absent worktree.`
`DEL | run-cov-full.sh | 0 callers. cd absent worktree || exit 99. Superseded by CI job `coverage`; output audit/ops/s36-aimpl-cov/ committed.`
`DEL | run-cov.sh | 0 callers. cd absent worktree || exit 99. Scoped duplicate of the above.`
`DEL | run-d-build.sh | 0 callers. cd .claude/worktrees/s37-deps (absent) || exit 99; writes into a second absent worktree.`
`DEL | run-d-clippy.sh | 0 callers. cd s37-deps (absent) || exit 99.`
`DEL | run-d-gate.sh | 0 callers. cd s37-deps (absent) || exit 99.`
`DEL | run-d-reqwest.sh | 0 callers. cd s37-deps (absent) || exit 99. Trialled reqwest 0.13, which was REJECTED — the decision is recorded in Cargo.toml:189-195.`
`DEL | run-d-tokio-revert.sh | 0 callers. cd s37-deps (absent) || exit 99. Its finding is preserved verbatim in Cargo.toml:73-78 (tokio ">=1.51, <1.52" + CF-S37-D-TOKIO-1.52-RELAY).`
`DEL | run-d6rerun-watch.sh | 0 callers. Hardcodes completed CI run 27145437614.`
`DEL | run-fcap-isolate.sh | 0 callers. cd .claude/worktrees/s37-verify (absent) || exit 99. Output audit/ops/s37-verifyC-lead/ committed.`
`DEL | run-lint.sh | 0 callers. cd s36-verify (absent) || exit 99. Superseded by CI fmt + clippy jobs.`
`DEL | run-main-ci-watch.sh | 0 callers. Hardcodes completed runs 27145437741 + 27145437614.`
`DEL | run-p4-x3.sh | 0 callers. cd s36-verify (absent) || exit 99. Its OUT dir audit/ops/s37-p4 was never committed.`
`DEL | run-p4-x3f.sh | 0 callers. One-line diff from run-p4-x3.sh (OUT path). OUT dir never committed.`
`DEL | run-resoak.sh | 0 callers (anchored grep). cd s36-verify (absent) || exit 99. Superseded by scripts/soak/release-soak-onbox.sh; output audit/soak/s37-soak-data/ committed (39 files).`
`DEL | run-watchdog.sh | 0 callers. Watches an absent worktree + an absent target dir + two S37 feature branches.`
`DEL | run-x3.sh | 0 callers. cd s36-verify (absent) || exit 99. Output audit/ops/s36-aimpl-x3/ committed.`
`DEL | run-x3-takeover.sh | 0 callers. cd s37-verify (absent) || exit 99. Output audit/ops/s37-verifyB-lead/ committed.`
`DEL | run-x3c-takeover.sh | 0 callers. Two-line diff from run-x3-takeover.sh. Output audit/ops/s37-verifyC-lead/ committed.`

**Sweeper note:** none of the 20 appears in a doc-lint tier-2 Recommendation
block (the walked-file scan found only `scripts/build-xdp.sh`,
`scripts/verify-xdp.sh`, `packaging/expressgateway.service`,
`scripts/ci/atomic-lint.sh`), so deleting them cannot turn doc-lint RED.
Deleting them also cannot affect coverage — no `.rs` is touched.

---

## PROPOSED ARCHIVE (move to `scripts/archive/`, not delete)

Rationale, per the S40 precedent already realised in `scripts/archive/`
(s26-*/s31-*): these are session-scoped drivers that document **how a published
number was produced**. Unlike the root 20, they still *run* (they `cd` to the
real repo root, not a deleted worktree) — but nothing calls them and their live
successors exist. Archiving keeps the repro off the active `scripts/` path.

`ARC | scripts/perf/s39-burnin.sh | Produced audit/perf/s39-burnin/ (BOUNDED=11 DRIFT=1 of 12) → audit/perf/s39-burnin.md, which docs/arch/backpressure.md:115,125 and docs/guide/PERFORMANCE.md:8,181 cite as the public 4-hour burn-in evidence. The 12-scenario ALL12 array + scale=1 concurrency rationale is the repro.`
`ARC | scripts/perf/s39-sweep.sh | Wrote audit/perf/s39-bench-data/sweep-summary.txt (its own "[s39-sweep …] protocols=… concs=1 8 32 64 128" header). Carries the co-location caveat and the box-independent CPU-us/request metric definition behind the published throughput table.`
`ARC | scripts/perf/s39-oha.sh | The author≠verifier cross-check: eg-bench's H1/H2 numbers validated against the external `oha` client. This is the stated basis for trusting the H3/QUIC numbers oha cannot produce — the credibility argument's repro.`
`ARC | scripts/perf/s39-x3.sh | The S39 R1 ×3 baseline gate. Referenced by s39-gate-feasible.sh:17; move both together so that comment stays valid.`
`ARC | scripts/perf/s39-gate-feasible.sh | Records WHY the S39 ×3 gate was reduced (CF-DISK-1: ~38-40G needed vs ~37G free, and the operator's Java caches were deliberately NOT deleted) and exactly what replaced it. A weakened-looking gate's justification — high knowledge value, zero operational value.`
`ARC | scripts/soak/s20-run.sh | "The exact, reproducible run definition" for the S20 soak, including the Mode-B 4-stream vs 1-stream split that exposed the multi-stream relay stall. Provenance for the S20 findings.`
`ARC | scripts/soak/s21-run.sh | The S21 sequential-batch design + its written justification (co-locating 8 scenarios measures the BOX not the GATEWAY — the S20 run1 anti-pattern). Provenance for the shippable-v1 soak (audit/soak/s21-report.md:157).`
`ARC | scripts/soak/s21-gate.sh | The S21 Phase-4 regression gate incl. the debuginfo=0 disk workaround and its "not a test weakening" justification. Cited at audit/soak/s21-report.md:230.`

**Move-safety check (done):** no workflow, script, `Cargo.toml`, `Dockerfile`,
`README`/`CONTRIBUTING`, or `docs/**` file references any `scripts/perf/*` or
`scripts/soak/s2*` path. The only references are historical audit prose, which
is out of scope and which the S40 precedent already accepted going stale for
`s26-*`/`s31-*`. None is doc-lint tier-2 HEAD-pinned.

---

## UNTOUCHABLE (gate-read / CI-invoked)

`GATE | scripts/ci/doc-lint.sh | ci.yml `doc-lint` job (line 106, fetch-depth: 0). FILES=() 19 entries, STALE_PATTERNS 9 rows, the `doc-lint-allow` marker, and the LOCATION_DIRS map are all read BY the gate.`
`GATE | scripts/ci/coverage-check.sh | ci.yml `coverage` job (line 278). Hot-path module list, the named loader.rs carve-out, and the S44 merged-DA scoring rule live in the script body.`
`GATE | scripts/ci/h3spec-check.sh | ci.yml `conformance` job (line 375). The 12 verbatim CF-QUICHE-UPGRADE waiver strings + MIN_EXAMPLES=40 are the gate.`
`GATE | scripts/ci/docker-smoke.sh | ci.yml `image-scan` job (line 428), between docker build and Trivy.`
`GATE | scripts/release-soak.sh | release.yml `soak-gate` job (line 66).`
`GATE | scripts/soak/release-soak-onbox.sh | invoked by scripts/release-soak.sh:97 on the provisioned EC2.`
`GATE | scripts/soak/run-soak.sh | invoked by release-soak-onbox.sh:50; documented entry point in docs/arch/DEV-SETUP.md:95-100.`
`GATE | scripts/soak/soak-verdict.sh | invoked by release-soak-onbox.sh:54 — the all-BOUNDED + panic=0 verdict.`
`GATE | scripts/build-xdp.sh | produces crates/lb-l4-xdp/src/lb_xdp.bin, loaded by ci.yml `xdp-smoke` (lines 455-468); named in .gitattributes:1; doc-lint tier-2 HEAD-pinned.`
`GATE | scripts/verify-xdp.sh | doc-lint tier-2 HEAD-pinned via a Recommendation: block at audit/ebpf/round-2-review.md:490; also the documented regeneration command inside all three audit/ebpf/verifier-logs/*.log.committed baselines. Do not move or rename.`
`GATE | packaging/expressgateway.service | doc-lint tier-2 HEAD-pinned; rendered by docs/guide/DEPLOYMENT.md; release asset.`
`GATE | .github/workflows/{ci,release,scheduled}.yml | the gates themselves; ci.yml + release.yml also pinned by manifest/required-artifacts.txt:11-12.`
`GATE | .github/actions/rust-setup/action.yml | 12 `uses:` sites across ci.yml + scheduled.yml.`
`GATE | .github/actions/free-disk/action.yml | ci.yml `test` (122) + `coverage` (264); removing it re-opens ENOSPC.`
`GATE | deny.toml | cargo-deny check (ci.yml:242). Ignore list = the gate.`
`GATE | .cargo/audit.toml | cargo audit -D warnings (ci.yml:225, scheduled.yml:36). Ignore list = the gate; declared mirror of deny.toml.`
`GATE | .trivyignore | trivy-action (ci.yml:430-436). Waiver list = the gate.`
`GATE | .halting-gate.sha256 + manifest/required-artifacts.txt + manifest/required-tests.txt | halting-gate.sh checks 4/5/7. The sha256 is over the CONCATENATED SORTED CONTENT of the two manifests — verified matching this session. ANY line added to or removed from either manifest turns check 7 RED unless the hash is regenerated in the same commit.`
`GATE | docker/Dockerfile + docker/smoke/gateway.toml | ci.yml `image-scan` + release.yml `docker`.`
`GATE | fuzz/Cargo.toml + fuzz/fuzz_targets/** + fuzz/corpus/** + fuzz/rust-toolchain.toml | ci.yml `fuzz-smoke` enumerates targets dynamically (`cargo +nightly fuzz list`) and fails closed if the list is empty (lines 186-190). Deleting a target file silently shrinks the gate; deleting a corpus seed cools the fuzzer.`
`GATE | tests/*.rs (all 90) | ci.yml `test` job runs the whole set; 54 are additionally pinned by manifest/required-artifacts.txt (halting-gate check 4).`
`GATE | Cargo.toml / Cargo.lock / rust-toolchain.toml | build + MSRV; the first three manifest-pinned.`
`GATE | .cargo/config.toml | BINDGEN_EXTRA_CLANG_ARGS — required for the BoringSSL/quiche build to resolve stddef.h. Behavioral, not cosmetic.`

---

## AMBIGUOUS (KEEP + flag for owner)

`AMB-1 | scripts/halting-gate.sh | KEEP (documented in SECURITY.md:25, docs/architecture.md:246, docs/arch/extending.md:67,183, and ADR-0001/0003/0010 — user-facing), but it is provably RED today and has been for many sessions. Measured this session: check 4 reports 16 MISSING required artifacts (crates/lb-h3/*, crates/lb-compression/*, tests/conformance_h{1,2,3}.rs, the 7 tests/compression_*.rs, docs/gap-analysis.md, docs/FINAL_REPORT.md); check 5 would fail on 7 missing test fns (AMB-3); and its check-7 remediation message points at docs/manifest-drift-proposal.md, which does not exist. Check 7 itself is GREEN (hash verified). No workflow runs it, so nothing is currently breaking — but the docs describe it as an enforcement mechanism it no longer performs. OWNER DECISION: re-baseline the two manifests + regenerate .halting-gate.sha256 in one commit, or downgrade the docs' claims. Out of a de-slop session's remit either way.`
`AMB-2 | manifest/required-artifacts.txt | KEEP. 16 of 141 entries name deleted files (see AMB-1). Cannot be edited in isolation: the content is sha256-pinned by .halting-gate.sha256 (check 7), so a fix must rehash in the same commit. Flagging, not proposing.`
`AMB-3 | manifest/required-tests.txt | KEEP. 7 of 59 entries name test fns that exist nowhere in tests/ or crates/: test_compression_{zstd,brotli,gzip,deflate}_roundtrip, test_compression_transcode_gzip_to_zstd, test_compression_bomb_cap_fires, test_compression_breach_posture_no_leak — all fallout from the CODE-2-15/L-001 lb-compression removal recorded in Cargo.toml:38-44. Same sha256 pinning as AMB-2.`
`AMB-4 | bench/criterion/{h1_throughput,h2_throughput,h3_throughput,xdp_pps,compression}.rs | KEEP. Five 10-11 line files whose entire body is `fn main() { // TODO: wire up criterion benchmarks …; eprintln!("… stub — not yet implemented"); }`. NOT wired as [[bench]] targets in any Cargo.toml; no benches/ dir exists in the workspace; cargo never compiles them. Known-deferred since round-7 (audit/round-7/SUMMARY.md:32 "bench/criterion/*.rs are stubs"). By the letter of the standard these are dead scaffolding — but they are pinned by manifest/required-artifacts.txt:106-110 (check 4) and therefore by the sha256 (check 7), so deleting them is a manifest change, not a file deletion. OWNER DECISION.`
`AMB-5 | bench/README.md | KEEP. Stale in four places: it points readers at crates/lb/benches/balancer_bench.rs (MISSING — no benches/ dir exists anywhere), bench/curl-format.txt (MISSING), docs/benchmark_results.md (MISSING), and `cargo bench -p expressgateway-lb` (no such package; the crate is `lb`, the binary `expressgateway`). It also does not mention that bench/criterion/*.rs are stubs, nor that the real perf evidence is audit/perf/s39-*. Not gate-read (absent from doc-lint's FILES array). This is a docs-content revision, not a de-slop deletion.`
`AMB-6 | docker/Dockerfile.test | KEEP. 4 lines; `FROM rust:1.85-bookworm` then `RUN cargo test --all --all-features`. MSRV moved 1.85 → 1.88 at S31, so this image cannot build the workspace. Zero callers outside manifest/required-artifacts.txt:14 — no workflow builds it. Dead in practice but manifest+sha256 pinned like AMB-4. OWNER DECISION: repin to 1.88, or retire it together with its manifest line.`
`AMB-7 | fuzz/README.md | KEEP. States "The main workspace is pinned to stable 1.85 for MSRV" (line 13) — stale since S31 (MSRV 1.88; rust-toolchain.toml channel = "1.88"). The rest of the file is accurate and its target table matches the 9 live targets. One-line factual correction, not slop.`
`AMB-8 | fuzz/findings/{h1_parser,h2_frame,h3_frame,quic_initial,tls_client_hello}.smoke.txt | KEEP. Five raw libFuzzer transcripts (40-109 lines each) checked in as evidence. No file cites them (only fuzz/README.md:89 names the directory). They were captured on a machine where the repo lived at /home/ubuntu/Programming/ExpressGateway — a path baked into every line — whereas the repo is now /home/ubuntu/Code/ExpressGateway, so they cannot be regenerated in place. They also cover only 5 of the 9 current targets. Genuine (if aged) fuzz provenance → KEEP, but flag: if the owner wants them, they should be refreshed for all 9 targets; if not, they belong under audit/.`
`AMB-9 | packaging/expressgateway.service | KEEP (GATE). Its header makes TWO claims about gates that do not exist: (a) lines 22-24, "systemd-analyze security … must score below 1.5; CI gate `systemd-analyze-security` enforces" — grep for systemd-analyze across .github/, scripts/, docs/, README, CONTRIBUTING returns ZERO hits, no such job exists in any of the 3 workflows; (b) lines 6-8, "the doc-lint job enforces that every directive named in DEPLOYMENT.md appears here" — doc-lint.sh does no such check (it only lists docs/guide/DEPLOYMENT.md in its tier-1 FILES array for stale-pattern grepping). A comment that claims a non-existent gate is worse than slop, because it tells a future editor the file is protected when it is not. Flagging for owner ruling — I am NOT proposing an edit, since correcting it means either building the gate or weakening the claim.`
`AMB-10 | .github/actions/rust-setup/action.yml (line 6) and scripts/ci/h3spec-check.sh (line 5) | KEEP. Both name workflows deleted in the S40 consolidation: the action's description says "Referenced by ci.yml, prod-readiness-gates.yml and scheduled-scans.yml" (only ci.yml and scheduled.yml exist), and h3spec-check.sh says h3spec is "pinned to the version in prod-readiness-gates.yml" (the pin is now ci.yml:293, H3SPEC_VER: "v0.1.13"). Both are one-word corrections and provably behavior-neutral — but h3spec-check.sh is gate-read, so I am flagging rather than proposing. NOTE the contrast: ci.yml:6 and scheduled.yml:8,25 also name the old workflows, but correctly and in the past tense ("were merged here", "renamed from") — those are LOAD-BEARING provenance and must NOT be touched.`

---

## DUPLICATED BOILERPLATE

Every DUP below lives inside files already proposed for DELETE or ARCHIVE, so
the recommended resolution is "deletion/archival subsumes it" — do **not** spend
a de-slop session refactoring scripts that are leaving.

`DUP-1 | run-p4-x3.sh, run-p4-x3f.sh | Byte-identical except line 7 (OUT=…/s37-p4 vs …/s37-p4f). `diff` reports exactly 1 changed line out of 32. Neither OUT dir was ever committed. | Subsumed: both are in PROPOSED DELETIONS.`
`DUP-2 | run-x3-takeover.sh, run-x3c-takeover.sh | Differ in exactly 2 lines of 48: the header sentence (B vs C binding) and OUT (s37-verifyB-lead vs s37-verifyC-lead). Includes a fully duplicated 10-line inline python LCOV parser. | Subsumed: both in PROPOSED DELETIONS.`
`DUP-3 | run-cov.sh, run-cov-full.sh, run-x3-takeover.sh, run-x3c-takeover.sh | The same inline python3 heredoc LCOV parser (SF:/DA: accumulate → per-file percent) is copy-pasted 4×. Note this logic is ALREADY single-sourced properly in scripts/ci/coverage-check.sh, which is the gate. | Subsumed: all 4 in PROPOSED DELETIONS. The canonical implementation (coverage-check.sh) stays.`
`DUP-4 | 13 files: run-{baseline,cov,cov-full,d-build,d-gate,d-reqwest,p4-x3,p4-x3f,resoak,watchdog,x3,x3-takeover,x3c-takeover}.sh | The identical helper pair `st(){ date -u +%H:%M:%S; }` and `fg(){ df --output=avail -BG /dev/root|tail -1|tr -dc 0-9; }`. | Subsumed: all 13 are in PROPOSED DELETIONS. No shared lib needed.`
`DUP-5 | 32 files (18 with CARGO_BUILD_JOBS=4) | `export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target [CARGO_BUILD_JOBS=4]` is repeated across root run-*.sh (dying), scripts/archive/* (already archived), scripts/perf/* + scripts/soak/s2* (proposed ARCHIVE). AFTER the sweep only 2 non-archived files retain it: scripts/soak/run-soak.sh:27 and scripts/soak/s21-gate.sh (also archiving) — and run-soak.sh's is only inside a `: "${CARGO_TARGET_DIR:?…}"` ERROR-MESSAGE hint, not a hardcode. | No action. Explicitly verified NOT a release-gate bug: release-soak.sh:96 exports CARGO_TARGET_DIR=/opt/eg-target and release-soak-onbox.sh:30 defaults it to $REPO_ROOT/../eg-target, so the EC2 soak path never touches the local box path. Do not "fix" this.`
`DUP-6 | 10 files: run-{baseline,d-gate,fcap-isolate,p4-x3,p4-x3f,x3,x3-takeover,x3c-takeover}.sh + 2 archived | The ×3 loop with the `grep -hE "test result:" | awk '{p+=$4;f+=$6;ig+=$8}'` summarizer + the disk-headroom abort guard. | Subsumed by deletion. The surviving equivalent is the CI `test` job.`
`DUP-7 | scripts/archive/s26-gate.sh vs s31-gate.sh (75 diff lines), s26-h3spec.sh vs s31-h3spec.sh (46 diff lines) | Per-session forks of the same gate/h3spec driver. | NO ACTION — this is the intended shape of an archive: each file is a frozen record of what that session actually ran. Deduplicating them would destroy the provenance the archive exists to hold.`

---

## Notes for the lead

1. **No commented-out code in the workflows.** I grepped
   `.github/workflows/*.yml` + `.github/actions/*/action.yml` for
   `^\s*#\s*(-|name:|run:|uses:|with:|if:|steps:|jobs:)`. All 8 hits are prose
   (`# --all-features is REQUIRED…`, `# --ignore-run-fail: …`, `# -k: skip cert
   validation…`) or `# ------` section rules — exactly the false-positive class
   the standard's calibration table predicts. The one genuine commented-out
   directive in my scope, `#WatchdogSec=15s` in the systemd unit, is
   LOAD-BEARING (a deferred-because note names the Wave-2 commit that re-enables
   it; I confirmed no `sd_notify(WATCHDOG=1)` heartbeat exists in crates/ or
   tests/ yet, so the note is still accurate).
2. **No stray or orphan files at root.** `git status --porcelain` shows only
   the untracked `audit/craft/` (this session's own directory). Every one of the
   29 tracked root entries is accounted for above.
3. **Root `tests/` is clean.** All 90 files carry at least one test attribute;
   none is an orphan, a stub, or unreachable by cargo. I made no judgement about
   their *contents* — that belongs to whichever scanner owns test comments.
4. **Coverage impact of my proposals: zero.** No `.rs` file is proposed for
   deletion or movement. The only `.rs` files in my scope are `tests/*.rs`
   (KEEP), `fuzz/**` (KEEP/GATE), and `bench/criterion/*.rs` (AMBIGUOUS, KEEP,
   and not compiled by cargo at all).
5. **Post-sweep verification I recommend:** re-run `bash scripts/ci/doc-lint.sh`
   (baseline this session: exit 0, tier-2 52 claims) and re-verify
   `cat manifest/required-artifacts.txt manifest/required-tests.txt | sort |
   sha256sum` still equals `.halting-gate.sha256` (baseline: 8b4ef334…, matching).
   Both are seconds-cheap and both are the exact things a careless sweep breaks.
6. **Coverage gap to confirm:** my brief scoped me to "everything NOT crates/**
   and NOT audit/**" but enumerated specific directories that omit `docs/**` and
   the root `*.md` (README, CHANGELOG, CONTRIBUTING, SECURITY, LICENSE). I read
   those as *reference* sources only and did not classify them. If no other
   scanner owns them, they are currently unscanned — 66 files.
