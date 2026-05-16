# Fix plan — OPS-06: distributed tracing actually called from L7 (pointer-only)

Owner: div-l7 (primary, per cross-review.md §B bundle 1); div-ops co-author on the tracing-extract/inject helper API and OTLP exporter wire-in.
Finding: ROUND8-OPS-06 (high) — `lb_observability::tracing_propagation` library ships W3C codec; zero L7 callsites use it.

This plan is intentionally short. The full implementation plan for the L7 wire-in (defer-101-until-upstream-completes + traceparent injection through the upgrade path + child span emission per request) is owned by div-l7 under bundle B-1 (L7-01 + OPS-06).

**See: `audit/round-8/fixes/L7-01-OPS-06.md` (div-l7's plan).**

The OPS-06 deliverables that div-ops still owns (and that the L7-01 plan should reference) are:

1. **OTLP exporter wire-in.** `[observability].otlp_endpoint` is parsed today (per the `OtlpConfig` struct in `crates/lb-observability`) but never opens a connection. The lb-observability crate must spawn an OTLP gRPC exporter task when the endpoint is configured, defaulted off. This is independent of the L7 callsites — the library should ship the exporter even before consumers exist.
2. **REL-2-07 status note update.** Re-tag REL-2-07 from `Verified-Fixed-Partial` to `Open` (or `Verified-Fixed-Partial` with the Round-8 reconcile note explicitly escalating severity per the finding). The OPS-09 doc-lint extension will then reject any future round that marks REL-2-07 Verified-Fixed without a corresponding integration test that asserts a span emerges from the exporter.
3. **Integration test scaffolding.** A `tests/tracing_e2e.rs` test that:
   - Spins up an in-process OTLP collector (or a mock that records `ExportTraceServiceRequest`).
   - Sends an H1 request with `traceparent: 00-<trace-id>-<parent-id>-01`.
   - Asserts the collector receives a span with that `trace_id` and our service name.
   - This test is the gate the L7 wire-in writes against.

Coverage-gap theme cited: **Theme 1 — "Verified-Fixed" snapshot of script existence, not capability.** REL-2-07 was graded `Verified-Fixed-Partial` *three rounds ago* with the wire-in gap disclosed; subsequent rounds did not close it. The audit-of-audit gate (L4-10+OPS-09 bundle) must make this status of "partial-with-disclosure-that-never-closes" un-extensible without an active follow-up plan.

Verification: Verified-Fixed for OPS-06 requires both the L7 callsites (owned by L7-01 plan) AND the OTLP exporter wire-in AND `tests/tracing_e2e.rs` passing. Without all three, the status remains Open.
