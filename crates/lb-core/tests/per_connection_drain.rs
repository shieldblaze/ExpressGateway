//! CODE-2-03 follow-on (Round-5 push-back) proof: the per-connection
//! select! pattern used in `crates/lb/src/main.rs:2074` honours the
//! SIGTERM drain budget — long-running upstream work is interrupted
//! by the shutdown token and the abort counter is bumped (not
//! silently dropped).
//!
//! The actual per-connection task lives in the `lb` binary (no lib
//! surface to test directly). This test reproduces the exact
//! `tokio::select! { biased; cancel => abort++; work => ... }`
//! shape over a `Shutdown` instance and asserts the contract:
//!
//! 1. With a drain budget LARGER than the upstream's response time
//!    (the happy path), the connection completes normally and the
//!    abort counter stays at zero.
//! 2. With a drain budget SMALLER than the upstream's response time
//!    (SIGTERM mid-request), the cancel arm fires, the abort counter
//!    increments, and `Shutdown::drain` returns `Clean` (the task
//!    exited cooperatively).
//!
//! Round-5 spec required: "spawn a long-running upstream that takes
//! 5s; send SIGTERM at 1s; assert: drain budget is honoured OR the
//! connection is aborted with the abort counter incremented (not
//! silently dropped)." This test compresses the wall-clock to ~200 ms
//! by using `tokio::time::sleep` against a real multi-thread runtime
//! (start_paused would make the test non-deterministic across the
//! select! drop boundary).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lb_core::{DrainOutcome, Shutdown};

/// Mimics the per-connection task body. The `work` future stands in
/// for the proxy session (any long-running async operation); the
/// `cancel` arm bumps `abort_counter` to mirror the real
/// `shutdown_aborted_connections_total` counter wired in
/// `crates/lb/src/main.rs`.
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
                // CODE-2-03 follow-on: this is the cancel arm in
                // main.rs:2074 — bump the abort counter and exit.
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

    // Wait long enough for the task to finish on its own, then drain.
    // The drain budget here is irrelevant because the task already
    // exited.
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

    // Upstream "responds" in 5 s (matches the round-5 brief). The
    // drain budget is 200 ms so SIGTERM at "1 s" (we send it
    // immediately for test compression) cancels the connection well
    // before the upstream answers.
    simulate_per_connection_task(
        shutdown.clone(),
        Duration::from_secs(5),
        Arc::clone(&aborts),
        Arc::clone(&completes),
    )
    .await;

    // Give the spawn a tick to register on the tracker so drain
    // observes the live task. Without this the test races spawn vs.
    // drain — drain could return Clean before the task is tracked.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Trigger the SIGTERM-equivalent: cancel the token, then drain
    // with a 200 ms budget. The cancel arm should fire first (biased
    // select!), the abort counter should bump, and the tracker
    // should report Clean because the task exited cooperatively.
    let outcome = shutdown.drain(Duration::from_millis(200)).await;

    // The CODE-2-03 contract: either the drain finished cleanly
    // (task cooperated with cancel) OR the abort counter bumped
    // (task was tracked and forced down). The "silently dropped"
    // failure mode is the only thing this test forbids.
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

/// Negative control: a task that *ignores* the cancel token blocks
/// past the drain deadline. The tracker should still report
/// `TimedOut { remaining: 1 }` — the per-conn pattern this complements
/// is built on `Shutdown::drain`, and a regression that broke the
/// timeout would falsely report Clean here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_uncooperative_task_times_out() {
    let shutdown = Shutdown::new();

    shutdown.tracker().spawn(async {
        // Intentionally ignore the cancel token to model a
        // misbehaving task. 30 s is well past any reasonable test
        // budget — the drain deadline below MUST elapse first.
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
