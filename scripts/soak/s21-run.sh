#!/usr/bin/env bash
# S21 re-soak launcher — the clean re-soak (shippable-v1 gate).
#
# Why batches: the 8-core box cannot co-locate all 8 scenarios at the harness's
# baked concurrencies without OS-thrash (load ~32 = 4x oversubscription) — that
# measures the BOX, not the GATEWAY (the S20 run1 anti-pattern; R9/R2). S20 run2
# avoided it by removing sc5 + halving concurrency. S21's F-S20-2 fix makes Mode A
# churn FASTER (working streams via mint_retry=false), adding crypto load, so even
# 8-co-located saturates. We instead run SEQUENTIAL co-located BATCHES of 2-3
# scenarios — each gets a sustained, co-located, NON-saturated run. Verdict from the
# COMPLETED run only (R15).
#
# Usage: scripts/soak/s21-run.sh <per_batch_secs> <out_dir> [sample_secs]
set -uo pipefail

DUR="${1:?usage: s21-run.sh <per_batch_secs> <out_dir> [sample_secs]}"
OUT="${2:?out_dir required}"
SAMPLE="${3:-15}"

: "${CARGO_TARGET_DIR:?export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target}"
EG="$CARGO_TARGET_DIR/release/eg-soak"
[ -x "$EG" ] || { echo "release eg-soak not found: $EG" >&2; exit 1; }
[ -x "$CARGO_TARGET_DIR/release/expressgateway" ] || { echo "release expressgateway not found" >&2; exit 1; }
unset RUST_LOG
mkdir -p "$OUT"
echo "[s21-run] start $(date -u) per_batch=${DUR}s sample=${SAMPLE}s out=$OUT"

run_batch() { # batch_name, then "name env-assigns extra-args" triples via stdin lines
  local batch="$1"; shift
  declare -a PIDS=() NAMES=()
  echo "[s21-run] === batch $batch === $(date -u)"
  while (( "$#" >= 3 )); do
    local name="$1" envassign="$2" extra="$3"; shift 3
    # shellcheck disable=SC2086
    env $envassign "$EG" --duration-secs "$DUR" --sample-secs "$SAMPLE" --out "$OUT" $extra \
        > "$OUT/$name.stdout.log" 2>&1 &
    PIDS+=("$!"); NAMES+=("$name")
    echo "[s21-run]   launched $name pid=$!"
    sleep 1
  done
  for i in "${!PIDS[@]}"; do
    if wait "${PIDS[$i]}"; then echo "[s21-run]   ${NAMES[$i]} OK"; else echo "[s21-run]   ${NAMES[$i]} rc=$?"; fi
  done
  echo "[s21-run]   batch $batch load: $(uptime | sed 's/.*load average/load/')"
}

# Batch 1 — light/idle TCP fronts (H1, H1->H2, slowloris).
run_batch B1-tcp \
  sc1_h1h1      ""  "--scenario sc1_h1h1" \
  sc1b_h1h2     ""  "--scenario sc1b_h1h2" \
  sc3_slowloris ""  "--scenario sc3_slowloris"

# Batch 2 — TLS+H2 crypto fronts (rapid-reset/stream-flood, oversize+teardown).
run_batch B2-tls \
  sc2_h2h2        ""  "--scenario sc2_h2h2" \
  sc6_413teardown ""  "--scenario sc6_413teardown"

# Batch 3 — QUIC terminate/relay (Mode B): 4-stream (F-S20-1 fixed path) + healthy 1-stream.
run_batch B3-modeb \
  sc4_modeb          ""               "--scenario sc4_modeb" \
  sc4b_modeb_healthy "QUIC_STREAMS=1 QUIC_CONCURRENCY=3" "--scenario sc4_modeb --label sc4b_modeb_healthy"

# Batch 4 — QUIC passthrough (Mode A): F-S20-2 idle-reaper verdict (isolated; the
# historical resource polluter, now bounded by the sweep).
run_batch B4-modea \
  sc5_modea "QUIC_CONCURRENCY=2" "--scenario sc5_modea"

echo "[s21-run] done $(date -u). markers:"
for n in sc1_h1h1 sc1b_h1h2 sc3_slowloris sc2_h2h2 sc6_413teardown sc4_modeb sc4b_modeb_healthy sc5_modea; do
  m="$OUT/$n.soak_complete.marker"
  [ -f "$m" ] && echo "OK $n" || echo "MISSING MARKER: $n"
done
echo "[s21-run] COMPLETE"
