#!/usr/bin/env bash
# S28 binding gate: fmt + clippy (--all-targets --all-features -D warnings) + ×3 test (--workspace --all-features).
set -u
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
cd /home/ubuntu/Code/ExpressGateway
LOG="${1:-/home/ubuntu/Code/ExpressGateway/audit/websockets/s28-gate-$(date +%s).log}"
echo "=== S28 GATE @ $(git rev-parse --short HEAD) $(date -u) ===" > "$LOG"
echo "--- fmt ---" >> "$LOG"
cargo fmt --all --check >> "$LOG" 2>&1; echo "fmt_rc=$?" >> "$LOG"
echo "--- clippy ---" >> "$LOG"
cargo clippy --workspace --all-targets --all-features -- -D warnings >> "$LOG" 2>&1; echo "clippy_rc=$?" >> "$LOG"
for i in 1 2 3; do
  echo "--- test run $i ---" >> "$LOG"
  cargo test --workspace --all-features >> "$LOG" 2>&1; echo "test${i}_rc=$?" >> "$LOG"
done
echo "=== GATE DONE $(date -u) ===" >> "$LOG"
grep -E '_rc=|test result:|GATE DONE' "$LOG" | tail -40
