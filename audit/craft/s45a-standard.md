# S45A — the SLOP vs LOAD-BEARING standard

The single line every scanner and sweeper applies. Written by lead after a calibration pass on
the real tree (see "Calibration" below — the naive signatures produce mostly false positives here).

## The test

A comment is **SLOP** only if removing it loses **zero information**: a reader of the adjacent code
at normal competence learns nothing from it that the code does not already say.

A comment is **LOAD-BEARING** if removing it could let a future editor reintroduce a defect, violate
a spec, break a gate, or re-litigate a settled decision.

When the two readings compete, the comment is **AMBIGUOUS** and it is **KEPT**. A retained mediocre
comment costs nothing. A deleted invariant note costs a future regression.

## SLOP (remove)

- Restates the adjacent statement: `// increment the counter` above `count += 1;`.
- Doc-comment that only re-spells the signature: `/// Returns the name.` on `fn name(&self) -> &str`.
- Narrative/changelog/session commentary that is not attached to a technical reason:
  `// Fixed in S37`, `// As requested, now handles X`, `// Updated per review`.
- Sycophantic or explanatory filler aimed at a reader being taught to program.
- Commented-out **code** (real statements, not wrapped prose).
- Banner/section comments repeated verbatim across files that carry no local meaning.
- Dead scaffolding: a script or file with no caller and no provenance value.

## LOAD-BEARING (KEEP — removal is an R3 knowledge regression)

1. **WHY-notes that prevent a regression.** Canonical: the `get_mut` (not `entry().or_insert_with()`)
   note on the H3 egress path — that comment is what stops F-S29-1 being reintroduced. Any comment
   of the form "use X not Y because Y does Z" is sacred.
2. **RFC / spec citations** justifying conformance behavior; the h3spec 12-waiver rationales.
3. **`#[allow(...)]` justifications.** Never orphan an allow from its reason.
4. **`unsafe` safety comments — MANDATORY.** Never removed, never shortened. Non-negotiable.
5. **Panic-freedom / invariant / bound notes** (e.g. why an index cannot go out of range).
6. **"deferred because X / see CF-…"** notes mapping to carry-forward items.
7. **Negative-control test comments** encoding *why a test proves what it proves*. Delete it and a
   later "simplification" turns the test vacuous. Includes "this must FAIL if …", "non-vacuous
   because …", "proves PROPAGATION not just a drop".
8. **Anything a CI gate or script reads**: `doc-lint.sh` FILES arrays, waiver lists, coverage
   config, hot-path patterns in `coverage-check.sh`.
9. **`audit/**` — entirely out of scope.** Do not touch, do not propose.

## Calibration — the naive signatures LIE in this repo (measured, lead, Phase 0)

Do not trust grep. Every candidate is read in context before it is classified.

| Naive signature | Hits | Reality |
|---|---:|---|
| `//.*\bS[0-9]+\b` "session marker" | 591 | Overwhelmingly **load-bearing**. These are finding-IDs (`F-S20-1`, `CF-S27-2`, `F-S29-1`) attached to regression rationale. A bare `(S34)` provenance tag on a *substantive* note is not slop — the note is the payload. |
| `//\s*(let\|fn\|if\|for\|while\|return)` "commented-out code" | 51 | Mostly **wrapped prose** — second lines of a sentence that happen to begin with `for`/`while`/`return`. e.g. `// for the lifetime of the connection.` |
| `just\|simply\|note that` "filler" | many | Ordinary English inside substantive technical notes. |

Consequence: this codebase's comment density (26% of lines) is largely an evidence trail, not slop.
The expected true-slop yield is **low**, and a large deletion count would itself be the alarm.

## The `deny(missing_docs)` rule — retires the biggest expected category

**All 18 crates carry `#![deny(missing_docs)]`**, and CI runs
`clippy --all-targets --all-features -D warnings`.

Therefore a `///` or `//!` doc-comment on a **public item is MANDATORY**. Removing one fails the
lint and turns the gate RED. Doc-comments on `pub` items are **gate-load-bearing** (rule 8 above),
even when they only restate the signature — `/// Whether the client is currently connected.` on
`is_connected()` cannot be deleted.

This retires the mission's anticipated "redundant doc-comments that duplicate the signature"
category almost entirely. What remains removable:

- ordinary `//` comments *inside function bodies* that restate the adjacent statement;
- genuinely commented-out **code** (real statements, not wrapped prose);
- pure narrative/changelog comments with no technical payload;
- doc-comments on **private** items (`missing_docs` does not fire there) that add nothing.

## Provenance tags

A trailing `(S34)` / `(S44)` on an otherwise substantive comment is **provenance**, and provenance is
cheap and occasionally useful. Do **not** strip tags as a mechanical sweep — that is churn, it
touches every file, and it buys nothing. Only remove a session reference when the comment is
*nothing but* the reference.

## Behavior

NO behavior changes. Comments, dead files, and dead scripts only. Any code edit must be provably
behavior-neutral. Coverage must not move; if it does, something behavioral changed.
