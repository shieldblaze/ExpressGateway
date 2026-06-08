#!/usr/bin/env bash
# Watch the two post-merge main CI runs (ab638330) until both complete; report conclusions.
set -uo pipefail
L=/tmp/s37-main-ci.log
echo "[mainci $(date -u +%H:%M:%S)] watching main CI runs 27145437741 (CI) + 27145437614 (prod-readiness)" > "$L"
for run in 27145437741 27145437614; do
  gh run watch "$run" --interval 30 --exit-status >> "$L" 2>&1 || echo "[mainci] run $run exited non-zero (a job failed)" >> "$L"
  echo "[mainci $(date -u +%H:%M:%S)] run $run: $(gh run view "$run" --json status,conclusion --jq '.status+"/"+(.conclusion//"?")' 2>&1)" >> "$L"
done
echo "[mainci $(date -u +%H:%M:%S)] === FINAL main checks ===" >> "$L"
gh run view 27145437741 --json conclusion --jq '.conclusion' >> "$L" 2>&1
gh run view 27145437614 --json conclusion --jq '.conclusion' >> "$L" 2>&1
echo "MAINCI_DONE" >> "$L"
