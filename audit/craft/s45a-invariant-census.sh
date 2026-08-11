#!/usr/bin/env bash
#
# S45A knowledge-regression check (R3 "third kind", R13).
#
# A de-slop pass must not lose knowledge. Eyeballing a large deletion diff does
# not prove that; counting the load-bearing comment CLASSES does. This script
# censuses each class and compares against the Phase-0 baseline. Any class that
# SHRINKS is a knowledge regression and blocks the promote.
#
# It also asserts a set of named CANARIES: specific comments whose removal would
# reintroduce a known, already-paid-for defect. The canaries are matched by a
# distinctive phrase, so a reworded-but-preserved comment still passes while a
# deleted one fails loudly.
#
# Usage:
#   audit/craft/s45a-invariant-census.sh            # census + canaries
#   audit/craft/s45a-invariant-census.sh --baseline # print counts only

set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1

count() { grep -rn --include='*.rs' -E "$1" crates 2>/dev/null | wc -l; }
counti() { grep -rn --include='*.rs' -iE "$1" crates 2>/dev/null | wc -l; }

UNSAFE=$(count '\bunsafe\s*[{(]|unsafe fn|unsafe impl')
SAFETY=$(counti '^\s*(//|///)\s*SAFETY')
ALLOW=$(count '#!?\[allow\(')
RFC=$(count 'RFC ?[0-9]{4}')
CF=$(count '\bCF-[A-Z0-9-]+')
FS=$(count '\bF-S[0-9]+-[0-9A-Z]+')
H3SPEC=$(counti 'h3spec')

# Phase-0 baseline, measured on main @ ff39fa08 before any removal.
B_UNSAFE=95 B_SAFETY=61 B_ALLOW=186 B_RFC=547 B_CF=135 B_FS=132 B_H3SPEC=22

rc=0
chk() { # name current baseline
    if [ "$2" -lt "$3" ]; then
        echo "  FAIL  $1: $2 < baseline $3  (knowledge regression: $(($3-$2)) lost)"; rc=1
    else
        echo "  ok    $1: $2 (baseline $3)"
    fi
}

echo "S45A invariant census (baseline = main @ ff39fa08)"
chk "unsafe blocks   " "$UNSAFE"  "$B_UNSAFE"
chk "SAFETY comments " "$SAFETY"  "$B_SAFETY"
chk "allow() attrs   " "$ALLOW"   "$B_ALLOW"
chk "RFC citations   " "$RFC"     "$B_RFC"
chk "CF- references  " "$CF"      "$B_CF"
chk "F-S references  " "$FS"      "$B_FS"
chk "h3spec refs     " "$H3SPEC"  "$B_H3SPEC"

[ "${1:-}" = "--baseline" ] && exit 0

# Named canaries: phrase | file | what its removal would reintroduce.
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

if [ $rc -ne 0 ]; then
    echo "RESULT: KNOWLEDGE REGRESSION — do not promote."
else
    echo "RESULT: no knowledge regression detected."
fi
exit $rc
