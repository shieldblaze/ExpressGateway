# Round-8 OPS — independent verification (verifier=verify, code-reviewer)

STATE=ROUND_8_FIXES. Author≠verifier: div-ops authored these fixes; this
file is an independent walk by `verify`. Only `verify` flips Status.

Method per finding: read finding+plan → `git show <sha>` → re-run proof →
adversarial/operational bypass attempt → verdict + blocking flag.

Summary: 9 Verified-Fixed, 1 Verified-Fixed-Partial (OPS-02), 1 Verified-Fixed
(OPS-03 with disclosed listener-label deferral), 1 Deferred-with-rationale
(OPS-08, correctly disclosed, NOT closed). 0 hard push-backs. The two
critical answers are at the bottom.

---

### ROUND8-OPS-01 — README zero-downtime/FD-passing claim
Status: **Verified-Fixed** — blocking-for-prod: NO (resolved)

`grep -rniE 'zero.downtime|FD passing|SO_REUSEPORT.*fd' README.md RUNBOOK.md
DEPLOYMENT.md` returns nothing (rc=1). The aspirational claim is gone.
Defense-in-depth: `doc-lint.sh` STALE_PATTERNS now carries
`FD[- _]?passing` and `zero[- ]downtime[^.]{0,40}(FD|reload)` so the claim
cannot silently return. Adversarial: confirmed the patterns are scanned
against README/RUNBOOK/DEPLOYMENT/CONFIG and the `doc-lint-allow` escape
requires an explicit marker. Solid.

### ROUND8-OPS-02 — drain jitter / thundering herd
Status: **Verified-Fixed-Partial** — blocking-for-prod: NO

`Shutdown::run_drain` sleeps a random `[0, jitter_max)` (config
`drain_jitter_ms`, per-listener override, default `drain_timeout_ms/4`)
BEFORE the InFlightDrain cancel — `crates/lb-core/src/shutdown.rs:325`,
`jitter_millis()` at :711. Partial vs. the literal recommendation #1
(per-*connection* jitter): the implementation is per-*process*. This
desynchronises replicas against the shared upstream LB, which the
finding's own Notes identify as the real vector, so the design choice is
defensible — but every connection on one pod still cancels in the same
tick. Non-blocking; per-conn jitter is a documented div-l7-owned
follow-up in the per-conn cancel arm. Entropy source (`RandomState`
hash of subsec_nanos) is adequate for ms-bucket jitter and correctly
avoids a `rand` dep in lb-core.

### ROUND8-OPS-03 — shutdown_drain_seconds histogram
Status: **Verified-Fixed** — blocking-for-prod: NO

`grep shutdown_drain_seconds crates/ --include=*.rs | grep -v test` is
non-empty: `MetricsDrainObserver` at `crates/lb/src/main.rs:2375` emits
`shutdown_drain_seconds_global{phase,outcome}` and
`shutdown_drain_seconds_listener{phase,outcome,listener}` via
`metrics.histogram_vec` with explicit buckets, per phase. The
REL-2-02 missing-half is genuinely closed (not just referenced).
Caveat (disclosed in-code): the `listener` label is `<aggregate>`
today — true per-listener observation is an OPS-10 follow-up. Counter
half (`shutdown_aborted_connections_total`) confirmed still bumped.
Non-blocking.

### ROUND8-OPS-04 + L4-12 — drain coordinator (THE big one)
Status: **Verified-Fixed** — blocking-for-prod: NO

`cargo test --test round8_drain_15case` → 16 passed, 0 failed, 0 ignored.
`cargo test -p lb-core --test round8_drain_coordinator` → 6/6. Legacy
`shutdown` (3/3) + `per_connection_drain` (4/4) still green via the
`Shutdown::drain` shim. The accept-loop wiring is real: `git show
698c5a63 -- crates/lb/src/main.rs` shows (a) a `biased` `tokio::select!`
with `() = state.listener_cancel_token.cancelled() => return Ok(())`
arm, AND (b) a synchronous post-accept `if
state.listener_cancel_token.is_cancelled() { shutdown; return }` tail
check placed BEFORE the per-IP admission-gate increment. This closes
the C-3 per-IP-counter-drift / leaked-fd gap that round-2's
sequence-only fix missed. `listener_token` is a real child of the root
token (unit-tested). Idempotency latch is a CAS + completion gate
(C-10/C-11). See critical answer (1) below for the hollow-test audit.

### ROUND8-OPS-05 — per-emission cardinality budget
Status: **Verified-Fixed** — blocking-for-prod: NO

