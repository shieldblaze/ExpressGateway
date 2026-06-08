#!/usr/bin/env bash
# S38 Phase-3 re-gate on the FIXED tree (after the Phase-2 security fixes).
# fmt + clippy + ×3. Staggered from fuzz (R9 — the ×3 has timing-sensitive tests
# that flake under saturation, CF-S38-RELOAD-BOOT-FLAKE). Run with the box otherwise quiet.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_BUILD_JOBS=6
cd "$(dirname "$0")/../../.." || exit 99
LOG=audit/security/s38-logs
ts() { date '+%H:%M:%S'; }

echo "[$(ts)] === fmt ==="
cargo fmt --all --check 2>&1 | tee "$LOG/re-00-fmt.txt"; FMT=${PIPESTATUS[0]}
echo "[$(ts)] === clippy --all-targets --all-features -D warnings ==="
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tee "$LOG/re-01-clippy.txt" | tail -15; CLIPPY=${PIPESTATUS[0]}

PASS=0
for i in 1 2 3; do
  echo "[$(ts)] === re-test run $i/3 ==="
  cargo test --workspace --all-features --no-fail-fast 2>&1 | tee "$LOG/re-02-test-run$i.txt" | tail -4
  rc=${PIPESTATUS[0]}; echo "[$(ts)] run $i rc=$rc"; [ "$rc" -eq 0 ] && PASS=$((PASS+1))
  # capture any FAILED test names for flake triage
  grep -E '\.\.\. FAILED|test result: FAILED' "$LOG/re-02-test-run$i.txt" | head -20 > "$LOG/re-fails-run$i.txt" || true
done
echo "[$(ts)] === RE-GATE SUMMARY: fmt=$FMT clippy=$CLIPPY test=$PASS/3 ==="
echo "fmt=$FMT clippy=$CLIPPY test=$PASS/3" > "$LOG/regate-verdict.txt"
