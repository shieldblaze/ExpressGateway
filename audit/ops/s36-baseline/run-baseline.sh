#!/usr/bin/env bash
# S36 Phase 0 baseline: R1 gate on the base tip (feature/ops-layer-s36 @ f4d34247).
#   - clippy --all-targets --all-features -D warnings
#   - fmt --check
#   - cargo test --workspace --all-features --no-fail-fast  x3 (staggered, foreground)
# Shared CARGO_TARGET_DIR=eg-target; CARGO_BUILD_JOBS=4 (15GiB RAM OOM cap; 8G swap cushion now present).
# Reads PASS only from a COMPLETED run (R15). Logs every step.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway || exit 99
OUT=audit/ops/s36-baseline
mkdir -p "$OUT"

stamp() { date -u +%H:%M:%S; }

echo "[baseline $(stamp)] tip: $(git rev-parse --short HEAD) on $(git branch --show-current)"

echo "[baseline $(stamp)] === clippy --all-targets --all-features -D warnings ==="
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$OUT/clippy.log" 2>&1
CLIPPY=$?
echo "[baseline $(stamp)] clippy rc=$CLIPPY ($(tail -1 "$OUT/clippy.log"))"

echo "[baseline $(stamp)] === fmt --check ==="
cargo fmt --all -- --check > "$OUT/fmt.log" 2>&1
FMT=$?
echo "[baseline $(stamp)] fmt rc=$FMT"

# Build test artifacts once (no-run) so the x3 runs measure test execution, surface compile errors early.
echo "[baseline $(stamp)] === build test artifacts (--no-run) ==="
cargo test --workspace --all-features --no-run > "$OUT/build-norun.log" 2>&1
BUILD=$?
echo "[baseline $(stamp)] build --no-run rc=$BUILD ($(tail -1 "$OUT/build-norun.log"))"
df -h /dev/root | tail -1

if [ "$BUILD" -ne 0 ]; then
  echo "[baseline $(stamp)] FATAL: test build failed; aborting x3"
  exit 1
fi

PASS_RUNS=0
for i in 1 2 3; do
  echo "[baseline $(stamp)] === cargo test x3 — run $i ==="
  cargo test --workspace --all-features --no-fail-fast > "$OUT/test-run$i.log" 2>&1
  RC=$?
  SUMMARY=$(grep -hE "test result:" "$OUT/test-run$i.log" | tail -5 | tr '\n' ' ')
  FAILED=$(grep -hcE "^test .* FAILED" "$OUT/test-run$i.log")
  echo "[baseline $(stamp)] run $i rc=$RC failed_lines=$FAILED"
  echo "[baseline $(stamp)] run $i summary: $SUMMARY"
  if [ "$RC" -eq 0 ]; then PASS_RUNS=$((PASS_RUNS+1)); fi
done

echo "[baseline $(stamp)] === RESULT clippy=$CLIPPY fmt=$FMT build=$BUILD pass_runs=$PASS_RUNS/3 ==="
echo "DONE $(date -u)" > "$OUT/baseline.marker"
