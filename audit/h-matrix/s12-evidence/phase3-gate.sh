#!/usr/bin/env bash
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
cd /home/ubuntu/Code/ExpressGateway
D=audit/h-matrix/s12-evidence
for i in 1 2 3; do
  echo "===== PHASE3 RUN$i START $(date -u +%H:%M:%S) ====="
  cargo test --workspace --all-features > "$D/phase3-run$i.log" 2>&1
  rc=$?
  fails=$(grep -cE 'test result: FAILED' "$D/phase3-run$i.log")
  oks=$(grep -cE 'test result: ok\.' "$D/phase3-run$i.log")
  # spot-check the session's load-bearing smuggle/header tests
  h1h3=$(grep -oE 'h1h3_fmd4_(truncation_burst_current_thread|response_truncation_chunked_burst_current_thread) \.\.\. (ok|FAILED)' "$D/phase3-run$i.log" | tr '\n' ' ')
  h3conv=$(grep -oE 'cf_h3_head_h3_to_h[12]_full_response_headers_round_trip \.\.\. (ok|FAILED)' "$D/phase3-run$i.log" | tr '\n' ' ')
  fcap=$(grep -oE 'fcap1_h2_over_cap_upload_yields_413 \.\.\. (ok|FAILED)' "$D/phase3-run$i.log" | head -1)
  echo "PHASE3_RUN${i}_EXIT=$rc  failed_suites=$fails  ok_suites=$oks"
  echo "  fcap1=[$fcap]"
  echo "  h1h3_bursts=[$h1h3]"
  echo "  h3_conv=[$h3conv]"
done
echo "===== CLIPPY START $(date -u +%H:%M:%S) ====="
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$D/phase3-clippy.log" 2>&1
echo "CLIPPY_EXIT=$?"
echo "===== FMT START $(date -u +%H:%M:%S) ====="
cargo fmt --check > "$D/phase3-fmt.log" 2>&1
echo "FMT_EXIT=$?"
echo "===== PHASE3 DONE $(date -u +%H:%M:%S) ====="
