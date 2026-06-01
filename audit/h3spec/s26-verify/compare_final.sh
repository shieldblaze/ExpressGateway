#!/usr/bin/env bash
# Phase-3 h3spec second-read (verifier, independent). Usage:
#   compare_final.sh <final-h3spec-log>
# Diffs the FINAL (lb-h3-DELETED) per-case ✔/✘ set against BOTH references:
#   - s22-h3spec-postfix.log (hand-rolled stack, 19 fail)
#   - s26-h3spec-preview.log (migrated tip pre-deletion, 12 fail)
# Dedups the runner's "=== h3spec summary ===" tail (it re-prints the QPACK 3.1
# line, inflating a naive grep by 1). Authoritative totals come from each log's
# own "N examples, M failures" line.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
FINAL="${1:?usage: compare_final.sh <final-h3spec-log>}"

extract() { # path -> "case<TAB>mark" lines, deduped, sorted, ONLY the spec body
  # stop at "=== h3spec summary ===" so the duplicated tail line is excluded
  awk '/=== h3spec summary ===/{exit} {print}' "$1" \
    | grep -E 'MUST send .* \[(✔|✘)\]' \
    | sed -E 's/^[[:space:]]*//; s/ \[(✔|✘)\][[:space:]]*$/\t\1/' \
    | sort -u
}

S22="$HERE/s22-ref.cases.txt"
PREV="$HERE/prev-ref.cases.txt"
FIN="$HERE/final.cases.txt"
extract /home/ubuntu/Code/ExpressGateway/audit/h3spec/s22-h3spec-postfix.log > "$S22"
extract /home/ubuntu/Code/ExpressGateway/audit/h3spec/s26-h3spec-preview.log > "$PREV"
extract "$FINAL" > "$FIN"

echo "=== authoritative 'examples, failures' lines ==="
grep -hE 'examples, .* failures' /home/ubuntu/Code/ExpressGateway/audit/h3spec/s22-h3spec-postfix.log \
  /home/ubuntu/Code/ExpressGateway/audit/h3spec/s26-h3spec-preview.log "$FINAL"
echo "=== sentinel of final run (must be present = COMPLETED; rc echoed) ==="
grep -E 'S26-H3SPEC-.*-DONE rc=' "$FINAL" || echo "!!! NO SENTINEL — run not complete, DO NOT verdict"
echo
echo "case counts (deduped): s22=$(wc -l <"$S22")  preview=$(wc -l <"$PREV")  final=$(wc -l <"$FIN")"
echo
echo "=== [A] FINAL vs PREVIEW — MUST be identical (deletion changes no prod behavior) ==="
if diff "$PREV" "$FIN" >/dev/null; then echo "  PASS: final == preview (byte-identical ✔/✘ set)"; else
  echo "  *** MISMATCH ***"; diff "$PREV" "$FIN"; fi
echo
echo "=== [B] regressions vs PREVIEW (✔→✘) — MUST be NONE ==="
join -t$'\t' "$PREV" "$FIN" 2>/dev/null \
  | awk -F'\t' '$2=="✔" && $3=="✘"{print "  REGRESSION:",$1}' || true
echo "  (no lines above = zero ✔→✘ regressions)"
echo
echo "=== [C] the 7 carried CLOSED by migration (✘ in s22 -> ✔ in final) — expect exactly these 7 ==="
join -t$'\t' "$S22" "$FIN" \
  | awk -F'\t' '$2=="✘" && $3=="✔"{print "  CLOSED:",$1}'
echo
echo "=== [D] still ✘ in final (expect 12: transport #1-10 + QPACK #23/#25) ==="
awk -F'\t' '$2=="✘"{print "  FAIL:",$1}' "$FIN"
echo
echo "=== [E] sanity: #11-15 + #22 (the S22-fixed H3/QPACK cases) still ✔ in final ==="
for pat in 'DATA is received before HEADERS' 'pseudo-header is duplicated' \
           'mandatory pseudo-header fields are absent' 'prohibited pseudo-header fields are present' \
           'pseudo-header fields exist after fields' 'invalid static table index'; do
  m=$(grep -F "$pat" "$FIN" | sed -E 's/.*\t//')
  echo "  [$m] $pat"
done
