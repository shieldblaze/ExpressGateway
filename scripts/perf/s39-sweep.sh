#!/usr/bin/env bash
# S39 perf campaign — concurrency sweep per protocol, on the FRESH/QUIET box.
#
# For each (protocol, concurrency): one eg-bench closed-loop run → achieved RPS
# + p50/p99/p999 + gateway CPU%/RSS/fd. The sweep finds the throughput knee
# (rps plateaus / gateway CPU saturates) and the latency at moderate load.
# CO-LOCATED on one 8-core box (client+gateway+backend share cores): throughput
# is a SYSTEM number (caveat in the report); the box-independent signal is the
# gateway CPU-us/request (cpu% / rps). Completed runs only (R15).
#
# Usage: scripts/perf/s39-sweep.sh <out_dir> [protocols...]
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
BIN="$CARGO_TARGET_DIR/release/eg-bench"
[ -x "$BIN" ] || { echo "build eg-bench first" >&2; exit 1; }

OUT="${1:?out_dir required}"; shift || true
mkdir -p "$OUT"
PROTOS=("$@"); [ "${#PROTOS[@]}" -eq 0 ] && PROTOS=(h1 h2 h3 quic_modea ws_h1)
CONCS=(1 8 32 64 128)
MEASURE="${MEASURE:-15}"; WARMUP="${WARMUP:-5}"; PAYLOAD="${PAYLOAD:-0}"

SUMMARY="$OUT/sweep-summary.txt"
echo "[s39-sweep $(date -u)] protocols=${PROTOS[*]} concs=${CONCS[*]} measure=${MEASURE}s warmup=${WARMUP}s payload=${PAYLOAD}" | tee "$SUMMARY"
for p in "${PROTOS[@]}"; do
  echo "--- $p ---" | tee -a "$SUMMARY"
  for c in "${CONCS[@]}"; do
    line=$(timeout 120 "$BIN" --protocol "$p" --connections "$c" \
            --duration-secs "$MEASURE" --warmup-secs "$WARMUP" --payload "$PAYLOAD" \
            --out "$OUT" --label "$p" 2>/dev/null | grep -E "^$p|gateway:")
    # eg-bench overwrites <label>-c<C>.json; rename to keep each concurrency.
    [ -f "$OUT/$p-c$c.json" ] && cp "$OUT/$p-c$c.json" "$OUT/$p-c$c.json"
    echo "$line" | tr '\n' ' ' | sed 's/  */ /g' | tee -a "$SUMMARY"
    echo "" | tee -a "$SUMMARY"
  done
done
echo "[s39-sweep $(date -u)] DONE" | tee -a "$SUMMARY"
