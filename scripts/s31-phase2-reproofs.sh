#!/usr/bin/env bash
# S31 Phase-2 R8 re-proofs + R13 F-MD-4 bursts on quiche 0.29.1 / Rust 1.88.
# Captures gauge EVIDENCE (--nocapture) so the bound is shown non-vacuous + body-
# size-independent on 0.29, and stresses the reset-vs-EOF mapping with an OUTER
# loop ON TOP of each burst test's built-in ITERS=60 (>=50 per R13).
# R15: each verdict read from the COMPLETED run (sentinels). Usage: s31-phase2-reproofs.sh
set -u
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
cd /home/ubuntu/Code/ExpressGateway || exit 99
OUT=audit/deps/s31-phase2
mkdir -p "$OUT"

run() { # run <logfile> <test-name-filters...>
  local log="$1"; shift
  echo "=== cargo test --workspace --all-features [$*] $(date -u +%FT%TZ) ===" | tee -a "$log"
  cargo test --workspace --all-features "$@" -- --nocapture --test-threads=1 >> "$log" 2>&1
  local rc=${PIPESTATUS[0]}
  echo "RC=$rc" | tee -a "$log"
  return $rc
}

# ---------- R8 re-proofs (body-size-independent bound + backpressure, all 4 quiche paths) ----------
R8LOG="$OUT/r8-reproofs.log"; : > "$R8LOG"
echo "##### R8 RE-PROOFS (0.29) #####" | tee -a "$R8LOG"
run "$R8LOG" \
  t5_single_large_data_frame_is_memory_bounded_through_stalled_upstream \
  r2_response_memory_bounded_through_stalled_client \
  r3_slow_client_backpressures_upstream_read \
  s16_b2_backpressure_client_throttled_then_complete_on_resume \
  ws_over_h3_outbound_backpressure_plateaus_then_drains \
  grpc_h3_server_stream_bounded_memory_r8
R8RC=$?
echo "=== R8 gauge evidence (plateau / bounded / retained lines) ===" | tee -a "$R8LOG"
grep -iE 'plateau|bounded|retained|ceiling|R8|backpress|delta|peak' "$R8LOG" | grep -viE 'cargo test|^RC=' | tee -a "$R8LOG"
echo "S31-R8-REPROOFS-DONE rc=$R8RC" | tee -a "$R8LOG"

# ---------- R13 F-MD-4 reset-vs-EOF: bursts (E1+E2) + reset-not-clean + NEGATIVE CONTROLS ----------
# Outer loop x3 on top of each test's internal ITERS=60 => >=180 effective burst iters.
R13LOG="$OUT/r13-fmd4-bursts.log"; : > "$R13LOG"
echo "##### R13 F-MD-4 RESET-vs-EOF (0.29) — outer x3 over built-in ITERS=60 #####" | tee -a "$R13LOG"
R13FAIL=0
for round in 1 2 3; do
  echo "########## OUTER ROUND $round/3 $(date -u +%FT%TZ) ##########" | tee -a "$R13LOG"
  run "$R13LOG" \
    h3h3_e2e_client_reset_midrequest_burst_current_thread \
    h3h3_e2e_upstream_reset_midresponse_burst_current_thread \
    h1h3_fmd4_response_truncation_burst_current_thread \
    h1h3_fmd4_response_truncation_chunked_burst_current_thread \
    h1h3_fmd4_truncation_burst_current_thread \
    h2h3_fmd4_response_truncation_chunked_burst_current_thread \
    grpc_h3_burst_50_unary_cycles \
    ws_over_h3_burst_50_upgrade_relay_close_cycles || R13FAIL=1
done
echo "########## R13 single-shot reset-not-clean + NEGATIVE CONTROLS ##########" | tee -a "$R13LOG"
run "$R13LOG" \
  h3h3_e2e_client_reset_midrequest_rsts_upstream_no_truncated_request \
  h3h3_e2e_upstream_reset_midbody_resets_client_no_fin \
  h2h3_fmd4_response_truncation_cl_never_false_complete \
  h3h3_e2e_content_length_truncation_resets_no_clean_complete \
  grpc_h3_backend_reset_midresponse_not_laundered_to_clean_status \
  s16_b2_client_reset_does_not_become_clean_fin_upstream \
  discriminator_clean_fin_is_observable_on_happy_path \
  r4_empty_response_body_clean_fin \
  h3h3_e2e_no_cl_truncated_data_delivered_quiche_028_frame_completeness_gap || R13FAIL=1
echo "=== R13 smuggling/clean-fin evidence ===" | tee -a "$R13LOG"
grep -iE 'BURST|smuggl|baseline|after_burst|clean.fin|never.*complete|reset' "$R13LOG" | grep -viE 'cargo test|^RC=|^===' | tail -40 | tee -a "$R13LOG"
echo "S31-R13-FMD4-DONE r13fail=$R13FAIL" | tee -a "$R13LOG"

echo "S31-PHASE2-REPROOFS-COMPLETE r8=$R8RC r13fail=$R13FAIL $(date -u +%FT%TZ)"
