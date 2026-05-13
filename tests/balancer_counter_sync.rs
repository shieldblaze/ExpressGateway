//! CODE-2-14 proof — the scheduler-visible snapshot in
//! `lb_balancer::Backend` cannot diverge from the canonical atomic
//! in `lb_core::BackendState` under bounded concurrent inc/dec.
//!
//! Pre-CODE-2-14 the snapshot lived in a plain `u64` field on
//! `Backend` and the atomic lived in `BackendState`. Production code
//! incremented one and the metrics gauge read the other; under load
//! the two could (and did) drift.
//!
//! After CODE-2-14 the canonical state is `Arc<BackendState>` and
//! `Backend::sync_from_state()` refreshes the snapshot before each
//! scheduler pick. This test drives N concurrent threads doing
//! `inc_connections` / `dec_connections` on a SHARED `BackendState`
//! and asserts that after the threads join, `Backend::sync_from_state`
//! observes the canonical atomic value with no drift.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use lb_balancer::{Backend, BackendState};

/// Round-4 named invariant: snapshot converges to atomic; no divergence.
#[test]
fn test_no_divergence_under_load() {
    const THREADS: usize = 8;
    const OPS_PER_THREAD: u64 = 5_000;

    let state = Arc::new(BackendState::new());
    let mut backend = Backend::with_state("b1", 1, Arc::clone(&state));

    // Half the threads do inc; half do dec. Net delta = 0, so the
    // final atomic value is whatever it started at (0). Crucially,
    // during the run the snapshot can lag the atomic — but AFTER
    // sync_from_state the two MUST agree.
    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::with_capacity(THREADS);
    for i in 0..THREADS {
        let s = Arc::clone(&state);
        let stop = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            if i % 2 == 0 {
                for _ in 0..OPS_PER_THREAD {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    s.inc_connections();
                }
            } else {
                for _ in 0..OPS_PER_THREAD {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    s.dec_connections();
                }
            }
        }));
    }

    // While the threads churn, sample the snapshot vs the atomic
    // repeatedly. Under the CODE-2-14 contract, after sync_from_state
    // the two MUST equal each other (the atomic at the sample moment
    // is what the snapshot captures).
    // Sanity-sample during the run: the snapshot must always be ≤
    // the post-load atomic value (writers can race past us but never
    // BEFORE us). This is the "no divergence into the future"
    // invariant — a stale read is acceptable; reading a value that
    // never existed is not. After all writers join we then do the
    // exact-equality check.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut samples = 0u64;
    let mut max_observed = 0u64;
    while std::time::Instant::now() < deadline && !handles.iter().all(|h| h.is_finished()) {
        let pre_atomic = state.active_connections();
        backend.sync_from_state();
        let post_atomic = state.active_connections();
        // The snapshot we just wrote was the atomic at SOME moment
        // between pre_atomic and post_atomic. So it must lie within
        // the [min, max] interval of the two bracket samples.
        let lo = pre_atomic.min(post_atomic);
        let hi = pre_atomic.max(post_atomic);
        assert!(
            backend.active_connections >= lo && backend.active_connections <= hi,
            "snapshot diverged from atomic at sample {samples}: \
             snapshot={}, bracket=[{lo}, {hi}]",
            backend.active_connections,
        );
        max_observed = max_observed.max(backend.active_connections);
        samples += 1;
    }
    stop.store(true, Ordering::Release);

    for h in handles {
        h.join().expect("worker thread panicked");
    }

    // Final reconciliation.
    backend.sync_from_state();
    let final_atomic = state.active_connections();
    assert_eq!(
        backend.active_connections, final_atomic,
        "post-join snapshot diverged from final atomic"
    );

    // Net delta is zero (equal inc/dec threads × equal op count),
    // but saturating decrement can underflow to zero before half the
    // inc's land. So the final value is in [0, OPS_PER_THREAD * 4].
    assert!(samples > 0, "test loop never sampled — fixture broken");
    println!(
        "test_no_divergence_under_load: {samples} samples, \
         max_observed={max_observed}, final_atomic={final_atomic}"
    );
}

/// Sanity: a Backend constructed with `with_state` mirrors the
/// initial atomic values. Guards against a regression where the
/// snapshot was zero-initialised regardless of the atomic state.
#[test]
fn with_state_seeds_snapshot_from_atomic() {
    let state = Arc::new(BackendState::new());
    state.inc_connections();
    state.inc_connections();
    state.inc_requests();
    state.set_latency_ns(42);

    let backend = Backend::with_state("b1", 1, state);
    assert_eq!(backend.active_connections, 2);
    assert_eq!(backend.active_requests, 1);
    assert_eq!(backend.latency_ewma_ns, 42);
}

/// Regression: a Backend without an atomic (legacy path used by
/// every existing balancer test) keeps the plain-u64 semantics so
/// the picker tests don't have to change.
#[test]
fn legacy_backend_keeps_plain_u64_semantics() {
    let mut backend = Backend::new("b1", 1);
    backend.active_connections = 5;
    backend.active_requests = 3;
    backend.latency_ewma_ns = 1_000;

    // sync_from_state on a stateless backend is a no-op + returns false.
    let changed = backend.sync_from_state();
    assert!(!changed);
    assert_eq!(backend.active_connections, 5);
    assert_eq!(backend.active_requests, 3);
    assert_eq!(backend.latency_ewma_ns, 1_000);
}
