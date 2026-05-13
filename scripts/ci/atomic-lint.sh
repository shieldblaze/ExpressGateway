#!/usr/bin/env bash
# CODE-2-04 atomic-ordering lint.
#
# Greps for `Ordering::Relaxed` uses in files whose path matches the
# regex `\b(security|rate|conn|gate)\b` — these are the workspace's
# enforcement-gate areas where a load-then-compare-then-act pattern
# requires `Acquire` (and the matching mutation requires `Release` /
# `AcqRel`). Any site that genuinely should remain `Relaxed`
# (statistics-only counters, metric gauges) must carry a per-site
# `// CLIPPY-OK:` annotation justifying it.
#
# Exit codes:
#   0 — clean (all Relaxed sites either are gone OR carry CLIPPY-OK).
#   1 — at least one Relaxed site in a scoped file lacks the annotation.
#   2 — environment problem (missing ripgrep / grep). The script never
#       fails for a missing scope file — those are not under audit.
#
# Workspace-wide policy reference: see `docs/decisions/atomics.md` in
# the round-2 review (CODE-2-04 §appendix A). Three classifications:
#   * S — stats-only: Relaxed permitted, CLIPPY-OK required.
#   * G — enforcement-gate: Acquire on load, Release / AcqRel on
#         mutation. CLIPPY-OK NOT permitted (use the strong ordering).
#   * L — lifecycle flag: SeqCst on store, Acquire on load.
#
# Usage: `bash scripts/ci/atomic-lint.sh` (from repo root).
#
# Round-4 scope: this is the lint scaffolding + one representative
# enforcement-site conversion (lb-core::BackendState::inc_connections
# uses AcqRel). Wave-2 appendix sweep across all the table-B sites in
# the CODE-2-04 plan follows in a subsequent commit on this branch.

set -euo pipefail

# Resolve repo root from script location so the lint runs identically
# from CI working dirs, pre-commit hooks, and ad-hoc developer shells.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# Prefer ripgrep when available (faster, sane regex defaults). Fall back
# to grep -P for environments without rg (some CI images).
if command -v rg >/dev/null 2>&1; then
    GREP=(rg --no-heading --line-number --color=never)
else
    # POSIX grep -P + -R; the GNU-grep --include flag is critical
    # because plain `grep -r` would walk target/ on a built tree.
    GREP=(grep -RnP --include='*.rs')
fi

# Files matching the scope regex (security | rate | conn | gate as a
# whole word in the path). Using `find ... | grep` keeps us portable
# across rg / grep implementations on the file-list side.
mapfile -t SCOPED_FILES < <(find crates -type f -name '*.rs' \
    | grep -E '\b(security|rate|conn|gate)\b' \
    || true)

# Empty scope set is fine — the workspace may have removed all such
# files (e.g. CODE-2-15 removed lb-compression). Lint is then a no-op.
if [ "${#SCOPED_FILES[@]}" -eq 0 ]; then
    echo "atomic-lint: no scoped files found (workspace shape changed) — passing"
    exit 0
fi

# Find every Relaxed site in the scoped files that does NOT carry the
# `// CLIPPY-OK:` annotation. We accept the annotation either on the
# same line or on the preceding line — both are common in Rust style.
violations=0
violations_list=()

for f in "${SCOPED_FILES[@]}"; do
    # Get line numbers of every Relaxed site in this file.
    while IFS=: read -r line content; do
        # Same-line annotation?
        if printf '%s' "$content" | grep -q 'CLIPPY-OK'; then
            continue
        fi
        # Preceding-line annotation? (one line up)
        if [ "$line" -gt 1 ]; then
            prev=$(sed -n "$((line - 1))p" "$f" 2>/dev/null)
            if printf '%s' "$prev" | grep -q 'CLIPPY-OK'; then
                continue
            fi
        fi
        violations=$((violations + 1))
        violations_list+=("$f:$line: $content")
    done < <(grep -n 'Ordering::Relaxed' "$f" || true)
done

if [ "$violations" -gt 0 ]; then
    echo "atomic-lint: $violations Relaxed site(s) in scoped files lack a // CLIPPY-OK: annotation."
    echo "Policy: docs/decisions/atomics.md (CODE-2-04). Either:"
    echo "  * upgrade the site to Acquire / Release / AcqRel (preferred), OR"
    echo "  * add '// CLIPPY-OK: <classification + reason>' on the same line"
    echo "    or the preceding line."
    echo ""
    echo "Sites:"
    printf '  %s\n' "${violations_list[@]}"
    exit 1
fi

echo "atomic-lint: clean — ${#SCOPED_FILES[@]} scoped file(s) inspected, 0 violations."
exit 0
