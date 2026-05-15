### ROUND8-OPS-03 — `shutdown_drain_seconds` histogram from REL-2-02 spec never landed

Reference: `audit/round-8/research/haproxy.md` lesson 20 ("Persistent stats across reloads via GUIDs … hot reload without stats continuity blinds your SRE team to incidents that span the reload boundary."); `audit/round-8/research/envoy.md` lesson 18 ("hot restart is not 'stop, then start'; it is 'start, drain in parallel, then stop' — and your observability must understand the overlap.").
Our equivalent: REL-2-02 status comment claims drain ordering shipped (`audit/reliability/round-2-review.md:77`). The recommendation block at REL-2-02 line 134 explicitly required: `emit shutdown_drain_seconds histogram + shutdown_aborted_connections_total counter`. The counter shipped (`crates/lb/src/main.rs:1973-1977`); the histogram did NOT — `grep "shutdown_drain_seconds" crates/` returns zero hits.

Severity: medium
Status:   Verified-Fixed(verifier=verify, 2dbbc127+698c5a63)   <!-- shutdown_drain_seconds_{global,listener} histogram emitted via MetricsDrainObserver (main.rs:2375) with buckets+phase/outcome labels; REL-2-02 missing-half closed. listener label is <aggregate> today (disclosed OPS-10 follow-up). See audit/round-8/verify/ops.md. -->
Status (pre-verify):   Open

Divergence:
- The audit spec for REL-2-02 required two metrics. Round 7 marked REL-2-02 Verified-Fixed. One of the two metrics is missing.
- Without `shutdown_drain_seconds`, an operator cannot answer the question "how long did the last drain actually take?" from `/metrics`. They have to grep `journalctl` for the log lines "entering drain" → "drain completed cleanly" and subtract timestamps by hand.
- HAProxy and Envoy explicitly call out hot-reload-observability as a *required* property. We have the counter half but not the timing half.

Impact:
- Doc/code drift: `RUNBOOK.md:182-194` describes the `LbShutdownAborted` alert and references `shutdown_aborted_connections_total` (present). The runbook does NOT describe how to compute a drain duration percentile — and we don't expose the histogram that would let it be computed.
- Round-7 audit "Verified-Fixed" was incorrect: the REL-2-02 spec was partially implemented and the audit team did not catch it.

Reproduction:
- `curl 127.0.0.1:9090/metrics | grep -i drain` — returns only `shutdown_aborted_connections_total`. No histogram family for drain duration.
- Walk REL-2-02 recommendation line 134 against the source; one of the two required metrics is absent.

Recommendation:
1. In `crates/lb/src/main.rs` around line 1924 ("entering drain"), capture `Instant::now()`. At the end of the drain block (line 1984 after `match shutdown.drain(...)`), observe `metrics.histogram_vec("shutdown_drain_seconds", ..., &["outcome"], &[0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0]).with_label_values(&[outcome_label]).observe(elapsed.as_secs_f64())` where `outcome_label` is `"clean"` or `"timed_out"`.
2. Add the family to `lb_observability::label_budget::CANONICAL_LABELS` so the integration test in `crates/lb-observability/tests/red_label_budget.rs` enforces it.
3. Re-open REL-2-02 status — Round-7 marked it Verified-Fixed but the histogram is the unfinished half. Refile the verification entry as `Verified-Fixed-Partial` with this delta called out.
4. Update `RUNBOOK.md` `LbShutdownAborted` alert section to add a complementary `LbShutdownSlow` alert: `histogram_quantile(0.99, sum by (le)(rate(shutdown_drain_seconds_bucket[15m]))) > 0.8 * drain_timeout_ms` would warn that drains are routinely brushing the budget.
