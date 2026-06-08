#!/usr/bin/env bash
# S37-D lead takeover build (dep-eng idled per-step; R9 takeover). Stage 2 compile-confirm:
# prometheus 0.14 + object 0.39 + aya 0.13.2 (stage1 tokio1.52.3/socket2 already in lock).
# Full --no-run on cleaned target. Logs to D audit path.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s37-deps || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify/audit/ops/s37-d-lead
mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root|tail -1|tr -dc 0-9; }
echo "[d $(st)] settling lock (prometheus/object/aya bumps); disk $(fg)G" | tee "$OUT/build.console"
cargo update -p prometheus -p object -p aya -p aya-obj > "$OUT/update.log" 2>&1
echo "[d $(st)] update rc=$?; resolved:" | tee -a "$OUT/build.console"
for c in tokio socket2 prometheus object aya aya-obj; do echo -n "  $c: " | tee -a "$OUT/build.console"; awk -v n="$c" '$1=="name" && $3=="\""n"\""{getline; gsub(/"/,"",$3); print $3}' Cargo.lock | tr '\n' ' ' | tee -a "$OUT/build.console"; echo | tee -a "$OUT/build.console"; done
echo "[d $(st)] cargo test --workspace --all-features --no-run (full); disk $(fg)G" | tee -a "$OUT/build.console"
cargo test --workspace --all-features --no-run > "$OUT/norun.log" 2>&1
RC=$?
echo "[d $(st)] --no-run rc=$RC; disk $(fg)G" | tee -a "$OUT/build.console"
if [ $RC -ne 0 ]; then
  echo "[d $(st)] BUILD ERRORS:" | tee -a "$OUT/build.console"
  grep -E 'error(\[|:)' "$OUT/norun.log" | head -20 | tee -a "$OUT/build.console"
fi
echo "DONE norun_rc=$RC $(date -u)" > "$OUT/build.marker"
echo "[d $(st)] DONE norun_rc=$RC" | tee -a "$OUT/build.console"