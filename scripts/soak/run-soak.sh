#!/usr/bin/env bash
# S20 chaos/soak runner — launch every soak scenario concurrently against the
# real expressgateway binary, each as its own eg-soak process writing its own
# per-scenario time-series CSV + verdict.json to OUT_DIR.
#
# Stability soak (NOT throughput): co-located load+gateway is acceptable (owner
# ruling). Each scenario isolates its own gateway child so per-scenario RSS/fd
# are clean. Read the verdict ONLY from the completed run (R15).
#
# Usage:
#   scripts/soak/run-soak.sh <duration_secs> <out_dir> [sample_secs] [scale] [scenarios...]
#
# Env:
#   CARGO_TARGET_DIR must point at the shared target dir (eg-target).
set -uo pipefail

DURATION="${1:?usage: run-soak.sh <duration_secs> <out_dir> [sample_secs] [scale] [scenarios...]}"
OUT="${2:?out_dir required}"
SAMPLE="${3:-15}"
SCALE="${4:-1}"
shift $(( $# < 4 ? $# : 4 ))
SCENARIOS=("$@")
if [ "${#SCENARIOS[@]}" -eq 0 ]; then
  SCENARIOS=(sc1_h1h1 sc1b_h1h2 sc2_h2h2 sc3_slowloris sc4_modeb sc5_modea sc6_413teardown)
fi

: "${CARGO_TARGET_DIR:?export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target}"
EGSOAK="$CARGO_TARGET_DIR/release/eg-soak"
[ -x "$EGSOAK" ] || EGSOAK="$CARGO_TARGET_DIR/debug/eg-soak"
[ -x "$EGSOAK" ] || { echo "eg-soak binary not found; build it first" >&2; exit 1; }

mkdir -p "$OUT"
echo "[run-soak] start $(date -u) duration=${DURATION}s sample=${SAMPLE}s scale=${SCALE} scenarios=${SCENARIOS[*]}"
echo "[run-soak] eg-soak=$EGSOAK out=$OUT"

declare -a PIDS=()
for sc in "${SCENARIOS[@]}"; do
  # Bounded per-scenario stdout log (heartbeats); the CSV is the durable record.
  "$EGSOAK" --scenario "$sc" --duration-secs "$DURATION" --sample-secs "$SAMPLE" \
            --scale "$SCALE" --out "$OUT" > "$OUT/$sc.stdout.log" 2>&1 &
  PIDS+=("$!")
  echo "[run-soak] launched $sc pid=$! "
  sleep 1   # stagger ephemeral-port reservation
done

echo "[run-soak] all launched; waiting for completion (this is the soak)…"
FAIL=0
for i in "${!PIDS[@]}"; do
  pid="${PIDS[$i]}"
  if wait "$pid"; then
    echo "[run-soak] scenario ${SCENARIOS[$i]} (pid $pid) completed OK"
  else
    rc=$?
    echo "[run-soak] scenario ${SCENARIOS[$i]} (pid $pid) exited rc=$rc"
    FAIL=$((FAIL+1))
  fi
done

echo "[run-soak] done $(date -u). markers:"
for sc in "${SCENARIOS[@]}"; do
  m="$OUT/$sc.soak_complete.marker"
  if [ -f "$m" ]; then cat "$m"; else echo "MISSING MARKER: $sc"; FAIL=$((FAIL+1)); fi
done
echo "[run-soak] failures=$FAIL"
exit 0
