#!/usr/bin/env bash
# Periodic maintenance for the H3-green build loop.
# - git worktree prune: always (cheap, safe).
# - cargo clean: ONLY when no cargo/rustc build is in flight, so we never
#   abort an in-progress gate-runner / h3-eng build (cold rebuilds are
#   multi-minute and would stall the loop).
set -u
cd "$(git -C "$(dirname "$0")/.." rev-parse --show-toplevel)" || exit 0
ts() { date -u +%FT%TZ; }
log() { echo "[$(ts)] periodic-clean: $*"; }

git worktree prune -v 2>&1 | sed 's/^/[worktree] /' || true

# Build-in-flight guard: any cargo/rustc/llvm-cov process => skip cargo clean.
if pgrep -x cargo >/dev/null 2>&1 \
   || pgrep -x rustc >/dev/null 2>&1 \
   || pgrep -f 'cargo-llvm-cov|cargo build|cargo test|cargo clippy' >/dev/null 2>&1; then
  log "build in flight (cargo/rustc running) -> skipping cargo clean"
  exit 0
fi

# Also skip if target/ was modified in the last 5 min (build likely just active).
if [ -d target ] && [ -n "$(find target -maxdepth 2 -newermt '-5 minutes' -print -quit 2>/dev/null)" ]; then
  log "target/ touched <5min ago -> skipping cargo clean"
  exit 0
fi

before=$(du -sh target 2>/dev/null | cut -f1)
cargo clean 2>&1 | sed 's/^/[cargo clean] /' || true
log "cargo clean done (target was ${before:-n/a})"
