# Plan for CODE-2-05 + REL-2-09 — Per-listener Semaphore + max_inflight
Finding-ref:     CODE-2-05 / REL-2-09 (critical, Open)
Files touched:
  - `crates/lb-config/src/runtime.rs`               (`max_inflight_connections`)
  - `crates/lb/src/main.rs`                         (semaphore wiring at accept)
  - `crates/lb-l7/src/listener.rs`                  (per-listener override)
  - `crates/lb-observability/src/metrics.rs`        (`accept_inflight` gauge — rel exports)

Approach:
Per-listener `Arc<tokio::sync::Semaphore>` placed alongside
`ListenerState`. Accept loop tries to acquire an owned permit
non-blockingly; on failure, sheds the connection and increments
`shed_total{reason="over_inflight"}`.

Step 1 — Config. `RuntimeConfig` gains:
```rust
pub max_inflight_connections: Option<u32>,   // default 65_536, range 1..=2_097_152
```
Per-listener override in `ListenerConfig::max_inflight: Option<u32>`
(falls back to runtime default). Validated in
`lb-config::validate::runtime`.

Step 2 — Listener state. In `crates/lb-l7/src/listener.rs` or wherever
`ListenerState` lives:
```rust
pub struct ListenerState {
    pub shutdown: Shutdown,                      // from CODE-2-03
    pub inflight: Arc<Semaphore>,                // NEW
    pub inflight_max: u32,
    // …
}
```
Constructed in `spawn_listener` with `Semaphore::new(max as usize)`.

Step 3 — Accept-site gate. `crates/lb/src/main.rs:1126` becomes:
```rust
let permit = match state.inflight.clone().try_acquire_owned() {
    Ok(p) => p,
    Err(TryAcquireError::NoPermits) => {
        metrics::accept_shed_total().with_label_values(&["over_inflight"]).inc();
        // Behaviour per protocol:
        match listener_protocol {
            Proto::PlainTcp | Proto::Tls => {
                // RST: drop the stream, kernel sends RST on remaining data.
                drop(client_stream);
            }
            Proto::Http1 | Proto::Http2 => {
                // 503 with Connection: close, then drop.
                send_503_close(&mut client_stream).await;
            }
        }
        continue;
    }
    Err(TryAcquireError::Closed) => break, // semaphore closed at drain
};
state.shutdown.tracker().spawn(async move {
    let _permit = permit; // dropped on task exit
    /* … existing handler body … */
});
```

Step 4 — Metric. `rel` owns the gauge definition; `code` publishes the
read site:
```rust
metrics::accept_inflight_gauge(&listener.name)
    .set(state.inflight_max as i64 - state.inflight.available_permits() as i64);
```
sampled by the 1-Hz metrics loop at `main.rs:892` (already plumbed
through CODE-2-03).

Step 5 — Cooperative drain. On `Shutdown::drain`, the semaphore is
*not* closed — in-flight permits drain naturally as handlers exit.
This preserves the CODE-2-03 timeline. New accepts have already been
stopped by the accept-loop cancel arm.

Proof:
- `crates/lb/tests/inflight.rs::caps_at_max`: bind listener with
  `max_inflight = 4`, open 4 long-poll connections, attempt 5th —
  assert TCP shed (RST) for plain mode and 503 for H1 mode.
  `accept_shed_total{reason="over_inflight"}` increments by 1.
- `crates/lb/tests/inflight.rs::permit_released_on_handler_exit`:
  short-poll 8 sequential connections through `max_inflight = 1` —
  all 8 complete; no shed.
- `crates/lb/tests/inflight.rs::gauge_reports_accurate_inflight`:
  open N=3 long-polls, sample `accept_inflight` gauge → 3.

Risk / blast radius:
- A wrongly-low default could shed legitimate traffic. 65 536 is high
  enough that any realistic node hits OS limits first.
- 503 send path requires a small writer; if the handshake hasn't yet
  produced a hyper Service, we fall back to RST (drop). For TLS, we
  always RST (no plaintext bytes pre-handshake).
- `try_acquire_owned` is O(1) and atomic; no lock contention added.
- Interacts with CODE-2-04: the semaphore's internal counter is
  AcqRel; existing `active_connections` gauge stays Relaxed-S
  (verified in CODE-2-04 appendix).

Cross-ref:    REL-2-09 (gauge + alert + runbook),
              SEC-2-04 (per-IP cap is the second axis, sec-owned),
              CODE-2-01 (`admit_connection` SecurityHooks trait — sec call site)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
