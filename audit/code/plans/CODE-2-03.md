# Plan for CODE-2-03 — graceful drain via workspace Shutdown token
Finding-ref:     CODE-2-03 (critical, Open) — REL-2-02 + PROTO-2-11 layer atop
Files touched:
  - `crates/lb-core/src/shutdown.rs`                (NEW — Shutdown token)
  - `crates/lb-core/src/lib.rs`                     (re-export)
  - `crates/lb/src/main.rs`                         (single-source token, SIGTERM, drain)
  - `crates/lb-l7/src/h1_proxy.rs`                  (spawn-site cancel plumbing)
  - `crates/lb-l7/src/h2_proxy.rs`                  (spawn-site cancel plumbing)
  - `crates/lb-l7/src/grpc_proxy.rs`                (spawn-site cancel plumbing)
  - `crates/lb-l7/src/ws_proxy.rs`                  (spawn-site cancel plumbing)
  - `crates/lb-quic/src/conn_actor.rs`              (cancel-aware tick)
  - `crates/lb-io/src/dns.rs`                       (cancel-aware refresh task)
  - `crates/lb-io/src/http2_pool.rs`                (cancel-aware idle reaper)
  - `crates/lb-config/src/runtime.rs`               (`drain_timeout_ms`, default 10000)

Approach:
Introduce a single `Shutdown` newtype that wraps
`tokio_util::sync::CancellationToken` + a `TaskTracker`. One instance
constructed in `main`, cloned into every long-lived spawn site.

Step 1 — `crates/lb-core/src/shutdown.rs`:
```rust
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

#[derive(Clone)]
pub struct Shutdown {
    token: CancellationToken,
    tracker: TaskTracker,
}
impl Shutdown {
    pub fn new() -> Self { /* tracker.close() called at drain time */ }
    pub fn token(&self) -> &CancellationToken { &self.token }
    pub fn tracker(&self) -> &TaskTracker { &self.tracker }
    pub fn child(&self) -> Self { /* derived for per-listener nesting */ }
    pub async fn drain(self, deadline: Duration) -> DrainOutcome { /* cancel + tracker.wait + timeout */ }
}
pub enum DrainOutcome { Clean, TimedOut { remaining: usize } }
```

Step 2 — Plumb through every spawn. The 33 spawn sites enumerated in
CODE-2-03 location list each become:
```rust
let shutdown = shutdown.clone();
shutdown.tracker().spawn(async move {
    tokio::select! {
        biased;
        _ = shutdown.token().cancelled() => { /* cooperative exit */ }
        _ = work_future() => {}
    }
});
```
Concrete insertion points:
- `main.rs:701` `spawn_listener` — clone into `ListenerState`.
- `main.rs:1099–1106` accept loop — `select! { _ = sd.cancelled() => break, … }`.
- `main.rs:1126` per-conn handler — use `tracker.spawn(...)`.
- `main.rs:892` metrics sampler, `:233` TLS ticket rotator —
  `select!` between cancel and `ticker.tick()`.
- `lb-l7/h1_proxy.rs:432,581,659`, `h2_proxy.rs:400,487`,
  `grpc_proxy.rs:250`, `ws_proxy.rs` upgrade — accept `&Shutdown`
  parameter, propagate.
- `lb-quic/conn_actor.rs:296,307,320` — actor `tokio::select!` already
  has the structure; thread the token in.
- `lb-io/dns.rs:340`, `http2_pool.rs:284` — replace
  `loop { interval.tick().await; … }` with select.

Step 3 — SIGTERM. `main.rs::shutdown_signal` (currently :1279) becomes:
```rust
let drain_ms = config.runtime.drain_timeout_ms.unwrap_or(10_000);
match shutdown.drain(Duration::from_millis(drain_ms)).await {
    DrainOutcome::Clean => tracing::info!("clean drain"),
    DrainOutcome::TimedOut { remaining } =>
        tracing::warn!(remaining, "drain timeout, aborting stragglers"),
}
```

Step 4 — Config knob. `lb-config::RuntimeConfig::drain_timeout_ms:
Option<u64>`, default 10_000, validated 100..=300_000.

Insertion points published for layering (per synthesis §D):
- `rel` lands `accept_inflight` gauge atop the same `TaskTracker` count
  (REL-2-09).
- `rel` lands `accepting: AtomicBool` set/cleared from `Shutdown`
  state for `/livez` (REL-2-04).
- `proto` lands `h2.graceful_shutdown()` and H3 CONNECTION_CLOSE
  inside the per-connection `select!` arms (PROTO-2-11).

Proof:
- `crates/lb/tests/drain.rs::sigterm_drains_inflight_within_budget`:
  spawn 100 long-poll handlers via test listener, send SIGTERM,
  assert tracker count → 0 within `drain_timeout_ms`, asserting all
  in-flight responses completed (no RST observed via `tokio::io::copy`
  returning `UnexpectedEof`).
- `crates/lb/tests/drain.rs::sigterm_budget_exceeded_aborts`: handlers
  block on `pending()`; assert `DrainOutcome::TimedOut` reported and
  process exits cleanly.
- `crates/lb-core/tests/shutdown.rs`: unit tests for `Shutdown::child`
  and tracker semantics.

Risk / blast radius:
- 33 spawn sites edited; high mechanical surface, low semantic
  surface. The `tokio_util::task::TaskTracker` dependency is already
  in `tokio-util` v0.7.10+. Pinned in `Cargo.toml` accordingly.
- Per-connection futures must not hold non-cancel-safe state across
  await points within the `select!`. Round 4 audit each new arm.
- The `biased;` keyword ensures cancel wins ties, preventing a final
  request from sneaking past the drain.

Cross-ref:    REL-2-02 (drain), PROTO-2-11 (GOAWAY/CONN_CLOSE),
              REL-2-04 (/livez accepting flag), REL-2-09 (inflight gauge)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
