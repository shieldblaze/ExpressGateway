#!/usr/bin/env bash
# S37 Phase-4 full re-soak (all scenarios BOUNDED on B+C+D, tokio 1.51.1). Conservative
# concurrency (<=4/batch) to avoid analyzer saturation false-DRIFT; sc9 alone @1800s for a
# direct compare to the S36 BOUNDED ~22MB reference. run-soak.sh blocks per batch (it waits).
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify/audit/soak/s37-soak-data
mkdir -p "$OUT"
RS=scripts/soak/run-soak.sh
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root|tail -1|tr -dc 0-9; }
EGS=/home/ubuntu/Code/eg-target/debug/eg-soak
echo "[resoak $(st)] HEAD $(git rev-parse --short HEAD); disk $(fg)G; building eg-soak bin (cargo test --no-run skips bins)" | tee "$OUT/resoak.console"
cargo build -p lb-soak --bin eg-soak --all-features >> "$OUT/resoak.console" 2>&1
[ -x "$EGS" ] || { echo "[resoak $(st)] eg-soak BUILD FAILED" | tee -a "$OUT/resoak.console"; echo "EGSOAK_BUILD_FAIL" > "$OUT/resoak.marker"; exit 1; }
echo "[resoak $(st)] eg-soak present; disk $(fg)G" | tee -a "$OUT/resoak.console"
run_batch(){ local dur="$1"; shift; echo "[resoak $(st)] BATCH dur=${dur}s: $* ; disk $(fg)G" | tee -a "$OUT/resoak.console"; bash "$RS" "$dur" "$OUT" 12 1 "$@" >> "$OUT/resoak.console" 2>&1; echo "[resoak $(st)] batch rc=$? done; disk $(fg)G" | tee -a "$OUT/resoak.console"; }
run_batch 900  sc1_h1h1 sc1b_h1h2 sc2_h2h2 sc3_slowloris
run_batch 900  sc4_modeb sc5_modea sc6_413teardown sc7_h3terminate
run_batch 900  sc8_ws_h1 sc8b_ws_h2 sc8c_ws_h3
run_batch 1800 sc9_grpc_h3
echo "[resoak $(st)] === VERDICTS ===" | tee -a "$OUT/resoak.console"
DRIFT=0
for f in "$OUT"/*.verdict.json; do
  [ -f "$f" ] || continue
  sc=$(basename "$f" .verdict.json)
  ov=$(python3 -c "import json,sys; print(json.load(open('$f')).get('overall','?'))" 2>/dev/null || echo '?')
  echo "  $sc: $ov" | tee -a "$OUT/resoak.console"
  [ "$ov" != "BOUNDED" ] && DRIFT=$((DRIFT+1))
done
echo "[resoak $(st)] sc9 rss_kb (vs S36 ref last~22036):" | tee -a "$OUT/resoak.console"
python3 -c "import json; d=json.load(open('$OUT/sc9_grpc_h3.verdict.json')); c=[x for x in d['columns'] if x['column']=='rss_kb'][0]; print('  rss_kb',c['verdict'],'first=%s last=%s max=%s'%(c['first'],c['last'],c['max']))" 2>/dev/null | tee -a "$OUT/resoak.console" || echo "  (sc9 verdict parse failed)" | tee -a "$OUT/resoak.console"
echo "DONE non_bounded=$DRIFT $(date -u)" > "$OUT/resoak.marker"
echo "[resoak $(st)] ALL DONE non_bounded=$DRIFT" | tee -a "$OUT/resoak.console"