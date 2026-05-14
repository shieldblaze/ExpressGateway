#!/usr/bin/env bash
# REL-2-01: doc-lint guardrail.
#
# Walks the operator-facing docs at repo root and fails the build on
# stale references. Each pattern below is a regression the round-4
# audit found and fixed; without this gate, the doc-drift class of
# bug returns silently.
#
# Run locally:
#   ./scripts/ci/doc-lint.sh
#
# CI: invoked from .github/workflows/ci.yml `doc-lint` job.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"

# Files scanned. Add new operator-facing docs here when they land.
FILES=(
    "README.md"
    "RUNBOOK.md"
    "DEPLOYMENT.md"
    "METRICS.md"
    "CHANGELOG.md"
    "CONFIG.md"
    "SECURITY.md"
)

# Patterns that MUST NOT appear in any of the above files. Each row is
# `pattern || description`. The pattern is an ERE passed to `grep -E`.
#
# When updating: prefer narrow patterns (e.g. `/usr/local/bin/lb\b`)
# over broad ones (`\blb\b`) to keep false positives down.
STALE_PATTERNS=(
    'lb-compression||lb-compression crate (removed by L-001 in round-1)'
    '/usr/local/bin/lb([^a-z]|$)||/usr/local/bin/lb (binary is named expressgateway; REL-2-14)'
    'target/release/lb([^-a-z]|$)||target/release/lb (binary is named expressgateway; REL-2-14)'
    'cargo build --release -p lb($|[^ ]| [^-])||cargo build for lb without --bin expressgateway (REL-2-14)'
    'ExecStart=/usr/local/bin/lb ||systemd ExecStart using /usr/local/bin/lb (REL-2-14)'
    'strings /usr/local/bin/lb ||runbook reference to strings on /usr/local/bin/lb (REL-2-14)'
)

# Quoted-out exceptions. Lines matching these substrings are dropped
# BEFORE pattern matching. This lets us reference the stale strings
# inside CHANGELOG entries describing the fix.
#
# Use very narrow markers: a literal "doc-lint-allow" substring is the
# easiest. We also exempt scripts/ci/doc-lint.sh itself when scanned.
allow_substr='doc-lint-allow'

fail=0
fail_lines=()

for f in "${FILES[@]}"; do
    if [ ! -f "$f" ]; then
        continue
    fi
    # Filter out exempt lines first.
    while IFS= read -r row; do
        pat="${row%%||*}"
        desc="${row#*||}"
        # grep -n returns "line:content"; we suppress the leading file
        # since it's already in $f.
        if hits=$(grep -nE -- "$pat" "$f" | grep -v "$allow_substr" || true); [ -n "$hits" ]; then
            while IFS= read -r hit; do
                fail_lines+=("$f:$hit  [stale: $desc]")
            done <<<"$hits"
            fail=1
        fi
    done < <(printf '%s\n' "${STALE_PATTERNS[@]}")
done

if [ "$fail" -ne 0 ]; then
    echo "doc-lint: stale references found in operator-facing docs:" >&2
    for line in "${fail_lines[@]}"; do
        echo "  $line" >&2
    done
    echo "" >&2
    echo "Fix the line(s) above, or — if the reference is intentional" >&2
    echo "(e.g. describing the fix in CHANGELOG) — append the marker" >&2
    echo "'$allow_substr' to the line." >&2
    exit 1
fi

echo "doc-lint: OK"
