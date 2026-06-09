#!/usr/bin/env bash
# S39 oha cross-validation — drive H1/H1s/H2 with the trusted external load tool
# `oha` against an eg-bench --serve-secs gateway (real expressgateway binary +
# real backend). Two purposes: (1) independent percentiles+RPS for the TCP-HTTP
# paths from a battle-tested client; (2) author≠verifier on eg-bench — if
# eg-bench's H1/H2 numbers agree with oha within noise, the custom harness is
# trustworthy (lending credibility to the H3/QUIC numbers oha can't measure).
#
# Usage: scripts/perf/s39-oha.sh <out_dir>
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
BIN="$CARGO_TARGET_DIR/release/eg-bench"
OUT="${1:?out_dir required}"; mkdir -p "$OUT"
CONCS=(1 8 32 64)
DUR="${DUR:-15s}"
SUMMARY="$OUT/oha-summary.txt"
: > "$SUMMARY"

PARSE='
import sys,json
try:
  d=json.load(sys.stdin); s=d.get("summary",{}); p=d.get("latencyPercentiles",{})
  us=lambda x:int(float(x)*1e6) if x is not None else -1
  print("c=%s rps=%.0f ok=%.3f p50=%dus p90=%dus p99=%dus p999=%dus avg=%dus" % (
    sys.argv[1], s.get("requestsPerSec",0), s.get("successRate",0),
    us(p.get("p50")), us(p.get("p90")), us(p.get("p99")), us(p.get("p99.9")), us(s.get("average"))))
except Exception as e: print("c=%s parse-fail: %s" % (sys.argv[1], e))
'

oha_sweep() {  # url label http2flag
  local url="$1" label="$2" h2="$3"
  echo "--- $label ($url) ---" | tee -a "$SUMMARY"
  local c
  for c in "${CONCS[@]}"; do
    oha -c "$c" -z "$DUR" --no-tui --insecure $h2 --output-format json "$url" \
      > "$OUT/oha-$label-c$c.json" 2>/dev/null
    python3 -c "$PARSE" "$c" < "$OUT/oha-$label-c$c.json" | tee -a "$SUMMARY"
  done
}

wait_ready() {  # logfile
  local log="$1" i
  for i in $(seq 1 100); do grep -q "^READY" "$log" 2>/dev/null && return 0; sleep 0.2; done
  return 1
}

echo "[s39-oha $(date -u)] oha $(oha --version)" | tee -a "$SUMMARY"

# H1 (plaintext): serve h1_front; oha http/1.1 over plain TCP.
H1LOG="$OUT/serve-h1.log"; : > "$H1LOG"
"$BIN" --protocol h1 --serve-secs 600 > "$H1LOG" 2>>"$H1LOG" &
H1PID=$!
if wait_ready "$H1LOG"; then
  L=$(grep "^LISTENER" "$H1LOG" | awk '{print $1}' | cut -d= -f2)
  oha_sweep "http://$L/" "h1" ""
else echo "h1 serve never READY" | tee -a "$SUMMARY"; fi
kill "$H1PID" 2>/dev/null; wait "$H1PID" 2>/dev/null

# H1s + H2 share one TLS front (h1s_front advertises h2 then http/1.1).
H2LOG="$OUT/serve-h2.log"; : > "$H2LOG"
"$BIN" --protocol h2 --serve-secs 900 > "$H2LOG" 2>>"$H2LOG" &
H2PID=$!
if wait_ready "$H2LOG"; then
  L=$(grep "^LISTENER" "$H2LOG" | awk '{print $1}' | cut -d= -f2)
  oha_sweep "https://$L/" "h1s" ""
  oha_sweep "https://$L/" "h2" "--http2"
else echo "h2 serve never READY" | tee -a "$SUMMARY"; fi
kill "$H2PID" 2>/dev/null; wait "$H2PID" 2>/dev/null

echo "[s39-oha $(date -u)] DONE" | tee -a "$SUMMARY"
