#!/usr/bin/env bash
# S24 baseline ×3 gate. No foreground `sleep`, no sandbox tricks.
# Forced exit 0 so the harness never reports a spurious failure; the
# verdict is read from baseline-meta.log (R15: completed runs only).
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
cd /home/ubuntu/Code/ExpressGateway || exit 0
LOG=audit/h3spec/s24-logs

: > "$LOG/baseline-meta.log"
echo "=== BASELINE GATE START $(date -u +%H:%M:%S) tip=$(git rev-parse --short HEAD) ===" >> "$LOG/baseline-meta.log"

allpass=1
for i in 1 2 3; do
  echo "--- run $i start $(date -u +%H:%M:%S) ---" >> "$LOG/baseline-meta.log"
  cargo test --workspace --all-features > "$LOG/baseline-run$i.log" 2>&1
  ec=$?
  passed=$(grep -hoE '[0-9]+ passed' "$LOG/baseline-run$i.log" | awk '{s+=$1} END{print s+0}')
  failed=$(grep -hoE '[0-9]+ failed' "$LOG/baseline-run$i.log" | awk '{s+=$1} END{print s+0}')
  echo "--- run $i exit=$ec passed=$passed failed=$failed end $(date -u +%H:%M:%S) ---" >> "$LOG/baseline-meta.log"
  if [ "$ec" -ne 0 ] || [ "$failed" -ne 0 ]; then allpass=0; fi
done
echo "=== BASELINE GATE DONE $(date -u +%H:%M:%S) allpass=$allpass ===" >> "$LOG/baseline-meta.log"
exit 0
