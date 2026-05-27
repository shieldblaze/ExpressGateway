#!/usr/bin/env bash
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
cd /home/ubuntu/Code/ExpressGateway
D=audit/h-matrix/s12-evidence
for i in 1 2 3; do
  echo "===== POSTFIX RUN$i START $(date -u +%H:%M:%S) ====="
  cargo test --workspace --all-features > "$D/postfix-run$i.log" 2>&1
  rc=$?
  fc=$(grep -E 'fcap1_h2_over_cap_upload_yields_413 \.\.\. (ok|FAILED)' "$D/postfix-run$i.log" | head -1)
  fails=$(grep -E 'test result: FAILED' "$D/postfix-run$i.log" | wc -l)
  echo "POSTFIX_RUN${i}_EXIT=$rc  fcap1=[$fc]  failed_suites=$fails"
done
echo "===== POSTFIX DONE $(date -u +%H:%M:%S) ====="