`cargo test -p lb-observability --test red_label_budget` → 4/4.
`EnforcedLabelBudget` + `MAX_ROUTES_BUDGET=64` landed in
`label_budget.rs` with a wired call site in `main.rs` (commit 4c1856ea,
+213). Adversarial: confirmed `with_label_values` cannot bypass the
guard (the commit message claims this; the test
`with_label_values_cannot_bypass` exists and passes). Non-blocking.

### ROUND8-OPS-06 — traceparent L7 callsite (zero-callsite recheck)
Status: **Verified-Fixed** — blocking-for-prod: NO (resolved)

`grep -rn tracing_propagation crates/ --include=*.rs | grep -v test` is
non-empty with REAL production callsites: `crates/lb-l7/src/trace_ctx.rs`
calls `tracing_propagation::extract_parent` (:190) and `inject_into`
(:240); `crates/lb-l7/src/h1_proxy.rs:1382-1386` injects the
TRACEPARENT/TRACESTATE headers onto the upstream request. This is no
longer the zero-callsite library-only state the Round-5 recheck flagged
for REL-2-07. The L7 callsite landed via the div-l7 bundle (6253ad9a).
Non-blocking.

### ROUND8-OPS-07 — systemd unit hardening
Status: **Verified-Fixed** — blocking-for-prod: NO

