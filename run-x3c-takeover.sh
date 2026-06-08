#!/usr/bin/env bash
# S37 LEAD TAKEOVER of the C binding ×3 (verifier x3 orphaned-on-idle again at run1=459, R9 takeover); 6e43b09c
# Independent of B's author (config-eng); runs against s37-b-config in the verifier worktree
# (warm cache, fresh binary @ a3da4db2). Logs to the lead's audit path (committable on feature/ops-bcd-s37).
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
WT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s37-verify
cd "$WT" || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify/audit/ops/s37-verifyC-lead
mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root | tail -1 | tr -dc 0-9; }
echo "[x3L $(st)] HEAD $(git rev-parse --short HEAD) branch $(git branch --show-current); disk $(fg)G" | tee "$OUT/console.log"
P=0
for i in 1 2 3; do
  if [ "$(fg)" -lt 3 ]; then echo "[x3L $(st)] ABORT disk<3G before run $i" | tee -a "$OUT/console.log"; echo "DISK_ABORT" > "$OUT/marker"; exit 2; fi
  echo "[x3L $(st)] run $i" | tee -a "$OUT/console.log"
  cargo test --workspace --all-features --no-fail-fast > "$OUT/run$i.log" 2>&1
  rc=$?
  sums=$(grep -hE "test result:" "$OUT/run$i.log" | awk '{p+=$4;f+=$6;ig+=$8} END{printf "passed=%d failed=%d ignored=%d",p,f,ig}')
  fails=$(grep -hE "^test .* \.\.\. FAILED" "$OUT/run$i.log" | sed 's/ \.\.\. FAILED//' | head -20)
  echo "[x3L $(st)] run $i rc=$rc $sums; disk $(fg)G" | tee -a "$OUT/console.log"
  [ -n "$fails" ] && { echo "[x3L $(st)] run $i FAILED:" | tee -a "$OUT/console.log"; echo "$fails" | tee -a "$OUT/console.log"; }
  [ $rc -eq 0 ] && P=$((P+1))
done
echo "[x3L $(st)] x3 DONE pass_runs=$P/3" | tee -a "$OUT/console.log"
echo "[x3L $(st)] clippy" | tee -a "$OUT/console.log"
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$OUT/clippy.log" 2>&1; C=$?
echo "[x3L $(st)] clippy rc=$C ($(tail -1 "$OUT/clippy.log"))" | tee -a "$OUT/console.log"
echo "[x3L $(st)] fmt" | tee -a "$OUT/console.log"
cargo fmt --all -- --check > "$OUT/fmt.log" 2>&1; F=$?
echo "[x3L $(st)] fmt rc=$F" | tee -a "$OUT/console.log"
echo "[x3L $(st)] scoped lb-config coverage" | tee -a "$OUT/console.log"
cargo llvm-cov nextest -p lb-config --all-features --ignore-run-fail --lcov --output-path "$OUT/lbconfig.lcov" > "$OUT/cov.log" 2>&1; CV=$?
if [ -s "$OUT/lbconfig.lcov" ]; then
  python3 - "$OUT/lbconfig.lcov" > "$OUT/cov-summary.txt" 2>&1 <<'PY'
import sys
tot=hit=0; cur=None; per={}
for ln in open(sys.argv[1]):
    if ln.startswith('SF:'): cur=ln[3:].strip(); per.setdefault(cur,[0,0])
    elif ln.startswith('DA:') and cur:
        c=int(ln[3:].split(',')[1]); per[cur][0]+=1; per[cur][1]+=(1 if c>0 else 0)
for f,(t,h) in sorted(per.items()):
    if 'lb-config' in f and t: print(f"{h/t*100:6.2f}%  {h}/{t}  {f.split('/crates/')[-1]}")
PY
  cat "$OUT/cov-summary.txt" | tee -a "$OUT/console.log"
fi
echo "DONE $(date -u) pass_runs=$P/3 clippy=$C fmt=$F cov=$CV" > "$OUT/marker"
echo "[x3L $(st)] ALL DONE pass_runs=$P/3 clippy=$C fmt=$F" | tee -a "$OUT/console.log"