# S45A — the comment standard (OWNER-REVISED, supersedes the Phase-0 standard)

**Owner directive:** *"I want 90% comment reduction — comment should only be present if reading the
code doesn't make sense or there is a catch."*

This replaces the conservative Phase-0 standard. The bar is now aggressive by default.

## THE RULE

A comment survives ONLY if one of these is true:

1. **The code does not make sense without it.** A reader of normal competence cannot tell what is
   going on from the code alone.
2. **There is a catch.** A non-obvious hazard, invariant, ordering constraint, spec requirement,
   safety condition, or "if you change this, X breaks".

Everything else goes. In particular, DELETE:

- any comment restating what the adjacent code plainly does;
- descriptive prose explaining a function that reads clearly;
- narrative, changelog, session commentary, review chatter, status blocks;
- historical rationale for how the code got here, when the current code is self-evident;
- long worked examples, ASCII tables, and essays where one line does the job;
- doc-comment bodies that elaborate on an obvious signature.

**Default to deleting.** Under the previous standard ambiguity meant KEEP; now ambiguity means
CUT unless it is a genuine catch.

## Compression, not just deletion — the main lever

65% of all comment lines are doc comments (18,552 of 28,491), averaging 5.0 lines per block, and
656 blocks run 8+ lines (up to 95). Most of that is prose elaboration.

**The dominant move is COMPRESS A BLOCK TO ONE LINE**, not delete it. A 30-line doc essay becomes a
single-line summary. This is where the reduction comes from.

## HARD CONSTRAINTS — these are gates, not taste. Violating them turns CI RED.

1. **`#![deny(missing_docs)]` on all 18 crates.** Every `pub` item MUST retain **at least one**
   doc line. Compress to one line — never delete to zero. (Private items have no such floor.)
2. **Comments asserted on by tests.** These exact strings are read out of production source by
   integration tests and must survive verbatim:
   - `ROUND8-L7-10 — take-and-discard upstream stream pattern` (lb-l7/src/h1_proxy.rs)
   - `ROUND8-L7-10 — API contract for future H1 upstream reuse` (lb-io/src/pool.rs)
   - the `enable_connect_protocol()` / `if self.h2_extended_connect_enabled` pins (lb-l7/src/h2_proxy.rs)
   - `round8_underscore_policy.rs` pins text in h1_proxy.rs and h2_proxy.rs
   Before touching h1_proxy.rs, h2_proxy.rs, pool.rs: read the asserting test first.
3. **`unsafe` SAFETY comments stay.** A safety condition is the definition of a catch.
4. **`#[allow(...)]` justifications stay** (one line is fine). An unexplained allow is a catch lost.
5. **Anything a CI script reads** (doc-lint FILES arrays, coverage hot-path patterns, waiver lists).
6. **`audit/**` is out of scope entirely.**

## Catches that specifically survive (non-exhaustive)

These are the paid-for lessons; they are exactly what clause 2 of the rule protects. Compress the
prose hard, but the *fact* must remain:

- `lb-quic/src/conn_actor.rs:628` — `get_mut`, not `entry().or_insert_with()` (prevents F-S29-1
  gRPC-over-H3 trailer drop). 14 lines → may become 2, but the "not or_insert_with, because it
  replays the stale End and discards a buffered trailer" fact stays.
- Reset-vs-FIN arm-swap notes; `StreamReset` vs `StreamStopped`.
- Smuggling defenses (`check_te_strict`, TE-must-equal-trailers, pseudo-header leak rejects).
- Zero-RTT LRU-not-FIFO; HMAC-not-multiply-shift.
- `biased;` select ordering; pinning contracts; free-list invariants.
- h3spec waiver rationales; RFC citations that justify a behavior a reader would otherwise
  "fix" — keep the citation, drop the recitation.
- Negative-control test intent ("this must FAIL pre-fix") — one line, not a paragraph.

## Behavior

NO behavior changes. Comments only, plus the already-approved dead-script removals. Coverage must
not move. The invariant census (`s45a-invariant-census.sh`) checks that the catch CLASSES survive;
its counts may legitimately fall as prose is compressed, so it is now a review aid, not a gate —
the named canaries still bind.

## Measured ceiling (why 90% needs one more decision)

| scenario | result |
|---|---|
| compress every doc block to 1 line + delete EVERY plain comment | 28,491 → 3,741 = **86.9%** |
| the same, but honoring clause 2 (catches survive) | ≈ **78%** |
| 90%+ | requires dropping `#![deny(missing_docs)]` from all 18 crates |

86.9% is the hard lint-safe ceiling. We drive to the maximum the rule allows and report the real
number rather than hitting a target by deleting catches.
