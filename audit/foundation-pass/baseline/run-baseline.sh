#!/usr/bin/env bash
# Baseline per R1. Run from repo root. Captures everything verbatim.
set -uo pipefail
cd /home/ubuntu/Code/ExpressGateway
OUT=audit/foundation-pass/baseline
mkdir -p "$OUT"

echo "=== fmt check ===" | tee "$OUT/fmt.log"
cargo fmt --check >> "$OUT/fmt.log" 2>&1
echo "FMT_EXIT=$?" >> "$OUT/fmt.log"

for i in 1 2 3; do
  echo "=== baseline test run $i START $(date -u +%H:%M:%S) ==="
  cargo test --workspace --all-features -- --test-threads=8 \
    > "$OUT/test-run-$i.log" 2>&1
  echo "TEST_RUN_${i}_EXIT=$?" | tee -a "$OUT/test-run-$i.log"
  grep -E "test result:|running [0-9]+ test|FAILED|panicked|error\[" \
    "$OUT/test-run-$i.log" | tail -40 > "$OUT/test-run-$i.summary"
done

echo "=== clippy ===" | tee "$OUT/clippy.log"
cargo clippy --all-targets --all-features -- -D warnings >> "$OUT/clippy.log" 2>&1
echo "CLIPPY_EXIT=$?" >> "$OUT/clippy.log"

echo "BASELINE_SCRIPT_DONE $(date -u +%H:%M:%S)"
