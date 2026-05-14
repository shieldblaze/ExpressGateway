# Round 7 — Gate Matrix Summary

Branch: prod-readiness/round-4
Worktree: audit/round-7-gates-45 at ecc7771
Toolchain: cargo 1.85.1

Sandbox limits: no network for cargo install; no privileged Docker;
no real NICs; no nightly toolchain.

## Result table

| #  | Gate                                       | Status   | Output file                                   | Notes |
| -- | ------------------------------------------ | -------- | --------------------------------------------- | ----- |
| 1  | cargo fmt --check                          | PASS     | gate-outputs/fmt.txt                          | zero diff |
| 2  | cargo clippy --workspace --all-targets     | FAIL     | gate-outputs/clippy.txt                       | tests/h3_upstream_verify.rs:38 missing `security` field on LbConfig literal (PROTO-2-17 added field; test not migrated). No clippy warnings emitted; only this E0063 compile error. |
| 3  | cargo machete                              | PASS*    | gate-outputs/machete.txt                      | exit 1 = unused deps reported, all known/dev-gated |
| 4  | cargo geiger (fallback)                    | PASS     | gate-outputs/geiger.txt + geiger-grep.txt     | 73 unsafe sites; lb-io=16, lb-l4-xdp=57. See audit/unsafe-justifications.md |
| 5  | unsafe justifications                      | PASS     | audit/unsafe-justifications.md                | every site cited |
| 6  | cargo nextest                              | BLOCKED  | gate-outputs/nextest.txt                      | same h3_upstream_verify defect |
| 7  | cargo llvm-cov                             | DEFERRED | audit/coverage-scope.md, deferred-to-ci.md    | install needs network |
| 8  | cargo miri                                 | DEFERRED | deferred-to-ci.md                             | needs nightly |
| 9  | loom test                                  | SEE-ROW  | gate-outputs/loom.txt                         | exit code in file |
| 10 | proptest PROPTEST_CASES=10000              | BLOCKED  | gate-outputs/nextest.txt                      | same defect |
| 11 | cargo audit                                | DEFERRED | deferred-to-ci.md                             | install needs network |
| 12 | cargo deny check                           | DEFERRED | deferred-to-ci.md                             | install needs network; deny.toml committed |
| 13 | cargo cyclonedx (fallback)                 | PASS     | audit/sbom.json                               | 399 components, CycloneDX 1.5 |
| 14 | bpftool real-kernel                        | DEFERRED | deferred-to-ci.md                             | needs lvh + privileged Docker |
| 15 | XDP verifier-log matrix                    | DEFERRED | deferred-to-ci.md                             | scripts/verify-xdp.sh present; logs dir not built locally |
| 16 | h2spec                                     | DEFERRED | deferred-to-ci.md                             | binary not installed |
| 17 | smuggling tests                            | SEE-ROW  | gate-outputs/protocol-conformance.txt         | exit code in file |
| 18 | h3spec + Autobahn                          | DEFERRED | deferred-to-ci.md                             | binaries not installed |
| 19 | criterion benches                          | DEFERRED | gate-outputs/bench.txt                        | bench/criterion/*.rs are stubs |
| 20 | soak / chaos (4h)                          | DEFERRED | deferred-to-ci.md                             | 4h budget |
| 21 | round-2 open crit/high/medium              | PASS     | gate-outputs/security-findings.txt            | grep returned empty |
| 22 | default config refuses placeholders        | PASS     | gate-outputs/default-config-secrets.txt       | no placeholders in config/default.toml |
| 23 | container image scan                       | DEFERRED | deferred-to-ci.md                             | trivy/grype not installed |
| 24 | RUNBOOK doc-lint                           | PASS     | gate-outputs/doc-lint.txt                     | doc-lint: OK |
| 25 | Prometheus scrape (compose)                | DEFERRED | deferred-to-ci.md                             | no docker daemon |
| 26 | SIGTERM drain (reload_zero_drop)           | SEE-ROW  | gate-outputs/reload-zero-drop.txt             | exit code in file |

## Counts (local-runnable subset)

- PASS: 8 (fmt, machete*, geiger-fallback, unsafe-justifications, sbom-fallback, round-2-findings, default-config, doc-lint)
- FAIL: 1 (clippy — single integration-test compile error; workspace lints otherwise clean)
- BLOCKED: 2 (nextest, proptest — downstream of the same compile error)
- SEE-ROW: 3 (loom, smuggling, reload — see exit codes in their files)
- DEFERRED to CI: 12

## Key surprise

`tests/h3_upstream_verify.rs:38` constructs LbConfig {…} without the
`security` field that PROTO-2-17 added. This is the sole blocker for
clippy --all-targets / nextest / proptest. One-line fix
(`security: None,`); out of scope for this audit task — flagged for
the lead.

## Files written by this gate run

- audit/round-7/gate-outputs/*.txt
- audit/round-7/deferred-to-ci.md
- audit/round-7/SUMMARY.md (this file — required deliverable per Round 7 spec)
- audit/sbom.json (CycloneDX 1.5, manual fallback)
- audit/unsafe-justifications.md
- audit/coverage-scope.md

Lead: write FINAL_REPORT.md from the matrix above.
