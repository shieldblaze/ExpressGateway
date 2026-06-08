#!/usr/bin/env bash
# S36-A binding ×3 — built+run from the WORKTREE (consistent build path; lead-run,
# author=recycle-eng). Logs to an ABSOLUTE main-root path so they live with the report.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/audit/ops/s36-aimpl-x3
mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root | tail -1 | tr -dc 0-9; }
echo "[x3 $(st)] HEAD $(git rev-parse --short HEAD) (worktree); disk $(fg)G"
echo "[x3 $(st)] build --no-run (FULL rebuild, eg-target was reset)"
cargo test --workspace --all-features --no-run > "$OUT/build.log" 2>&1
if [ $? -ne 0 ]; then echo "[x3 $(st)] BUILD FAIL"; grep -E 'error(\[|:)|No space' "$OUT/build.log" | head; echo "BUILD_FAIL" > "$OUT/x3.marker"; exit 1; fi
echo "[x3 $(st)] build done; disk $(fg)G"
P=0
for i in 1 2 3; do
  if [ "$(fg)" -lt 2 ]; then echo "[x3 $(st)] ABORT disk<2G before run $i"; echo "DISK_ABORT" > "$OUT/x3.marker"; exit 2; fi
  echo "[x3 $(st)] run $i"
  cargo test --workspace --all-features --no-fail-fast > "$OUT/run$i.log" 2>&1
  rc=$?
  sums=$(grep -hE "test result:" "$OUT/run$i.log" | awk '{p+=$4;f+=$6;ig+=$8} END{printf "passed=%d failed=%d ignored=%d",p,f,ig}')
  fails=$(grep -hE "^test .* \.\.\. FAILED" "$OUT/run$i.log" | sed 's/ \.\.\. FAILED//' | head -10)
  echo "[x3 $(st)] run $i rc=$rc $sums; disk $(fg)G"
  [ -n "$fails" ] && { echo "[x3 $(st)] run $i FAILED:"; echo "$fails"; }
  [ $rc -eq 0 ] && P=$((P+1))
done
echo "[x3 $(st)] DONE pass_runs=$P/3"
echo "DONE $(date -u) pass_runs=$P/3" > "$OUT/x3.marker"
