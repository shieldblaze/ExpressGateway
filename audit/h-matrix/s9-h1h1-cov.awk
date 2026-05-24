#!/usr/bin/awk -f
# S9 / H1→H1 (M-D-lite) — compute the H1 SESSION-CODE coverage sub-metric
# from an lcov trace and list the uncovered lines. Scoped to the NET-NEW H1
# session code builder-1 added in crates/lb-l7/src/h1_proxy.rs on branch
# s9/builder-1 (I1 b01a13d2 pump + StreamBody, I2 0a479579 gauge).
#
# Convention (mirrors audit/h-matrix/s8-md-cov.awk): each range spans the
# `fn`/`struct`/`impl` signature line through the last line of that item's
# body; lcov only emits DA: for instrumentable lines, so signature/comment/
# blank lines never count toward the denominator.
#
#   H1PumpAbort type + Display/Error impls (F-MD-4)               [121-129]
#   validate_h1_request_trailers (Q-H3)                          [149-169]
#   proxy_request — the WHOLE Branch-B-only ingress pump:
#     F-MD-1 strip, dial+handshake w/ H1PumpAbort error type,
#     bounded mpsc + in-flight gauge wiring, StreamBody poll
#     decrement, the pump loop (clean-end None terminator,
#     trailers validate+forward / Err-before-close, Q-H4 cap
#     Err-before-close, send_chunked! receiver-gone →
#     drain_and_validate!, F-MD-4 Some(Err) abort arm), the
#     send_request timeout/err arms, and the verdict-relay gate  [1173-1531]
#   record_retained_h1 (F-MD-3 test-gauge CAS-max)               [2433-2447]
# Binding bar: >= 80% on this SESSION sub-metric (NOT the whole file).
BEGIN { FS="[:,]" }
/^SF:/ { infile = ($0 ~ /h1_proxy\.rs/) }
infile && /^DA:/ {
  line=$2+0; cnt=$3+0
  if ((line>=121&&line<=129)||(line>=149&&line<=169)||(line>=1173&&line<=1531)||(line>=2433&&line<=2447)) {
    tot++; if(cnt>0) cov++; else uncov[++u]=line
    if(line>=121&&line<=129){pat++; if(cnt>0)pac++}
    else if(line>=149&&line<=169){vtt++; if(cnt>0)vtc++}
    else if(line>=1173&&line<=1531){prt++; if(cnt>0)prc++}
    else {rrt++; if(cnt>0)rrc++}
  }
}
END {
  printf "H1PumpAbort type            : %d/%d = %.2f%%\n", pac,pat, pat?100*pac/pat:0
  printf "validate_h1_request_trailers: %d/%d = %.2f%%\n", vtc,vtt, vtt?100*vtc/vtt:0
  printf "proxy_request (pump)        : %d/%d = %.2f%%\n", prc,prt, prt?100*prc/prt:0
  printf "record_retained_h1          : %d/%d = %.2f%%\n", rrc,rrt, rrt?100*rrc/rrt:0
  printf "SESSION TOTAL (H1 M-D-lite) : %d/%d = %.2f%%  (need >=80%% => >=%d covered)\n", cov,tot, tot?100*cov/tot:0, int(0.8*tot+0.999)
  printf "UNCOVERED session lines (%d):\n", u
  line=""; for(i=1;i<=u;i++) line=line uncov[i] " "
  print line
}
