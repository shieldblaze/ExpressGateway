#!/usr/bin/awk -f
# S10 / H2→H2 (R8 bounded-incremental streaming) — SESSION-CODE coverage
# sub-metric from an lcov trace, scoped to the NET-NEW / converted H2→H2
# session code builder-1 changed in crates/lb-l7/src/h2_proxy.rs at builder
# tip 30918809. Mirrors audit/h-matrix/s9-h1h1-cov.awk / s8-md-cov.awk: each
# range spans the `fn` signature through the closing line of its body; lcov
# only emits DA: for instrumentable lines, so signature/comment/blank lines
# never count toward the denominator.
#
#   proxy_h2_to_h2 (orchestrator + status mapping)             [1911-1941]
#   proxy_h2_to_h2_request (M-D mirror pump: lookahead/Branch A,
#     Branch B streaming, F-MD-4 reset guard, F-CAP-1 verdict arm,
#     verdict-relay gate)                                       [1950-2284]
#   build_h2_upstream_request_parts (head-only H2→H2 norm)      [2335-2414]
#   upstream_h2_response_to_h2 (Leg 2 streaming relay)          [2528-2560]
# Binding bar: >= 80% on this SESSION sub-metric (NOT the whole file).
BEGIN { FS="[:,]" }
/^SF:/ { infile = ($0 ~ /h2_proxy\.rs/) }
infile && /^DA:/ {
  line=$2+0; cnt=$3+0
  if ((line>=1911&&line<=1941)||(line>=1950&&line<=2284)||(line>=2335&&line<=2414)||(line>=2528&&line<=2560)) {
    tot++; if(cnt>0) cov++; else uncov[++u]=line
    if(line>=1911&&line<=1941){ot++; if(cnt>0)oc++}
    else if(line>=1950&&line<=2284){pt++; if(cnt>0)pc++}
    else if(line>=2335&&line<=2414){bt++; if(cnt>0)bc++}
    else {rt++; if(cnt>0)rc++}
  }
}
END {
  printf "proxy_h2_to_h2 (orchestrator)     : %d/%d = %.2f%%\n", oc,ot, ot?100*oc/ot:0
  printf "proxy_h2_to_h2_request (pump)     : %d/%d = %.2f%%\n", pc,pt, pt?100*pc/pt:0
  printf "build_h2_upstream_request_parts   : %d/%d = %.2f%%\n", bc,bt, bt?100*bc/bt:0
  printf "upstream_h2_response_to_h2 (relay): %d/%d = %.2f%%\n", rc,rt, rt?100*rc/rt:0
  printf "SESSION TOTAL (H2->H2 R8)         : %d/%d = %.2f%%  (need >=80%% => >=%d covered)\n", cov,tot, tot?100*cov/tot:0, int(0.8*tot+0.999)
  printf "UNCOVERED session lines (%d):\n", u
  line=""; for(i=1;i<=u;i++) line=line uncov[i] " "
  print line
}
