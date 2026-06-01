#!/usr/bin/env bash
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
cd /home/ubuntu/Code/ExpressGateway
LOGDIR="$1"
LABEL="$2"
mkdir -p "$LOGDIR"
for i in 1 2 3; do
  LOG="$LOGDIR/${LABEL}-run${i}.log"
  echo "=== RUN $i START $(date -u +%H:%M:%S) ===" > "$LOG"
  cargo test --workspace --all-features >> "$LOG" 2>&1
  ec=$?
  echo "=== RUN $i EXIT=$ec $(date -u +%H:%M:%S) ===" >> "$LOG"
  # summarize pass/fail counts
  PASS=$(grep -hoE 'test result: ok\. [0-9]+ passed; [0-9]+ failed' "$LOG" | awk '{p+=$4; f+=$6} END{print p"/"f}')
  FAILED=$(grep -hoE 'test result: FAILED\. [0-9]+ passed; [0-9]+ failed' "$LOG" | awk '{p+=$4; f+=$6} END{if(NR>0)print p" passed/"f" failed"; else print "none"}')
  echo "RUN${i}_SUMMARY exit=$ec aggregate_ok=$PASS failed_suites=$FAILED" | tee -a "$LOGDIR/${LABEL}-summary.txt"
done
echo "ALL_RUNS_DONE $(date -u +%H:%M:%S)" | tee -a "$LOGDIR/${LABEL}-summary.txt"
