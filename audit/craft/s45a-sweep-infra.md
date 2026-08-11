# sweeper-infra — execution report (S45A)

Scope: `scripts/`, root files, `.github/`. NOT `crates/**`, NOT `audit/**`.
Branch `feature/de-slop-s45a`. Three commits: `cd2fd953`, `ccdb5aa2`, `aa866336`.

## TASK 1 — 20 dead root scripts DELETED (`cd2fd953`)

Re-verified before deleting, two independent passes over **all tracked files**:

1. anchored `(^|[^A-Za-z0-9/_-])<base>\.sh` — 0 hits for all 20 (the only two
   matches were this session's own `audit/craft/s45a-slop-inventory.md` prose).
2. unanchored `git grep -F` — 1 raw hit total, `run-resoak.sh` matching the
   substring `audit/soak/s33-run-resoak.sh`, a **different, untracked** file.
   Exactly the false positive the Phase-0 inventory predicted.
3. a targeted grep restricted to live-caller locations (`.github`, `scripts`,
   `docs`, README/CONTRIBUTING/SECURITY/CHANGELOG, `Cargo.toml`, `docker`,
   `packaging`, `manifest`, `config`, `bench`, `fuzz`, `tests`) — **no hits**.

**No script turned out to have a live caller.** Inertness re-confirmed on disk:
`.claude/worktrees/` empty, `git worktree list` shows only the primary checkout,
`/home/ubuntu/Code/eg-target` absent.

Deleted (557 lines): `run-baseline`, `run-ci-watch`, `run-cov-full`, `run-cov`,
`run-d-build`, `run-d-clippy`, `run-d-gate`, `run-d-reqwest`, `run-d-tokio-revert`,
`run-d6rerun-watch`, `run-fcap-isolate`, `run-lint`, `run-main-ci-watch`,
`run-p4-x3`, `run-p4-x3f`, `run-resoak`, `run-watchdog`, `run-x3-takeover`,
`run-x3`, `run-x3c-takeover`.

## TASK 2 — 8 provenance scripts ARCHIVED (`ccdb5aa2`)

`git mv` into `scripts/archive/` (all 8 detected by git as pure renames).
`scripts/perf/` is now empty and gone; `scripts/soak/` retains its 3 live files.

### Path references FIXED (6)

| file | was | now |
|---|---|---|
| `s39-burnin.sh:10` | `# Usage: scripts/perf/s39-burnin.sh` | `scripts/archive/…` |
| `s39-sweep.sh:11` | `# Usage: scripts/perf/s39-sweep.sh` | `scripts/archive/…` |
| `s39-oha.sh:9` | `# Usage: scripts/perf/s39-oha.sh` | `scripts/archive/…` |
| `s20-run.sh:14` | `# Usage: scripts/soak/s20-run.sh` | `scripts/archive/…` |
| `s21-run.sh:13` | `# Usage: scripts/soak/s21-run.sh` | `scripts/archive/…` |
| `s39-gate-feasible.sh:17` | ``run the full `s39-x3.sh` instead`` | ``…`scripts/archive/s39-x3.sh` instead`` |

The `s39-gate-feasible.sh` → `s39-x3.sh` reference was by **bare basename**, not
by path, so moving both together kept it resolvable; it is now explicit anyway.

### Outbound reference checked and left correct

`s39-burnin.sh:27` calls `scripts/soak/run-soak.sh`, which is **not** moving. The
call is repo-root-relative and the script `cd`s to the repo root on line 13, so it
remains valid. Target confirmed present.

`bash -n` clean on all 8. `s39-x3.sh` and `s21-gate.sh` needed no edit (0 diff).

### Stale references found and LEFT (audit/** — report-only, per brief)

- `audit/soak/s21-handoff.md:8` → `scripts/soak/s20-run.sh`
- `audit/soak/s21-report.md:157` → `scripts/soak/s21-run.sh`
- `audit/soak/s21-report.md:230` → `scripts/soak/s21-gate.sh`

Same class as the pre-existing `audit/deps/s31-report.md` citations that S40
already accepted going stale for `s26-*`/`s31-*`. **Zero** stale references in
`docs/**`, `.github/**`, `scripts/**`, README/CONTRIBUTING, Cargo, Docker,
packaging, manifest or config. None is doc-lint tier-2 HEAD-pinned (confirmed by
running the gate after the move).

## TASK 3 — comment standard applied (`aa866336`)

| area | lines | comment lines | reduction |
|---|---|---|---|
| `scripts/` (excl. `archive/`) | 2548 → 2267 | **755 → 474** | **-37.2%** |
| `.github/**/*.yml` | 783 → 705 | **166 → 88** | **-47.0%** |
| combined | — | **921 → 562** | **-39.0%** |

### Why this is far below the 90% headline

This population is almost entirely **catches**, not prose. The crates' 90% ceiling
comes from compressing doc-comment essays; scripts/ and .github/ have none. What
survives here is gate rationale, waiver justification and why-a-thing-is-pinned —
clause 2 of the standard protects all of it. What was actually deleted was
narrative and decoration: the five `SECTION N —` banner boxes in `ci.yml`, the
ASCII rules in `release.yml` and `docker-smoke.sh`, restated step-by-step
preambles, and GitHub's boilerplate `dependabot.yml` header.

### NO BEHAVIOR CHANGES — verified mechanically, not by eye

- **12 scripts**: with comment lines stripped, each file is byte-identical to
  HEAD. `doc-lint.sh` needed trailing-comment stripping too, because of one
  `: # …` line where I replaced a commented-out debug `echo`; its executable
  token is `:` before and after. All 12 pass `bash -n`.
- **4 workflows**: parsed with PyYAML and compared leaf-by-leaf. Identical
  165-leaf key set; 5 leaves differ, all `run:` strings, and all identical once
  shell comments are stripped. **0 command changes.**

### Gate content preserved (the three ci/ scripts)

**`coverage-check.sh`** (73 → 45) — `REQUIRED` patterns and the `EXEMPT` regex
untouched. Kept: the S44 merged-DA metric rationale including the dual-instantiation
evidence and the "CORRECTION not relaxation, it moves numbers BOTH ways" defense;
the named `lb-l4-xdp/src/loader.rs` carve-out with its root/D2 justification; the
fail-closed rule; and the `ALWAYS assign` DA trap note (`0 > 0 is False`) verbatim
in substance.

**`doc-lint.sh`** (122 → 45) — `FILES` array still **19 entries**, `STALE_PATTERNS`
still **9 rows**, `doc-lint-allow` marker and `LOCATION_DIRS` intact. Every
per-pattern regression mapping is in the array's `|| description` field, which is
code and was not touched. Kept as catches: the EBPF-2-07 origin story (compressed),
the `-Partial` exemption reason, the three `Verified-Fixed(...)` SHA shapes, Test 1
being advisory-only, the README.md no-op-disguise exclusion, and the HEAD fallback.
**Gate re-run after editing: exit 0, tier-1 OK, tier-2 OK, 52 claims checked.**

**`h3spec-check.sh`** (36 → 29) — all **12** named CF-QUICHE-UPGRADE waiver strings
byte-identical, `MIN_EXAMPLES=40` unchanged, both group rationales kept as one line
each, honesty contract kept.

### Deliberate scope decision — `scripts/archive/` NOT swept

15 files, 733 lines, **142 comment lines**, left frozen. An archive's comments *are*
the provenance it exists to hold — `s39-gate-feasible.sh` is archived precisely
*because* of its CF-DISK-1 rationale. The S40-archived `s26-*`/`s31-*` files were
left frozen on the same reasoning, and Phase-0 DUP-7 rules the same way. Flagging
so the lead can override: sweeping them would add roughly 100 lines to the total.

### `scripts/halting-gate.sh` left as-is (10 comment lines)

Eight are one-line `# Check N — …` labels. The **numbers** are cited externally
(`docs/architecture.md:246`, `docs/arch/extending.md`, ADR-0001/0003/0010), so the
labels carry information the code does not. The other two are the awk
`#[cfg(test)]`-skip note and the panic-grep rule — both catches.

### One factual correction made

`h3spec-check.sh` line 5 said h3spec was "pinned to the version in
prod-readiness-gates.yml" — a workflow **deleted at S40** (Phase-0 AMB-10). Since I
was rewriting that header anyway, leaving a provably-false pointer would have been
worse than compressing it; it now reads "version pinned in ci.yml" without a line
number. Behavior-neutral: the pin is a workflow env var, not read from this script.

**Left alone, deliberately:** the past-tense S40 provenance in `ci.yml` and
`scheduled.yml:8` ("merged here", "renamed from") is correct and load-bearing.
`.github/actions/rust-setup/action.yml` also names the deleted workflows, but in its
`description:` **value**, not a comment — editing it changes YAML content, so it
stays an owner decision (AMB-10 unchanged).

## Final gate state

- `scripts/ci/doc-lint.sh` → **exit 0** (tier-1 OK, tier-2 OK, 52 claims). Run at
  baseline, after the archive moves, and after the comment sweep.
- halting-gate manifest hash still `8b4ef334…`, matching `.halting-gate.sha256` —
  neither manifest was touched.
- No cargo was run (2-core box under load), per brief.
- Phase-0 AMB-1 … AMB-10 are all untouched and still open for the owner.
