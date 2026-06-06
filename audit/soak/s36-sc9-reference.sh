#!/usr/bin/env bash
# S36 Phase 0: reproduce the sc9_grpc_h3 RSS STAIRCASE as the pre-fix
# negative-control reference for workstream A (connection recycling).
#
# sc9_grpc_h3 = a quiche::h3 client sends opaque gRPC over H3; the gateway
# H3-terminates and proxies to an H2 gRPC backend. The CF-GRPC-H3-CHURN-RSS
# leak (S32: quiche StreamMap::collected grows insert-only; OUR lifecycle gap
# = conn_actor never caps requests / never GOAWAYs, so one H3 connection
# accumulates streams unboundedly). Pre-fix expectation: DRIFT (staircase).
#
# Reads verdict ONLY from the COMPLETED run (R15). 1800s @ 12s = 150 samples
# (>> the ~60-sample analyzer floor). Isolated (sc9 only) for a clean RSS curve.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway || exit 99
OUT=audit/soak/s36-soak-data/sc9-prefix-reference
mkdir -p "$OUT"
DUR=1800; SAMPLE=12; SCALE=1

echo "[sc9-ref $(date -u)] build release binaries (gateway + eg-soak)"
cargo build --release -p lb --bin expressgateway 2>&1 | tail -2
cargo build --release -p lb-soak --bin eg-soak 2>&1 | tail -2
[ -x "$CARGO_TARGET_DIR/release/expressgateway" ] || { echo "FATAL: expressgateway release bin missing"; exit 1; }
[ -x "$CARGO_TARGET_DIR/release/eg-soak" ] || { echo "FATAL: eg-soak release bin missing"; exit 1; }

echo "[sc9-ref $(date -u)] run sc9_grpc_h3 isolated for ${DUR}s (staircase reference)"
scripts/soak/run-soak.sh $DUR "$OUT" $SAMPLE $SCALE sc9_grpc_h3

echo "[sc9-ref $(date -u)] DONE. verdict + markers:"
for f in "$OUT"/*verdict*.json "$OUT"/*.soak_complete.marker; do
  [ -f "$f" ] && { echo "--- $f ---"; cat "$f"; }
done
echo "[sc9-ref] overall verdict line(s):"
grep -rhE 'overall=|SOAK .* — (BOUNDED|DRIFT)' "$OUT"/*.stdout.log 2>/dev/null | sort -u
echo "DONE $(date -u)" > "$OUT/sc9-reference.marker"
