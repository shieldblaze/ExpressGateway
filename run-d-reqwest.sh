#!/usr/bin/env bash
# S37-D stage 6: reqwest 0.13 (dev) compile + RUN the 2 consumer tests (catch the aws-lc/ring
# crypto-provider runtime risk that --no-run can't see). + settle idna_adapter/time.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s37-deps || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify/audit/ops/s37-d-lead
mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root|tail -1|tr -dc 0-9; }
echo "[req $(st)] settle idna_adapter + reqwest; disk $(fg)G" | tee "$OUT/reqwest.console"
cargo update -p idna_adapter -p reqwest > "$OUT/req-update.log" 2>&1
for c in reqwest idna_adapter time; do echo -n "  $c: " | tee -a "$OUT/reqwest.console"; awk -v n="$c" '$1=="name" && $3=="\""n"\""{getline; gsub(/"/,"",$3); printf "%s ",$3}' Cargo.lock | tee -a "$OUT/reqwest.console"; echo | tee -a "$OUT/reqwest.console"; done
echo "[req $(st)] compile-confirm (--no-run); disk $(fg)G" | tee -a "$OUT/reqwest.console"
cargo test --workspace --all-features --no-run > "$OUT/req-norun.log" 2>&1
RC=$?
echo "[req $(st)] --no-run rc=$RC" | tee -a "$OUT/reqwest.console"
if [ $RC -ne 0 ]; then echo "COMPILE ERRORS:" | tee -a "$OUT/reqwest.console"; grep -E 'error(\[|:)' "$OUT/req-norun.log" | head -15 | tee -a "$OUT/reqwest.console"; echo "DONE norun_rc=$RC" > "$OUT/reqwest.marker"; exit 1; fi
echo "[req $(st)] RUN the 2 reqwest consumer tests (crypto-provider runtime check)" | tee -a "$OUT/reqwest.console"
cargo test --all-features --test h2_proxy_e2e --test h2h1_md_streaming_verify -- --test-threads=1 > "$OUT/req-tests.log" 2>&1
TRC=$?
echo "[req $(st)] reqwest-tests rc=$TRC :: $(grep -hE 'test result:' "$OUT/req-tests.log" | awk '{p+=$4;f+=$6} END{printf "passed=%d failed=%d",p,f}')" | tee -a "$OUT/reqwest.console"
if [ $TRC -ne 0 ]; then echo "TEST FAILURES (crypto provider?):" | tee -a "$OUT/reqwest.console"; grep -iE 'FAILED|cryptoprovider|panic|no process-level' "$OUT/req-tests.log" | head -10 | tee -a "$OUT/reqwest.console"; fi
echo "DONE norun_rc=$RC tests_rc=$TRC" > "$OUT/reqwest.marker"
echo "[req $(st)] DONE norun=$RC tests=$TRC" | tee -a "$OUT/reqwest.console"