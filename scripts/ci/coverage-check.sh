#!/usr/bin/env bash
#
# D-6 coverage gate — per-module hot-path line coverage >= 80% (S34).
#
# The charter metric (audit/coverage-scope.md) is PER-MODULE, not an aggregate: the
# workspace average (~76.6%) hides an under-covered hot module. No averaging — any
# hot-path module below threshold turns this RED.
#
# ONE carve-out, named and justified: lb-l4-xdp/src/loader.rs performs the privileged
# XDP load / map-population syscalls, structurally unreachable without root. The
# charter defers these to CI integration (audit/round-7/deferred-to-ci.md); the load
# path is smoke-validated by D2-xdp-verifier-smoke, which loads the real object into
# the runner-kernel verifier. NAMED, not blanket.
#
# Fails closed if EVERY required pattern matches zero files (LCOV paths are wrong); a
# single unmatched pattern only warns, so a rename surfaces without wedging the gate.
#
# METRIC (S44 correction) — score merged DA: records, NEVER the LF:/LH: summary. 26
# modules incl. lb-l7 are compiled twice; llvm-cov merges both instantiations into one
# SF: record whose LF: counts shared source lines twice while the DA: records do not
# (h2_proxy.rs declared LF:1887 vs 1780 distinct DA:). The second instantiation is the
# lib-unit-test build, which CANNOT reach the request hot path at all
# (`hyper::body::Incoming` has no public constructor), so its unhit lines scored as
# genuine misses — the gate was a coin flip, 3 RED / 3 green on byte-identical source.
# A line is now counted ONCE, hit if ANY instantiation executed it.
#
# This is a CORRECTION, not a relaxation: it moves numbers in BOTH directions
# (random.rs 86.21 -> 85.71, conn_gate.rs 91.14 -> 90.91) because double-counted *hit*
# lines leave the numerator too; a loosening could only move them up. Threshold
# unchanged, nothing exempted, nothing falls below 80%.
# 31-module before/after: audit/ci/s44-coverage-metric-rebaseline.md
#
# Usage: coverage-check.sh <coverage.lcov>

set -uo pipefail
LCOV="${1:?usage: coverage-check.sh <coverage.lcov>}"
test -f "$LCOV" || { echo "::error::LCOV file $LCOV not found"; exit 1; }
THRESHOLD=80.0

python3 - "$LCOV" "$THRESHOLD" <<'PY'
import re, sys, collections

lcov_path, threshold = sys.argv[1], float(sys.argv[2])

# Merge every record and instantiation of a file; see the METRIC note in the header.
# `declared_lf` is retained ONLY to surface dual-instantiation, never to score.
lines_by_file = collections.defaultdict(dict)   # file -> {line_no: max_count}
declared_lf = collections.Counter()             # file -> sum of LF: across records
cur = None
for line in open(lcov_path):
    line = line.strip()
    if line.startswith("SF:"):
        cur = line[3:]
    elif line.startswith("LF:") and cur is not None:
        declared_lf[cur] += int(line[3:])
    elif line.startswith("DA:") and cur is not None:
        # DA:<line>,<count>[,<checksum>]
        parts = line[3:].split(",")
        try:
            ln, cnt = int(parts[0]), int(parts[1])
        except (ValueError, IndexError):
            continue
        # ALWAYS assign: `if cnt > d.get(ln, 0)` is the trap — 0 > 0 is False, so a
        # line whose every instantiation reports 0 never enters the dict, silently
        # leaving the DENOMINATOR and scoring every file 100%.
        d = lines_by_file[cur]
        d[ln] = max(d.get(ln, 0), cnt)
    elif line == "end_of_record":
        cur = None

files = {}
dual = []
for f, lns in lines_by_file.items():
    if not lns:
        continue
    files[f] = 100.0 * sum(1 for c in lns.values() if c > 0) / len(lns)
    if declared_lf[f] > len(lns):
        dual.append((f, declared_lf[f], len(lns)))

# Charter hot-path modules mapped to the CURRENT file layout. CHARTER DRIFT
# (audit/ci/s34-report.md): the charter's `lb-config::validate` and
# `lb-observability::metrics` no longer exist as standalone files, so they are not
# asserted; the request/packet hot path below is what the gate guards.
REQUIRED = [
    r"lb-l7/src/h1_proxy\.rs$",
    r"lb-l7/src/h2_proxy\.rs$",
    r"lb-l7/src/h[123]_to_h[123]\.rs$",          # bridges::*
    r"lb-l4-xdp/src/stats_export\.rs$",
    r"lb-balancer/src/[a-z_]+\.rs$",             # all balancer modules
    r"lb-security/src/(hooks|conn_gate|watchdog|ticket|smuggle)\.rs$",
    r"lb-quic/src/(conn_actor|listener)\.rs$",
    r"lb-observability/src/admin_http\.rs$",
]
EXEMPT = r"lb-l4-xdp/src/loader\.rs$"            # named, justified (see header)

def hit(pat, name): return re.search(pat, name) is not None

below, checked, empty_pats = [], [], []
exempt_hit = next(((n, p) for n, p in files.items() if hit(EXEMPT, n)), None)

for pat in REQUIRED:
    matches = [(n, p) for n, p in files.items() if hit(pat, n) and not hit(EXEMPT, n)]
    if not matches:
        empty_pats.append(pat); continue
    for n, p in sorted(matches):
        (below if p + 1e-9 < threshold else checked).append((n, p))

print(f"D-6 per-module hot-path coverage gate (threshold {threshold:.0f}% lines)")
print("  metric: merged DA: per-line (each source line counted once, hit if ANY "
      "instantiation ran it)")
print(f"  {len(checked)} hot-path modules passed, {len(below)} below, "
      f"{len(empty_pats)} pattern(s) unmatched")

# Visibility, not a gate: surface dual-instantiated files so the artifact cannot
# silently drift back. Scoring off their LF:/LH: would double-count them.
dual_hot = sorted((f, lf, n) for f, lf, n in dual
                  if any(hit(p, f) for p in REQUIRED) or hit(EXEMPT, f))
if dual_hot:
    print(f"  note: {len(dual_hot)} hot-path module(s) are dual-instantiated "
          "(declared LF: > distinct source lines); scored once each:")
    for f, lf, n in dual_hot[:6]:
        print(f"        {f.split('crates/')[-1]:<40} LF:{lf} distinct:{n} (+{lf-n})")
    if len(dual_hot) > 6:
        print(f"        … and {len(dual_hot)-6} more")
for n, p in sorted(checked):
    print(f"    OK     {p:6.2f}%  {n}")
if exempt_hit:
    print(f"    EXEMPT {exempt_hit[1]:6.2f}%  {exempt_hit[0]}  "
          "(XDP load needs root; validated by D2-xdp-verifier-smoke)")
else:
    print("::warning::carve-out lb-l4-xdp/src/loader.rs not present in LCOV "
          "(verify it was built/measured)")
for pat in empty_pats:
    print(f"::warning::hot-path pattern matched no files (renamed?): {pat}")

if len(empty_pats) == len(REQUIRED):
    print("::error::no hot-path files matched at all — LCOV paths look wrong."); sys.exit(1)
if below:
    for n, p in sorted(below):
        print(f"::error::hot-path module below {threshold:.0f}%: {n} = {p:.2f}%")
    print(f"::error::D-6 FAILED: {len(below)} hot-path module(s) under the charter threshold.")
    sys.exit(1)
print(f"PASS: every charter hot-path module is >= {threshold:.0f}% line coverage "
      "(loader.rs carved out + D2-validated).")
PY
