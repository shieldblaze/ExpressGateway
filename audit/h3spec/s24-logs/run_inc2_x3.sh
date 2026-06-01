#!/usr/bin/env bash
# S24 INC-2 full-workspace ×3 gate. Forced exit 0; verdict read from meta.
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
cd /home/ubuntu/Code/ExpressGateway || exit 0
LOG=audit/h3spec/s24-logs
: > "$LOG/inc2-x3-meta.log"
echo "=== INC-2 ×3 GATE START $(date -u +%H:%M:%S) tip=$(git rev-parse --short HEAD) ===" >> "$LOG/inc2-x3-meta.log"
allpass=1
for i in 1 2 3; do
  echo "--- run $i start $(date -u +%H:%M:%S) ---" >> "$LOG/inc2-x3-meta.log"
  cargo test --workspace --all-features > "$LOG/inc2-x3-run$i.log" 2>&1
  ec=$?
  passed=$(grep -hoE '[0-9]+ passed' "$LOG/inc2-x3-run$i.log" | awk '{s+=$1} END{print s+0}')
  failed=$(grep -hoE '[0-9]+ failed' "$LOG/inc2-x3-run$i.log" | awk '{s+=$1} END{print s+0}')
  echo "--- run $i exit=$ec passed=$passed failed=$failed end $(date -u +%H:%M:%S) ---" >> "$LOG/inc2-x3-meta.log"
  if [ "$ec" -ne 0 ] || [ "$failed" -ne 0 ]; then allpass=0; fi
done
echo "=== INC-2 ×3 GATE DONE $(date -u +%H:%M:%S) allpass=$allpass ===" >> "$LOG/inc2-x3-meta.log"
exit 0
