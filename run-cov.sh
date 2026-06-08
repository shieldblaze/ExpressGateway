#!/usr/bin/env bash
# S36-A scoped per-module coverage on 4480fb83 (lead-run). Charter D-6 hot-path
# gate asserts lb-quic/src/(conn_actor|listener).rs >= 80%. Scope to the changed
# crates; CI's full-workspace D6 is the authoritative gate on push.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/audit/ops/s36-aimpl-cov; mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root|tail -1|tr -dc 0-9; }
echo "[cov $(st)] HEAD $(git rev-parse --short HEAD); disk $(fg)G"
echo "[cov $(st)] llvm-cov nextest -p lb-quic -p lb-config -p lb-observability --all-features --ignore-run-fail"
cargo llvm-cov nextest --all-features --ignore-run-fail \
  -p lb-quic -p lb-config -p lb-observability \
  --lcov --output-path "$OUT/cov.lcov" > "$OUT/cov-run.log" 2>&1
RC=$?
echo "[cov $(st)] llvm-cov rc=$RC; disk $(fg)G; lcov lines=$(wc -l < "$OUT/cov.lcov" 2>/dev/null || echo 0)"
if [ ! -s "$OUT/cov.lcov" ]; then echo "COV_FAIL no lcov" > "$OUT/cov.marker"; tail -8 "$OUT/cov-run.log"; exit 1; fi
echo "[cov $(st)] charter per-module gate (coverage-check.sh):"
bash scripts/ci/coverage-check.sh "$OUT/cov.lcov" > "$OUT/cov-gate.log" 2>&1; GATE=$?
cat "$OUT/cov-gate.log"
echo "[cov $(st)] === per-module extract (conn_actor/listener + new config/metric) ==="
python3 - "$OUT/cov.lcov" > "$OUT/cov-modules.txt" 2>&1 <<'PY'
import sys,re
cur=None; tot={}; hit={}
for ln in open(sys.argv[1]):
    if ln.startswith('SF:'): cur=ln[3:].strip(); tot.setdefault(cur,0); hit.setdefault(cur,0)
    elif ln.startswith('DA:') and cur:
        _,c=ln[3:].split(',')[:2]; tot[cur]+=1; hit[cur]+= (1 if int(c)>0 else 0)
want=('conn_actor.rs','listener.rs','lb-config/src/lib.rs','quic_h3_recycle_metrics.rs')
for f in sorted(tot):
    if any(w in f for w in want) and tot[f]:
        print(f"{hit[f]/tot[f]*100:6.2f}%  {hit[f]:4}/{tot[f]:<4}  {f}")
PY
cat "$OUT/cov-modules.txt"
echo "DONE gate=$GATE $(date -u)" > "$OUT/cov.marker"
