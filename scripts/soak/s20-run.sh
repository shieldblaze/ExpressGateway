#!/usr/bin/env bash
# S20 primary soak launcher — the exact, reproducible run definition.
#
# Launches every soak scenario concurrently against the real release
# expressgateway binary, each as its own eg-soak process writing its own
# per-scenario time-series CSV + verdict.json into OUT. Mode B is run in TWO
# configs: the default 4-concurrent-stream config (exposes the multi-stream
# relay stall under sustained load) and a healthy 1-stream baseline
# (QUIC_STREAMS=1) for clean bounded-state evidence on the working path.
#
# Stability soak (NOT throughput): co-located load+gateway is acceptable (owner
# ruling). Read the verdict ONLY from the COMPLETED run (R15).
#
# Usage: scripts/soak/s20-run.sh <duration_secs> <out_dir> [sample_secs]
set -uo pipefail

DUR="${1:?usage: s20-run.sh <duration_secs> <out_dir> [sample_secs]}"
OUT="${2:?out_dir required}"
SAMPLE="${3:-15}"

: "${CARGO_TARGET_DIR:?export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target}"
EG="$CARGO_TARGET_DIR/release/eg-soak"
[ -x "$EG" ] || { echo "release eg-soak not found: $EG" >&2; exit 1; }
[ -x "$CARGO_TARGET_DIR/release/expressgateway" ] || { echo "release expressgateway not found" >&2; exit 1; }
# Quiet gateway logs for a long run (warn only); the CSV is the durable record.
unset RUST_LOG

mkdir -p "$OUT"
echo "[s20-run] start $(date -u) dur=${DUR}s sample=${SAMPLE}s out=$OUT"

declare -a PIDS=() NAMES=()
launch() { # name, env-assignments, extra-args...
  local name="$1"; shift
  local envassign="$1"; shift
  env $envassign "$EG" --duration-secs "$DUR" --sample-secs "$SAMPLE" --out "$OUT" "$@" \
      > "$OUT/$name.stdout.log" 2>&1 &
  PIDS+=("$!"); NAMES+=("$name")
  echo "[s20-run] launched $name pid=$!"
  sleep 1   # stagger ephemeral-port reservation
}

launch sc1_h1h1          ""               --scenario sc1_h1h1
launch sc1b_h1h2         ""               --scenario sc1b_h1h2
launch sc2_h2h2          ""               --scenario sc2_h2h2
launch sc3_slowloris     ""               --scenario sc3_slowloris
launch sc4_modeb         ""               --scenario sc4_modeb
launch sc4b_modeb_healthy "QUIC_STREAMS=1" --scenario sc4_modeb --label sc4b_modeb_healthy
launch sc5_modea         ""               --scenario sc5_modea
launch sc6_413teardown   ""               --scenario sc6_413teardown

echo "[s20-run] all ${#PIDS[@]} scenarios launched; waiting for completion (the soak)…"
for i in "${!PIDS[@]}"; do
  if wait "${PIDS[$i]}"; then echo "[s20-run] ${NAMES[$i]} OK"; else echo "[s20-run] ${NAMES[$i]} rc=$?"; fi
done

echo "[s20-run] done $(date -u). markers:"
for n in "${NAMES[@]}"; do
  m="$OUT/$n.soak_complete.marker"
  [ -f "$m" ] && cat "$m" || echo "MISSING MARKER: $n"
done
echo "[s20-run] COMPLETE"
