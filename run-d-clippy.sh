#!/usr/bin/env bash
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s37-deps || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify/audit/ops/s37-d-lead
st(){ date -u +%H:%M:%S; }
echo "[clip3 $(st)] fmt --all (write)" | tee "$OUT/clippy3.console"
cargo fmt --all > "$OUT/fmt3.log" 2>&1
echo "[clip3 $(st)] clippy --all-targets --all-features -D warnings" | tee -a "$OUT/clippy3.console"
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$OUT/clippy3.log" 2>&1; C=$?
echo "[clip3 $(st)] clippy rc=$C ($(tail -1 "$OUT/clippy3.log"))" | tee -a "$OUT/clippy3.console"
[ $C -ne 0 ] && grep -E 'error(\[|:)|deprecated' "$OUT/clippy3.log" | head -10 | tee -a "$OUT/clippy3.console"
cargo fmt --all -- --check > "$OUT/fmt3check.log" 2>&1; F=$?
echo "[clip3 $(st)] fmt --check rc=$F" | tee -a "$OUT/clippy3.console"
echo "DONE clippy=$C fmt=$F" > "$OUT/clippy3.marker"
echo "[clip3 $(st)] DONE clippy=$C fmt=$F" | tee -a "$OUT/clippy3.console"
