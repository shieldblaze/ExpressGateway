#!/usr/bin/env bash
# S37 lead stall-watchdog (R9). Heartbeats teammate/build/disk state every 60s.
# Exits (re-invoking the lead) on: critical disk (<3G), or after ~25 min routine check-in.
# Teammate completion/messages re-invoke the lead independently; this catches hard hangs + disk.
set -uo pipefail
GIT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify
TGT=/home/ubuntu/Code/eg-target
LOG=/tmp/s37-watchdog.log
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root | tail -1 | tr -dc 0-9; }
age(){ local f; f=$(find "$TGT" -type f -printf '%T@\n' 2>/dev/null | sort -rn | head -1); [ -z "$f" ] && { echo NA; return; }; awk -v t="$f" 'BEGIN{print int(systime()-t)"s"}'; }
lastci(){ git -C "$GIT" log -1 --format='%cr %h %s' "$1" 2>/dev/null | cut -c1-70; }
echo "[wd $(st)] START — watching s37-b-config / s37-d-deps; disk $(fg)G" > "$LOG"
for i in $(seq 1 25); do
  sleep 60
  D=$(fg); A=$(age); NC=$(pgrep -c -f 'cargo|rustc' 2>/dev/null || echo 0)
  echo "[wd $(st)] t=${i}m disk=${D}G tgt_newest=${A} cargo/rustc=${NC}" >> "$LOG"
  echo "         b-config: $(lastci s37-b-config)" >> "$LOG"
  echo "         d-deps  : $(lastci s37-d-deps)" >> "$LOG"
  if [ "$D" -lt 3 ]; then echo "[wd $(st)] CRITICAL disk<3G — exiting to alert lead" >> "$LOG"; echo "DISK_CRITICAL ${D}G"; exit 3; fi
done
echo "[wd $(st)] routine 25m check-in — exiting to re-invoke lead" >> "$LOG"
echo "ROUTINE_CHECKIN"