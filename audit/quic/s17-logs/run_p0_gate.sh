#!/usr/bin/env bash
# Phase 0 R1 baseline: x3 sequential workspace --all-features test gate.
# Known OPEN blocker s16_b2_multistream may fail intermittently (~22%) — expected.
# Everything ELSE must be green across all 3 runs.
set -u
cd /home/ubuntu/Code/ExpressGateway
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
for i in 1 2 3; do
  log="audit/quic/s17-logs/p0_test_run${i}.log"
  echo "GATE RUN ${i} START $(date -u +%H:%M:%S)" > "$log"
  cargo test --workspace --all-features 2>&1 | tee -a "$log"
  ec=${PIPESTATUS[0]}
  echo "GATE RUN ${i} EXIT=${ec} $(date -u +%H:%M:%S)" >> "$log"
  echo "---- RUN ${i}: failures ----" >> "$log"
  grep -E '^test result:|FAILED|^failures:|^    [a-z].*::|panicked' "$log" | grep -iE 'FAILED|failures|panic' >> "$log" 2>/dev/null || true
done
echo "ALL THREE RUNS COMPLETE $(date -u +%H:%M:%S)"
