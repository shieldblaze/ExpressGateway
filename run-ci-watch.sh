#!/usr/bin/env bash
# Watch PR #228 CI until all checks complete, then exit (re-invokes lead). Fail-tolerant.
set -uo pipefail
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify
L=/tmp/s37-ci-watch.log
echo "[ciwatch $(date -u +%H:%M:%S)] watching PR #228" > "$L"
gh pr checks 228 --watch --interval 30 >> "$L" 2>&1 || true
echo "[ciwatch $(date -u +%H:%M:%S)] === FINAL ===" >> "$L"
gh pr checks 228 >> "$L" 2>&1 || true
echo "CIWATCH_DONE" >> "$L"
