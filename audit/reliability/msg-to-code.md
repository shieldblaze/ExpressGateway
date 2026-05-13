# `rel` → `code` — round 1 handoff

Several reliability defects bottom out in code shape. Listing them so
ownership is clear before round-2 findings land:

1. **Unbounded `tokio::spawn` per accept** — `crates/lb/src/main.rs:1126`.
   No semaphore, no max-inflight gate. Joint finding expected; suggest
   you own implementation, I own the saturation metric
   (`listener_inflight_gauge` per listener) + runbook entry.

2. **Accept-loop tight retry on EMFILE** — `crates/lb/src/main.rs:1100-1106`.
   Logs `warn` and `continue` without backoff. Will spin a worker at
   100% the moment fd's exhaust. Fix: classify the error
   (EMFILE/ENFILE → exponential backoff; ECONNABORTED → continue;
   other → consider exiting).

3. **Listener tasks aborted, not cancelled gracefully** —
   `main.rs:1037-1060`. `JoinHandle::abort()` severs mid-`await`;
   in-flight per-connection tasks aren't tracked at all. Need a
   `CancellationToken` per listener + a shared `Arc<AtomicUsize>`
   inflight counter so SIGTERM can wait.

4. **`shutdown_signal()` only handles SIGTERM/SIGINT** — `:1279`. No
   `SignalKind::hangup`. The RUNBOOK reload section is fictional. I am
   raising this as REL-2-01 (RUNBOOK truthfulness) but the *wiring* is
   yours.

5. **`http_latency_buckets()`** — please confirm bucket coverage. I
   haven't yet read `crates/lb-observability/src/lib.rs` end-to-end. If
   the ladder doesn't cover 100 µs → 30 s with reasonable density, I'd
   like to refine it for round-2.

6. **`set_nonblocking` after `lb_io::Runtime::listen`** — `main.rs:1087`.
   Worth checking whether the `Runtime::listen` impl already returns
   the listener in the desired blocking mode; if so, the call is
   redundant.

7. **`HealthChecker` (`crates/lb-health/src/lib.rs`) is phantom** — no
   call sites. Either delete or wire into the balancer in round-2.
   Round-2 finding: REL-2-09 (proposed).

8. **Hot-path sampler is not cancelled at shutdown** — `main.rs:892`.
   Background task runs through SIGTERM; harmless today but should be
   `select! { _ = ticker.tick() => ..., _ = shutdown.cancelled() => return }`.

Pointers in `audit/reliability/round-1-inventory.md`: §1 F-11/F-17/F-22,
§2.1/§2.2, §5.2, §9.
