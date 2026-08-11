//! Real-runtime race test for the snapshot-vs-atomic contract, complementing the abstract loom
//! model in `loom_atomic_counter.rs`.
//!
//! Loom proves publication ORDERING on two modelled threads; this catches what that abstraction
//! cannot — a real call site mutating the snapshot without going through the atomic.

use std::sync::Arc;

use lb_balancer::round_robin::RoundRobin;
use lb_balancer::{Backend, LoadBalancer};
use lb_core::BackendState;
use tokio::task::JoinSet;

/// Exceeds the CI runner's thread count so the schedule genuinely interleaves.
const TASKS: usize = 16;

/// Enough iterations that a non-atomic publish diverges reliably.
const ITERS: usize = 1000;

/// After concurrent producers bump the atomic, a final `sync_from_state()` must leave the
/// snapshot equal to it.
///
/// Fails if an increment is ever published outside `inc_connections`, or if `sync_from_state`
/// loads too weakly. x86 hides the ordering half, but the divergence half still shows here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_no_divergence_under_concurrent_increment() {
    let state = Arc::new(BackendState::new());

    // `pick` stands in for the scheduler hot path refreshing before each selection.
    let mut set: JoinSet<()> = JoinSet::new();
    for task_id in 0..TASKS {
        let state = Arc::clone(&state);
        set.spawn(async move {
            // Distinct `Backend`s sharing ONE atomic — that sharing is the property under test.
            let mut backend = Backend::with_state("b", 1, Arc::clone(&state));
            let mut backends = vec![backend.clone()];
            let mut rr = RoundRobin::new();
            for i in 0..ITERS {
                // Bump through the real AcqRel path.
                state.inc_connections();
                // The pick result is irrelevant; only that no snapshot outruns the atomic.
                backend.sync_from_state();
                backends[0] = backend.clone();
                let _ = rr.pick(&backends);
                // A snapshot ABOVE the live atomic is unambiguously a memory-order bug.
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

    while let Some(res) = set.join_next().await {
        res.expect("producer task panicked");
    }

    // Exactly TASKS × ITERS increments, or one was lost.
    let expected = (TASKS * ITERS) as u64;
    let live = state.active_connections();
    assert_eq!(
        live, expected,
        "atomic counter divergence: expected {expected} got {live}"
    );

    // The contract: after a final sync the snapshot equals the atomic.
    let mut backend = Backend::with_state("b", 1, Arc::clone(&state));
    backend.sync_from_state();
    assert_eq!(
        backend.active_connections, live,
        "Backend.active_connections snapshot must equal state.active_connections() \
         after sync_from_state(); snap={} live={}",
        backend.active_connections, live
    );
}

/// A `Backend` built mid-traffic must pre-seed from the CURRENT atomic, not zero, or its first
/// pick is made on a phantom idle backend.
#[tokio::test]
async fn test_with_state_seeds_snapshot_from_atomic() {
    let state = Arc::new(BackendState::new());
    // Non-zero before construction, or the test is vacuous.
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
