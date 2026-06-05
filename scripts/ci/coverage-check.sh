#!/usr/bin/env bash
#
# D-6 coverage gate — PER-MODULE hot-path threshold (S34).
#
# The coverage charter (audit/coverage-scope.md) defines the gate as
# "Per-module line coverage >= 80%" on the named hot-path modules — NOT a
# whole-workspace (or whole-package) AGGREGATE. The previous CI job applied
# `--fail-under-lines 80` to a 7-package aggregate, which (a) never matched the
# charter metric and (b) is an average that hides an under-covered hot module
# (the full-workspace aggregate is only ~76.6%, yet every hot-path module is
# >= 80% — except the carve-out below). This script enforces the charter's real
# metric: it reads per-file line coverage from an LCOV report and requires EACH
# hot-path file to be >= 80%.
#
# HONESTY CONTRACT (mirrors the h3spec named-waiver gate):
#   * A hot-path module that drops below 80% turns this RED. No averaging.
#   * Exactly ONE carve-out, named + justified: lb-l4-xdp/src/loader.rs. It
#     performs the privileged XDP load / map-population syscalls that a unit
#     harness cannot exercise without root, so its load-path lines are
#     structurally unreachable here. The charter itself defers these to CI
#     integration (audit/coverage-scope.md, audit/round-7/deferred-to-ci.md);
#     the load path is instead smoke-validated by D2-xdp-verifier-smoke, which
#     loads the real object into the runner-kernel verifier. NAMED, not blanket.
#   * If EVERY required pattern matches zero files the gate fails closed (the
#     LCOV paths are wrong); a single non-matching pattern is loud-warned so a
#     legitimate rename surfaces without wedging the gate.
#
# Usage: coverage-check.sh <coverage.lcov>

set -uo pipefail
LCOV="${1:?usage: coverage-check.sh <coverage.lcov>}"
test -f "$LCOV" || { echo "::error::LCOV file $LCOV not found"; exit 1; }
THRESHOLD=80.0

python3 - "$LCOV" "$THRESHOLD" <<'PY'
import re, sys

lcov_path, threshold = sys.argv[1], float(sys.argv[2])

# Parse LCOV: per record, SF:<file> ... LF:<found> LH:<hit> end_of_record.
files = {}
cur = None
lf = lh = 0
for line in open(lcov_path):
    line = line.strip()
    if line.startswith("SF:"):
        cur, lf, lh = line[3:], 0, 0
    elif line.startswith("LF:"):
        lf = int(line[3:])
    elif line.startswith("LH:"):
        lh = int(line[3:])
    elif line == "end_of_record" and cur is not None:
        if lf > 0:
            files[cur] = 100.0 * lh / lf
        cur = None

# Charter hot-path modules mapped to the CURRENT file layout. "bridges::*" =
# the cross-protocol h*_to_h*.rs files; lb-balancer "* (all)" = every
# lb-balancer/src/*.rs. CHARTER DRIFT (documented in audit/ci/s34-report.md):
# the charter's `lb-config::validate` and `lb-observability::metrics` no longer
# exist as standalone files, so they are not asserted; the request/packet hot
# path below is what the gate guards.
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
print(f"  {len(checked)} hot-path modules passed, {len(below)} below, "
      f"{len(empty_pats)} pattern(s) unmatched")
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
