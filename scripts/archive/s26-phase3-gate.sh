#!/usr/bin/env bash
# S26 Phase-3 gate: build --no-run, x3 deterministic test passes, clippy, fmt.
# One completed-run sentinel (R15). Logs to audit/s26-logs/phase3-gate.log.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
cd /home/ubuntu/Code/ExpressGateway || exit 99
LOG=audit/s26-logs/phase3-gate.log
: > "$LOG"
{
  echo "===== S26 PHASE-3 GATE start $(date -u +%FT%TZ) HEAD $(git rev-parse --short HEAD) branch $(git rev-parse --abbrev-ref HEAD) ====="
  echo "--- df:"; df -h /home/ubuntu | tail -1
} | tee -a "$LOG"

echo "=== STAGE build (--no-run) ===" | tee -a "$LOG"
cargo test --workspace --all-features --no-run >> "$LOG" 2>&1
BRC=${PIPESTATUS[0]}
echo "BUILD_RC=$BRC" | tee -a "$LOG"
if [ "$BRC" -ne 0 ]; then echo "S26-PHASE3-DONE rc=99 (build fail)" | tee -a "$LOG"; exit 99; fi

for i in 1 2 3; do
  echo "=== STAGE TEST PASS $i $(date -u +%FT%TZ) ===" | tee -a "$LOG"
  cargo test --workspace --all-features >> "$LOG.pass$i" 2>&1
  RC=${PIPESTATUS[0]}
  PASS=$(grep -oE '[0-9]+ passed' "$LOG.pass$i" | awk '{s+=$1} END{print s}')
  FAIL=$(grep -oE '[0-9]+ failed' "$LOG.pass$i" | awk '{s+=$1} END{print s}')
  IGN=$(grep -oE '[0-9]+ ignored' "$LOG.pass$i" | awk '{s+=$1} END{print s}')
  echo "PASS$i rc=$RC passed=$PASS failed=$FAIL ignored=$IGN" | tee -a "$LOG"
done

echo "=== STAGE clippy ===" | tee -a "$LOG"
cargo clippy --workspace --all-targets --all-features -- -D warnings >> "$LOG" 2>&1
echo "CLIPPY_RC=${PIPESTATUS[0]}" | tee -a "$LOG"

echo "=== STAGE fmt --check ===" | tee -a "$LOG"
cargo fmt --all --check >> "$LOG" 2>&1
echo "FMT_RC=${PIPESTATUS[0]}" | tee -a "$LOG"

{
  echo "--- df after:"; df -h /home/ubuntu | tail -1
  echo "===== S26 PHASE-3 GATE end $(date -u +%FT%TZ) ====="
  echo "S26-PHASE3-DONE rc=0"
} | tee -a "$LOG"
