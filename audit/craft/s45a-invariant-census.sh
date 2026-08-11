#!/usr/bin/env bash
#
# S45A knowledge-regression check (R3 "third kind", R13).
#
# Written in Phase 0 under the conservative standard, when nothing was to be
# removed and any drop in a comment class meant something was lost. The owner
# then directed a 90% comment reduction ("only if the code doesn't make sense or
# there is a catch"), which makes raw MENTION counts meaningless: collapsing a
# block that cited RFC 9114 five times down to one citation is the goal, not a
# regression.
#
# So the metric is now the one that actually maps to knowledge:
#
#   BINDING   unsafe-block count is unchanged (a comment pass must not touch code)
#   BINDING   the named canaries survive
#   ADVISORY  which DISTINCT identifiers (RFC / CF- / F-S / ROUND8- / SEC-)
#             disappeared from the tree ENTIRELY — a halved mention count is
#             fine, a vanished identifier is worth a human look
#
# Usage: audit/craft/s45a-invariant-census.sh

set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1
BASE="${1:-main}"
rc=0

echo "S45A invariant census (baseline = $BASE)"

# ── BINDING: no code change ────────────────────────────────────────────────────
nu=$(grep -rhE '\bunsafe\s*[{(]|unsafe fn|unsafe impl' crates --include='*.rs' | wc -l)
if [ "$nu" -ne 95 ]; then
    echo "  FAIL  unsafe blocks: $nu != 95 — a comment pass must not change code"; rc=1
else
    echo "  ok    unsafe blocks: $nu (unchanged)"
fi

# Every unsafe block should still carry a SAFETY note within 4 lines. Measured
# on the baseline as 32 unannotated of 84 `unsafe {` blocks; it must not worsen.
unann=$(python3 - <<'PY'
import glob, re
bad = 0
for f in glob.glob('crates/**/*.rs', recursive=True):
    L = open(f, errors='ignore').read().split('\n')
    for i, l in enumerate(L):
        if re.search(r'\bunsafe\s*\{', l) and 'SAFETY' not in '\n'.join(L[max(0, i-4):i]).upper():
            bad += 1
print(bad)
PY
)
if [ "$unann" -gt 32 ]; then
    echo "  FAIL  unsafe blocks without a nearby SAFETY note: $unann > 32 (baseline)"; rc=1
else
    echo "  ok    unsafe blocks without a nearby SAFETY note: $unann (baseline 32)"
fi

# ── BINDING: named canaries ───────────────────────────────────────────────────
echo "canaries:"
canary() {
    if grep -qF "$1" "$2" 2>/dev/null; then
        echo "  ok    $3"
    else
        echo "  FAIL  CANARY LOST in $2 — $3"; rc=1
    fi
}
canary 'entry().or_insert_with()' crates/lb-quic/src/conn_actor.rs \
    'F-S29-1 gRPC-over-H3 trailer drop (get_mut, not or_insert_with)'
canary 'or_insert(0)' crates/lb-security/src/conn_gate.rs \
    'conn_gate re-insert-on-next-admit invariant'
canary 'ROUND8-L7-10 — take-and-discard upstream stream pattern' crates/lb-l7/src/h1_proxy.rs \
    'gate-read string asserted by tests/round8_body_overread.rs'
canary 'ROUND8-L7-10 — API contract for future H1 upstream reuse' crates/lb-io/src/pool.rs \
    'gate-read string asserted by lb-l7 tests/round8_body_overread.rs'
canary 'GHSA-ghc4-35x6-crw5' crates/lb-l7/src/h1_proxy.rs \
    'XFF append-not-insert: the Envoy silent-drop header class'

# ── ADVISORY: identifiers that vanished entirely ──────────────────────────────
echo "distinct identifiers lost entirely (advisory — mention counts are expected to fall):"
python3 - "$BASE" <<'PY'
import subprocess, re, glob, sys
base = sys.argv[1]
pats = {'RFC': r'RFC ?\d{4}', 'CF-': r'\bCF-[A-Z0-9][A-Z0-9-]+',
        'F-S': r'\bF-S\d+-[0-9A-Z]+', 'ROUND8': r'\bROUND8-[A-Z0-9-]+',
        'SEC-': r'\bSEC-\d-\d+'}

def collect(get, names):
    d = {k: set() for k in pats}
    for f in names:
        if not f.endswith('.rs') or not f.startswith('crates/'):
            continue
        try:
            s = get(f)
        except Exception:
            continue
        for k, p in pats.items():
            d[k].update(re.findall(p, s))
    return d

mainf = subprocess.run(['git', 'ls-tree', '-r', '--name-only', base],
                       capture_output=True, text=True).stdout.split('\n')
old = collect(lambda f: subprocess.run(['git', 'show', f'{base}:{f}'],
                                       capture_output=True, text=True).stdout, mainf)
new = collect(lambda f: open(f, errors='ignore').read(),
              glob.glob('crates/**/*.rs', recursive=True))
for k in pats:
    lost = sorted(old[k] - new[k])
    tag = f"  {k:7s} {len(old[k]):3d} -> {len(new[k]):3d} distinct"
    print(f"{tag}; lost {len(lost)}" + (f": {', '.join(lost)}" if lost else ""))
PY

[ $rc -ne 0 ] && echo "RESULT: BINDING CHECK FAILED — do not promote." \
              || echo "RESULT: no code change, no canary lost."
exit $rc
