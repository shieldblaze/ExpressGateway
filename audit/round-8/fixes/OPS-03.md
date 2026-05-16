# Fix plan — OPS-03: `shutdown_drain_seconds` histogram

Owner: div-ops.
Finding: ROUND8-OPS-03 (medium) — REL-2-02 spec required `shutdown_drain_seconds` histogram + `shutdown_aborted_connections_total` counter. Counter shipped; histogram did NOT. Round-7 still marked REL-2-02 Verified-Fixed.

Coverage-gap theme cited: **Theme 1 — "Verified-Fixed" snapshot of script existence, not capability.** REL-2-02 was marked Verified-Fixed because the drain *ordering* shipped; one of the two required metrics was missing. The audit team didn't walk the recommendation block line-by-line against `/metrics`.

This finding is directly enabled by the OPS-04+L4-12 drain coordinator, which emits the histogram per phase. Without the coordinator, this plan would need to instrument the legacy drain block separately; with the coordinator, this plan just describes the histogram contract and observations.

## A. Histogram contract

```
shutdown_drain_seconds{phase, outcome, listener} histogram
  labels:
    phase    ∈ {"ReadinessSettle", "ListenerCancel", "InFlightDrain", "XdpDetach", "Total"}
    outcome  ∈ {"clean", "timed_out"}
    listener — present only when phase is listener-scoped (ListenerCancel, InFlightDrain). For non-listener-scoped phases this label is omitted from the family entirely (different MetricVec).
  buckets: [0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0]
    — extends to 300s to accommodate the OPS-10 Pingora 5-min budget.
```

Two MetricVec instances to avoid label cardinality blowup:
- `shutdown_drain_seconds_global{phase, outcome}` for ReadinessSettle, XdpDetach, Total.
- `shutdown_drain_seconds_listener{phase, outcome, listener}` for ListenerCancel, InFlightDrain.

Both observed by the OPS-04 drain coordinator at the end of each phase. The `outcome` label is `"clean"` if the phase completed within its deadline, `"timed_out"` if the coordinator hit the deadline and fell through.

## B. Cardinality budget update

The OPS-05 startup cardinality check must register both families in `CANONICAL_LABELS`. Worst-case series:
- `_global`: 5 phases × 2 outcomes = 10 series (constant, fits the LabelBudget trivially).
- `_listener`: 2 phases × 2 outcomes × `cfg.listeners.len()` series ≤ 4 × N_listeners. For typical N≤8, ≤32 series.

Add to `crates/lb-observability/src/label_budget.rs` `CANONICAL_LABELS` and to `crates/lb-observability/tests/red_label_budget.rs` snapshot.

## C. Code-level changes

1. `crates/lb-observability/src/metrics_facade.rs` (or wherever histograms register): add the two families with documented bucket boundaries.
2. `crates/lb-core::shutdown::Shutdown::run_drain` (per OPS-04+L4-12 plan): each phase records its duration via the metrics facade.
3. `crates/lb/src/main.rs` legacy drain block is removed by the OPS-04+L4-12 plan; this plan inherits that simplification.

## D. Runbook

`RUNBOOK.md` `LbShutdownAborted` alert section: add a complementary alert spec:
```promql
# LbShutdownSlow — drains routinely brush the budget
(
  histogram_quantile(0.99,
    sum by (le, listener) (rate(shutdown_drain_seconds_listener_bucket{phase="InFlightDrain"}[15m]))
  ) > 0.8 * (
    max by (listener) (lb_drain_timeout_ms_listener) / 1000
  )
)
```
(`lb_drain_timeout_ms_listener` is a build-info-style gauge we expose anyway for the per-listener budget per OPS-10.)

## E. REL-2-02 status re-grading

Re-open REL-2-02 to `Verified-Fixed-Partial` with the status note:
> Drain ordering shipped (`f2bf64c`); `shutdown_aborted_connections_total` counter shipped. `shutdown_drain_seconds` histogram landed in Round-8 fix bundle OPS-04+L4-12 + OPS-03 (sha=TBD). Verifier walked REL-2-02 recommendation block line-by-line against /metrics.

The L4-10+OPS-09 doc-lint extension will assert that a Verified-Fixed claim referencing REL-2-02 has a matching commit that touches `shutdown_drain_seconds`.

## F. Tests

- **Integration:** `tests/metrics_shutdown_drain_seconds.rs` — spin up, trigger drain, scrape `/metrics`, assert all 5 phases × at least 1 observation. Already half-covered by the OPS-04 coordinator integration tests; this plan adds the explicit metric scrape assertion.
- **Snapshot:** `crates/lb-observability/tests/red_label_budget.rs` extended to include the two new families.

## G. Verification

- `curl 127.0.0.1:9090/metrics | grep shutdown_drain_seconds` returns ≥10 series after one drain cycle.
- `RUNBOOK.md` `LbShutdownSlow` alert is documented.
- REL-2-02 status is `Verified-Fixed-Partial` with the histogram disclosed as the unfinished half, then re-graded `Verified-Fixed` once the OPS-03 commit lands.
