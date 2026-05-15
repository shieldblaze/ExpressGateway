### ROUND8-OPS-09 — `doc-lint.sh` catches stale binary names but not stale capability claims, drain values, or aspirational features

Reference: `audit/round-8/research/haproxy.md` lesson 20 ("hot reload without stats continuity blinds your SRE team"); the broader "treat docs as code" pattern across Pingora / Envoy / nginx (each reference's CHANGELOG describes the runtime *behaviour* and CI gates assert the docs match).
Our equivalent: `scripts/ci/doc-lint.sh:34-41` — six patterns, all narrowly scoped to the REL-2-14 binary-name issue. The doc-lint gate has ZERO patterns for:
- The aspirational FD-passing claim (ROUND8-OPS-01).
- Stale drain budget values (`30-second` in `RUNBOOK.md` was the original REL-2-02 lie; the current correct value is `10 000 ms`).
- Stale capability claims (`CAP_SYS_ADMIN` references in DEPLOYMENT.md outside the explicit pre-5.8 fallback table).
- The "JSON to stdout" promise vs the current behaviour (REL-2-06 lies were *correct* docs but the gate would not catch *new* lies).

Severity: medium
Status:   Verified-Fixed(verifier=verify, 61e678a5)   <!-- doc-lint.sh Tier-2 audit-of-audit walker PROVEN: two /tmp injections both rejected exit 1 — (A) real SHA d46d0f48 not adding rec-cited verifier-logs path -> 'did not add a matching non-README file'; (B) nonexistent SHA -> 'SHA(s) not in repo'. Control passes (52 claims). Plan deviation: Tier-1 Rust-const-assertion binary descoped to STALE_PATTERN additions; audit-of-audit centerpiece shipped + works. See audit/round-8/verify/ops.md. -->
Status (pre-verify):   Open

Divergence:
- The doc-lint script is a stub built around the single class of bug REL-2-14 exposed. The REL-2-01 finding called out *six* drift items; only one (binary name) became a CI gate.
- A reviewer following the round-8 stance walking the doc patterns would find:
  - `README.md:23` "FD passing" — would not trigger anything.
  - `RUNBOOK.md:70-72` "TimeoutStopSec=30 — should exceed drain_timeout_ms" — the documented number stayed in sync, but no gate enforces it (DEPLOYMENT.md says 30, CONFIG/main says 10 000 + 1 000 + 2 000 = 13 s, both true but unenforced).
  - `DEPLOYMENT.md:97` "CAP_SYS_ADMIN" — inside the documented pre-5.8 fallback table, fine; if someone accidentally adds CAP_SYS_ADMIN to the *primary* recommendation outside the table, no gate catches it.

Impact:
- The doc-lint gate is a "ratchet not a guard": it catches future regressions of *the exact bug it was written for* and nothing else. Round-8 stance is that this is exactly the kind of audit-self-grading we should not trust.
- Each new doc claim that mentions an unimplemented capability (FD passing today, future ACME integration, future xDS) needs a new pattern. There is no enforcement that authors add the pattern.

Recommendation:
1. Extend `STALE_PATTERNS` in `scripts/ci/doc-lint.sh` with:
   ```
   'FD.?passing||README claim that does not match source (ROUND8-OPS-01)'
   'TimeoutStopSec=30([^0-9]|$)||drain budget should be derived from CONFIG.md not hard-coded'
   'CAP_SYS_ADMIN||CAP_SYS_ADMIN reference outside the documented pre-5.8 fallback table'
   'ArcSwap<TlsStore>||legacy doc reference to deleted type (REL-2-01)'
   'SIGHUP.*ConfigManager||SIGHUP-reload claim that does not match source (REL-2-05 still partial)'
   ```
   Each with a `doc-lint-allow-<short-tag>` opt-out marker for legitimate references (e.g. the pre-5.8 fallback table can keep CAP_SYS_ADMIN behind the marker).
2. Add a positive-assertion test: every claim of the form "default X is N ms" in `RUNBOOK.md` must match the corresponding `const fn default_*_ms` in `crates/lb-config/src/lib.rs`. Easier-to-write version: a Rust integration test in `tests/runbook_defaults.rs` that imports the default constants and asserts their values appear *verbatim* in `RUNBOOK.md`.
3. Replace the bash script with a Rust binary (`cargo doc-lint`) that imports the actual default consts at compile time — drift becomes a compile error, not a CI shell-grep miss.
4. Cross-reference: the `audit/protocol/round-5-verifies-rel.md` verification process found that REL-2-08 status note (canonical labels table) drifted from the actual emit-site labels. That drift would have been caught by a "the CANONICAL_LABELS table in code matches the table in METRICS.md" assertion, which doc-lint does not perform.

Notes:
- Round-8 finding is broader than the bash script — the *gate philosophy* is too narrow. Production references treat operator docs as a contract that ops must be able to compile-check.
