#!/usr/bin/awk -f
# S10 / H2→H2 (R8 bounded-incremental streaming) — SESSION-CODE coverage
# sub-metric from an lcov trace, scoped to the NET-NEW / converted H2→H2
# session code. ROUND 2: updated for the F-MD-4-fix tip 9173bd97 (detached
# send task + inject_abort + reset_peer call sites grew proxy_h2_to_h2_request;
# reset_peer is new shared API in crates/lb-io/src/http2_pool.rs).
#
# Mirrors audit/h-matrix/s9-h1h1-cov.awk / s8-md-cov.awk: each range spans the
# `fn` signature through the closing line of its body; lcov only emits DA: for
# instrumentable lines, so signature/comment/blank lines never count toward
# the denominator.
#
#   crates/lb-l7/src/h2_proxy.rs:
#     proxy_h2_to_h2 (orchestrator + status mapping)            [1925-1955]
#     proxy_h2_to_h2_request (M-D mirror pump: lookahead/Branch A,
#       Branch B streaming, F-MD-4 inject_abort, the DETACHED send
#       task + verdict gate + reset_peer call sites, F-CAP-1 arm)  [1964-2414]
#     build_h2_upstream_request_parts (head-only H2→H2 norm)      [2465-2544]
#     upstream_h2_response_to_h2 (Leg 2 streaming relay)          [2658-2690]
#   crates/lb-io/src/http2_pool.rs:
#     reset_peer (F-MD-4 connection-teardown backstop)            [314-319]
# Binding bar: >= 80% on this SESSION sub-metric (NOT the whole file).
BEGIN { FS="[:,]" }
/^SF:/ {
  in_l7 = ($0 ~ /h2_proxy\.rs/)
  in_io = ($0 ~ /http2_pool\.rs/)
}
in_l7 && /^DA:/ {
  line=$2+0; cnt=$3+0
  if ((line>=1925&&line<=1955)||(line>=1964&&line<=2414)||(line>=2465&&line<=2544)||(line>=2658&&line<=2690)) {
    tot++; if(cnt>0) cov++; else uncov[++u]=line
    if(line>=1925&&line<=1955){ot++; if(cnt>0)oc++}
    else if(line>=1964&&line<=2414){pt++; if(cnt>0)pc++}
    else if(line>=2465&&line<=2544){bt++; if(cnt>0)bc++}
    else {rt++; if(cnt>0)rc++}
  }
}
in_io && /^DA:/ {
  line=$2+0; cnt=$3+0
  if (line>=314&&line<=319) {
    tot++; if(cnt>0) cov++; else uncov[++u]=("io:" line)
    rpt++; if(cnt>0)rpc++
  }
}
END {
  printf "proxy_h2_to_h2 (orchestrator)     : %d/%d = %.2f%%\n", oc,ot, ot?100*oc/ot:0
  printf "proxy_h2_to_h2_request (pump+task): %d/%d = %.2f%%\n", pc,pt, pt?100*pc/pt:0
  printf "build_h2_upstream_request_parts   : %d/%d = %.2f%%\n", bc,bt, bt?100*bc/bt:0
  printf "upstream_h2_response_to_h2 (relay): %d/%d = %.2f%%\n", rc,rt, rt?100*rc/rt:0
  printf "reset_peer (http2_pool.rs)        : %d/%d = %.2f%%\n", rpc,rpt, rpt?100*rpc/rpt:0
  printf "SESSION TOTAL (H2->H2 R8 + fix)   : %d/%d = %.2f%%  (need >=80%% => >=%d covered)\n", cov,tot, tot?100*cov/tot:0, int(0.8*tot+0.999)
  printf "UNCOVERED session lines (%d):\n", u
  line=""; for(i=1;i<=u;i++) line=line uncov[i] " "
  print line
}
