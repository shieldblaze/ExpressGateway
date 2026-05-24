#!/usr/bin/awk -f
# S9 / F-CAP-1 (H2→H1 cell) — coverage of the NEW caller-side send_request
# error arm in crates/lb-l7/src/h2_proxy.rs (commit 23b45d6f). This arm now
# consults the pump's classified verdict (BodyTooLarge/BadRequest) before
# falling through to the generic Upstream(502). Range = the `Ok(Err(e))` arm
# body [1822-1849]; lcov emits DA only for instrumentable lines (the long
# comment block 1823-1834 / 1839-1841 never count).
BEGIN { FS="[:,]" }
/^SF:/ { infile = ($0 ~ /h2_proxy\.rs/) }
infile && /^DA:/ {
  line=$2+0; cnt=$3+0
  if (line>=1822 && line<=1849) {
    tot++; if(cnt>0) cov++; else uncov[++u]=line
    printf "  DA %d = %d\n", line, cnt
  }
}
END {
  printf "F-CAP-1 H2 send-error arm: %d/%d = %.2f%%\n", cov,tot, tot?100*cov/tot:0
  printf "UNCOVERED (%d): ", u
  for(i=1;i<=u;i++) printf "%d ", uncov[i]
  print ""
}
