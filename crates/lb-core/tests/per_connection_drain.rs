//! The per-connection drain contract: a cancelled connection must bump the abort counter, never
//! be silently dropped.
//!
//! The real task lives in the `lb` binary with no lib surface, so this REPRODUCES its
//! `select! { biased; cancel => abort++; work => ... }` shape. Real time, not `start_paused` —
//! paused time is non-deterministic across the select! drop boundary here.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_core::{DrainOutcome, Shutdown};

/// Stands in for the per-connection task body; `abort_counter` mirrors
/// `shutdown_aborted_connections_total`.
async fn simulate_per_connection_task(
    shutdown: Shutdown,
    upstream_delay: Duration,
    abort_counter: Arc<AtomicU64>,
    completion_counter: Arc<AtomicU64>,
) {
    let cancel = shutdown.token().clone();
    shutdown.tracker().spawn(async move {
        let work = async {
            tokio::time::sleep(upstream_delay).await;
            "upstream-response-body"
        };
        let _result: Result<&str, &str> = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                // The cancel arm: bump and exit.
                abort_counter.fetch_add(1, Ordering::AcqRel);
                Err("connection cancelled by shutdown")
            }
            r = work => {
                completion_counter.fetch_add(1, Ordering::AcqRel);
                Ok(r)
            }
        };
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_inflight_request_completes_when_under_budget() {
    let shutdown = Shutdown::new();
    let aborts = Arc::new(AtomicU64::new(0));
    let completes = Arc::new(AtomicU64::new(0));

    // Upstream "responds" in 50 ms; drain budget is 500 ms — generous.
    simulate_per_connection_task(
        shutdown.clone(),
        Duration::from_millis(50),
        Arc::clone(&aborts),
        Arc::clone(&completes),
    )
    .await;

    // The task already exited, so the drain budget is irrelevant here.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let outcome = shutdown.drain(Duration::from_millis(500)).await;

    assert_eq!(
        outcome,
        DrainOutcome::Clean,
        "drain must return Clean when the task already exited"
    );
    assert_eq!(
        completes.load(Ordering::Acquire),
        1,
        "happy path: work arm must have fired once"
    );
    assert_eq!(
        aborts.load(Ordering::Acquire),
        0,
        "happy path: abort arm must NOT have fired"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_inflight_request_completes_or_cancels_on_sigterm() {
    let shutdown = Shutdown::new();
    let aborts = Arc::new(AtomicU64::new(0));
    let completes = Arc::new(AtomicU64::new(0));

    // A 5 s upstream against a 200 ms budget: cancel must win.
    simulate_per_connection_task(
        shutdown.clone(),
        Duration::from_secs(5),
        Arc::clone(&aborts),
        Arc::clone(&completes),
    )
    .await;

    // REQUIRED: without this tick drain can return Clean before the task is even tracked.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    // The biased select must take the cancel arm, bump, and exit cooperatively.
    let outcome = shutdown.drain(Duration::from_millis(200)).await;

    // Clean drain OR a bumped counter; "silently dropped" is the only forbidden outcome.
    assert_eq!(
        outcome,
        DrainOutcome::Clean,
        "drain must return Clean — the task observed cancel and exited cooperatively"
    );
    assert_eq!(
        completes.load(Ordering::Acquire),
        0,
        "the upstream is 5 s away — the work arm MUST NOT have completed"
    );
    assert_eq!(
        aborts.load(Ordering::Acquire),
        1,
        "abort counter must have incremented; this is the \
         shutdown_aborted_connections_total signal — the round-5 brief \
         forbids the silent-drop failure mode"
    );
}

/// NEGATIVE CONTROL: a task ignoring the token must report `TimedOut { remaining: 1 }`. A broken
/// timeout would make the positive cases above pass vacuously.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_uncooperative_task_times_out() {
    let shutdown = Shutdown::new();

    shutdown.tracker().spawn(async {
        // Deliberately ignores the token; 30 s guarantees the deadline elapses first.
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    tokio::task::yield_now().await;

    let outcome = shutdown.drain(Duration::from_millis(50)).await;

    match outcome {
        DrainOutcome::TimedOut { remaining } => {
            assert_eq!(
                remaining, 1,
                "exactly one uncooperative task must remain at the deadline"
            );
        }
        DrainOutcome::Clean => panic!(
            "regression: an uncooperative task slept past the drain budget \
             but drain reported Clean — the timeout arm of Shutdown::drain \
             must fire here"
        ),
    }
}
