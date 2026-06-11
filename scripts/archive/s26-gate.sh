#!/usr/bin/env bash
# S26 full-workspace gate. Usage: s26-gate.sh <run-label> <logfile>
# R15: a verdict is read ONLY from a COMPLETED run (exit code captured at end).
set -u
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
LABEL="${1:-run}"
LOG="${2:-/tmp/s26-gate-${LABEL}.log}"
cd /home/ubuntu/Code/ExpressGateway || exit 99
{
  echo "===== S26 GATE ${LABEL} START $(date -u +%FT%TZ) ====="
  echo "--- HEAD: $(git rev-parse HEAD) branch: $(git rev-parse --abbrev-ref HEAD)"
  echo "--- df:"; df -h /home/ubuntu | tail -1
} >> "$LOG" 2>&1
cargo test --workspace --all-features >> "$LOG" 2>&1
RC=$?
{
  echo "===== S26 GATE ${LABEL} END $(date -u +%FT%TZ) rc=${RC} ====="
  echo "--- df:"; df -h /home/ubuntu | tail -1
} >> "$LOG" 2>&1
echo "GATE_${LABEL}_EXIT=${RC}"
exit $RC
