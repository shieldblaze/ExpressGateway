//! `Shutdown` drain proofs: a cooperating task returns `Clean`, an ignoring one returns
//! `TimedOut { remaining: 1 }`.

use std::time::Duration;

use lb_core::{DrainOutcome, Shutdown};

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn test_drain_cancels_clean() {
    let shutdown = Shutdown::new();
    let token = shutdown.token().clone();

    let handle = shutdown.tracker().spawn(async move {
        tokio::select! {
            biased;
            () = token.cancelled() => "cancelled-clean",
            // Long enough that a regression fails rather than passing by luck.
            () = tokio::time::sleep(Duration::from_secs(3600)) => "sleep-finished",
        }
    });

    // Paused time wakes the cancel arm immediately.
    let outcome = shutdown.drain(Duration::from_secs(60)).await;

    assert_eq!(
        outcome,
        DrainOutcome::Clean,
        "cooperative task must exit cleanly within deadline"
    );
    let exit_reason = handle.await.expect("task did not panic");
    assert_eq!(exit_reason, "cancelled-clean");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn test_drain_times_out_returns_remaining() {
    let shutdown = Shutdown::new();

    let _handle = shutdown.tracker().spawn(async move {
        // Deliberately no cancel arm.
        tokio::time::sleep(Duration::from_secs(3600)).await;
    });

    let outcome = shutdown.drain(Duration::from_millis(50)).await;

    match outcome {
        DrainOutcome::TimedOut { remaining } => {
            assert_eq!(
                remaining, 1,
                "expected exactly one uncooperative task at deadline"
            );
        }
        DrainOutcome::Clean => panic!("non-cooperative task drained as Clean — regression"),
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn drain_zero_tasks_is_clean() {
    let shutdown = Shutdown::new();
    let outcome = shutdown.drain(Duration::from_millis(1)).await;
    assert_eq!(outcome, DrainOutcome::Clean);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn child_shares_orchestration() {
    // Confirms `child()` does NOT detach the tracker.
    let parent = Shutdown::new();
    let a = parent.child();
    let b = parent.child();

    for child in [&a, &b] {
        let tok = child.token().clone();
        child.tracker().spawn(async move {
            tok.cancelled().await;
        });
    }

    let outcome = parent.drain(Duration::from_secs(1)).await;
    assert_eq!(outcome, DrainOutcome::Clean);
}
