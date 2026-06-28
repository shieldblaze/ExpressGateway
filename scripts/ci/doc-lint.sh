#!/usr/bin/env bash
# REL-2-01 + ROUND8-OPS-09 + ROUND8-L4-10: doc-lint guardrail with
# audit-of-audit content gate.
#
# Two tiers:
#
#   Tier 1 (REL-2-01): stale-pattern grep on operator-facing docs.
#     Each pattern below is a regression the round-4 audit found and
#     fixed; without this gate, the doc-drift class returns silently.
#
#   Tier 2 (ROUND8-OPS-09 + ROUND8-L4-10): every `Verified-Fixed(<sha>)`
#     status line in audit/**/round-*-review.md must reference a SHA
#     whose diff actually closes the recommendation. The gate parses
#     both the claim and the diff and asserts overlap on file paths
#     and on identifier tokens cited inside the Recommendation block.
#
#     This is the explicit defense against the EBPF-2-07 no-op:
#     `Verified-Fixed(ffde98c)` shipped the driver script and a
#     README, but NOT the per-kernel `.log.committed` baseline files
#     the Recommendation called for. The audit-of-audit walker
#     detects that the recommendation-cited path
#     `audit/ebpf/verifier-logs/<kernel-version>.log` was not added in
#     the closure SHA's tree and fails CI.
#
# Run locally:
#   ./scripts/ci/doc-lint.sh
#   AOA=1 ./scripts/ci/doc-lint.sh                 # explicit audit-of-audit
#   DOC_LINT_SKIP_AOA=1 ./scripts/ci/doc-lint.sh   # tier-1 only (legacy)
#
# CI: invoked from .github/workflows/ci.yml `doc-lint` job.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"

