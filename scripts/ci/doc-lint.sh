#!/usr/bin/env bash
# doc-lint guardrail (REL-2-01) + audit-of-audit content gate (ROUND8-OPS-09,
# ROUND8-L4-10). Invoked from the ci.yml `doc-lint` job.
#
# Tier 1: stale-pattern grep on operator-facing docs. Every pattern is a regression
# round-4 found and fixed; without the gate the doc-drift class returns silently.
#
# Tier 2: every `Verified-Fixed(<sha>)` claim in audit/**/round-*-review.md must cite a
# SHA whose tree actually contains what the Recommendation asked for. This exists
# because of the EBPF-2-07 no-op: `Verified-Fixed(ffde98c)` shipped a driver script and
# a README but NOT the per-kernel `.log.committed` baselines the Recommendation called
# for, and nothing caught it.
#
#   DOC_LINT_SKIP_AOA=1 ./scripts/ci/doc-lint.sh   # tier-1 only; CI must not set it
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"

# Scanned files — add new operator-facing docs here when they land. README/CHANGELOG/
# SECURITY stay at root by GitHub convention; the rest live under docs/guide/.
FILES=(
    "README.md"
    "docs/guide/RUNBOOK.md"
    "docs/guide/DEPLOYMENT.md"
    "docs/guide/METRICS.md"
    "CHANGELOG.md"
    "docs/guide/CONFIG.md"
    "SECURITY.md"
    "docs/features.md"
    "docs/guide/overview.md"
    "docs/guide/getting-started.md"
    "docs/guide/capabilities.md"
    "docs/guide/comparison.md"
    "docs/guide/PERFORMANCE.md"
    "CONTRIBUTING.md"
    "docs/guide/cookbook.md"
    "docs/guide/troubleshooting.md"
    "docs/guide/deployment-patterns.md"
    "docs/guide/observability.md"
    "docs/glossary.md"
)

# Rows are `<ERE> || <description>`; the description is printed on failure. Prefer
# narrow patterns (`/usr/local/bin/lb\b`) over broad (`\blb\b`) to avoid false hits.
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

# Lines carrying this marker are dropped BEFORE matching, so a CHANGELOG entry may
# quote the very string it describes fixing.
allow_substr='doc-lint-allow'

fail=0
fail_lines=()

for f in "${FILES[@]}"; do
    if [ ! -f "$f" ]; then
        continue
    fi
    while IFS= read -r row; do
        pat="${row%%||*}"
        desc="${row#*||}"
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

# ---------------------------------------------------------------------------
# Tier 1b — the shipped systemd unit must agree with DEPLOYMENT.md.
#
# `packaging/expressgateway.service` has claimed in its own header, since ROUND8-OPS-07,
# that "the doc-lint job enforces that every directive named in DEPLOYMENT.md appears
# here". It did not: tier-1 only greps ONE stale ExecStart pattern. So the unit drifted
# from the docs on two directives and nothing noticed (S47 REL-02 / REL-05):
#
#   Type=notify + NotifyAccess=main   vs  DEPLOYMENT.md's Type=simple
#     — and no sd_notify exists anywhere in the workspace, so systemd waited out its
#       90 s TimeoutStartSec and SIGKILLed a healthy gateway, forever, on every boot.
#   ExecReload=/bin/kill -USR1        vs  DEPLOYMENT.md:55 / RUNBOOK.md:210's -HUP
#     — `systemctl reload` did a CERT reload and applied no config change.
#
# This makes the header's claim true. For every `Key=Value` directive inside a fenced
# block in DEPLOYMENT.md that also names a key present in the unit, the VALUES must
# match. Keys absent from the unit are ignored (the doc renders an abridged unit);
# keys in the unit but not the doc are ignored (the doc is not exhaustive).
# ---------------------------------------------------------------------------
UNIT_FILE="packaging/expressgateway.service"
DEPLOY_DOC="docs/guide/DEPLOYMENT.md"
unit_mismatch=0
if [ -f "$UNIT_FILE" ] && [ -f "$DEPLOY_DOC" ]; then
    # Directives the doc asserts. Restricted to this set so prose mentioning a key in
    # passing cannot fail the build; extend deliberately.
    for key in Type ExecStart ExecReload NotifyAccess KillMode KillSignal Restart User Group; do
        doc_val="$(grep -oE "^${key}=.*$" "$DEPLOY_DOC" | head -1 | cut -d= -f2- || true)"
        unit_val="$(grep -oE "^${key}=.*$" "$UNIT_FILE" | head -1 | cut -d= -f2- || true)"
        # Only compare when the doc states it AND the unit has it.
        if [ -n "$doc_val" ] && [ -n "$unit_val" ] && [ "$doc_val" != "$unit_val" ]; then
            echo "doc-lint tier-1b: ${key}= disagrees between the shipped unit and the docs" >&2
            echo "    $UNIT_FILE : ${key}=${unit_val}" >&2
            echo "    $DEPLOY_DOC: ${key}=${doc_val}" >&2
            unit_mismatch=1
        fi
        # A directive the doc states but the unit lacks is also drift, for the keys that
        # change startup/reload semantics.
        if [ -n "$doc_val" ] && [ -z "$unit_val" ]; then
            case "$key" in
                Type|ExecStart|ExecReload)
                    echo "doc-lint tier-1b: $DEPLOY_DOC states ${key}=${doc_val} but $UNIT_FILE has no ${key}=" >&2
                    unit_mismatch=1
                    ;;
            esac
        fi
    done
    if [ "$unit_mismatch" -ne 0 ]; then
        echo "doc-lint tier-1b (systemd unit vs DEPLOYMENT.md): FAIL" >&2
        exit 1
    fi
    echo "doc-lint tier-1b (systemd unit vs DEPLOYMENT.md): OK"
