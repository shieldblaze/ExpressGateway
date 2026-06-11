#!/usr/bin/env bash
# release-soak-onbox.sh — runs ON the dedicated soak EC2.
#
# Build the release binaries, run the FULL 12-scenario lb-soak for a release-
# grade duration, decide the all-BOUNDED + panic=0 verdict, and emit a COMPACT
# summary (no raw CSVs). Optionally upload the summary + markers to S3 so the
# controller (scripts/release-soak.sh) can fetch the result and tear the box
# down. Single-sources the soak driver (scripts/soak/run-soak.sh) and the gate
# (scripts/soak/soak-verdict.sh).
#
# Run from the repo root on the soak box (the controller's user-data clones the
# repo@REF, then invokes this).
#
# Usage: release-soak-onbox.sh [duration_secs] [sample_secs] [s3_dest]
#   duration_secs  default 14400 (4h — the S39 release burn-in duration)
#   sample_secs    default 30
#   s3_dest        optional s3://bucket/prefix to upload the summary + markers
#
# Env:
#   CARGO_TARGET_DIR  shared target dir (default ./eg-target alongside the repo)
set -uo pipefail

DUR="${1:-14400}"
SAMPLE="${2:-30}"
S3_DEST="${3:-}"

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/../eg-target}"
OUT="$REPO_ROOT/release-soak-out"
mkdir -p "$OUT" "$CARGO_TARGET_DIR"

ALL12=(sc1_h1h1 sc1b_h1h2 sc2_h2h2 sc3_slowloris sc4_modeb sc5_modea \
       sc6_413teardown sc7_h3terminate sc8_ws_h1 sc8b_ws_h2 sc8c_ws_h3 sc9_grpc_h3)

echo "[onbox $(date -u)] release soak start: dur=${DUR}s sample=${SAMPLE}s scenarios=${#ALL12[@]}"
echo "[onbox] CARGO_TARGET_DIR=$CARGO_TARGET_DIR  out=$OUT"

# --- build (release) -------------------------------------------------------
echo "[onbox] building release eg-soak + expressgateway…"
cargo build --release -p lb-soak --bin eg-soak
cargo build --release -p lb --bin expressgateway
[ -x "$CARGO_TARGET_DIR/release/eg-soak" ]        || { echo "::error:: eg-soak not built"; exit 2; }
[ -x "$CARGO_TARGET_DIR/release/expressgateway" ] || { echo "::error:: expressgateway not built"; exit 2; }

# --- soak (the full 12, scale=1, concurrent) -------------------------------
# run-soak.sh isolates a gateway child per scenario and writes per-scenario
# markers + bounded stdout logs. It exits 0 regardless; the GATE is the verdict.
scripts/soak/run-soak.sh "$DUR" "$OUT" "$SAMPLE" 1 "${ALL12[@]}"

# --- verdict (the gate) ----------------------------------------------------
set +e
scripts/soak/soak-verdict.sh "$OUT" "${ALL12[@]}"
VERDICT_RC=$?
set -e
echo "[onbox] verdict rc=$VERDICT_RC"

# --- disk hygiene: keep summary + markers, drop the bulky time-series -------
# (S39 disk lesson — never ship/retain multi-100M CSVs; the verdict is enough.)
find "$OUT" -name '*.csv' -delete 2>/dev/null || true
# cap any large scenario stdout logs to their last 2000 lines (panic context).
for log in "$OUT"/*.stdout.log; do
  [ -f "$log" ] || continue
  tail -n 2000 "$log" > "$log.tail" 2>/dev/null && mv "$log.tail" "$log" || true
done

# --- handoff ---------------------------------------------------------------
if [ -n "$S3_DEST" ]; then
  echo "[onbox] uploading summary + markers to $S3_DEST"
  aws s3 cp "$OUT/release-soak-summary.txt" "$S3_DEST/release-soak-summary.txt" || true
  aws s3 cp "$OUT/" "$S3_DEST/markers/" --recursive --exclude '*' --include '*.soak_complete.marker' || true
  # the DONE sentinel carries the verdict rc so the controller can gate on it.
  echo "verdict_rc=$VERDICT_RC done=$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$OUT/DONE"
  aws s3 cp "$OUT/DONE" "$S3_DEST/DONE" || true
fi

echo "[onbox $(date -u)] release soak done (verdict_rc=$VERDICT_RC)"
exit "$VERDICT_RC"
