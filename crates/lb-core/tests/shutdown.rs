//! CODE-2-03 proof — the `Shutdown` graceful-drain primitive.
//!
//! Two named invariants the round-4 task brief calls out:
//!
//! * `test_drain_cancels_clean` — a tracker-bound task that cooperates
//!   with `token().cancelled()` exits inside the deadline; `drain`
//!   returns `Clean`.
//! * `test_drain_times_out_returns_remaining` — a tracker-bound task
//!   that ignores cancellation (intentionally blocks on
//!   `tokio::time::sleep(huge)`) is still live at the deadline;
//!   `drain` returns `TimedOut { remaining: 1 }`.

use std::time::Duration;

use lb_core::{DrainOutcome, Shutdown};

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn test_drain_cancels_clean() {
    let shutdown = Shutdown::new();
    let token = shutdown.token().clone();

    // Cooperative task: exits as soon as the token fires.
    let handle = shutdown.tracker().spawn(async move {
        tokio::select! {
            biased;
            () = token.cancelled() => "cancelled-clean",
            // Long sleep so that under a regression we'd hit the
            // timeout branch rather than passing accidentally.
            () = tokio::time::sleep(Duration::from_secs(3600)) => "sleep-finished",
        }
    });

    // Drain with a generous deadline. Tokio's paused-time runtime
    // advances virtual time to wake the cancel arm immediately.
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

    // Uncooperative task: ignores the cancel token and blocks for
    // a virtual hour. With paused time + a small deadline the drain
    // must hit the timeout branch and report the remaining count.
    let _handle = shutdown.tracker().spawn(async move {
        // Deliberately NO `select!` on cancel; this is the
        // "drain budget exceeded" path the orchestration logs.
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
    // Sanity: an idle Shutdown should drain Clean instantly.
    let shutdown = Shutdown::new();
    let outcome = shutdown.drain(Duration::from_millis(1)).await;
    assert_eq!(outcome, DrainOutcome::Clean);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn child_shares_orchestration() {
    // Two child handles spawn cooperative tasks; the parent's drain
    // wakes both. Confirms `child()` does NOT detach the tracker.
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
