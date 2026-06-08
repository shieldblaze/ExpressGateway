#!/usr/bin/env bash
# S37-D attribution + fix-test: revert tokio 1.52.3 -> 1.51.1 (suspect: 1.52 LIFO-revert
# regressed the H2->H3 relay throughput; h2h3_fcap1 stalls ~0.5MB/s on a quiet box on D,
# passed pre-D). Clean rebuild, then isolate h2h3_fcap1 (3x) + grpc_h3_trailer (2x).
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s37-deps || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify/audit/ops/s37-p4
st(){ date -u +%H:%M:%S; }
echo "[rev $(st)] revert tokio -> 1.51.1" | tee "$OUT/tokio-revert.console"
cargo update -p tokio --precise 1.51.1 > "$OUT/tokio-revert-update.log" 2>&1
echo "  tokio now: $(awk '$1=="name" && $3=="\"tokio\""{getline; gsub(/"/,"",$3); print $3}' Cargo.lock)" | tee -a "$OUT/tokio-revert.console"
echo "[rev $(st)] clean + rebuild test binaries" | tee -a "$OUT/tokio-revert.console"
rm -rf /home/ubuntu/Code/eg-target/debug 2>/dev/null
cargo test --workspace --all-features --no-run > "$OUT/tokio-revert-build.log" 2>&1
if [ $? -ne 0 ]; then echo "[rev $(st)] BUILD FAIL" | tee -a "$OUT/tokio-revert.console"; grep -E 'error' "$OUT/tokio-revert-build.log"|head | tee -a "$OUT/tokio-revert.console"; echo BUILD_FAIL > "$OUT/tokio-revert.marker"; exit 1; fi
echo "[rev $(st)] build done; isolate h2h3_fcap1 x3 (tokio 1.51.1)" | tee -a "$OUT/tokio-revert.console"
for i in 1 2 3; do
  timeout 120 cargo test --workspace --all-features h2h3_fcap1_over_cap_upload_never_complete -- --test-threads=1 --nocapture > "$OUT/rev-h2h3fcap1-$i.log" 2>&1
  echo "  iter$i: $(grep -hE 'test result:' "$OUT/rev-h2h3fcap1-$i.log"|tail -1) :: $(grep -hoE 'sent=[0-9]+|cap-[a-z()]+' "$OUT/rev-h2h3fcap1-$i.log"|tr '\n' ' ')" | tee -a "$OUT/tokio-revert.console"
done
echo "[rev $(st)] grpc_h3_trailer x2" | tee -a "$OUT/tokio-revert.console"
for i in 1 2; do
  timeout 120 cargo test -p lb-quic --test grpc_h3_e2e --all-features grpc_h3_trailer_survives_all_response_sizes -- --test-threads=1 > "$OUT/rev-grpc-$i.log" 2>&1
  echo "  grpc iter$i: $(grep -hE 'test result:' "$OUT/rev-grpc-$i.log"|tail -1)" | tee -a "$OUT/tokio-revert.console"
done
echo "DONE $(date -u)" > "$OUT/tokio-revert.marker"
echo "[rev $(st)] DONE" | tee -a "$OUT/tokio-revert.console"
