# Round-8 Phase E regression — NEW findings (Phase-D trigger gate)

A "new finding" here = a gate FAIL that is a **NEW medium+** defect
**introduced by the Round-8 tree** (would trigger a Phase-D loop).

## Verdict: NONE

No NEW medium+ regression was found. Two gates reported a sub-failure;
both were root-caused to **pre-existing test-harness/test-strategy
defects predating Round-8 Phase E**, with **no product-code regression**:

### Gate 04 (cargo test --workspace --release) — 1 test FAIL, non-blocking
- Failing test: `lb-integration-tests` `reload_zero_drop.rs ::
  drain_tests::test_sigterm_drains_h2_with_goaway`.
- Symptom: panic "gateway did not start accepting within 5s"
  (`tests/reload_zero_drop.rs:267`, fixed 5s deadline at line 256).
- Root cause: cold start of the release `expressgateway` binary
  (process spawn + listener bind + self-signed TLS load) exceeds the
  hard-coded 5s budget on this 2-vCPU sandbox. Reproduced parallel,
  serial (H1 also times out serially -> not a parallelism bug), and
  in full isolation. A successful `HTTP/1.1 200 OK` + GOAWAY exchange
  was observed in one run -> the gateway boots & serves correctly;
  only the 5s budget is hostile to a 2-vCPU runner.
- Provenance: introduced commit `1f7ab4bb` "REL-2-02 — multi-protocol
  drain integration test" (2026-05-14), **before** Round-8 Phase E.
- 398/399 workspace tests PASS.
- Classification: **pre-existing test-harness defect. NOT a new
  finding, no Phase-D loop. SUPERSEDED — see "REMEDIATED — task#80"
  below: root cause was the 0o664 TLS-key-perm rejection, not
  hardware timing; fixed test-file-only. No longer DEFERRED-ENV.**

### Gate 05 (PROPTEST_CASES=20000) — 1 sub-suite FAIL, non-blocking
- Failing sub-suite: `lb-h1` `proptest_parser.rs ::
  request_line_no_panic` (sibling `headers_no_panic` PASSES).
- Symptom: proptest aborts "Too many local rejects — successes: 299,
  local rejects: 65536 at 'printable ASCII'".
- Root cause: `proptest_parser.rs:45` builds the request-target as
  `vec(any::<u8>().prop_filter("printable ASCII", 0x20..0x7F), 0..256)`.
  The per-byte filter rejects ~25% of every sampled byte; with the
  20000-case budget the cumulative per-byte rejects exceed proptest's
  default `max_local_rejects` (65536) -> proptest ABORTS the property
  after 299 successful generations. NOT a parser counterexample (all
  299 generated requests satisfied the no-panic + bounded-consumption
  invariants); NOT a product/parser regression.
- The other 3 proptest suites PASS the full 20000-case contract:
  `lb-h2/proptest_hpack`, `lb-h3/proptest_qpack`,
  `lb-quic/proptest_header`.
- Provenance: introduced commit `560c1c25` "CODE-2-11 — proptest /
  loom / miri scaffolding (Wave 1)" (2026-05-13), **before** Round-8
  Phase E. The CODE-2-11 doc-comment prescribes a high CI
  `PROPTEST_CASES` budget but the reject-by-filter generator was never
  made reject-efficient (proper fix: generate
  `prop::collection::vec(0x20u8..0x7F, 0..256)` directly, or raise
  `max_local_rejects`) — a pre-existing reconciliation gap.
- Classification: **pre-existing test-strategy/budget-mismatch defect
  (test-only, parser invariants demonstrably hold). Low / non-blocking.
  NOT a new finding. No Phase-D loop. REMEDIATED — see "REMEDIATED —
  task#80" below; passes fully at PROPTEST_CASES=20000. No longer
  DEFERRED-ENV.**

## Phase-D loop: NOT triggered
Neither failure is a NEW medium+ product defect. Both are pre-existing
test-infrastructure issues with no product-code regression. Phase E
introduces no new Phase-D work item from these gates.

## REMEDIATED — task#80 (2026-05-16, prod-readiness/round-4)

Both pre-existing test-infra defects are now **REMEDIATED** and are
**NO LONGER DEFERRED-ENV**. Test-FILE-only fixes; zero product-source
changes; the audit mandate forbids deferring runnable checks.

### Gate 04 — root cause corrected + fixed
The earlier diagnosis ("5s cold-start deadline too tight for 2-vCPU
box; gateway boots & serves") was INCOMPLETE. Empirically, the
`test_sigterm_drains_h2_with_goaway` gateway never bound the listener
even at a 90s boot budget. Direct repro showed the gateway exits
**before binding any listener** with:
`Error: H1s TLS setup failed ... TLS key permission check failed ...
mode 0o664 permits group/other access (strict mode); chmod 0600 to
fix`. The h1s/QUIC config generators in `tests/reload_zero_drop.rs`
wrote the rcgen private key via `std::fs::write` (umask-derived 0o664);
the product's strict-mode TLS-key-perm check (`lb_security`) correctly
rejects a world/group-readable key. The "200+GOAWAY observed once"
note was an artifact (likely a pre-strict-check / cached-state run).
Two test-FILE fixes:
1. Boot deadline now reads `LB_TEST_BOOT_TIMEOUT_SECS` (parse, default
   30), applied to `spawn_gateway` (H1/H2 polling loop, ceiling only)
   and `spawn_gateway_udp` (H3 warm-up; default 750ms unchanged unless
   the env var is explicitly set). No drain/GOAWAY assertion weakened.
2. New `write_key_0600` helper writes the TLS key with mode 0600
   (forced even on a reused stale temp file); used by both the h1s and
   QUIC config generators.
Result: `LB_TEST_BOOT_TIMEOUT_SECS=30 cargo test --test
reload_zero_drop --release -- --skip ignored` → 4/4 PASS
(test_sigterm_drains_h1/h2/h3 + reload soak), finished 31.63s.

### Gate 05 — fixed
`crates/lb-h1/tests/proptest_parser.rs` `arb_target()` replaced the
per-byte `any::<u8>().prop_filter("printable ASCII", 0x20..0x7F)`
(~25% reject rate; at PROPTEST_CASES=20000 the cumulative per-byte
rejects exceed proptest's default `max_local_rejects` 65536, aborting
after 299 successes) with the reject-free direct range strategy
`prop::collection::vec(0x20u8..0x7Fu8, 0..256)`. Identical value space
(printable ASCII, len 0..256); the no-panic + bounded-consumption
invariants are unchanged; the case budget was NOT lowered.
Result: `PROPTEST_CASES=20000 cargo test -p lb-h1 --release --features
proptest --test proptest_parser` → 2/2 PASS (request_line_no_panic +
headers_no_panic), finished 0.19s, no max_local_rejects abort.

Both gates flip FAIL→PASS in SUMMARY.md. Still verified clean:
`cargo fmt --check` exit 0, `cargo clippy -p lb-h1 --all-targets
-- -D warnings` exit 0 (incl. `--features proptest`),
`cargo test --test round8_drain_15case` 16/16. The genuinely
env-bound items D-1..D-6 in deferred-env.md are untouched.
