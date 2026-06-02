#!/usr/bin/env bash
# S31 full-workspace gate: build --no-run, x3 deterministic test passes, clippy, fmt.
# One completed-run sentinel (R15). Usage: s31-gate.sh <label> [logdir]
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
cd /home/ubuntu/Code/ExpressGateway || exit 99
LABEL="${1:-run}"
LOGDIR="${2:-audit/deps/s31-gate-${LABEL}}"
mkdir -p "$LOGDIR"
LOG="$LOGDIR/gate.log"
: > "$LOG"
{
  echo "===== S31 GATE ${LABEL} start $(date -u +%FT%TZ) HEAD $(git rev-parse --short HEAD) branch $(git rev-parse --abbrev-ref HEAD) ====="
  echo "--- df:"; df -h /home/ubuntu | tail -1
  echo "--- quiche/tokio-quiche locked:"; grep -A1 'name = "quiche"' Cargo.lock | grep version; grep -A1 'name = "tokio-quiche"' Cargo.lock | grep version
} | tee -a "$LOG"

echo "=== STAGE build (--no-run) $(date -u +%FT%TZ) ===" | tee -a "$LOG"
cargo test --workspace --all-features --no-run >> "$LOG" 2>&1
BRC=${PIPESTATUS[0]}
echo "BUILD_RC=$BRC" | tee -a "$LOG"
if [ "$BRC" -ne 0 ]; then echo "S31-GATE-${LABEL}-DONE rc=99 (build fail)" | tee -a "$LOG"; exit 99; fi

for i in 1 2 3; do
  echo "=== STAGE TEST PASS $i $(date -u +%FT%TZ) ===" | tee -a "$LOG"
  cargo test --workspace --all-features >> "$LOG.pass$i" 2>&1
  RC=${PIPESTATUS[0]}
  PASS=$(grep -oE '[0-9]+ passed' "$LOG.pass$i" | awk '{s+=$1} END{print s}')
  FAIL=$(grep -oE '[0-9]+ failed' "$LOG.pass$i" | awk '{s+=$1} END{print s}')
  IGN=$(grep -oE '[0-9]+ ignored' "$LOG.pass$i" | awk '{s+=$1} END{print s}')
  echo "PASS$i rc=$RC passed=$PASS failed=$FAIL ignored=$IGN" | tee -a "$LOG"
done

echo "=== STAGE clippy $(date -u +%FT%TZ) ===" | tee -a "$LOG"
cargo clippy --workspace --all-targets --all-features -- -D warnings >> "$LOG" 2>&1
echo "CLIPPY_RC=${PIPESTATUS[0]}" | tee -a "$LOG"

echo "=== STAGE fmt --check $(date -u +%FT%TZ) ===" | tee -a "$LOG"
cargo fmt --all --check >> "$LOG" 2>&1
echo "FMT_RC=${PIPESTATUS[0]}" | tee -a "$LOG"

{
  echo "--- df after:"; df -h /home/ubuntu | tail -1
  echo "===== S31 GATE ${LABEL} end $(date -u +%FT%TZ) ====="
  echo "S31-GATE-${LABEL}-DONE rc=0"
} | tee -a "$LOG"
