#!/usr/bin/env bash
# S27 Phase 3 gate: fmt + clippy(--all-targets --all-features) + test(--workspace --all-features) x3.
# R1 deterministic full 8-core; R10 full-workspace scope. Run AFTER all increments land + disk trimmed.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
cd /home/ubuntu/Code/ExpressGateway
OUT=audit/websockets/s27-phase3-gate
echo "=== PHASE3 GATE START $(git rev-parse --short HEAD) on $(git branch --show-current) ==="
df -h /home/ubuntu | tail -1

echo "--- [1/5] fmt --check ---"
cargo fmt --all --check > "$OUT/fmt.log" 2>&1; echo "FMT_EXIT=$?"

echo "--- [2/5] clippy --all-targets --all-features -D warnings ---"
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$OUT/clippy.log" 2>&1; echo "CLIPPY_EXIT=$?"

for i in 1 2 3; do
  echo "--- [$((i+2))/5] test run $i ---"
  cargo test --workspace --all-features > "$OUT/test_run$i.log" 2>&1
  echo "TEST_RUN${i}_EXIT=$?"
  passed=$(grep -oE "[0-9]+ passed" "$OUT/test_run$i.log" | awk '{s+=$1} END{print s}')
  failed=$(grep -oE "[0-9]+ failed" "$OUT/test_run$i.log" | awk '{s+=$1} END{print s}')
  ignored=$(grep -oE "[0-9]+ ignored" "$OUT/test_run$i.log" | awk '{s+=$1} END{print s}')
  echo "RUN${i}_TOTALS passed=$passed failed=$failed ignored=$ignored"
  grep -hE "test result: FAILED|panicked|error\[" "$OUT/test_run$i.log" | head -20 > "$OUT/test_run${i}.failures" || true
done
echo "=== PHASE3 GATE DONE ==="; df -h /home/ubuntu | tail -1
