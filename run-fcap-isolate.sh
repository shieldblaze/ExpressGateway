#!/usr/bin/env bash
# S37-C: isolate-confirm the 3 known throughput flakes (R2) on 6e43b09c, serialized + off-load,
# to prove the pass_runs=1/3 ×3 failures are CF-FCAP1-FLAKE / fcap1-family / CF-S35-T5-FLAKE,
# not a C-seam regression. Each run alone (--test-threads=1), 3 iterations; want consistent pass.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s37-verify || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify/audit/ops/s37-verifyC-lead
mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
TESTS="fcap1_h2_over_cap_upload_yields_413 h2h3_fcap1_over_cap_upload_never_complete t5_single_large_data_frame_is_memory_bounded_through_stalled_upstream"
echo "[iso $(st)] HEAD $(git rev-parse --short HEAD); isolating known flakes serialized" | tee "$OUT/isolate.log"
for t in $TESTS; do
  P=0
  for i in 1 2 3; do
    cargo test --workspace --all-features "$t" -- --test-threads=1 > "$OUT/iso-$t-$i.log" 2>&1
    rc=$?
    res=$(grep -hE "test result:" "$OUT/iso-$t-$i.log" | tail -1)
    [ $rc -eq 0 ] && P=$((P+1))
    echo "[iso $(st)] $t iter$i rc=$rc :: $res" | tee -a "$OUT/isolate.log"
  done
  echo "[iso $(st)] === $t isolated pass=$P/3 ===" | tee -a "$OUT/isolate.log"
done
echo "DONE $(date -u)" > "$OUT/isolate.marker"
echo "[iso $(st)] ALL DONE" | tee -a "$OUT/isolate.log"