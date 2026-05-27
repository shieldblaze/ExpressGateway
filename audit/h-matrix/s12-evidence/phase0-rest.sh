#!/usr/bin/env bash
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
cd /home/ubuntu/Code/ExpressGateway
D=audit/h-matrix/s12-evidence
echo "===== RUN2 START $(date -u +%H:%M:%S) ====="
cargo test --workspace --all-features > "$D/baseline-run2.log" 2>&1
echo "RUN2_EXIT=$?"
echo "===== RUN3 START $(date -u +%H:%M:%S) ====="
cargo test --workspace --all-features > "$D/baseline-run3.log" 2>&1
echo "RUN3_EXIT=$?"
echo "===== CLIPPY START $(date -u +%H:%M:%S) ====="
cargo clippy --all-targets --all-features -- -D warnings > "$D/baseline-clippy.log" 2>&1
echo "CLIPPY_EXIT=$?"
echo "===== FMT START $(date -u +%H:%M:%S) ====="
cargo fmt --check > "$D/baseline-fmt.log" 2>&1
echo "FMT_EXIT=$?"
echo "===== DONE $(date -u +%H:%M:%S) ====="
