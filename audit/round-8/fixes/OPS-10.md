# Fix plan — OPS-10: drain budget default vs Pingora norm + per-listener override

Owner: div-ops.
Finding: ROUND8-OPS-10 (medium) — `drain_timeout_ms` default 10000 is correct for short-request HTTP listeners but materially insufficient for long-poll H1 / gRPC bidi / SSE / WebSocket workloads. Pingora ships `EXIT_TIMEOUT=300s` for this exact reason. No per-listener override.

Coverage-gap theme cited: **Theme 2 — Operational-vs-laboratory test posture.** Round 1-7 ran against single-instance test setups serving request/response traffic. The streaming-workload regression is invisible until the operator deploys for one of those workloads.

## A. The decision

Keep the 10 s gateway-level default. Add a per-listener `drain_timeout_ms` override. Document the per-workload selection table. This is a "different design, possibly wrong" finding where the design choice is *defensible for the most common case* and the fix is **operability + per-listener tuning**, not a default change.

We do **not** raise the gateway-level default to 300 s because:
- Most listeners are short-request HTTP; raising the default makes every restart of a normal listener take up to 5 min in the worst case.
- The selection table makes the choice explicit per workload.
- A per-listener override is the right granularity (matches HAProxy `hard-stop-after` per-frontend, nginx `worker_shutdown_timeout` per-worker).

## B. Config schema change

```toml
# Top-level: gateway-wide default
[runtime]
drain_timeout_ms = 10000     # default, applies to listeners that don't override
drain_jitter_ms  = 2500      # OPS-02, derived = drain_timeout_ms / 4

# Per-listener override
[[listeners]]
name = "grpc-public"
bind = "0.0.0.0:443"
proto = "h2"
drain_timeout_ms = 300000    # 5 min — Pingora default
drain_jitter_ms  = 0         # disable jitter for this listener (operator preference)
```

The validation range from `lib.rs:843-848` stays 100..=300_000 ms.

## C. Code-level changes

1. `crates/lb-config/src/lib.rs`: add `drain_timeout_ms: Option<u64>` and `drain_jitter_ms: Option<u64>` to `ListenerConfig`. Both `None` means "inherit from `[runtime]`".
2. `crates/lb-config/src/lib.rs` validation: per-listener values must satisfy the same `100..=300_000` range; jitter must satisfy `0..=drain_timeout`.
3. `crates/lb/src/main.rs` drain-coordinator construction (per OPS-04+L4-12 `DrainSpec::from_runtime_config(...)`): the spec accumulates a `BTreeMap<ListenerName, Duration>` for per-listener `drain_timeout`; the InFlightDrain phase per-conn awaits the *listener's* budget, not a global one. Implementation note: each per-conn task knows its listener name, so the cancel arm reads `spec.per_listener_timeout(&listener_name)`. The phase-level `tokio::time::timeout` uses `max(spec.per_listener_timeout(_)) for all listeners`.
4. Emit `lb_drain_timeout_ms_listener{listener}` build-info-style gauge so `/metrics` reflects the effective per-listener budget. Used by the `LbShutdownSlow` alert (OPS-03).

## D. Documentation

### D.1 — Per-workload selection table in `CONFIG.md`

```markdown
## Drain budget — per-workload selection

| Workload | Recommended `drain_timeout_ms` | Rationale |
|---|---|---|
| Short-request HTTP (request/response, p99 <1s) | 10_000 (default) | Pingora `CLOSE_TIMEOUT` analogue. |
| Streaming HTTP / SSE / long-poll (p99 <60s) | 60_000 | Allow one full poll cycle to complete. |
| gRPC bidi / WebSocket | 300_000 | Pingora `EXIT_TIMEOUT` default. |

Set per-listener via `[[listeners]].drain_timeout_ms`. The gateway-level
`[runtime].drain_timeout_ms` is the default for listeners that do not
override.
```

### D.2 — RUNBOOK section

`RUNBOOK.md` "Drain (graceful shutdown)" section: add subsection "Tuning the drain budget" with the table, and:
> Use `shutdown_drain_seconds{phase="InFlightDrain", listener=...}` p99 to calibrate. If p99 routinely reaches `drain_timeout_ms` (i.e. the drain is timing out, not completing), raise the per-listener budget. If p99 is well below `drain_timeout_ms / 10`, the budget is over-generous; lower it to keep deploys snappy.

### D.3 — New alert

`RUNBOOK.md`:
```promql
# LbShutdownTruncatedStreams — drains routinely abort connections
# (likely under-budget drain for streaming workloads).
sum by (listener) (
  rate(shutdown_aborted_connections_total[1h])
) > 0
and on(listener) group_left
  (http_version{listener=~".+"} == "h2")
```

(Pseudocode — depends on the per-listener emission of `http_version`. The point is "alert if streaming-workload listener is aborting" not the exact PromQL.)

## E. Tests

- `crates/lb-config/tests/per_listener_drain.rs::override_takes_precedence` — listener overrides gateway default; assert `DrainSpec::from_runtime_config` reflects the override.
- `crates/lb-config/tests/per_listener_drain.rs::validation_range` — values outside `100..=300_000` rejected.
- `tests/reload_zero_drop.rs::test_per_listener_drain_budget` — set two listeners with different budgets; drain; assert each respects its own budget.
- `tests/runbook_defaults.rs::test_per_workload_table_matches_config_validation_range` — the selection table values are all within the validation range.

## F. Verification

- `CONFIG.md` per-workload table exists; doc-lint (OPS-09) imports the range bounds from `lb-config` and asserts the table entries fall inside.
- `/metrics` exposes `lb_drain_timeout_ms_listener` gauge.
- Per-listener override works in `tests/reload_zero_drop.rs`.
- `LbShutdownTruncatedStreams` alert documented in RUNBOOK.

## G. Risk

- Misconfigured listener with 300_000 ms budget but short-request workload: a stuck operator-perceived restart for up to 5 min. Mitigation: the runbook explicitly says "use shutdown_drain_seconds p99 to calibrate"; operator-actionable.
