#!/usr/bin/env python3
# Phase 3 — isolate S6-product line coverage from s6.lcov.
# Methodology mirrors audit/h3-program/s5-evidence/phase2-coverage/isolate_s5.py.
#
# S6 product delta = commit range 60a13ddc..9cb91cee, restricted to the
# 5 product SOURCE files (tests / evidence / inherited code excluded).
# Added post-image line ranges were derived programmatically from
#   git diff 60a13ddc..9cb91cee -- <file>
# by walking hunk headers and counting only '+' lines (new code), then
# compressing to contiguous ranges.
#
# EXCLUSION: h3_bridge.rs added range 3083-3259 lies inside
#   `#[cfg(test)] mod tests` (starts line 2811) -> it is TEST code and is
# excluded per the task ("exclude tests, evidence, and inherited code").
# No other product file added lines inside a #[cfg(test)] module.
#
# Coverage of an added line is taken from the lcov DA: record for that
# file. Lines with no DA record (non-executable: decls, braces, comments,
# blank, type aliases, match arms w/o body) are not counted (same as S5).
import sys

lcov = sys.argv[1] if len(sys.argv) > 1 else "s6.lcov"
cov = {}
cur = None
for ln in open(lcov):
    ln = ln.rstrip("\n")
    if ln.startswith("SF:"):
        cur = ln[3:]
        cov.setdefault(cur, {})
    elif ln.startswith("DA:") and cur:
        a, b = ln[3:].split(",")[:2]
        cov[cur][int(a)] = int(b)


def pick(suffix):
    m = [k for k in cov if k.endswith(suffix)]
    if not m:
        return None
    return m[0]


# (label, file-suffix, list-of-(start,end) inclusive post-image ranges)
S6 = {
    "http2_pool.rs (I0.5 boxed-error widening)": (
        "crates/lb-io/src/http2_pool.rs",
        [(59, 77), (121, 121), (229, 229), (256, 256),
         (272, 272), (297, 297), (318, 318)],
    ),
    "h1_proxy.rs (translate_h1_request_to_h2 adapt)": (
        "crates/lb-l7/src/h1_proxy.rs",
        [(1608, 1608), (1680, 1688)],
    ),
    "h2_proxy.rs (translate_h2_request_to_h2 adapt)": (
        "crates/lb-l7/src/h2_proxy.rs",
        [(1482, 1482), (1570, 1575)],
    ),
    "conn_actor.rs (H2 wiring in poll_h3 + ActorParams)": (
        "crates/lb-quic/src/conn_actor.rs",
        [(40, 40), (125, 127), (793, 808), (812, 834), (836, 862)],
    ),
    "h3_bridge.rs (H3->H2 streaming path; tests 3083-3259 EXCLUDED)": (
        "crates/lb-quic/src/h3_bridge.rs",
        [(1457, 1608), (2136, 2371)],
    ),
}

te = th = 0
for lbl, (suf, rngs) in S6.items():
    fp = pick(suf)
    print("=== %s ===" % lbl)
    if fp is None:
        print("  !! file not in lcov: %s" % suf)
        continue
    fe = fh = 0
    miss = []
    for (a, b) in rngs:
        for l in range(a, b + 1):
            if l in cov[fp]:
                fe += 1
                if cov[fp][l] > 0:
                    fh += 1
                else:
                    miss.append(l)
    te += fe
    th += fh
    pct = 100.0 * fh / fe if fe else 100.0
    print("  %d/%d = %.1f%%%s" % (fh, fe, pct,
          "  MISSING=%s" % miss if miss else ""))

print("\nISOLATED S6 AGGREGATE: %d/%d = %.2f%%" % (th, te,
      100.0 * th / te if te else 100.0))
print("GATE (>=80%%): %s" % ("PASS" if te and 100.0 * th / te >= 80.0
                             else "FAIL"))
