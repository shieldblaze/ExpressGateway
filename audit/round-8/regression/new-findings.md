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
- Classification: **pre-existing environment/hardware-timing
  test-harness limitation (DEFERRED-ENV for that one test).
  Non-blocking for prod. NOT a new finding. No Phase-D loop.**

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
  (test-only, parser invariants demonstrably hold; passes fully at the
  file's default 256-case budget). Low / non-blocking. NOT a new
  finding. No Phase-D loop.**

## Phase-D loop: NOT triggered
Neither failure is a NEW medium+ product defect. Both are pre-existing
test-infrastructure issues with no product-code regression. Phase E
introduces no new Phase-D work item from these gates.
