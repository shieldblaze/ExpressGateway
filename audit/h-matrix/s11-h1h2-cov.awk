#!/usr/bin/awk -f
# S11 / H1→H2 (R8 both-legs-streaming) — SESSION-CODE coverage sub-metric
# from an lcov trace, scoped to the NET-NEW H1→H2 session code at the S11 tip
# 705916d1 (origin/feature/h-matrix-s11). Mirrors audit/h-matrix/
# s10-h2h2-cov.awk / s9-h1h1-cov.awk: each range spans the `fn` signature
# through the closing brace of its body; lcov only emits DA: for instrumentable
# lines, so signature/comment/blank lines never count toward the denominator.
#
# REPRODUCE (verifier note): the session code lives in crate lb-l7, but the
# streaming-verify test targets live in the ROOT package lb-integration-tests
# (workspace-root tests/). A scoped `cargo llvm-cov --test <...>` run on the
# root package AUTO-IGNORES every other workspace member (it injects
# `-ignore-filename-regex .../crates/lb-l7($|/)` etc. into `llvm-cov export`),
# so the lcov for h1_proxy.rs / h2_proxy.rs comes out EMPTY. And `--package
# lb-l7` CANNOT be combined with the root `--test` names in the build step
# (cargo then looks for those test targets inside lb-l7 and errors). So use a
# TWO-STEP recipe: build+run scoped to generate the profile, then SCOPE THE
# REPORT to lb-l7:
#   export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
#   cargo llvm-cov clean --workspace   # if deps were built non-instrumented
#   # step 1: instrument + run the 3 scoped suites (root-package test names)
#   cargo llvm-cov --no-report --all-features \
#     --test h1h2_md_streaming_verify --test h1h2_resp_streaming_verify \
#     --test h2h2_md_streaming_verify
#   # step 2: emit lcov scoped to lb-l7 (drops it from the auto-ignore set)
#   cargo llvm-cov report --package lb-l7 --lcov --output-path /tmp/s11_session.lcov
#   awk -f audit/h-matrix/s11-h1h2-cov.awk /tmp/s11_session.lcov
# (`--no-cfg-coverage` is NOT used: it leaves the dep rlibs uninstrumented and
# the report is empty regardless.)
#
#   crates/lb-l7/src/h1_proxy.rs:
#     proxy_h1_to_h2_request (streaming M-D-lite request leg:
#       lookahead/Branch A buffered + Branch B streamed pump,
#       verdict channel, F-MD-4 abort, F-CAP-1, drive_h2 call)   [1586-1899]
#     proxy_h1_to_h2 (dispatch shim + status->H1 mapping)         [1910-1943]
#     build_h1_to_h2_upstream_parts (H1->H2 head bridge norm)     [2418-2476]
#     h1_to_h2_proxy_err (ProxyErr -> H2ProxyErr map)             [2483-2490]
#     concat_h1_chunks (Branch-A lookahead concat)                [2497-2506]
#     upstream_response_to_h1 (STREAMING H2->H1 response relay)   [2578-2619]
#   crates/lb-l7/src/h2_proxy.rs:
#     drive_h2_upstream_send (SHARED I1 egress driver, exercised
#       by BOTH H2->H2 and H1->H2: detached send task + verdict
#       gate + reset_peer + head_rx await)                        [2585-2730]
# Binding bar: >= 80% on this SESSION sub-metric (NOT the whole file).
BEGIN { FS="[:,]" }
/^SF:/ {
  in_h1 = ($0 ~ /h1_proxy\.rs/)
  in_h2 = ($0 ~ /h2_proxy\.rs/)
}
in_h1 && /^DA:/ {
  line=$2+0; cnt=$3+0
  if ((line>=1586&&line<=1899)||(line>=1910&&line<=1943)||(line>=2418&&line<=2476)||(line>=2483&&line<=2490)||(line>=2497&&line<=2506)||(line>=2578&&line<=2619)) {
    tot++; if(cnt>0) cov++; else uncov[++u]=line
    if(line>=1586&&line<=1899){qt++; if(cnt>0)qc++}
    else if(line>=1910&&line<=1943){dt++; if(cnt>0)dc++}
    else if(line>=2418&&line<=2476){bt++; if(cnt>0)bc++}
    else if(line>=2483&&line<=2490){et++; if(cnt>0)ec++}
    else if(line>=2497&&line<=2506){ct++; if(cnt>0)cc++}
    else {rt++; if(cnt>0)rc++}
  }
}
in_h2 && /^DA:/ {
  line=$2+0; cnt=$3+0
  if (line>=2585&&line<=2730) {
    tot++; if(cnt>0) cov++; else uncov[++u]=("h2:" line)
    st++; if(cnt>0)sc++
  }
}
END {
  printf "proxy_h1_to_h2_request (stream pump): %d/%d = %.2f%%\n", qc,qt, qt?100*qc/qt:0
  printf "proxy_h1_to_h2 (dispatch shim)      : %d/%d = %.2f%%\n", dc,dt, dt?100*dc/dt:0
  printf "build_h1_to_h2_upstream_parts       : %d/%d = %.2f%%\n", bc,bt, bt?100*bc/bt:0
  printf "h1_to_h2_proxy_err (err map)        : %d/%d = %.2f%%\n", ec,et, et?100*ec/et:0
  printf "concat_h1_chunks                    : %d/%d = %.2f%%\n", cc,ct, ct?100*cc/ct:0
  printf "upstream_response_to_h1 (relay)     : %d/%d = %.2f%%\n", rc,rt, rt?100*rc/rt:0
  printf "drive_h2_upstream_send (h2_proxy.rs): %d/%d = %.2f%%\n", sc,st, st?100*sc/st:0
  printf "SESSION TOTAL (H1->H2 R8)           : %d/%d = %.2f%%  (need >=80%% => >=%d covered)\n", cov,tot, tot?100*cov/tot:0, int(0.8*tot+0.999)
  printf "UNCOVERED session lines (%d):\n", u
  line=""; for(i=1;i<=u;i++) line=line uncov[i] " "
  print line
}
