#!/usr/bin/env bash
# S31 scoped coverage: the quiche H3-integration surface (lb-quic) on 0.29.1/1.88.
# Collect once (--no-report), then emit lcov + summary. R15: read from completed run.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
cd /home/ubuntu/Code/ExpressGateway || exit 99
LOG=audit/deps/s31-phase2/cov.log
: > "$LOG"
echo "=== S31 COV start $(date -u +%FT%TZ) rustc=$(rustc --version) ===" | tee -a "$LOG"
echo "--- df:"; df -h /home/ubuntu | tail -1 | tee -a "$LOG"

echo "=== cargo clean (free disk; llvm-cov needs a fresh instrumented build) ===" | tee -a "$LOG"
cargo clean 2>&1 | tail -1 | tee -a "$LOG"
cargo llvm-cov clean --workspace 2>&1 | tail -1 | tee -a "$LOG"

echo "=== llvm-cov run lb-quic tests (instrumented) $(date -u +%FT%TZ) ===" | tee -a "$LOG"
cargo llvm-cov --no-report -p lb-quic --all-features --no-fail-fast >> "$LOG" 2>&1
RUNRC=$?
echo "COV_RUN_RC=$RUNRC" | tee -a "$LOG"

echo "=== lcov + summary ===" | tee -a "$LOG"
cargo llvm-cov report --lcov --output-path audit/deps/s31-cov.lcov >> "$LOG" 2>&1
cargo llvm-cov report --summary-only > audit/deps/s31-cov-summary.txt 2>> "$LOG"
echo "=== H3-surface coverage (conn_actor / h3_bridge / h3_config) ===" | tee -a "$LOG"
grep -E 'conn_actor\.rs|h3_bridge\.rs|h3_config\.rs|lb-quic|TOTAL|Filename|Region|Line' audit/deps/s31-cov-summary.txt | tee -a "$LOG"
echo "--- df after:"; df -h /home/ubuntu | tail -1 | tee -a "$LOG"
echo "S31-COV-DONE run_rc=$RUNRC $(date -u +%FT%TZ)" | tee -a "$LOG"
