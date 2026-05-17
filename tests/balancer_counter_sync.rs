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

    // Let the writers churn for a bounded window, then signal stop.
    //
    // NOTE (F-COR-2 / BL-1): the previous mid-flight 3-read bracket
    // assertion was DELETED here because it was mathematically
    // unsound, not because it is inconvenient (this is NOT
    // test-weakening, R5 — see commit message for the full proof).
    // It did three NON-atomic reads at t1<t2<t3 —
    // `pre=active_connections()`, `sync_from_state()` (snapshot =
    // atomic@t2), `post=active_connections()` — then asserted
    // `snapshot ∈ [min(pre,post), max(pre,post)]`. That containment
    // bracket is valid ONLY for a MONOTONIC counter. This counter is
    // NON-monotonic: 4 inc + 4 dec threads churn a saturating
    // AtomicU64 concurrently, so the atomic legitimately oscillates
    // and the t2 value can rise above / dip below BOTH bracket
    // endpoints and return between t1 and t3. A single AtomicU64
    // load cannot tear, so the captured counter-example
    // snapshot=8650 bracket=[8649,8649] (pre==post==8649) is a
    // *coherent* value the live counter genuinely held — the product
    // read the correct atomic. The deterministic monotonic coverage
    // is now provided by `snapshot_tracks_atomic_monotonic_midflight`
    // below, where the bracket invariant IS valid; the sound
    // post-join exact-equality check (the real CODE-2-14 contract) is
    // retained UNCHANGED and still fails if the product ever drifts.
    let churn_window = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < churn_window && !handles.iter().all(|h| h.is_finished()) {
        std::thread::yield_now();
    }
    stop.store(true, Ordering::Release);

    for h in handles {
        h.join().expect("worker thread panicked");
    }

    // Final reconciliation — the SOUND CODE-2-14 contract, retained
    // unchanged: after the writers join the snapshot MUST exactly
    // equal the canonical atomic. Still fails if the product drifts.
    backend.sync_from_state();
    let final_atomic = state.active_connections();
    assert_eq!(
        backend.active_connections, final_atomic,
        "post-join snapshot diverged from final atomic"
    );

    println!("test_no_divergence_under_load: final_atomic={final_atomic}");
}

/// F-COR-2 / BL-1 restored mid-flight coverage — DETERMINISTIC.
///
/// The deleted concurrent bracket was unsound because the counter is
/// non-monotonic under concurrent inc+dec. Here we drive it
/// single-threaded and inc-only, so the counter IS monotonic and the
/// three reads (`pre`, `sync_from_state`, `post`) are exact (no other
/// thread mutates between them). This restores sound mid-flight
/// coverage with ZERO added flake, and still FAILS if
/// `sync_from_state` ever returns a stale/wrong value or the atomic
/// regresses (verified by a local stub-stale negative guard,
/// demonstrated, not committed).
#[test]
fn snapshot_tracks_atomic_monotonic_midflight() {
    const ITERS: u64 = 10_000;

    let state = Arc::new(BackendState::new());
    let mut backend = Backend::with_state("b1", 1, Arc::clone(&state));

    let mut prev_snapshot = 0u64;
    for i in 1..=ITERS {
        state.inc_connections();

        // Single thread: these three reads are exact — nothing else
        // mutates `state` between them.
        let pre = state.active_connections();
        backend.sync_from_state();
        let post = state.active_connections();

        // Exact agreement: the snapshot is the atomic, no drift.
        assert_eq!(
            pre, post,
            "single-threaded reads disagreed at iter {i} (fixture broken)"
        );
        assert_eq!(
            backend.active_connections, pre,
            "snapshot != atomic at iter {i}: snapshot={}, atomic={pre} \
             (sync_from_state returned a stale/wrong value)",
            backend.active_connections,
        );
        assert_eq!(
            backend.active_connections, i,
            "snapshot did not track the {i}th increment: snapshot={}",
            backend.active_connections,
        );

        // The bracket invariant IS valid here (monotonic counter):
        // snapshot ∈ [pre, post] AND monotonically non-decreasing.
        assert!(
            backend.active_connections >= pre && backend.active_connections <= post,
            "snapshot {} outside monotonic bracket [{pre}, {post}] at iter {i}",
            backend.active_connections,
        );
        assert!(
            backend.active_connections >= prev_snapshot,
            "snapshot regressed: {} < previous {prev_snapshot} at iter {i}",
            backend.active_connections,
        );
        prev_snapshot = backend.active_connections;
    }

    assert_eq!(
        backend.active_connections, ITERS,
        "final snapshot {} != {ITERS} increments",
        backend.active_connections,
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
