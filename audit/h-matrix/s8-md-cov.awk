#!/usr/bin/awk -f
# S8 / M-D — compute the H2→H1 SESSION-CODE coverage sub-metric from an
# lcov trace and list the uncovered lines. Scoped to the NET-NEW M-D code
# in crates/lb-l7/src/h2_proxy.rs.
#
# RE-CALIBRATED for the FINAL integrated tip (feature/h-matrix-s8 5a5e633e):
# the two PROTO-smuggling fixes shifted every range vs the round-1 calibration
# (bc23b9f8) — 622ee624 added the `PumpAbort` type + the tx.send(Err(PumpAbort))
# inbound-abort/over-cap arms, and 0b43ef3b added the positive END_STREAM
# gating (is_end_stream) incl. the within-window Phase-1 reset rejection.
# Convention (unchanged): each range spans the `fn`/`struct` signature line
# through the last line of that item's body; lcov only emits DA: for
# instrumentable lines, so signature/comment/blank lines never count.
#
#   PumpAbort type + Display/Error impls (net-new, 622ee624)        [108-116]
#   proxy_request (lookahead + Branch A + Branch B pump:
#     drain-and-validate, is_end_stream gating, PumpAbort abort
#     arms, in-flight-counter body wrapper, Phase-1 reset reject)  [1333-1857]
#   validate_request_trailers                                      [2071-2088]
#   concat_chunks                                                  [2093-2102]
#   record_retained                                                [2471-2485]
# Binding bar: >= 80% on this sub-metric.
BEGIN { FS="[:,]" }
/^SF:/ { infile = ($0 ~ /h2_proxy\.rs/) }
infile && /^DA:/ {
  line=$2+0; cnt=$3+0
  if ((line>=108&&line<=116)||(line>=1333&&line<=1857)||(line>=2071&&line<=2088)||(line>=2093&&line<=2102)||(line>=2471&&line<=2485)) {
    tot++; if(cnt>0) cov++; else uncov[++u]=line
    if(line>=108&&line<=116){pat++; if(cnt>0)pac++}
    else if(line>=1333&&line<=1857){prt++; if(cnt>0)prc++}
    else if(line>=2071&&line<=2088){vtt++; if(cnt>0)vtc++}
    else if(line>=2093&&line<=2102){cct++; if(cnt>0)ccc++}
    else {rrt++; if(cnt>0)rrc++}
  }
}
END {
  printf "PumpAbort type            : %d/%d = %.2f%%\n", pac,pat, pat?100*pac/pat:0
  printf "proxy_request (pump)      : %d/%d = %.2f%%\n", prc,prt, prt?100*prc/prt:0
  printf "validate_request_trailers : %d/%d = %.2f%%\n", vtc,vtt, vtt?100*vtc/vtt:0
  printf "concat_chunks             : %d/%d = %.2f%%\n", ccc,cct, cct?100*ccc/cct:0
  printf "record_retained           : %d/%d = %.2f%%\n", rrc,rrt, rrt?100*rrc/rrt:0
  printf "SESSION TOTAL (M-D)       : %d/%d = %.2f%%  (need >=80%% => >=%d covered)\n", cov,tot, tot?100*cov/tot:0, int(0.8*tot+0.999)
  printf "UNCOVERED session lines (%d):\n", u
  line=""; for(i=1;i<=u;i++) line=line uncov[i] " "
  print line
}
