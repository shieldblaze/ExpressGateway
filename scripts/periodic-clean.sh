#!/usr/bin/env bash
# Periodic maintenance. `cargo clean` runs ONLY when no build is in flight —
# cleaning under a live build corrupts the dep-graph and costs a multi-minute
# cold rebuild.
set -u
cd "$(git -C "$(dirname "$0")/.." rev-parse --show-toplevel)" || exit 0
ts() { date -u +%FT%TZ; }
log() { echo "[$(ts)] periodic-clean: $*"; }

git worktree prune -v 2>&1 | sed 's/^/[worktree] /' || true

if pgrep -x cargo >/dev/null 2>&1 \
   || pgrep -x rustc >/dev/null 2>&1 \
   || pgrep -f 'cargo-llvm-cov|cargo build|cargo test|cargo clippy' >/dev/null 2>&1; then
  log "build in flight (cargo/rustc running) -> skipping cargo clean"
  exit 0
fi

# A target/ touched in the last 5 min means a build is probably still active.
if [ -d target ] && [ -n "$(find target -maxdepth 2 -newermt '-5 minutes' -print -quit 2>/dev/null)" ]; then
  log "target/ touched <5min ago -> skipping cargo clean"
  exit 0
fi

before=$(du -sh target 2>/dev/null | cut -f1)
# Privileged tests (D-1 sudo cargo) leave root-owned files that make clean EPERM.
if [ -d target ] && find target -not -user "$(id -un)" -print -quit 2>/dev/null | grep -q .; then
  sudo -n chown -R "$(id -un):$(id -gn)" target 2>/dev/null \
    && log "reclaimed root-owned target/ files before clean" \
    || log "warn: could not chown root-owned target/ files (clean may be partial)"
fi
cargo clean 2>&1 | sed 's/^/[cargo clean] /' || true
log "cargo clean done (target was ${before:-n/a})"
