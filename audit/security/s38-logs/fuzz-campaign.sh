#!/usr/bin/env bash
# S38 fuzz campaign driver. Runs AFTER the ×3 baseline (R9: stagger from the gate — OOM/disk).
# cargo-fuzz on the pinned nightly. Each target time-boxed; crashes saved + reported (R15: cite
# iterations actually reached; a killed run != "no crashes"). Production-critical target
# (quic_public_header — the Mode A parser) gets the longest box.
set -uo pipefail
# cargo-fuzz manages its OWN target dir (fuzz/target). Do NOT redirect it into the
# shared eg-target — that mixes nightly+ASAN artifacts into the stable build tree and
# inflates eg-target. Explicitly unset so fuzz/target is used (isolated + cleanable).
unset CARGO_TARGET_DIR
cd "$(dirname "$0")/../../.." || exit 99
LOG=audit/security/s38-logs
NIGHTLY=nightly-2026-01-15
ts() { date '+%H:%M:%S'; }

# target:seconds:rss_mb  — production-critical Mode A parser gets the longest box;
# test-codec + delegated-boundary targets are confirmatory (120s) since the prod
# parser is already hand-proven + covered by the `ours_never_panics` proptest.
CAMPAIGNS=(
  "quic_public_header:400:2048"
  "h1_chunked:60:2048"
  "h2_hpack:60:2048"
  "h1_request_line:60:2048"
  "h1_parser:60:2048"
  "h2_frame:60:2048"
  "h3_frame:60:2048"
  "quic_initial:60:2048"
  "tls_client_hello:60:2048"
)
# CF-DISK-1 disk guard: abort the campaign if free space drops below ~3 GiB.
disk_guard() {
  local free_kb; free_kb=$(df --output=avail / | tail -1)
  if [ "${free_kb:-0}" -lt 3145728 ]; then
    echo "[$(ts)] DISK GUARD: <3GiB free — aborting remaining fuzz to protect the box (CF-DISK-1)"
    return 1
  fi
}

echo "[$(ts)] fuzz campaign start; disk:" ; df -h / | tail -1
SUMMARY="$LOG/fuzz-summary.txt"; : > "$SUMMARY"
for entry in "${CAMPAIGNS[@]}"; do
  disk_guard || break
  tgt="${entry%%:*}"; rest="${entry#*:}"; secs="${rest%%:*}"; rss="${rest##*:}"
  echo "[$(ts)] === fuzz $tgt for ${secs}s (rss ${rss}MB) ==="
  out="$LOG/fuzz-$tgt.txt"
  # -workers/-jobs=4: use 4 of 8 cores per target (leave headroom). -print_final_stats for iters.
  cargo +$NIGHTLY fuzz run "$tgt" -- \
      -max_total_time="$secs" -rss_limit_mb="$rss" -workers=4 -jobs=4 \
      -print_final_stats=1 > "$out" 2>&1
  rc=$?
  # libFuzzer crash artifacts land in fuzz/artifacts/<tgt>/. Iterations from the merged worker logs.
  crashes=$(ls fuzz/artifacts/"$tgt"/ 2>/dev/null | grep -cE 'crash-|oom-|timeout-' || true)
  iters=$(grep -hoE 'stat::number_of_executed_units: [0-9]+' "$out" fuzz/*.log 2>/dev/null | grep -oE '[0-9]+' | paste -sd+ | bc 2>/dev/null || echo "?")
  echo "[$(ts)] $tgt rc=$rc crashes=$crashes iters=$iters" | tee -a "$SUMMARY"
  # reap stray worker logs to control disk (CF-DISK-1)
  rm -f fuzz/fuzz-*.log 2>/dev/null
done
echo "[$(ts)] fuzz campaign DONE; disk:"; df -h / | tail -1
echo "=== SUMMARY ==="; cat "$SUMMARY"
echo "=== crash artifacts ==="; find fuzz/artifacts -type f 2>/dev/null | head -50
