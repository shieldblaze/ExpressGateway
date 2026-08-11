#!/usr/bin/env bash
# S21 Phase-4 regression gate (changed tree). debuginfo=0 to fit disk (a cold
# --all-features debug build is ~47G; ~49G free). Runs the IDENTICAL test set —
# only DWARF symbols are omitted (irrelevant to pass/fail), NOT a test weakening.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export RUSTFLAGS="-C debuginfo=0"
cd /home/ubuntu/Code/ExpressGateway

echo "=== fmt --check ==="
cargo fmt --all --check && echo "FMT_OK" || echo "FMT_FAIL"

echo "=== clippy --workspace --all-targets --all-features -D warnings ==="
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -4
echo "CLIPPY_EXIT=${PIPESTATUS[0]}"
echo "disk after clippy: $(df -h /home/ubuntu | awk 'NR==2{print $4" free"}')"

echo "=== build test binaries (--no-run) ==="
cargo test --workspace --all-features --no-run 2>&1 | tail -3
echo "BUILD_EXIT=${PIPESTATUS[0]}"
echo "disk after build: $(df -h /home/ubuntu | awk 'NR==2{print $4" free"}')"

for i in 1 2 3; do
  echo "=== TEST PASS $i ==="
  cargo test --workspace --all-features 2>&1 | grep -E "test result:|error\[|^error:|FAILED|panicked" | tail -50
  echo "PASS_${i}_EXIT=${PIPESTATUS[0]}"
done
echo "=== S21 GATE DONE ==="
