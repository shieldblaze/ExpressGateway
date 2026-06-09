#!/usr/bin/env bash
# S39 R1 gate — cargo test --workspace --all-features --no-fail-fast x3.
# The eg-bench harness is an ADDITIVE change to lb-soak (new module + new bin,
# loadgen untouched), so this confirms the green baseline is preserved (R1).
# Job-capped (S33: 15GiB RAM OOMs --all-features at full parallelism).
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway || exit 99
OUT=audit/perf/s39-x3
mkdir -p "$OUT"
for run in 1 2 3; do
  echo "[x3 run $run START $(date -u)] free=$(df -h / | awk 'NR==2{print $4}')"
  cargo test --workspace --all-features --no-fail-fast > "$OUT/run$run.log" 2>&1
  rc=$?
  passed=$(grep -hoE "test result: ok\. [0-9]+ passed" "$OUT/run$run.log" | grep -oE "[0-9]+" | paste -sd+ | bc 2>/dev/null)
  failed=$(grep -hoE "[0-9]+ failed" "$OUT/run$run.log" | grep -oE "^[0-9]+" | paste -sd+ | bc 2>/dev/null)
  echo "[x3 run $run DONE $(date -u)] rc=$rc passed=${passed:-?} failed=${failed:-?} free=$(df -h / | awk 'NR==2{print $4}')"
  echo "run$run rc=$rc passed=${passed:-?} failed=${failed:-?}" >> "$OUT/x3-summary.txt"
  if [ "$rc" -ne 0 ] && grep -qiE "No space left|ENOSPC" "$OUT/run$run.log"; then
    echo "[x3] ENOSPC — aborting" >> "$OUT/x3-summary.txt"; exit 28
  fi
done
echo "[x3 ALL DONE $(date -u)]"
echo "DONE $(date -u)" >> "$OUT/x3-summary.txt"
