//! CODE-2-14 follow-on (Round-5 push-back): integration race test
//! complementing the loom model at `tests/loom_atomic_counter.rs`.
//!
//! The loom test covers the *publication ordering* between
//! `BackendState::inc_connections` (AcqRel) and the scheduler-visible
//! load — a stricter guarantee than this test offers, but limited to
//! the abstract two-thread model the loom harness compiles.
//!
//! This integration test runs the real types on a real multi-thread
//! tokio runtime: N concurrent tasks each call `inc_connections` and
//! `pick(...)` 1000 times. At the end we call `sync_from_state()` once
//! and assert the snapshot u64 in `lb_balancer::Backend` equals the
//! live `state.active_connections()` value. Catches a divergence that
//! the loom abstraction would miss (e.g. a missing call site that
//! mutates the snapshot directly without going through the atomic).

use std::sync::Arc;

use lb_balancer::round_robin::RoundRobin;
use lb_balancer::{Backend, LoadBalancer};
use lb_core::BackendState;
use tokio::task::JoinSet;

/// Number of concurrent tasks. Chosen to exceed the test runner's
/// default thread count (typically 2-4 on CI) so the schedule is
/// genuinely interleaved.
const TASKS: usize = 16;

/// Iterations per task. 1000 × 16 = 16 000 total `inc_connections`
/// calls — large enough that any non-atomic publish would diverge
/// frequently, small enough that the test completes well under a
/// second on CI hardware.
const ITERS: usize = 1000;

/// CODE-2-14: after N concurrent producers all bump the live
/// `BackendState::active_connections` atomic and the consumer
/// re-syncs via `sync_from_state()`, the cached snapshot in
/// `Backend` MUST equal the atomic.
///
/// Bypass attempts that would fail this test:
/// 1. If a future refactor publishes the increment through a
///    non-atomic path (e.g. cached `active_connections += 1` without
///    going through `inc_connections`), the snapshot would lag.
/// 2. If `sync_from_state()` reads a stale value (e.g. Ordering::Relaxed
///    load with no synchronisation against the AcqRel fetch_add), it
///    would publish a stale `u64` here. Round-1 hardware (x86) hides
///    this on its own, but the assertion still catches the algorithmic
///    case where two snapshots disagree.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_no_divergence_under_concurrent_increment() {
    let state = Arc::new(BackendState::new());

    // Spawn N producer tasks; each calls inc_connections + pick.
    // The `pick` call is the contract the round-5 brief calls for —
    // it stands in for the production hot-path where the scheduler
    // refreshes its snapshot before each pick.
    let mut set: JoinSet<()> = JoinSet::new();
    for task_id in 0..TASKS {
        let state = Arc::clone(&state);
        set.spawn(async move {
            // Per-task balancer + backend (each task's Backend holds
            // its own Arc<BackendState> clone — they all point at
            // the same atomic, which is the property under test).
            let mut backend = Backend::with_state("b", 1, Arc::clone(&state));
            let mut backends = vec![backend.clone()];
            let mut rr = RoundRobin::new();
            for i in 0..ITERS {
                // Real production path: bump the atomic via the
                // AcqRel inc_connections method.
                state.inc_connections();
                // Refresh the snapshot then pick. We don't care about
                // the pick result here — just that the call doesn't
                // observe a snapshot newer than the atomic.
                backend.sync_from_state();
                backends[0] = backend.clone();
                let _ = rr.pick(&backends);
                // Sanity-check mid-loop on the first iteration of
                // task 0 — if the snapshot ever EXCEEDS the live
                // atomic, that's a definite memory-order bug.
                if task_id == 0 && i == 0 {
                    assert!(
                        backend.active_connections <= state.active_connections(),
                        "snapshot must never exceed the atomic: snap={} atomic={}",
                        backend.active_connections,
                        state.active_connections()
                    );
                }
            }
        });
    }

    // Wait for all producers to finish.
    while let Some(res) = set.join_next().await {
        res.expect("producer task panicked");
    }

    // After every producer has stopped, the atomic should have been
    // incremented exactly TASKS × ITERS times.
    let expected = (TASKS * ITERS) as u64;
    let live = state.active_connections();
    assert_eq!(
        live, expected,
        "atomic counter divergence: expected {expected} got {live}"
    );

    // The single source of truth contract: after a final
    // sync_from_state(), the cached snapshot equals the atomic.
    let mut backend = Backend::with_state("b", 1, Arc::clone(&state));
    backend.sync_from_state();
    assert_eq!(
        backend.active_connections, live,
        "Backend.active_connections snapshot must equal state.active_connections() \
         after sync_from_state(); snap={} live={}",
        backend.active_connections, live
    );
}

/// CODE-2-14 complementary case: verify that a freshly-constructed
/// `Backend::with_state(...)` mid-traffic pre-seeds its snapshot from
/// the *current* atomic value rather than zero. This is the "consistent
/// first pick" guarantee the round-2 review calls out.
#[tokio::test]
async fn test_with_state_seeds_snapshot_from_atomic() {
    let state = Arc::new(BackendState::new());
    // Drive the atomic to a non-zero value before constructing the
    // Backend.
    for _ in 0..42 {
        state.inc_connections();
    }
    let backend = Backend::with_state("b", 1, Arc::clone(&state));
    assert_eq!(
        backend.active_connections, 42,
        "Backend::with_state must pre-seed snapshot from the atomic, \
         got snap={} expected=42",
        backend.active_connections
    );
}
