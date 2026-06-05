#!/usr/bin/env bash
# Summarize a soak run: RSS staircase (from side csv) + diag mallinfo/trim (from gateway.log).
SIDE="${1:?side csv}"; GWLOG="${2:-}"
echo "### RSS / RssAnon / VmData / fds / threads trend (side csv: $SIDE)"
awk -F, 'NR==1{next} {print $1"s\tRSS="$3"kb\tRssAnon="$5"kb\tVmData="$6"kb\tfds="$9"\tthr="$8}' "$SIDE" | awk 'NR==1||NR%4==0||1' | sed -n '1p;0~3p' 2>/dev/null || awk -F, 'NR==1{next}{print $1"s RSS="$3" RssAnon="$5" VmData="$6" fds="$9" thr="$8}' "$SIDE"
echo
echo "### first vs last RSS"
awk -F, 'NR==2{f=$3;ft=$1} END{print "first="f"kb@"ft"s last="$3"kb@"$1"s delta="($3-f)"kb"}' "$SIDE"
if [ -n "$GWLOG" ] && [ -f "$GWLOG" ]; then
  echo
  echo "### DIAG mallinfo2 lines (uordblks=main-arena live; fordblks=free; hblkhd=mmap)"
  grep 'S32 DIAG mallinfo2' "$GWLOG" | grep -oE 'rss_kb=[0-9]+|uordblks=[0-9]+|fordblks=[0-9]+|hblkhd=[0-9]+|arena=[0-9]+' | paste - - - - - | sed -n '1p;0~2p' | head -40
  echo
  echo "### DIAG malloc_trim reclaim lines"
  grep 'S32 DIAG malloc_trim' "$GWLOG" | grep -oE 'rss_before_kb=[0-9]+|rss_after_kb=[0-9]+|reclaimed_kb=[0-9]+' | paste - - - | head -40
fi
