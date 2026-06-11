#!/usr/bin/env bash
# S31 full-workspace gate: build --no-run, x3 deterministic test passes, clippy, fmt.
# One completed-run sentinel (R15). Usage: s31-gate.sh <label> [logdir]
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
cd /home/ubuntu/Code/ExpressGateway || exit 99
LABEL="${1:-run}"
LOGDIR="${2:-audit/deps/s31-gate-${LABEL}}"
mkdir -p "$LOGDIR"
LOG="$LOGDIR/gate.log"
: > "$LOG"
{
  echo "===== S31 GATE ${LABEL} start $(date -u +%FT%TZ) HEAD $(git rev-parse --short HEAD) branch $(git rev-parse --abbrev-ref HEAD) ====="
  echo "--- df:"; df -h /home/ubuntu | tail -1
  echo "--- quiche/tokio-quiche locked:"; grep -A1 'name = "quiche"' Cargo.lock | grep version; grep -A1 'name = "tokio-quiche"' Cargo.lock | grep version
} | tee -a "$LOG"

echo "=== STAGE build (--no-run) $(date -u +%FT%TZ) ===" | tee -a "$LOG"
cargo test --workspace --all-features --no-run >> "$LOG" 2>&1
BRC=${PIPESTATUS[0]}
echo "BUILD_RC=$BRC" | tee -a "$LOG"
if [ "$BRC" -ne 0 ]; then echo "S31-GATE-${LABEL}-DONE rc=99 (build fail)" | tee -a "$LOG"; exit 99; fi

# --no-fail-fast: cargo test defaults to fail-fast at the BINARY level — it stops
# running further test binaries after the first binary reports a failure. With the
# known CF-FCAP1-FLAKE (h2h1_md_streaming_verify, binary #~83 of 240 under 8-core
# saturation), a fail-fast run TRUNCATES the suite and never reaches the lb-quic H3
# tests (h3_*, s16_*, s19_*, grpc_h3 all sort AFTER h2h1) — exactly the tests a
# quiche upgrade must exercise. --no-fail-fast runs ALL binaries every pass and
# reports the COMPLETE failure set (strictly more rigorous; R15: a truncated run is
# an incomplete job). Known saturation flakes are then classified by isolation (R2).
for i in 1 2 3; do
  echo "=== STAGE TEST PASS $i $(date -u +%FT%TZ) ===" | tee -a "$LOG"
  cargo test --workspace --all-features --no-fail-fast >> "$LOG.pass$i" 2>&1
  RC=${PIPESTATUS[0]}
  PASS=$(grep -E 'test result:' "$LOG.pass$i" | grep -oE '[0-9]+ passed' | awk '{s+=$1} END{print s}')
  FAIL=$(grep -E 'test result:' "$LOG.pass$i" | grep -oE '[0-9]+ failed' | awk '{s+=$1} END{print s}')
  IGN=$(grep -E 'test result:' "$LOG.pass$i" | grep -oE '[0-9]+ ignored' | awk '{s+=$1} END{print s}')
  BINS=$(grep -cE 'test result:' "$LOG.pass$i")
  echo "PASS$i rc=$RC binaries=$BINS passed=$PASS failed=$FAIL ignored=$IGN" | tee -a "$LOG"
  echo "  failing tests:" | tee -a "$LOG"; grep -E '^test .* \.\.\. FAILED|FAILED$' "$LOG.pass$i" | grep -oE 'test [A-Za-z0-9_:]+' | sort -u | tee -a "$LOG"
done

echo "=== STAGE clippy $(date -u +%FT%TZ) ===" | tee -a "$LOG"
cargo clippy --workspace --all-targets --all-features -- -D warnings >> "$LOG" 2>&1
echo "CLIPPY_RC=${PIPESTATUS[0]}" | tee -a "$LOG"

echo "=== STAGE fmt --check $(date -u +%FT%TZ) ===" | tee -a "$LOG"
cargo fmt --all --check >> "$LOG" 2>&1
echo "FMT_RC=${PIPESTATUS[0]}" | tee -a "$LOG"

{
  echo "--- df after:"; df -h /home/ubuntu | tail -1
  echo "===== S31 GATE ${LABEL} end $(date -u +%FT%TZ) ====="
  echo "S31-GATE-${LABEL}-DONE rc=0"
} | tee -a "$LOG"
