# Plan for REL-2-14 — Binary name mismatch in systemd unit + runbook (batched low)
Finding-ref:     REL-2-14 (low, Open)
Files touched:
- `RUNBOOK.md` (lines around 161)
- `DEPLOYMENT.md` (lines 11, 44; full file sweep for `lb` path
  tokens)
- `packaging/expressgateway.service` (new — actual systemd unit
  shipped as a file, not just embedded in docs)
- `scripts/doc-lint.sh` (binary-path check, shared with REL-2-01
  umbrella)

Approach:
The binary built from `crates/lb` is named `expressgateway`
(confirmed by `docker/Dockerfile:34`,
`crates/lb/Cargo.toml [[bin]] name`). Every doc reference to
`/usr/local/bin/lb` or `target/release/lb` must change to
`/usr/local/bin/expressgateway` / `target/release/expressgateway`.

Single-paragraph fix because the change is mechanical and
covered by the REL-2-01 doc-lint sweep:
1. `sed -i 's|/usr/local/bin/lb |/usr/local/bin/expressgateway |g'`
   and `'s|target/release/lb |target/release/expressgateway |g'`
   across `RUNBOOK.md` + `DEPLOYMENT.md` (manual review on the
   diff because some "lb" tokens in prose are correct as crate
   abbreviation).
2. Extract the systemd unit from `DEPLOYMENT.md:32-65` into a
   real file `packaging/expressgateway.service`. The doc keeps
   a snippet but the canonical artefact lives in the file and is
   smoke-tested by CI (`systemd-analyze verify`).
3. The unit's `TimeoutStopSec` lines up with REL-2-02:
   `TimeoutStopSec=12` = `drain_timeout_ms(10s) + 2 s` slack.
4. `scripts/doc-lint.sh` (shared with REL-2-01) greps both md
   files for binary paths and fails on any `/usr/local/bin/lb`
   without a trailing alphanumeric (so `lb-` prefix tokens pass).

Proof:
- New CI job `doc-lint`: `bash scripts/doc-lint.sh` exits 0.
- New CI job `systemd-unit-verify`:
  `systemd-analyze verify packaging/expressgateway.service` exits 0.
- New test `tests/packaging.rs::test_unit_binary_path_matches_cargo`
  reads `[[bin]] name` from `crates/lb/Cargo.toml` and asserts the
  `ExecStart=` line in `packaging/expressgateway.service` contains
  the same name.

Risk / blast radius:
Documentation-only + a new packaging artefact. Zero source-code
change. Worst-case regression: prose mentions of "lb" that aren't
binary paths get rewritten — caught by reviewer.

Cross-ref:    REL-2-01 (umbrella doc-sweep), REL-2-02 (TimeoutStopSec
calibration).
Owner:        rel
Lead-approval: approved 2026-05-13 team-lead
