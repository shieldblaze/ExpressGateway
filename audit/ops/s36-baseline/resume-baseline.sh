#!/usr/bin/env bash
# S36 Phase 0 baseline RESUME (after accidental session exit killed the first run).
# clippy + fmt already confirmed clean on this tip (baseline.console.log, committed).
# This resumes the incremental test build then runs cargo test x3. Disk-guarded.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway || exit 99
OUT=audit/ops/s36-baseline
stamp() { date -u +%H:%M:%S; }
free_gb() { df --output=avail -BG /dev/root | tail -1 | tr -dc '0-9'; }

echo "[resume $(stamp)] tip $(git rev-parse --short HEAD); disk $(free_gb)G free"

echo "[resume $(stamp)] === build test artifacts (--no-run, incremental resume) ==="
cargo test --workspace --all-features --no-run > "$OUT/build-norun.log" 2>&1
BUILD=$?
echo "[resume $(stamp)] build --no-run rc=$BUILD ($(tail -1 "$OUT/build-norun.log")); disk $(free_gb)G free"
if [ "$BUILD" -ne 0 ]; then
  echo "[resume $(stamp)] FATAL: test build failed"; grep -E "error(\[|:)" "$OUT/build-norun.log" | head; exit 1
fi

PASS_RUNS=0
for i in 1 2 3; do
  if [ "$(free_gb)" -lt 2 ]; then echo "[resume $(stamp)] ABORT: disk < 2G before run $i"; exit 2; fi
  echo "[resume $(stamp)] === cargo test x3 — run $i ==="
  cargo test --workspace --all-features --no-fail-fast > "$OUT/test-run$i.log" 2>&1
  RC=$?
  PASS_FAIL=$(grep -hE "test result:" "$OUT/test-run$i.log" | awk '{p+=$4; f+=$6} END{printf "passed=%d failed=%d", p, f}')
  FAILED_NAMES=$(grep -hE "^test .* \.\.\. FAILED" "$OUT/test-run$i.log" | sed 's/ \.\.\. FAILED//' | head -10)
  echo "[resume $(stamp)] run $i rc=$RC $PASS_FAIL; disk $(free_gb)G free"
  [ -n "$FAILED_NAMES" ] && { echo "[resume $(stamp)] run $i FAILURES:"; echo "$FAILED_NAMES"; }
  if [ "$RC" -eq 0 ]; then PASS_RUNS=$((PASS_RUNS+1)); fi
done

echo "[resume $(stamp)] === RESULT build=$BUILD pass_runs=$PASS_RUNS/3 ==="
echo "DONE $(date -u) pass_runs=$PASS_RUNS/3" > "$OUT/baseline.marker"