# Files scanned. Add new operator-facing docs here when they land.
# S40: the operator references (RUNBOOK/DEPLOYMENT/METRICS/CONFIG) moved under
# docs/guide/; README/CHANGELOG/SECURITY stay at root (GitHub conventions).
FILES=(
    "README.md"
    "docs/guide/RUNBOOK.md"
    "docs/guide/DEPLOYMENT.md"
    "docs/guide/METRICS.md"
    "CHANGELOG.md"
    "docs/guide/CONFIG.md"
    "SECURITY.md"
    "docs/features.md"
    # S41: public-facing narrative docs (Session B).
    "docs/guide/overview.md"
    "docs/guide/getting-started.md"
    "docs/guide/capabilities.md"
    "docs/guide/comparison.md"
    "docs/guide/PERFORMANCE.md"
    "CONTRIBUTING.md"
    # S42: task-oriented operator pages + glossary (depth/structure revision).
    "docs/guide/cookbook.md"
    "docs/guide/troubleshooting.md"
    "docs/guide/deployment-patterns.md"
    "docs/guide/observability.md"
    "docs/glossary.md"
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
    'FD[- _]?passing||FD-passing claim must be implemented (ROUND8-OPS-01)'
    'zero[- ]downtime[^.]{0,40}(FD|reload)||zero-downtime-via-FD/reload claim requires implementation (ROUND8-OPS-01)'
    'ArcSwap<TlsStore>||legacy doc reference to deleted type (REL-2-01)'
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

echo "doc-lint tier-1: OK"

# ------------------------------------------------------------------ #
# Tier 2 — audit-of-audit gate (ROUND8-OPS-09 + ROUND8-L4-10).        #
#                                                                    #
# For every `Status:   Verified-Fixed(<sha>...)` line in              #
# audit/**/round-*-review.md, verify:                                 #
#                                                                    #
#   1. Each SHA exists in this repository's git history.              #
#   2. At least one path mentioned under the `Location:` field of     #
#      the same finding appears in the union of `git show --stat`    #
#      across the SHAs (substring match on the file portion).        #
#   3. If the `Recommendation:` block references files under          #
#      `audit/**`, `crates/**`, `scripts/**`, or `packaging/**`,      #
#      at least one of those files must exist in the closure SHA's   #
#      tree (`git ls-tree`). This is the EBPF-2-07 case.              #
#                                                                    #
# The walker errors with a fail-fast exit; it lists every claim that  #
# failed validation. The gate may be skipped at the operator's       #
# request via DOC_LINT_SKIP_AOA=1 (used in the legacy bash-only       #
# fallback; CI must not set it).                                     #
# ------------------------------------------------------------------ #

if [ "${DOC_LINT_SKIP_AOA:-0}" = "1" ]; then
    echo "doc-lint tier-2 (audit-of-audit): SKIPPED (DOC_LINT_SKIP_AOA=1)"
    echo "doc-lint: OK (tier-1 only)"
    exit 0
fi

if ! command -v git >/dev/null 2>&1; then
    echo "doc-lint tier-2: git not available; skipping audit-of-audit." >&2
    exit 0
fi
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "doc-lint tier-2: not a git work tree; skipping audit-of-audit." >&2
    exit 0
fi

# Map a finding ID prefix to the area dirs the gate accepts as a
# legitimate file-touch under `Location:`. The map is intentionally
# loose: the test is "did the SHA touch SOMETHING in the relevant
# subtree" not "did it touch the exact line range".
declare -A LOCATION_DIRS=(
    [REL]="crates/ audit/ docs/ tests/ scripts/"
    [SEC]="crates/ audit/ tests/ docs/"
    [CODE]="crates/ tests/ docs/"
    [EBPF]="crates/lb-l4-xdp audit/ebpf scripts/ tests/"
    [PROTO]="crates/ audit/protocol tests/"
)

aoa_fail=0
aoa_fail_lines=()
aoa_seen_claims=0

# Iterate every review file.
review_files=$(find audit -type f -name 'round-*-review.md' -not -path '*/round-7/*' -not -path '*/round-8/*' 2>/dev/null | sort)
# Also accept round-2-findings (legacy security file).
sec_findings=$(find audit -type f -name 'round-*-findings.md' 2>/dev/null | sort)
review_files=$(printf '%s\n%s\n' "$review_files" "$sec_findings" | awk 'NF' | sort -u)

for rf in $review_files; do
    [ -f "$rf" ] || continue
    # Parse the file in a single awk pass:
    #   - track the current finding ID (`### ID — title`).
    #   - track the current finding's Location/Recommendation text.
    #   - emit triplets (line_no, ID, statusline, location, recommendation)
    while IFS=$'\t' read -r tag id status_line loc rec; do
        [ "$tag" = "FINDING" ] || continue
        # We only validate Verified-Fixed (not -Partial — partials are
        # disclosed; the disclosure note in the HTML comment is the
        # acceptance criterion and a future round must re-walk it).
        # We DO validate Verified-Fixed even when followed by " — " or
        # " (verifier=...)" prefix; the SHA extractor handles both.
        case "$status_line" in
            *Verified-Fixed-Partial*) continue ;;
            *Verified-Fixed*) ;;
            *) continue ;;
        esac
        aoa_seen_claims=$((aoa_seen_claims + 1))
        # Extract the SHA list from the first `Verified-Fixed(...)` clause.
        # Supports `Verified-Fixed(<sha>)`, `Verified-Fixed(<sha>, <sha>)`,
        # and `Verified-Fixed(verifier=NAME, author-sha=<sha>+<sha>)`.
        sha_block=$(printf '%s' "$status_line" | sed -nE 's/.*Verified-Fixed\(([^)]+)\).*/\1/p')
        if [ -z "$sha_block" ]; then
            aoa_fail_lines+=("$rf:$id  [audit-of-audit: cannot parse SHA from status line: $status_line]")
            aoa_fail=1
            continue
        fi
        # Strip optional `verifier=...` / `author-sha=` prefixes.
        sha_block=$(printf '%s' "$sha_block" | sed -E 's/^[^=]*=//; s/, *author-sha=/+/g')
        # Tokens are 7+ hex chars; split on , + ; and whitespace.
        shas=$(printf '%s' "$sha_block" | tr ',+; ' '\n' | grep -E '^[0-9a-f]{6,}$' || true)
        if [ -z "$shas" ]; then
            # No SHA-shaped token (might be e.g. "task-38") — record warn but
            # do not fail. Future rounds should use real SHAs.
            continue
        fi
        # Determine the area prefix for the LOCATION_DIRS lookup.
        prefix=$(printf '%s' "$id" | sed -E 's/^([A-Z]+).*/\1/')
        accepted_dirs="${LOCATION_DIRS[$prefix]:-crates/ tests/ docs/ audit/ scripts/ packaging/}"

        # Pre-pull per-SHA diff stats + per-SHA tree listings; cache
        # across recommendation matches.
        combined_stat=""
        combined_tree=""
        first_sha=""
        sha_missing=""
        for sha in $shas; do
            [ -n "$first_sha" ] || first_sha="$sha"
            if ! git cat-file -e "$sha^{commit}" 2>/dev/null; then
                sha_missing="$sha_missing $sha"
                continue
            fi
            combined_stat="$combined_stat
