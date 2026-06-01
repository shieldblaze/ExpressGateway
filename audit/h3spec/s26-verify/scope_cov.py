#!/usr/bin/env python3
"""Phase-3 scoped llvm-cov analyzer (verifier).

Parses an lcov tracefile and reports LINE coverage for the migrated
production H3 surface, two ways per the llvm-cov-session-scope-method:
  (1) whole-file % (cross-check vs the `cargo llvm-cov` summary), and
  (2) session-scope sub-metric = DA per-line over the PRODUCTION ranges
      only (excluding the in-file #[cfg(test)] mod), and over the
      individual migrated fn ranges (E1 egress + E2 client).

Bar: >=80% line. Framing: S26 adds no new production code (deletion +
test relocation), so this is a NO-REGRESSION check on the E1+E2 surface.

Usage: scope_cov.py <lcov-path>
"""
import sys, collections

LCOV = sys.argv[1] if len(sys.argv) > 1 else "audit/h3spec/s26-verify/s26-cov.lcov"

# Production files of interest -> (prod_line_lo, prod_line_hi inclusive).
# Test mods excluded: h3_bridge #[cfg(test)] @2955; h3_config @94.
TARGETS = {
    "crates/lb-quic/src/h3_bridge.rs": (1, 2954),
    "crates/lb-quic/src/conn_actor.rs": (1, 10_000_000),  # external e2e tests; no in-file test mod
    "crates/lb-quic/src/h3_config.rs": (1, 93),
}
# Migrated fn line ranges in h3_bridge.rs (start -> just-before-next-fn),
# for the session-scope sub-metric (the headline E1 egress + E2 client).
FN_RANGES = {
    "validate_request_pseudo_headers (validator)": (473, 566),
    "stream_h1_response (E1->H1)": (880, 1195),
    "stream_h2_response (E1->H2)": (1196, 1332),
    "h3_to_h1_stream_resp (E1->H1 resp)": (1642, 1836),
    "h3_to_h2_stream_resp (E1->H2 resp)": (1905, 2232),
    "h3_to_h3_stream_resp (E1->H3 resp)": (2233, 2368),
    "stream_request_to_h3_upstream (E2 client)": (2369, 2954),
}

# lcov DA records keyed by source file -> {line: hits}
da = collections.defaultdict(dict)
cur = None
for raw in open(LCOV):
    line = raw.rstrip("\n")
    if line.startswith("SF:"):
        cur = line[3:]
        # normalize to repo-relative if absolute
        for t in TARGETS:
            if cur.endswith(t):
                cur = t
                break
    elif line.startswith("DA:") and cur in TARGETS:
        ln, hits = line[3:].split(",")[:2]
        da[cur][int(ln)] = int(hits)

def pct(covered, total):
    return (100.0 * covered / total) if total else float("nan")

def line_cov(fileda, lo, hi):
    lines = [(ln, h) for ln, h in fileda.items() if lo <= ln <= hi]
    total = len(lines)
    covered = sum(1 for _, h in lines if h > 0)
    return covered, total

print(f"lcov: {LCOV}")
print("=" * 72)
for t, (lo, hi) in TARGETS.items():
    if t not in da:
        print(f"\n{t}\n  !!! NO DA RECORDS — file not instrumented / not in lcov (check SF: path)")
        continue
    fileda = da[t]
    # whole-file (all instrumented lines present in lcov)
    wc, wt = line_cov(fileda, 1, 10_000_000)
    # production-scoped (exclude test mod)
    pc, pt = line_cov(fileda, lo, hi)
    print(f"\n{t}")
    print(f"  whole-file  : {wc}/{wt} = {pct(wc,wt):.2f}%  (cross-check vs llvm-cov summary)")
    label = "prod-scope " + (f"(lines {lo}-{hi}, test-mod excluded)" if hi < 10_000_000 else "(whole file; external tests)")
    bar = "PASS" if pct(pc, pt) >= 80 else "*** UNDER 80% ***"
    print(f"  {label}: {pc}/{pt} = {pct(pc,pt):.2f}%  [{bar}]")

# session-scope sub-metric over migrated fns (h3_bridge only)
HB = "crates/lb-quic/src/h3_bridge.rs"
if HB in da:
    print("\n" + "=" * 72)
    print(f"SESSION-SCOPE sub-metric — migrated E1+E2 fns in {HB}:")
    agg_c = agg_t = 0
    for name, (lo, hi) in FN_RANGES.items():
        c, t = line_cov(da[HB], lo, hi)
        agg_c += c; agg_t += t
        bar = "ok" if (t and pct(c, t) >= 80) else ("UNDER" if t else "no-lines")
        print(f"  {name:42s} {c:4d}/{t:<4d} = {pct(c,t):6.2f}%  [{bar}]")
    print(f"  {'— migrated-fns aggregate —':42s} {agg_c:4d}/{agg_t:<4d} = {pct(agg_c,agg_t):6.2f}%  "
          f"[{'PASS' if pct(agg_c,agg_t)>=80 else '*** UNDER 80% ***'}]")
