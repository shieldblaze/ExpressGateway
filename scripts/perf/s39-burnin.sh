#!/usr/bin/env bash
# S39 Phase 2 — EXTENDED burn-in: the full 12-scenario lb-soak under sustained
# load for HOURS (longer than the prior ~1800s spot-checks) — the slow-leak /
# fd-growth / degradation watch (the "pages after days of uptime" class). R8:
# memory/fd/conn must stay BOUNDED over the FULL duration; verdict from the
# COMPLETED run only (R15). All 12 scenarios run concurrently at scale=1 (the
# established S31/S33 full-suite pattern; each scenario is light at scale=1 and
# isolates its own gateway child so per-scenario RSS/fd are clean).
#
# Usage: scripts/perf/s39-burnin.sh [duration_secs=14400] [sample_secs=30]
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
cd /home/ubuntu/Code/ExpressGateway || exit 99
DUR="${1:-14400}"      # default 4h
SAMPLE="${2:-30}"      # 30s sample → DUR/30 samples (>> ~60 analyzer floor for 4h)
SCALE=1
OUT=audit/perf/s39-burnin
mkdir -p "$OUT"

ALL12=(sc1_h1h1 sc1b_h1h2 sc2_h2h2 sc3_slowloris sc4_modeb sc5_modea \
       sc6_413teardown sc7_h3terminate sc8_ws_h1 sc8b_ws_h2 sc8c_ws_h3 sc9_grpc_h3)

echo "[s39-burnin START $(date -u)] dur=${DUR}s sample=${SAMPLE}s scale=$SCALE scenarios=${#ALL12[@]}"
[ -x "$CARGO_TARGET_DIR/release/eg-soak" ] || { echo "build eg-soak first"; exit 1; }
[ -x "$CARGO_TARGET_DIR/release/expressgateway" ] || { echo "build expressgateway first"; exit 1; }

scripts/soak/run-soak.sh "$DUR" "$OUT" "$SAMPLE" "$SCALE" "${ALL12[@]}"

echo "[s39-burnin DONE $(date -u)] verdicts:"
for sc in "${ALL12[@]}"; do
  m="$OUT/$sc.soak_complete.marker"
  if [ -f "$m" ]; then echo "  $(cat "$m")"; else echo "  $sc: NO MARKER (incomplete!)"; fi
done | tee "$OUT/burnin-verdicts.txt"
# Headline BOUNDED/DRIFT count.
b=$(grep -c "overall=BOUNDED" "$OUT/burnin-verdicts.txt" 2>/dev/null)
d=$(grep -c "overall=DRIFT" "$OUT/burnin-verdicts.txt" 2>/dev/null)
echo "[s39-burnin] BOUNDED=$b DRIFT=$d of ${#ALL12[@]}" | tee -a "$OUT/burnin-verdicts.txt"
echo "DONE $(date -u)" > "$OUT/burnin.marker"