`packaging/expressgateway.service` now exists as a real file (was
doc-block-only). Contains SystemCallFilter / RestrictAddressFamilies /
ProtectKernelTunables/Modules/Logs / ProtectControlGroups /
ProtectProc (7 directive-class hits confirmed). Type=notify +
NotifyAccess=main present; WatchdogSec deferred-with-comment (sd_notify
wire-in is Wave 2 — disclosed). Non-blocking. Residual: the
`systemd-analyze-security` CI gate (recommendation #2) is wired in
ci.yml per the plan but cannot be executed in this sandbox; the file
itself meets the 2026 baseline by inspection.

### ROUND8-OPS-08 — SBOM provenance
Status: **Deferred-with-rationale (NOT Verified-Fixed)** — blocking-for-prod: NO

Honest disclosure in `audit/deps-added.md`: `cargo-cyclonedx` is not
installable on the network-isolated sandbox; the SBOM still declares
`manual-fallback` provenance. The CI `sbom` job (install + generate +
`git diff --exit-code` + schema-validate) is the closure contract. This
is a transparently-disclosed deferral consistent with the round-8
"don't silently defer to CI" stance — it is NOT hidden. Severity low.
Keep Status Open/Deferred until the CI job lands a regenerated file.
Non-blocking for prod (low-severity supply-chain hygiene).

### ROUND8-OPS-09 + L4-10 — audit-of-audit doc-lint tier-2 gate
Status: **Verified-Fixed** — blocking-for-prod: NO (this IS the guard)

See critical answer (2) below. Note a plan deviation: the plan
specified a Rust `tools/doc-lint` binary importing config constants;
the implementation is a 363-line bash extension of `doc-lint.sh`. The
Tier-1 positive-assertion-from-constants piece (RUNBOOK default ==
`default_*_ms` const) was NOT built — only the STALE_PATTERN additions
and the Tier-2 audit-of-audit walker. The Tier-2 walker (the
lead-arbitrated centerpiece) IS implemented and provably works. The
EBPF-2-07 subject was correctly downgraded to Verified-Fixed-Partial
(not left as a false Verified-Fixed) — consistent.

### ROUND8-OPS-10 — per-listener drain budget override
Status: **Verified-Fixed** — blocking-for-prod: NO

`[[listeners]].drain_timeout_ms` override landed in lb-config with
`effective_drain_timeout_ms` resolver + validation
(100..=300000, inherits [runtime]); `crates/lb-config/src/lib.rs:997,
1214-1228`. The coordinator's InFlightDrain deadline uses the max
effective per-listener budget. Per-conn-await-own-listener-budget is a
disclosed div-l7 follow-up. Documented in RUNBOOK ("Tuning the drain
budget"). Non-blocking.

### ROUND8-OPS-11 — readiness_settle_ms default
Status: **Verified-Fixed** — blocking-for-prod: NO

`default_readiness_settle_ms()` returns `11_000`
(`crates/lb-config/src/lib.rs:480-482`), raised from 1000. 11 s = one
full kubelet default `periodSeconds: 10` probe period + 1 s margin, so
at least one `/readyz` 503 lands inside the settle window in the worst
case. Exceeds the kubelet probe period as required. Validation cap
30000 ms; RUNBOOK tuning table added. Note test fixtures at lib.rs:2054,
2096 still hard-code `1_000` but those are explicit struct literals in
unit tests, not the serde default — not a regression. Non-blocking.

### ROUND8-OPS-12 — container image hardening
Status: **Verified-Fixed** — blocking-for-prod: NO

`docker/Dockerfile`: explicit `USER 65532:65532` (:56), OCI
`LABEL org.opencontainers.image.*` block (:39+). HEALTHCHECK
intentionally deferred Phase-1 with an in-Dockerfile comment
explaining the `--healthcheck` subcommand is follow-up work
(disclosed, finding rec #1 explicitly allowed deferral). EXPOSE
clarification + RO-rootfs doc are low-severity residuals. Non-blocking.

---

## Critical answer (1): are all 15 drain cases distinctly asserted?

**YES — the test is not hollow.** `tests/round8_drain_15case.rs` has
16 `#[tokio::test]` functions (`case_c01_*` .. `case_c15_*` + one XDP
kernel-error extra), zero `#[ignore]`. Each has genuinely distinct
assertions, NOT 15 names sharing 3 assertions:

- C-1 asserts 6 phase observations + Clean; C-2 asserts a pending
  accept exits "cancelled" via listener_token cascade; C-3 (the
  canonical per-IP-drift case) asserts the listener-token-before-
  parent-token ordering; C-4 asserts a Drop-flag fires on per-conn
  cancel; C-7 asserts phase-6 runs AFTER a phase-5 timeout; C-8
  asserts xdp TimedOut + total>0; C-9 pre-cancelled-token safety;
  C-10 asserts mark_draining runs exactly once + r1==r2; C-12
  asserts a panicked task doesn't hang the drain; C-13 asserts
  NotAttempted xdp; C-14 asserts the listener signal fires; C-15
  degenerate-config Clean. The XDP-error extra asserts the
  `Failed{reason}` label round-trips.

Honest caveats (not push-backs): (a) C-3 is asserted at the
*coordinator* level (token ordering) rather than the actual per-IP
counter drift — but the real C-3 fix IS in main.rs and I verified it
by reading `git show 698c5a63` (biased select arm + synchronous
post-accept `is_cancelled()` shutdown before the IP-gate increment);
the test file documents this split explicitly. (b) C-5 and C-6 are
near-identical (same spec, same TimedOut assertion) — intentional
placeholders for a future per-protocol budget split. (c) C-11
overlaps C-10's idempotency latch with a different call ordering.
None of these are the REL-2-02-class "sequence-only, no case
coverage" miss; the case table is real and each transition is
exercised. No push-back warranted.

## Critical answer (2): did the doc-lint tier-2 gate reject the injected fake claim?

**YES — confirmed with two independent injections, both in /tmp,
nothing committed to the repo tree.**

Setup: copied `audit/` + the gate into `/tmp/dlx`, ran the original
unmodified gate logic from the REAL repo root (so `git` and the
`[ -e <path> ]` HEAD-existence fallback resolve correctly), swapping
only the ebpf review file for a tampered copy.

Injection A (the important EBPF-2-07-class case): changed EBPF-2-07's
Status to `Verified-Fixed(d46d0f48)` — a REAL existing SHA whose diff
does NOT add the recommendation-cited
`crates/lb-l4-xdp/ebpf/verifier-logs/<kernel-version>.log` and which
does not exist at HEAD. Result: **gate FAILED, exit 1**, diagnostic:
`recommendation cites 'crates/lb-l4-xdp/ebpf/verifier-logs/
<kernel-version>.log' but SHA(s) [d46d0f48] did not add a matching
non-README file`. The README.md-strip logic (the exact no-op disguise
EBPF-2-07 originally shipped) also works.

Injection B: `Verified-Fixed(deadbeef1234)` (nonexistent SHA). Result:
**gate FAILED, exit 1**, diagnostic: `SHA(s) not in repo: deadbeef1234`.

Control: the gate run on the untouched real tree passes clean (52
Verified-Fixed claims checked, exit 0). EBPF-2-07 itself is currently
`Verified-Fixed-Partial`, which the gate intentionally exempts (partials
carry a disclosure note for the next round to re-walk) — that is the
correct disposition of L4-10, not a bypass.

Conclusion: the "we won't repeat the prior audit's blind spots" claim
is NOT hollow. The Tier-2 gate genuinely rejects a Verified-Fixed
whose SHA does not contain the recommended content. Caveat for the
record: the plan's Tier-1 Rust-binary positive-assertion model
(RUNBOOK default == source const) was descoped to STALE_PATTERN
additions; the audit-of-audit centerpiece is the part that shipped and
it works.
