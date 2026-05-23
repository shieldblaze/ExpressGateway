#!/usr/bin/awk -f
# S8 / M-D — compute the H2→H1 SESSION-CODE coverage sub-metric from an
# lcov trace and list the uncovered lines. Scoped to the NET-NEW M-D code
# in crates/lb-l7/src/h2_proxy.rs (REMEDIATED builder tip bc23b9f8; the F-MD-1
# version fix, F-MD-2 drain-and-validate and F-MD-3 in-flight counter all live
# inside proxy_request, so the ranges shifted vs the round-1 calibration):
#   proxy_request (bounded ingress pump + Branch B stream + verdict gate) [1306-1741]
#   validate_request_trailers                                            [1956-1973]
#   concat_chunks                                                        [1978-1987]
#   record_retained                                                      [2356-2370]
# Binding bar: >= 80% on this sub-metric.
BEGIN { FS="[:,]" }
/^SF:/ { infile = ($0 ~ /h2_proxy\.rs/) }
infile && /^DA:/ {
  line=$2+0; cnt=$3+0
  if ((line>=1306&&line<=1741)||(line>=1956&&line<=1973)||(line>=1978&&line<=1987)||(line>=2356&&line<=2370)) {
    tot++; if(cnt>0) cov++; else uncov[++u]=line
    if(line>=1306&&line<=1741){prt++; if(cnt>0)prc++}
    else if(line>=1956&&line<=1973){vtt++; if(cnt>0)vtc++}
    else if(line>=1978&&line<=1987){cct++; if(cnt>0)ccc++}
    else {rrt++; if(cnt>0)rrc++}
  }
}
END {
  printf "proxy_request (pump)      : %d/%d = %.2f%%\n", prc,prt, prt?100*prc/prt:0
  printf "validate_request_trailers : %d/%d = %.2f%%\n", vtc,vtt, vtt?100*vtc/vtt:0
  printf "concat_chunks             : %d/%d = %.2f%%\n", ccc,cct, cct?100*ccc/cct:0
  printf "record_retained           : %d/%d = %.2f%%\n", rrc,rrt, rrt?100*rrc/rrt:0
  printf "SESSION TOTAL (M-D)       : %d/%d = %.2f%%  (need >=80%% => >=%d covered)\n", cov,tot, tot?100*cov/tot:0, int(0.8*tot+0.999)
  printf "UNCOVERED session lines (%d):\n", u
  line=""; for(i=1;i<=u;i++) line=line uncov[i] " "
  print line
}
