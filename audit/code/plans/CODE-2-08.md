# Plan for CODE-2-08 — lb-quic per-CID DashMap leak (drop-guard)
Finding-ref:     CODE-2-08 (high, Open)
Files touched:
  - `crates/lb-quic/src/router.rs`                  (lines 370–379 spawn block)
  - `crates/lb-quic/src/cleanup_guard.rs`           (NEW — `DropGuard` helper)
  - `crates/lb-observability/src/metrics.rs`        (`quic_actor_panics_total`)

Approach:
Wrap the two DashMap entries in an RAII `DropGuard` so cleanup runs
unconditionally — on actor exit, panic, or future cancel.

Step 1 — Helper. New `crates/lb-quic/src/cleanup_guard.rs`:
```rust
use dashmap::DashMap;
use std::sync::Arc;

pub struct CidEntryGuard<K: Eq + std::hash::Hash, V> {
    map: Arc<DashMap<K, V>>,
    keys: [Option<K>; 2],
}
impl<K, V> CidEntryGuard<K, V>
where K: Eq + std::hash::Hash + Clone, V: Send + Sync {
    pub fn new(map: Arc<DashMap<K, V>>, router_key: K, header_dcid_key: K) -> Self {
        Self { map, keys: [Some(router_key), Some(header_dcid_key)] }
    }
}
impl<K, V> Drop for CidEntryGuard<K, V>
where K: Eq + std::hash::Hash {
    fn drop(&mut self) {
        for k in self.keys.iter_mut() {
            if let Some(k) = k.take() {
                self.map.remove(&k);
            }
        }
    }
}
```

Step 2 — Use at the spawn site (`router.rs:373–379`):
```rust
let guard = CidEntryGuard::new(
    Arc::clone(connections),
    router_key.clone(),
    header_dcid_key.clone(),
);
tokio::spawn(async move {
    let _guard = guard; // Drop on any exit path: clean, await-cancel, OR panic-unwind.
    let result = std::panic::AssertUnwindSafe(Box::pin(run_actor(actor)))
        .catch_unwind()
        .await;
    if let Err(_panic) = result {
        metrics::quic_actor_panics_total().inc();
        tracing::error!(?router_key, "quic actor panicked");
    }
    // _guard's Drop removes both DashMap entries here, regardless of `result`.
});
```
Note: under CODE-2-02 (`panic = "abort"` in release), the catch_unwind
arm is unreachable in release builds — the process aborts before
unwinding. In dev/test profiles unwind is preserved (per CODE-2-02
Step 1), and there the catch_unwind + guard combination matters for
proptest/loom tests of the router. The dual mechanism is intentional:
- release: process death (CODE-2-02)
- dev/test: cleanup + log + counter (this plan)
- async cancel mid-await: guard Drop still runs (cancellation drops the
  future, which drops `_guard`).

Step 3 — Periodic reaper sweep (defensive). A 60-s tick in the router
task scans for entries whose `mpsc::Sender::is_closed()` returns true
and removes them. This handles the "actor exits cleanly but the
spawned cleanup task itself dies" edge case (e.g. runtime drop).
Implementation lives at the existing `dispatch_to_actor` zero-byte
probe site (`router.rs:252–254`); promoted from on-demand to
periodic. Uses the `Shutdown` token from CODE-2-03 so it joins drain.

Step 4 — Metric. `quic_actor_panics_total` (counter). `rel` defines
the metric in lb-observability; `code` publishes the call site.

Proof:
- `crates/lb-quic/tests/router_panic.rs::actor_panic_cleans_dashmap`:
  install a `panic = unwind` (test profile), inject an actor that
  `panic!("boom")` after registration, assert the DashMap drops to 0
  entries within 100 ms and `quic_actor_panics_total == 1`.
- `crates/lb-quic/tests/router_panic.rs::cancel_drops_entries`:
  hold the actor in `pending()`, cancel via `Shutdown`, assert
  DashMap drops to 0.
- Loom test `tests/loom_router.rs::reaper_no_double_remove` (paired
  with CODE-2-11) models 2 threads (reaper + panicking actor) and
  asserts DashMap removal is idempotent.

Risk / blast radius:
- `Drop` of `CidEntryGuard` runs synchronously inside async drop —
  DashMap operations are non-async and bounded; safe.
- `catch_unwind` requires `AssertUnwindSafe` because the actor holds
  `&mut` state across awaits; safety obligation is documented at the
  call site.
- Periodic reaper adds 1 tokio interval per router instance; minor.
- Under release (`panic = "abort"`) the catch_unwind path is dead
  code (LLVM removes it); no runtime cost.

Cross-ref:    CODE-2-02 (release-mode interaction documented above),
              sec Q-1-09 (acceptability confirmed if CODE-2-02 lands)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
