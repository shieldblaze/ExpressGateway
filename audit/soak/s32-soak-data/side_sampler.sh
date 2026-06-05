#!/usr/bin/env bash
# Rich per-second-ish side-sampler for the gateway under a soak.
# Args: $1 = output csv, $2 = total seconds, $3 = interval seconds
OUT="${1:?out csv}"; TOTAL="${2:-1800}"; IVAL="${3:-15}"
echo "t_s,pid,vmrss_kb,vmhwm_kb,rssanon_kb,vmdata_kb,heap_kb,threads,fds,pool_idle,pool_acq,accept_inflight" > "$OUT"
start=$(date +%s)
while :; do
  now=$(date +%s); el=$((now-start))
  [ "$el" -ge "$TOTAL" ] && break
  pid=$(pgrep -x expressgateway | head -1)
  [ -z "$pid" ] && pid=$(pgrep -f 'eg-target/release/expressgateway ' | grep -v 'bin/bash' | head -1)
  if [ -n "$pid" ] && [ -d "/proc/$pid" ]; then
    st=$(cat /proc/$pid/status 2>/dev/null)
    vmrss=$(awk '/^VmRSS:/{print $2}' <<<"$st"); vmhwm=$(awk '/^VmHWM:/{print $2}' <<<"$st")
    rssanon=$(awk '/^RssAnon:/{print $2}' <<<"$st"); vmdata=$(awk '/^VmData:/{print $2}' <<<"$st")
    threads=$(awk '/^Threads:/{print $2}' <<<"$st")
    fds=$(ls /proc/$pid/fd 2>/dev/null | wc -l)
    # [heap] anon size from smaps_rollup is rollup; instead grab heap region from smaps
    heap=$(awk '/\[heap\]/{getline; while($1!="Size:" && NF>0){getline}} /^Size:/{} END{}' /proc/$pid/smaps 2>/dev/null)
    heap=$(awk 'BEGIN{s=0} /\[heap\]$/{f=1;next} f&&/^Size:/{s+=$2;f=0} {if($0 ~ /^[0-9a-f]+-/)f=0} END{print s}' /proc/$pid/smaps 2>/dev/null)
    # metrics port: find a listening TCP socket owned by pid that answers /metrics
    mport=""
    for p in $(ss -tlnp 2>/dev/null | grep "pid=$pid," | grep -oP '127.0.0.1:\K[0-9]+' | sort -u); do
      if curl -s -m 1 "http://127.0.0.1:$p/metrics" 2>/dev/null | grep -q '^pool_\|^accept_\|^panic_'; then mport=$p; break; fi
    done
    pool_idle=""; pool_acq=""; ainf=""
    if [ -n "$mport" ]; then
      met=$(curl -s -m 1 "http://127.0.0.1:$mport/metrics" 2>/dev/null)
      pool_idle=$(awk '/^pool_idle_gauge /{print $2}' <<<"$met")
      pool_acq=$(awk '/^pool_acquires_total /{print $2}' <<<"$met")
      ainf=$(awk '/^accept_inflight /{print $2}' <<<"$met")
    fi
    echo "$el,$pid,${vmrss:-},${vmhwm:-},${rssanon:-},${vmdata:-},${heap:-},${threads:-},${fds:-},${pool_idle:-},${pool_acq:-},${ainf:-}" >> "$OUT"
  else
    echo "$el,,,,,,,,,,," >> "$OUT"
  fi
  sleep "$IVAL"
done
