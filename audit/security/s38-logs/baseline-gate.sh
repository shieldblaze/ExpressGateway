#!/usr/bin/env bash
# S38 Phase-0 baseline gate: confirm main @ b8a99078 (branched as feature/security-audit-s38)
# is honest-green BEFORE the audit changes anything. Cold build warms the shared target dir.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_BUILD_JOBS=6   # 8-core box, leave headroom (S33 OOM lesson: cap jobs)
cd "$(dirname "$0")/../../.." || exit 99
LOG=audit/security/s38-logs
ts() { date '+%H:%M:%S'; }

echo "[$(ts)] === fmt ==="
cargo fmt --all --check 2>&1 | tee "$LOG/00-fmt.txt"
FMT=${PIPESTATUS[0]}

echo "[$(ts)] === clippy --all-targets --all-features -D warnings ==="
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tee "$LOG/01-clippy.txt" | tail -20
CLIPPY=${PIPESTATUS[0]}

PASS=0
for i in 1 2 3; do
  echo "[$(ts)] === test run $i/3 (--workspace --all-features --no-fail-fast) ==="
  cargo test --workspace --all-features --no-fail-fast 2>&1 | tee "$LOG/02-test-run$i.txt" | tail -5
  rc=${PIPESTATUS[0]}
  echo "[$(ts)] run $i rc=$rc"
  [ "$rc" -eq 0 ] && PASS=$((PASS+1))
done

echo "[$(ts)] === BASELINE SUMMARY ==="
echo "fmt rc=$FMT  clippy rc=$CLIPPY  test-passes=$PASS/3"
echo "fmt=$FMT clippy=$CLIPPY test=$PASS/3" > "$LOG/baseline-verdict.txt"
