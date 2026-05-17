#!/bin/bash
set -o pipefail
cd ~/Code/ExpressGateway
E=audit/h3-program/s5-evidence
{ echo "BASELINE_TIP=$(git rev-parse HEAD)"; echo "BRANCH=$(git branch --show-current)"; date -u; } > $E/p0-meta.txt
echo "=== FMT $(date -u) ==="
cargo fmt --check > $E/p0-fmt.txt 2>&1; echo "FMT_EXIT=$?" >> $E/p0-fmt.txt
echo "=== CLIPPY $(date -u) ==="
cargo clippy --all-targets --all-features -- -D warnings > $E/p0-clippy.txt 2>&1; echo "CLIPPY_EXIT=$?" >> $E/p0-clippy.txt
for i in 1 2 3; do
  echo "=== TEST RUN $i $(date -u) ==="
  cargo test --workspace --all-features > $E/p0-test-run$i.txt 2>&1
  echo "TEST_RUN${i}_EXIT=$?" >> $E/p0-test-run$i.txt
done
echo "P0_DONE $(date -u)" > $E/p0-status.txt
