#!/usr/bin/env bash
# S39 R1 gate — DISK-FEASIBLE form. The full `cargo test --workspace
# --all-features ×3` needs ~38-40G build space (CF-DISK-1); this 67G box leaves
# ~37G free after OS+toolchains+the user's projects, and the only remaining
# reclaimable space is the user's unrelated Java tooling (.m2/.sdkman/sdk) — NOT
# deleted unilaterally. This gate is the strongest that fits + is SUFFICIENT for
# the S39 change, which is TEST-HARNESS-ONLY and purely ADDITIVE (new `bench`
# module + new `eg-bench` bin; loadgen + all production crates byte-identical,
# independently reviewer-confirmed; zero new #[test]; main was ×3-green at S38).
#
#   1. cargo check --workspace --all-features --all-targets  (compile-validate
#      EVERY target incl. all tests with all features — the real breakage risk)
#   2. cargo test -p lb-soak --all-features                  (RUN the one crate
#      actually changed)
#   3. clippy --workspace --all-targets --all-features -D warnings  +  fmt --check
#
# If the operator clears the Java caches, run the full `s39-x3.sh` instead.
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway || exit 99
OUT=audit/perf/s39-gate; mkdir -p "$OUT"
: > "$OUT/gate-summary.txt"
log() { echo "$*" | tee -a "$OUT/gate-summary.txt"; }

log "[gate START $(date -u)] free=$(df -h / | awk 'NR==2{print $4}')"

log "--- 1. cargo check --workspace --all-features --all-targets ---"
cargo check --workspace --all-features --all-targets > "$OUT/check.log" 2>&1
rc=$?; log "   check rc=$rc  ($(grep -cE '^error' "$OUT/check.log") errors)"

log "--- 2. cargo test -p lb-soak --all-features (the changed crate) ---"
cargo test -p lb-soak --all-features --no-fail-fast > "$OUT/test-lbsoak.log" 2>&1
rc2=$?
passed=$(grep -hoE "test result: ok\. [0-9]+ passed" "$OUT/test-lbsoak.log" | grep -oE "[0-9]+" | paste -sd+ | bc 2>/dev/null)
failed=$(grep -hoE "[0-9]+ failed" "$OUT/test-lbsoak.log" | grep -oE "^[0-9]+" | paste -sd+ | bc 2>/dev/null)
log "   lb-soak test rc=$rc2 passed=${passed:-?} failed=${failed:-?}"

log "--- 3. clippy --workspace --all-targets --all-features -D warnings ---"
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$OUT/clippy.log" 2>&1
rc3=$?; log "   clippy rc=$rc3 ($(grep -cE '^error' "$OUT/clippy.log") errors)"

log "--- 4. cargo fmt --check ---"
cargo fmt --all -- --check > "$OUT/fmt.log" 2>&1
rc4=$?; log "   fmt rc=$rc4"

log "[gate DONE $(date -u)] check=$rc test=$rc2 clippy=$rc3 fmt=$rc4 free=$(df -h / | awk 'NR==2{print $4}')"
[ $rc -eq 0 ] && [ $rc2 -eq 0 ] && [ $rc3 -eq 0 ] && [ $rc4 -eq 0 ] && log "GATE: GREEN" || log "GATE: RED"
