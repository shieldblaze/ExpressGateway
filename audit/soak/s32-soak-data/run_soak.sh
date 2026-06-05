#!/usr/bin/env bash
# Reusable soak launcher. Runs eg-soak + the side-sampler, both backgrounded,
# then waits (foreground) for the summary marker. Usage:
#   run_soak.sh <label> <duration> <binary> [extra env "K=V K=V"]
set -u
LABEL="${1:?label}"; DUR="${2:?dur}"; BIN="${3:?binary}"; EXTRA="${4:-}"
HERE="/home/ubuntu/Code/ExpressGateway/audit/soak/s32-soak-data"
OUT="$HERE/$LABEL"
SIDE="$HERE/$LABEL-side.csv"
echo "[run_soak] label=$LABEL dur=${DUR}s bin=$BIN extra='$EXTRA'"
nohup "$HERE/side_sampler.sh" "$SIDE" "$((DUR+40))" 15 >"$HERE/$LABEL-side.log" 2>&1 &
sleep 1
env CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target RUST_LOG=warn $EXTRA \
  nohup "$BIN" --scenario sc9_grpc_h3 --duration-secs "$DUR" --sample-secs 15 \
  --label "$LABEL" --out "$OUT" >"$HERE/$LABEL.log" 2>&1 &
echo "[run_soak] launched soak pid=$! ; waiting for $OUT/$LABEL.summary.txt"
