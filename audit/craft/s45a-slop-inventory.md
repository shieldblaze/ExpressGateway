# S45A — classified slop inventory + proposed deletion set

Base: `main @ ff39fa08`. Branch: `feature/de-slop-s45a`. Standard: [s45a-standard.md](s45a-standard.md).
Per-area detail: [lb-quic](s45a-inv-lb-quic.md) · [lb-l7 group](s45a-inv-lb-l7.md) ·
[lb/xdp/soak](s45a-inv-core.md) · [support crates](s45a-inv-support.md) · [infra](s45a-inv-infra.md).

Five scanners read **508 files** (283 code + 225 infra), every comment-bearing region, read-only.
Author≠verifier: the lead independently spot-verified the highest-impact claims (below).

## Headline — the mission's premise is substantially refuted by measurement

The brief expected "~44 agent-driven sessions left the repo full of AI slop". Measured:

| | count |
|---|---:|
| comment lines in `crates/**` | 28,494 |
| **true slop comments found** | **7** |
| load-bearing notables catalogued | 126 |
| stale-but-load-bearing (flagged, KEEP) | 89 |
| commented-out code blocks found | **0** |
| `TODO`/`FIXME`/`HACK` in production source | **1** (a tracked deferral, `TODO(L4-06)`) |

**0.02% of comment lines are slop.** All 7 are one class — *stale factual claims*, true when
written and false now — not filler, not narrative padding. This codebase does not have an AI-slop
comment problem.

Three independent reasons the naive signatures mislead here, each measured:

1. **The 591 "session marker" comments are finding-IDs** (`F-S20-1`, `CF-S27-2`, `F-S29-1`) carrying
   regression rationale. The canonical example — the note that prevents F-S29-1 — literally opens
   with `SESSION 29` (`crates/lb-quic/src/conn_actor.rs:628`). A pattern sweep deletes exactly the
   comment the brief calls sacred.
2. **All 18 crates `#![deny(missing_docs)]`** and CI runs `clippy -D warnings`. A doc-comment on a
   `pub` item cannot be removed at all. This retires the single biggest anticipated category.
3. **Some comments are literally tested.** `crates/lb-l7/tests/round8_body_overread.rs:88` asserts
   `src.contains("ROUND8-L7-10 — take-and-discard upstream stream pattern")` against
   `h1_proxy.rs`, failing with *"Restore the doc-block before removing the assertion."* A second
   asserts a doc-block in `lb-io/src/pool.rs`; `h2_connect_protocol_settings.rs` and
   `round8_underscore_policy.rs` pin source text the same way. **The repo already built a CI gate
   against precisely the mistake this session risks.**

## PROPOSED DELETION SET

### A. Code comments — 7 items, all "stale factual claim" (PRE-AUTHORIZED class)

| # | location | why it is slop |
|---|---|---|
| 1 | `lb-quic/tests/h3_h1_resp_stream_e2e.rs:31-40` | "SCAFFOLD STATUS … `#[ignore]`d" — refuted in the same file at :1118-1126; verified zero real `#[ignore]` attributes (all 4 grep hits are mentions inside these two comments). Delete the false block only; the accurate status at :1115-1136 stays. |
| 2 | `lb-l7/src/grpc_proxy.rs:611` | "Make the `IncomingBody` type alias usable in tests" — `IncomingBody` is an import rename, not an alias; `mod tests { use super::*; }` needs no note. |
| 3 | `lb-l4-xdp/tests/elf_sections.rs:11-14` | "until `build-xdp.sh` is re-run … sections absent" — `readelf -S` confirms license/.BTF/.BTF.ext all present in the committed ELF. |
| 4 | `lb-l4-xdp/tests/round8_conntrack_state.rs:16-17` | "NUM_SLOTS bumps with this commit from 13 to 15" — changelog prose, and wrong: NUM_SLOTS is 16. |
| 5 | `lb-soak/src/loadgen.rs:2839` | "`u16::MAX` sentinel = do not respond" — describes a sentinel that does not exist. |
| 6 | `lb-observability/src/xdp_metrics.rs:217-226` | `_gauge_vec_anchor()` — empty private fn existing only "so reviewers grep and find it". Zero callers. Remove fn + doc together so the `#[allow(dead_code)]` is not orphaned. |
| 7 | `lb-security/tests/tls_versions.rs:13` | **clause-scoped**: drop "Sec was OK with the single-line touch in `ticket.rs`;" (inter-agent review narrative). The rest of the sentence is load-bearing API-compat rationale and stays. Skip entirely if clause-scoped editing is not clean. |

None touches a coverage-gated file. Items 6 and 7 are the only ones with any code/prose surgery.

### B. Dead scaffolding — 20 root `run-*.sh`, proven dead (PRE-AUTHORIZED class)

All 20 tracked root scripts, ~538 lines. Every one has **zero callers** (anchored `git grep` across
workflows, scripts, docs, Cargo/Docker/packaging). Deadness proven on disk this session:

- `.claude/worktrees/` **is empty** and `git worktree list` shows only the primary checkout — 14 of
  the 20 `cd` into `.claude/worktrees/{s36-verify,s37-deps,s37-verify}` guarded by `|| exit 99`,
  so they abort before doing anything.
