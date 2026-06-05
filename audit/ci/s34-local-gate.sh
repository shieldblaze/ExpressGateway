#!/usr/bin/env bash
# Session-local R1 baseline: build once, then cargo test --all-features --no-fail-fast x3.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_BUILD_JOBS=4   # 15GiB/0-swap box: cap to avoid OOM (S33 lesson)
cd /home/ubuntu/Code/ExpressGateway
LOG=audit/ci/s34-local
echo "=== BUILD (--no-run) $(date -u +%H:%M:%S) ===" | tee $LOG-build.log
cargo test --workspace --all-features --no-run 2>&1 | tail -5 | tee -a $LOG-build.log
echo "build rc=${PIPESTATUS[0]}" | tee -a $LOG-build.log
for i in 1 2 3; do
  echo "=== RUN $i $(date -u +%H:%M:%S) ===" | tee $LOG-run$i.log
  cargo test --workspace --all-features --no-fail-fast 2>&1 | tee -a $LOG-run$i.log | tail -2
  echo "run$i rc=${PIPESTATUS[0]}" | tee -a $LOG-run$i.log
done
echo "=== DONE $(date -u +%H:%M:%S) ===" | tee $LOG-done.log