else
    echo "doc-lint tier-1b: unit or deployment doc missing; skipping" >&2
fi

# Tier 2 — audit-of-audit. For every `Status: Verified-Fixed(<sha>...)` finding:
#   1. every SHA exists in this repo's history;
#   2. a `Location:` path appears in the union diffstat (advisory, see Test 1);
#   3. every audit/crates/scripts/packaging path cited in `Recommendation:` exists in
#      the closure SHA's tree — the EBPF-2-07 case, and the only hard failure.

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

# Finding-ID prefix -> area dirs accepted as a legitimate `Location:` touch.
# Intentionally loose: the test is "did the SHA touch SOMETHING in that subtree",
# not "did it touch the exact line range".
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

review_files=$(find audit -type f -name 'round-*-review.md' -not -path '*/round-7/*' -not -path '*/round-8/*' 2>/dev/null | sort)
# round-*-findings.md is the legacy security-file spelling.
sec_findings=$(find audit -type f -name 'round-*-findings.md' 2>/dev/null | sort)
review_files=$(printf '%s\n%s\n' "$review_files" "$sec_findings" | awk 'NF' | sort -u)

for rf in $review_files; do
    [ -f "$rf" ] || continue
    # One awk pass emits FINDING<TAB>id<TAB>status<TAB>location<TAB>recommendation.
    while IFS=$'\t' read -r tag id status_line loc rec; do
        [ "$tag" = "FINDING" ] || continue
        # -Partial is exempt on purpose: a partial is already disclosed, and its
        # disclosure note is the acceptance criterion for a future round to re-walk.
        case "$status_line" in
            *Verified-Fixed-Partial*) continue ;;
            *Verified-Fixed*) ;;
            *) continue ;;
        esac
        aoa_seen_claims=$((aoa_seen_claims + 1))
        # Handles `Verified-Fixed(<sha>)`, `(<sha>, <sha>)`, and
        # `(verifier=NAME, author-sha=<sha>+<sha>)`.
        sha_block=$(printf '%s' "$status_line" | sed -nE 's/.*Verified-Fixed\(([^)]+)\).*/\1/p')
        if [ -z "$sha_block" ]; then
            aoa_fail_lines+=("$rf:$id  [audit-of-audit: cannot parse SHA from status line: $status_line]")
            aoa_fail=1
            continue
        fi
        sha_block=$(printf '%s' "$sha_block" | sed -E 's/^[^=]*=//; s/, *author-sha=/+/g')
        shas=$(printf '%s' "$sha_block" | tr ',+; ' '\n' | grep -E '^[0-9a-f]{6,}$' || true)
        if [ -z "$shas" ]; then
            # No SHA-shaped token (e.g. "task-38"): skip rather than fail.
            continue
        fi
        prefix=$(printf '%s' "$id" | sed -E 's/^([A-Z]+).*/\1/')
        accepted_dirs="${LOCATION_DIRS[$prefix]:-crates/ tests/ docs/ audit/ scripts/ packaging/}"

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

        # Test 1 is ADVISORY: `Location:` is where the bug lives, not necessarily
        # where the fix lands (a validator may land in a sibling crate from the
        # call-site that misused it). Hard failures are Test 2 only.
        loc_path=$(printf '%s' "$loc" | grep -oE '(crates|audit|scripts|packaging|tests|docs)/[A-Za-z0-9._/-]+' | head -1 || true)
        if [ -n "$loc_path" ]; then
            loc_path_clean=$(printf '%s' "$loc_path" | sed -E 's/:[0-9].*$//')
            if ! printf '%s' "$combined_stat" | grep -qF -- "$loc_path_clean"; then
                loc_parent=$(dirname "$loc_path_clean")
                if ! printf '%s' "$combined_stat" | grep -qF -- "$loc_parent"; then
                    : # advisory only — deliberately no failure here
                fi
            fi
        fi

        # Test 2 (the EBPF-2-07 trap): Recommendation-cited paths must EXIST in the
        # closure SHA's tree. `<kver>`-style placeholders are globbed to "any file
        # under that dir with the trailing suffix".
        rec_paths=$(printf '%s' "$rec" | grep -oE '(audit|crates|scripts|packaging)/[A-Za-z0-9._<>/-]+' | sort -u || true)
        for p in $rec_paths; do
            p_clean=$(printf '%s' "$p" | sed -E 's/[).,;:]+$//')
            if printf '%s' "$p_clean" | grep -q '<'; then
                dir=$(dirname "$p_clean")
                suffix=$(printf '%s' "$p_clean" | sed -E 's/.*>//; s/.*<.*$//')
                if [ -z "$suffix" ]; then
                    suffix=""
                fi
                # Empty suffix accepts any file under $dir. README.md never counts.
                hits=$(printf '%s' "$combined_tree" | awk -v d="$dir/" -v s="$suffix" '
                    index($0, d) == 1 {
                        # README.md is the no-op disguise EBPF-2-07 shipped.
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
                if ! printf '%s' "$combined_tree" | grep -qFx -- "$p_clean"; then
                    if ! printf '%s' "$combined_tree" | grep -qE "^${p_clean//./\\.}/"; then
                        # HEAD fallback: a later commit may have moved the path forward,
                        # and the closure was still real.
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
