#!/usr/bin/env bash
set -uo pipefail
L=/tmp/s37-d6rerun.log
echo "[d6rr $(date -u +%H:%M:%S)] watching main prod-readiness run 27145437614 (D6 re-run)" > "$L"
gh run watch 27145437614 --interval 30 --exit-status >> "$L" 2>&1 || echo "[d6rr] run failed" >> "$L"
echo "[d6rr $(date -u +%H:%M:%S)] conclusion: $(gh run view 27145437614 --json conclusion --jq '.conclusion' 2>&1)" >> "$L"
DJOB=$(gh run view 27145437614 --json jobs --jq '.jobs[]|select(.name=="D6-coverage")|.databaseId' 2>/dev/null)
gh run view 27145437614 --job "$DJOB" --log 2>&1 | grep -E 'modules passed|below 80|h2_proxy.rs =|D-6 FAILED|PASS: every' | tail -3 >> "$L"
echo "D6RR_DONE" >> "$L"
