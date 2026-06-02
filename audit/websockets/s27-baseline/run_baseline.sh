#!/usr/bin/env bash
# S27 Phase 0 baseline: fmt + clippy + test x3 on feature/websockets-s27 @ base (== main 33a0d068)
# R1: deterministic full 8-core parallelism, x3 ALL-PASS; clippy -D warnings; fmt clean.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
cd /home/ubuntu/Code/ExpressGateway
OUT=audit/websockets/s27-baseline
echo "=== BASELINE START $(git rev-parse --short HEAD) on $(git branch --show-current) ==="

echo "--- [1/5] fmt --check ---"
cargo fmt --all --check > "$OUT/fmt.log" 2>&1
echo "FMT_EXIT=$?"

echo "--- [2/5] clippy -D warnings ---"
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$OUT/clippy.log" 2>&1
echo "CLIPPY_EXIT=$?"

for i in 1 2 3; do
  echo "--- [$((i+2))/5] test run $i ---"
  cargo test --workspace --all-features > "$OUT/test_run$i.log" 2>&1
  ec=$?
  echo "TEST_RUN${i}_EXIT=$ec"
  # extract result lines
  grep -E "test result:" "$OUT/test_run$i.log" | grep -vE "0 passed; 0 failed" | tail -40 > "$OUT/test_run${i}.summary"
  passed=$(grep -oE "[0-9]+ passed" "$OUT/test_run$i.log" | awk '{s+=$1} END{print s}')
  failed=$(grep -oE "[0-9]+ failed" "$OUT/test_run$i.log" | awk '{s+=$1} END{print s}')
  ignored=$(grep -oE "[0-9]+ ignored" "$OUT/test_run$i.log" | awk '{s+=$1} END{print s}')
  echo "RUN${i}_TOTALS passed=$passed failed=$failed ignored=$ignored"
done
echo "=== BASELINE DONE ==="