- `/home/ubuntu/Code/eg-target` — the `CARGO_TARGET_DIR` all of them export — **does not exist**.
- The three `gh` watchers hardcode completed one-off IDs (PR #228, runs 27145437614 / 27145437741).
- `run-watchdog.sh` watches S37 branches in the same dead worktree.

They landed across five S37 commits (2026-06-07/08) described as "lead harness scripts".
*Correction to my own Phase-0 finding:* `run-resoak.sh`'s single apparent reference was an
unanchored substring match on a different path (`audit/soak/s33-run-resoak.sh`); anchored, **all 20
have zero references.**

### C. Archive, not delete — 8 provenance scripts (autonomous per brief, S40 precedent)

`scripts/perf/{s39-burnin,s39-sweep,s39-oha,s39-x3,s39-gate-feasible}.sh` and
`scripts/soak/{s20-run,s21-run,s21-gate}.sh` → `scripts/archive/`. Each documents how a *published
number* was produced (s39-burnin backs `audit/perf/s39-burnin.md`, cited by
`docs/arch/backpressure.md`; s39-oha is the author≠verifier cross-check behind the perf claims).
Move `s39-x3.sh` and `s39-gate-feasible.sh` together — the latter references the former by path.

## AMBIGUOUS — 89 items, ALL KEPT (this is the session's real finding)

The scanners flagged **89 comments that are load-bearing in intent but now misstate the code**.
They are kept verbatim. The repo's comment defect is not slop, it is a **false map**: prose that a
future editor would reasonably trust and that is no longer true. Deleting them is an R3 knowledge
regression; leaving them uncorrected keeps the false map. A correction pass is a *different*
session with a different risk profile — flagged, not attempted here.

## FINDINGS surfaced by the sweep (exit condition (c)) — comments KEPT

1. **EWMA is unfed — confirmed with evidence.** `lb-balancer/src/lib.rs:72-75` asserts EWMA "is
   updated on response completion in lb-l7". `set_latency_ns` / `latency_ewma_ns =` have **zero
   writers** outside `lb-balancer`/`lb-core`, so `Ewma::pick` always sees `0` and every backend
   scores equally. Corroborates the S41 "EWMA-unfed" note; the limitation is documented nowhere.
2. **`lb-cp-client` is STUB-DEAD.** 119 LOC, not linked into the binary (absent from
   `crates/lb/Cargo.toml`), zero references tree-wide outside itself. `connect()` performs no I/O —
   it sets a bool. Its module doc claims it exchanges configuration with the control plane.
   Compiled and linted by CI; reachable by nothing. **Owner decision — no deletion proposed.**
3. **`lb-health` is STUB-BUT-WIRED.** Constructed per-backend in `main.rs:2564`, bound to
   `_health_seed` to stay in scope, never driven: `record_success`/`record_failure` have zero
   callers outside the crate's own tests, so every checker is permanently `Unknown`. The binary
   says so honestly at `main.rs:2553-2559` — a prior round's UNUSED finding was answered with a
   construction site rather than a caller.
4. **Two vacuous tests.** `lb-io/src/quic_pool.rs` `per_peer_max_enforced` and `total_max_enforced`
   do not exercise the bounds they name (synthetic conns are never `is_established`, so `Drop`
   never re-parks them). A 40-line comment says so honestly. Fix the tests, not the comment.
5. **Self-contradicting security doc.** `lb-security/src/retry.rs:171-184` says `mint` panics via
   `assert!` on an over-long `odcid`; its `# Panics` section says it never panics; the code
   silently truncates to 255. One of the three is wrong on a security-relevant input path.
6. **Stale "no auth" claim.** `lb-observability/src/admin_http.rs:14-15` still tells readers the
   admin surface has no auth; SEC-2-06 added `serve_with_auth`/`AdminAuthGate` in that same file.
7. **Orphaned doc blocks.** Four private fns in `h1_proxy.rs`/`h2_proxy.rs` have their doc block
   fused onto the *wrong* function (rustdoc attaches it to the neighbour), leaving
   `build_body_with_trailers` / `build_h2_body_with_trailers` undocumented. Fix = move, not delete.

## Gate feasibility — a constraint the owner should rule on

This box is **2 cores / 7.8 GB RAM / 67 GB disk**. A cold
`cargo test --workspace --all-features --no-run` reached **13 GB of `target/` with 2.9 GB free**
and was stopped to avoid ENOSPC wedging the box (R9). **R1's local ×3 at full parallelism is not
feasible here.** The 16-job CI on GitHub runners does run the full suite, and main @ ff39fa08 has
just gone green on it (`test` passed; only `Release Build` outstanding at time of writing).

Proposed binding gate for a comments-only change, for owner approval:
full 16-job CI green on the branch + local `clippy --all-targets` + `fmt` + coverage unmoved +
the invariant census ([s45a-invariant-census.sh](s45a-invariant-census.sh)) + the named canaries.

## Coverage baseline (must not move)

Source: `scripts/ci/coverage-check.sh` merged-DA metric; 31 hot-path modules, all ≥80%, gate PASS
at `ff39fa08` (job "Coverage (per-module hot-path >= 80%)" = success). Per-module values are the
"new (DA)" column of `audit/ci/s44-coverage-metric-rebaseline.md`. A comments-only change must move
none of them.
