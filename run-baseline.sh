#!/usr/bin/env bash
# S37 Phase-0 baseline: full --all-features build, ×3 --no-fail-fast, clippy, fmt.
# Lead-run on feature/ops-bcd-s37 @ main 21ee3c65 (author=lead; verifier re-runs binding).
# Shared target dir; JOBS=4 (15GiB box OOM cushion). Logs to main-root audit path.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
WT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify
cd "$WT" || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/audit/ops/s37-phase0
mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root | tail -1 | tr -dc 0-9; }
echo "[base $(st)] HEAD $(git rev-parse --short HEAD) branch $(git branch --show-current); disk $(fg)G"

echo "[base $(st)] fmt --all --check"
cargo fmt --all -- --check > "$OUT/fmt.log" 2>&1; F=$?
echo "[base $(st)] fmt rc=$F"

echo "[base $(st)] build --workspace --all-features --no-run (full)"
cargo test --workspace --all-features --no-run > "$OUT/build.log" 2>&1
if [ $? -ne 0 ]; then echo "[base $(st)] BUILD FAIL"; grep -E 'error(\[|:)|No space' "$OUT/build.log" | head; echo "BUILD_FAIL" > "$OUT/baseline.marker"; exit 1; fi
echo "[base $(st)] build done; disk $(fg)G"

P=0
for i in 1 2 3; do
  if [ "$(fg)" -lt 3 ]; then echo "[base $(st)] ABORT disk<3G before run $i"; echo "DISK_ABORT" > "$OUT/baseline.marker"; exit 2; fi
  echo "[base $(st)] x3 run $i"
  cargo test --workspace --all-features --no-fail-fast > "$OUT/run$i.log" 2>&1
  rc=$?
  sums=$(grep -hE "test result:" "$OUT/run$i.log" | awk '{p+=$4;f+=$6;ig+=$8} END{printf "passed=%d failed=%d ignored=%d",p,f,ig}')
  fails=$(grep -hE "^test .* \.\.\. FAILED" "$OUT/run$i.log" | sed 's/ \.\.\. FAILED//' | head -20)
  echo "[base $(st)] run $i rc=$rc $sums; disk $(fg)G"
  [ -n "$fails" ] && { echo "[base $(st)] run $i FAILED:"; echo "$fails"; }
  [ $rc -eq 0 ] && P=$((P+1))
done
echo "[base $(st)] x3 DONE pass_runs=$P/3"

echo "[base $(st)] clippy --all-targets --all-features -D warnings"
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$OUT/clippy.log" 2>&1; C=$?
echo "[base $(st)] clippy rc=$C ($(tail -1 "$OUT/clippy.log"))"

echo "DONE $(date -u) pass_runs=$P/3 fmt=$F clippy=$C" > "$OUT/baseline.marker"
echo "[base $(st)] ALL DONE pass_runs=$P/3 fmt=$F clippy=$C"