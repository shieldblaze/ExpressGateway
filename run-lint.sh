#!/usr/bin/env bash
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/audit/ops/s36-aimpl-x3; mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
echo "[lint $(st)] clippy --all-targets --all-features -D warnings"
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$OUT/clippy.log" 2>&1; C=$?
echo "[lint $(st)] clippy rc=$C ($(tail -1 "$OUT/clippy.log"))"
echo "[lint $(st)] fmt --all --check"
cargo fmt --all -- --check > "$OUT/fmt.log" 2>&1; F=$?
echo "[lint $(st)] fmt rc=$F"
echo "DONE clippy=$C fmt=$F" > "$OUT/lint.marker"
