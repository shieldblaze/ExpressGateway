# Plan for CODE-2-14 — Single source of truth for backend counters
Finding-ref:     CODE-2-14 (medium, Open)
Files touched:
  - `crates/lb-balancer/src/backend.rs`             (REMOVE duplicate counters)
  - `crates/lb-balancer/src/picker.rs`              (consume `&[Arc<BackendState>]`)
  - `crates/lb-balancer/src/lib.rs`                 (re-export adjustments)
  - `crates/lb-l7/src/upstream.rs`                  (drop `RoundRobinUpstreams` or rebase on `lb-balancer`)
  - `crates/lb-l7/src/h1_proxy.rs`                  (use picker output directly)
  - `crates/lb-l7/src/h2_proxy.rs`                  (same)
  - `crates/lb-l7/src/grpc_proxy.rs`                (same)

Approach:
Adopt `lb_core::BackendState` (atomic) as the single source of truth.
`lb-balancer` becomes a thin scheduler over `&[Arc<BackendState>]`.
The `lb-l7::upstream::RoundRobinUpstreams` representation is dropped.

Step 1 — Remove duplicates. `crates/lb-balancer/src/backend.rs`
currently defines its own `Backend { active_connections: u64, ... }`.
Delete the duplicate fields; keep only static config (address, weight,
health hint reference, etc.):
```rust
pub struct Backend {
    pub addr: SocketAddr,
    pub weight: u32,
    pub state: Arc<lb_core::BackendState>,   // shared atomic
}
impl Backend {
    pub fn active_connections(&self) -> u64 {
        // Acquire per CODE-2-04 G-classification of backend.rs:87.
        self.state.active_connections.load(Ordering::Acquire)
    }
    pub fn is_healthy(&self) -> bool { self.state.is_healthy() }
}
```

Step 2 — Picker refactor. `lb-balancer::picker::Picker::select` now
returns `Option<Arc<Backend>>` from `&[Arc<Backend>]`. P2C, least-conn,
and round-robin all read from `state.active_connections.load(Acquire)`.

Step 3 — Drop `RoundRobinUpstreams`. `crates/lb-l7/src/upstream.rs` is
either:
- (preferred) Replaced by a thin facade that constructs a
  `lb_balancer::Picker` and returns `Arc<Backend>` to the proxy.
- (alternative) Deleted entirely; proxies hold a `Picker` directly.

Decision: replace with a facade `UpstreamRegistry` whose only job is
hot-path lookup by cluster name → picker. Picker logic lives in
`lb-balancer`.

Step 4 — Increment site. The proxy bridges (h1/h2/grpc) call:
```rust
let backend = registry.pick(&cluster_name)?;
let _guard = backend.state.inc_active();   // RAII increment+decrement
let resp = forward(backend.addr, req).await?;
```
`BackendState::inc_active()` returns a guard that:
- Increments `active_connections` with `Release`-ordered fetch_add.
- Decrements on drop with the saturating-CAS loop (already exists at
  `lb-core/backend.rs:98-110`, ordering corrected per CODE-2-04).
This pattern is exactly what CODE-2-04's G classification specifies.

Step 5 — Metrics. The admin endpoint already reads
`BackendState::active_connections.load(Acquire)`; nothing to change.
With duplicates gone, gauge and scheduler agree by construction.

Proof:
- `crates/lb-balancer/tests/single_source.rs::scheduler_and_gauge_agree`:
  open N concurrent connections to a 3-backend cluster; sample
  per-backend gauge and the picker's view; assert exact match within
  the load-balance algorithm's expected dispersion.
- `crates/lb-balancer/tests/picker.rs::least_conn_picks_lowest_load`:
  pre-set state's `active_connections` to [5, 1, 3], call
  `picker.select` 1000 times against frozen state — assert all 1000
  return index 1.
- `cargo check --workspace`: gating signal that no caller of the
  removed `lb-balancer::Backend.active_connections: u64` field
  survives.
- `cargo machete` shows no orphaned deps post-refactor.

Risk / blast radius:
- The proxy bridge code touches the per-request increment site; if
  CODE-2-03 lands first, the RAII guard sits inside the spawned
  handler future and is dropped on cancel automatically.
- Picker performance: an extra Arc deref per backend at pick time.
  Negligible (< 1 ns) and outweighed by removing the
  scheduler-vs-gauge sync bug.
- API change for downstream users of `lb-balancer::Backend` —
  internal only; no public crates affected (confirmed via `cargo
  rustdoc -p lb-balancer` — no `pub use` of the deprecated field).

Cross-ref:    CODE-2-04 (atomic ordering on `BackendState`),
              proto Q-CODE-1-07 (scope question now resolved)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
