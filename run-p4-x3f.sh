#!/usr/bin/env bash
# S37 Phase-4 LEAD x3 (verifier disk-cut its runs; R9 takeover). Clean build for headroom,
# then x3 + clippy + fmt on feature/ops-bcd-s37 (B+C+D). Reliable from the main loop.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify/audit/ops/s37-p4f
mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root|tail -1|tr -dc 0-9; }
echo "[p4 $(st)] HEAD $(git rev-parse --short HEAD); clean build; disk $(fg)G" | tee "$OUT/console"
cargo test --workspace --all-features --no-run > "$OUT/build.log" 2>&1
if [ $? -ne 0 ]; then echo "[p4 $(st)] BUILD FAIL" | tee -a "$OUT/console"; grep -E 'error(\[|:)|No space' "$OUT/build.log"|head | tee -a "$OUT/console"; echo BUILD_FAIL > "$OUT/marker"; exit 1; fi
echo "[p4 $(st)] build done; disk $(fg)G" | tee -a "$OUT/console"
P=0
for i in 1 2 3; do
  if [ "$(fg)" -lt 3 ]; then echo "[p4 $(st)] ABORT disk<3G run $i" | tee -a "$OUT/console"; echo DISK_ABORT > "$OUT/marker"; exit 2; fi
  echo "[p4 $(st)] run $i" | tee -a "$OUT/console"
  cargo test --workspace --all-features --no-fail-fast > "$OUT/run$i.log" 2>&1; rc=$?
  s=$(grep -hE "test result:" "$OUT/run$i.log"|awk '{p+=$4;f+=$6;ig+=$8} END{printf "passed=%d failed=%d ignored=%d",p,f,ig}')
  fl=$(grep -hE "^test .* \.\.\. FAILED" "$OUT/run$i.log"|sed 's/ \.\.\. FAILED//'|head -20)
  echo "[p4 $(st)] run $i rc=$rc $s; disk $(fg)G" | tee -a "$OUT/console"
  [ -n "$fl" ] && { echo "FAILED:" | tee -a "$OUT/console"; echo "$fl" | tee -a "$OUT/console"; }
  [ $rc -eq 0 ] && P=$((P+1))
done
echo "[p4 $(st)] x3 pass_runs=$P/3" | tee -a "$OUT/console"
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$OUT/clippy.log" 2>&1; C=$?
echo "[p4 $(st)] clippy rc=$C ($(tail -1 "$OUT/clippy.log"))" | tee -a "$OUT/console"
cargo fmt --all -- --check > "$OUT/fmt.log" 2>&1; F=$?
echo "[p4 $(st)] fmt rc=$F" | tee -a "$OUT/console"
echo "DONE pass_runs=$P/3 clippy=$C fmt=$F $(date -u)" > "$OUT/marker"
echo "[p4 $(st)] ALL DONE pass_runs=$P/3 clippy=$C fmt=$F" | tee -a "$OUT/console"