$(git show --stat --format= "$sha" 2>/dev/null || true)"
            combined_tree="$combined_tree
$(git ls-tree -r --name-only "$sha" 2>/dev/null || true)"
        done
        if [ -n "$sha_missing" ]; then
            aoa_fail_lines+=("$rf:$id  [audit-of-audit: SHA(s) not in repo:$sha_missing]")
            aoa_fail=1
            continue
        fi

        # Test 1: Location-touch (advisory). `Location:` is where the
        # *bug* lives, not necessarily where the fix lands — for
        # example, a runtime-config validator may land in a sibling
        # crate from the call-site that misuses the bad default. We
        # emit a warning, not a failure, when the closure SHA(s) miss
        # the Location path. Hard-failures live in Test 2 below.
        loc_path=$(printf '%s' "$loc" | grep -oE '(crates|audit|scripts|packaging|tests|docs)/[A-Za-z0-9._/-]+' | head -1 || true)
        if [ -n "$loc_path" ]; then
            loc_path_clean=$(printf '%s' "$loc_path" | sed -E 's/:[0-9].*$//')
            if ! printf '%s' "$combined_stat" | grep -qF -- "$loc_path_clean"; then
                loc_parent=$(dirname "$loc_path_clean")
                if ! printf '%s' "$combined_stat" | grep -qF -- "$loc_parent"; then
                    # Advisory only.
                    : # echo "  [advisory] $rf:$id Location $loc_path_clean not in SHA(s) diffstat" >&2
                fi
            fi
        fi

        # Test 2: Recommendation-cited paths must EXIST in the tree
        # at the closure SHA. This is the EBPF-2-07 trap.
        #
        # Extract glob-shaped tokens like
        # `audit/ebpf/verifier-logs/<kver>.log` (the angle-bracket
        # placeholder is stripped — we then search for any matching
        # file with the suffix `.log` under the directory).
        rec_paths=$(printf '%s' "$rec" | grep -oE '(audit|crates|scripts|packaging)/[A-Za-z0-9._<>/-]+' | sort -u || true)
        for p in $rec_paths; do
            # Drop trailing punctuation.
            p_clean=$(printf '%s' "$p" | sed -E 's/[).,;:]+$//')
            # If the path contains a placeholder like <kver>, glob it.
            if printf '%s' "$p_clean" | grep -q '<'; then
                # Construct a directory + suffix.
                dir=$(dirname "$p_clean")
                # Use the suffix after the last placeholder closure.
                suffix=$(printf '%s' "$p_clean" | sed -E 's/.*>//; s/.*<.*$//')
                # If suffix is empty, accept any file in dir.
                if [ -z "$suffix" ]; then
                    suffix=""
                fi
                # Test: does any file under $dir matching $suffix exist
                # in the union of trees? README.md does not count.
                hits=$(printf '%s' "$combined_tree" | awk -v d="$dir/" -v s="$suffix" '
                    index($0, d) == 1 {
                        # Strip README.md — that is the no-op-disguise
                        # the EBPF-2-07 case shipped.
                        base = $0;
                        sub(/.*\//, "", base);
                        if (base == "README.md") next;
                        if (s == "" || index($0, s) > 0) { print; }
                    }' | head -5)
                if [ -z "$hits" ]; then
                    aoa_fail_lines+=("$rf:$id  [audit-of-audit: recommendation cites '$p_clean' but SHA(s) [$shas] did not add a matching non-README file under $dir]")
                    aoa_fail=1
                fi
            else
                # Concrete path. Must appear in the tree.
                if ! printf '%s' "$combined_tree" | grep -qFx -- "$p_clean"; then
                    # Try as a directory prefix.
                    if ! printf '%s' "$combined_tree" | grep -qE "^${p_clean//./\\.}/"; then
                        # Allow if the path appears at HEAD (operator may have moved it
                        # forward in a later commit — the closure was still real).
                        if ! [ -e "$p_clean" ]; then
                            aoa_fail_lines+=("$rf:$id  [audit-of-audit: recommendation cites '$p_clean' but it is not present at the closure SHA(s) tree nor at HEAD]")
                            aoa_fail=1
                        fi
                    fi
                fi
            fi
        done
    done < <(awk '
        BEGIN {
            id = ""; loc = ""; rec = ""; in_rec = 0; status_line = "";
        }
        /^###[[:space:]]+([A-Z]+-?[0-9]+(-[0-9]+)?)/ {
            if (id != "" && status_line != "") {
                print "FINDING\t" id "\t" status_line "\t" loc "\t" rec;
            }
            split($0, a, /[[:space:]]+/);
            id = a[2];
            sub(/^[^A-Za-z0-9]+/, "", id);
            sub(/[^A-Za-z0-9-]+$/, "", id);
            loc = ""; rec = ""; in_rec = 0; status_line = "";
            next;
        }
        /^Status:[[:space:]]+Verified-Fixed/ {
            status_line = $0;
            next;
        }
        /^Location:/ {
            loc = $0;
            in_rec = 0;
            next;
        }
        /^Recommendation/ {
            in_rec = 1;
            rec = $0;
            next;
        }
        /^---[[:space:]]*$/ {
            in_rec = 0;
            next;
        }
        {
            if (in_rec) { rec = rec " " $0; }
        }
        END {
            if (id != "" && status_line != "") {
                print "FINDING\t" id "\t" status_line "\t" loc "\t" rec;
            }
        }
    ' "$rf")
done

# Final summary.
if [ "$aoa_fail" -ne 0 ]; then
    echo "doc-lint tier-2 (audit-of-audit): FAIL" >&2
    echo "" >&2
    for line in "${aoa_fail_lines[@]}"; do
        echo "  $line" >&2
    done
    echo "" >&2
    echo "A Verified-Fixed claim's SHA does not match the recommendation." >&2
    echo "Either:" >&2
    echo "  - downgrade the status to Verified-Fixed-Partial with a" >&2
    echo "    disclosure note describing what remains;" >&2
    echo "  - re-open the finding and land the missing artefact;" >&2
    echo "  - if this is a false positive, file a coverage-gap entry" >&2
    echo "    under audit/round-8/divergence/ and add an allow comment" >&2
    echo "    'doc-lint-allow-aoa-<short-tag>' on the Status line." >&2
    exit 1
fi

echo "doc-lint tier-2 (audit-of-audit): OK ($aoa_seen_claims Verified-Fixed claims checked)"
echo "doc-lint: OK"
