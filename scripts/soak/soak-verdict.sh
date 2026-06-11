#!/usr/bin/env bash
# soak-verdict.sh — the RELEASE soak GATE verdict.
#
# Reads a completed soak OUT_DIR (per-scenario `*.soak_complete.marker` +
# `*.stdout.log` produced by scripts/soak/run-soak.sh) and decides PASS/FAIL:
#
#   PASS iff  — every EXPECTED scenario has a marker AND its `overall=BOUNDED`
#             — AND zero panics across all scenario logs (panic=0, R8)
#
# It writes a COMPACT summary (release-soak-summary.txt) — per-scenario verdict
# + sample count + any panic hits — NOT the raw multi-sample CSVs (the S37/S39
# disk lesson: ship verdicts/summaries, not 100M-row time-series).
#
# Exit 0 on PASS, non-zero on FAIL. Read the verdict ONLY from a COMPLETED run
# (R15) — a missing marker == FAIL (incomplete scenario), not a skip.
#
# Usage: soak-verdict.sh <out_dir> [scenario ...]
#   With no scenario list, defaults to the canonical 12-scenario release set.
set -uo pipefail

OUT="${1:?usage: soak-verdict.sh <out_dir> [scenario ...]}"
shift || true
EXPECTED=("$@")
if [ "${#EXPECTED[@]}" -eq 0 ]; then
  EXPECTED=(sc1_h1h1 sc1b_h1h2 sc2_h2h2 sc3_slowloris sc4_modeb sc5_modea \
            sc6_413teardown sc7_h3terminate sc8_ws_h1 sc8b_ws_h2 sc8c_ws_h3 sc9_grpc_h3)
fi

SUMMARY="$OUT/release-soak-summary.txt"
: > "$SUMMARY"

note() { echo "$@" | tee -a "$SUMMARY"; }

note "=== ExpressGateway release-soak verdict ==="
note "out_dir=$OUT  generated=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
note "expected scenarios: ${#EXPECTED[@]}"
note ""

fail=0
bounded=0
for sc in "${EXPECTED[@]}"; do
  marker="$OUT/$sc.soak_complete.marker"
  log="$OUT/$sc.stdout.log"

  if [ ! -f "$marker" ]; then
    note "  FAIL  $sc — NO completion marker (scenario did not finish)"
    fail=$((fail + 1))
    continue
  fi

  overall=$(sed -nE 's/.*overall=([A-Za-z]+).*/\1/p' "$marker")
  samples=$(sed -nE 's/.*samples=([0-9]+).*/\1/p' "$marker")

  # panic=0 (R8): scan this scenario's log for a Rust panic hook message OR a
  # genuinely non-zero `panic_total` metric. Precise awk avoids the false match
  # on the zero-valued metric line `panic_total 0` (a plain grep for
  # `panic_total[^0]` matches the trailing space and over-counts).
  panics=0
  if [ -f "$log" ]; then
    panics=$(awk '
      /panicked/ { c++ }
      /panic_total[ {]/ { v=$NF; if (v ~ /^[0-9]+$/ && v+0 > 0) c++ }
      END { print c+0 }' "$log" 2>/dev/null)
    panics=${panics:-0}
  fi

  if [ "$overall" = "BOUNDED" ] && [ "$panics" -eq 0 ]; then
    note "  PASS  $sc — overall=BOUNDED samples=${samples:-?} panics=0"
    bounded=$((bounded + 1))
  else
    note "  FAIL  $sc — overall=${overall:-MISSING} samples=${samples:-?} panics=${panics}"
    fail=$((fail + 1))
  fi
done

note ""
note "BOUNDED=${bounded}/${#EXPECTED[@]}  FAILURES=${fail}"

if [ "$fail" -ne 0 ]; then
  note "VERDICT: FAIL — release soak gate NOT satisfied (see failures above)."
  exit 1
fi
note "VERDICT: PASS — every scenario BOUNDED, panic=0 (R8 held)."
exit 0
