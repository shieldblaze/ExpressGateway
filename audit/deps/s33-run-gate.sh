#!/usr/bin/env bash
# S33 gate runner — fmt + clippy + test x3 (--no-fail-fast). Arg1 = label (e.g. baseline, phase1).
# Shared target dir per R9. Sequential (no parallel cargo) to avoid OOM per R9.
set -u
LABEL="${1:?usage: s33-run-gate.sh <label>}"
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
cd /home/ubuntu/Code/ExpressGateway || exit 99
OUT="audit/deps/s33-gate-${LABEL}"
mkdir -p "$OUT"
echo "[s33-gate:${LABEL}] start  toolchain=$(rustc --version)"

echo "=== FMT ==="
cargo fmt --all --check > "${OUT}/fmt.log" 2>&1
echo "fmt_exit=$?" | tee "${OUT}/fmt.exit"

echo "=== CLIPPY (--all-targets --all-features -D warnings) ==="
cargo clippy --all-targets --all-features -- -D warnings > "${OUT}/clippy.log" 2>&1
echo "clippy_exit=$?" | tee "${OUT}/clippy.exit"

for i in 1 2 3; do
  echo "=== TEST RUN ${i} (--workspace --all-features --no-fail-fast) ==="
  cargo test --workspace --all-features --no-fail-fast > "${OUT}/test-run${i}.log" 2>&1
  echo "test_run${i}_exit=$?" | tee "${OUT}/test-run${i}.exit"
  # summary line: count passed/failed/ignored across all binaries
  grep -hE '^test result:' "${OUT}/test-run${i}.log" | \
    awk '{for(j=1;j<=NF;j++){if($j=="passed;")p+=$(j-1); if($j=="failed;")f+=$(j-1); if($j=="ignored;")ig+=$(j-1)}} END{printf "RUN%s TOTALS passed=%d failed=%d ignored=%d\n","'$i'",p,f,ig}' \
    | tee "${OUT}/test-run${i}.summary"
done

echo "[s33-gate:${LABEL}] DONE"
