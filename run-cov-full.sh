#!/usr/bin/env bash
# S36-A AUTHORITATIVE full-workspace coverage on 4480fb83 (matches CI D-6 methodology:
# full --workspace --all-features instrumented run so integration tests exercise the
# hot paths). The scoped run under-counted conn_actor (WS/Mode-B paths are workspace-only).
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/audit/ops/s36-aimpl-cov; mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root|tail -1|tr -dc 0-9; }
echo "[covfull $(st)] HEAD $(git rev-parse --short HEAD); disk $(fg)G"
echo "[covfull $(st)] llvm-cov nextest --workspace --all-features --ignore-run-fail"
cargo llvm-cov nextest --workspace --all-features --ignore-run-fail \
  --lcov --output-path "$OUT/cov-full.lcov" > "$OUT/covfull-run.log" 2>&1
RC=$?
echo "[covfull $(st)] rc=$RC; disk $(fg)G; lcov lines=$(wc -l < "$OUT/cov-full.lcov" 2>/dev/null || echo 0)"
if [ ! -s "$OUT/cov-full.lcov" ]; then echo "COVFULL_FAIL no lcov" > "$OUT/covfull.marker"; tail -10 "$OUT/covfull-run.log"; exit 1; fi
echo "[covfull $(st)] charter D-6 gate:"
bash scripts/ci/coverage-check.sh "$OUT/cov-full.lcov" > "$OUT/covfull-gate.log" 2>&1; GATE=$?
cat "$OUT/covfull-gate.log"
echo "[covfull $(st)] === S36-A modules ==="
python3 - "$OUT/cov-full.lcov" > "$OUT/covfull-modules.txt" 2>&1 <<'PY'
import sys
tot={}; hit={}; cur=None
for ln in open(sys.argv[1]):
    if ln.startswith('SF:'): cur=ln[3:].strip(); tot.setdefault(cur,0); hit.setdefault(cur,0)
    elif ln.startswith('DA:') and cur:
        c=int(ln[3:].split(',')[1]); tot[cur]+=1; hit[cur]+=(1 if c>0 else 0)
want=('conn_actor.rs','lb-quic/src/listener.rs','lb-config/src/lib.rs','quic_h3_recycle_metrics.rs')
for f in sorted(tot):
    if any(w in f for w in want) and tot[f]:
        print(f"{hit[f]/tot[f]*100:6.2f}%  {hit[f]:4}/{tot[f]:<4}  {f.split('/crates/')[-1]}")
PY
cat "$OUT/covfull-modules.txt"
echo "DONE gate=$GATE $(date -u)" > "$OUT/covfull.marker"
