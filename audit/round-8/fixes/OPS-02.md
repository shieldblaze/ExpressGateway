# Fix plan — OPS-02: drain jitter / probabilistic close

Owner: div-ops.
Finding: ROUND8-OPS-02 (medium) — drain fires `shutdown_token.cancel()` synchronously; every per-connection task observes cancellation in the same scheduler tick; deploy-wide thundering herd risk against the upstream LB.

Coverage-gap theme cited: **Theme 2 — Operational-vs-laboratory test posture.** The bug is invisible at single-instance test scale. References (Envoy `drain_manager_impl.cc`) hit this in production with stateful upstream LBs at >2-3 replicas. We have not yet deployed at scale, so this is a "lesson-not-yet-paid-for."

This plan plugs into the OPS-04+L4-12 drain coordinator (phase 5 / InFlightDrain). The jitter is a per-connection cancel-arm sleep, not a global drain delay.

## A. Design — per-connection cancel-arm jitter

Per Envoy's pattern: `P(close) = elapsed / drain_timeout`, distributing close events over the drain window. The simpler form (Envoy also documents it as the "first quarter" variant) is: each per-connection cancel arm sleeps a random `Duration::from_millis(0..jitter_max_ms)` before propagating cancel into the protocol layer.

We adopt the simpler form for v1 (uniform jitter over `[0, jitter_max_ms)`). Envoy's elapsed-proportional variant is a follow-up that requires tracking per-conn elapsed since `set_draining`.

### A.1 — Where the jitter applies

In `crates/lb/src/main.rs:2484-2501` per-connection `biased select!`. The cancel arm currently:
```rust
() = conn_cancel.cancelled() => { /* propagate */ }
```
becomes:
```rust
() = conn_cancel.cancelled() => {
    let jitter = rand::thread_rng().gen_range(0..jitter_max.as_millis() as u64);
    tokio::time::sleep(Duration::from_millis(jitter)).await;
    /* propagate */
}
```

### A.2 — `jitter_max` default and config

- New config key `runtime.drain_jitter_ms`. Default: `drain_timeout_ms / 4` (Envoy's "first quarter" recipe).
- Validation range: `0..=drain_timeout_ms`. Zero disables jitter (single-instance / testing).
- Per-listener override `[listeners.X].drain_jitter_ms` matches the OPS-10 per-listener `drain_timeout_ms` override (both are listener-scoped).

### A.3 — RNG choice

`rand::thread_rng()` is per-task and stateless from our viewpoint. The Cargo dep is already in the tree (verify with `cargo tree -p rand` before plan approval; if not present, add `rand = "0.8"` to `crates/lb/Cargo.toml`). The per-task seeding is sufficient — we do not need crypto-quality randomness for drain jitter.

## B. Metric

The `shutdown_drain_seconds` histogram (OPS-03) labelled by `phase` already gives the operator the distribution. Adding a separate `shutdown_drain_jitter_seconds` histogram would be over-instrumentation; the phase-5 histogram is the bottom-up evidence the distribution is in fact spread.

## C. Documentation

`RUNBOOK.md` "Drain (graceful shutdown)" section: add subsection "Drain jitter" with:
> Each connection's cancel signal is delayed by a per-connection random duration uniformly distributed in `[0, drain_jitter_ms)`. Default `drain_jitter_ms = drain_timeout_ms / 4`. Set to 0 for single-instance deployments where deterministic drain is desired (e.g. testing).

`CONFIG.md`: add `drain_jitter_ms` to the runtime config table with the default formula and validation range.

## D. Tests

- **Unit:** `crates/lb-core/src/shutdown.rs::tests::jitter_uniform_distribution` — 1000 cancel arms, assert mean within 5% of `jitter_max/2`, p99 ≤ `jitter_max`.
- **Integration (`tests/reload_zero_drop.rs`):**
  - `test_drain_jitter_spreads_closes` — drive 1000 keep-alive H1 connections; trigger drain with `drain_jitter_ms=200`; assert close timestamps span ≥150 ms (p10..p90).
  - `test_drain_jitter_zero_synchronous` — same as above with `drain_jitter_ms=0`; assert closes fall within a 10ms window.
- **Multi-replica reproducer (deferred to Pillar 4b-3 load gate per the coverage-gap recommendation):** spin up 3 instances + a client fleet against all three, restart one, measure RPS divergence on the other two. The test is the acceptance criterion the coverage-gap §Theme-2 action item calls for.

## E. Verification

- `RUNBOOK.md` and `CONFIG.md` doc-lint includes the new key.
- `/metrics` `shutdown_drain_seconds{phase="InFlightDrain"}` histogram has spread when `drain_jitter_ms > 0` (visible in the integration test fixtures).
- The OPS-04+L4-12 drain coordinator phase 5 uses the jitter on every per-conn cancel arm.
