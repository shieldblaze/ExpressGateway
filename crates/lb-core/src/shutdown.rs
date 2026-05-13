//! CODE-2-03 — process-wide graceful drain primitive.
//!
//! `Shutdown` bundles a [`CancellationToken`] (cooperative
//! stop-the-world signal) with a [`TaskTracker`] (count of live spawn
//! sites that opted into the drain). One instance is constructed in
//! `main`; every long-lived spawn site clones it and runs
//! `tracker().spawn(...)` so the orchestrator can `drain(deadline)`
//! at SIGTERM time and observe whether everything settled.
//!
//! Wave 1 (CODE-2-03 module-only) ships the type + tests. Wave 2 is
//! the accept-site / per-spawn plumbing in `crates/lb/src/main.rs` and
//! across the L7 / QUIC / IO crates — serialised after sec/rel/proto
//! land their pieces. See the round-4 task brief.
//!
//! ## Drain semantics
//!
//! `drain(deadline)` performs four steps in order:
//!
//! 1. `tracker.close()` — refuses any *new* `tracker.spawn(...)`. The
//!    accept loop is expected to break out of its loop on the same
//!    `token` so no new connection-handler tasks are accepted.
//! 2. `token.cancel()` — wakes every cooperative `select!` arm. Each
//!    task is responsible for cleaning up its own state and exiting.
//! 3. `tokio::time::timeout(deadline, tracker.wait())` — bounded wait
//!    for everything to settle.
//! 4. Return `Clean` if all tasks exited inside `deadline`, otherwise
//!    `TimedOut { remaining }` with the live-task count at deadline.
//!    Callers (Wave 2 SIGTERM handler) translate `TimedOut` into a
//!    `tracing::warn!` + best-effort abort of stragglers.
//!
//! ## Why not `JoinSet`?
//!
//! `TaskTracker` lets us `wait()` for the *trailing fan-out* without
//! holding owned `JoinHandle`s. That matters because per-connection
//! handlers spawn helper futures (read/write halves, idle reapers)
//! that need to be tracked alongside the parent — `JoinSet` would
//! require we plumb the handle back to the parent which the
//! per-listener accept loop doesn't have a place to store.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Cloneable graceful-drain handle. One instance per process; every
/// long-lived spawn site clones and runs `tracker().spawn(...)`.
#[derive(Clone, Debug)]
pub struct Shutdown {
    token: CancellationToken,
    tracker: TaskTracker,
}

impl Shutdown {
    /// Fresh shutdown handle. The token starts un-cancelled and the
    /// tracker starts un-closed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            tracker: TaskTracker::new(),
        }
    }

    /// The cancellation token. Cooperative select arms should poll
    /// `token().cancelled()` first (use `biased;` in the `select!`).
    #[must_use]
    pub const fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// The task tracker. Spawn long-lived tasks via
    /// `shutdown.tracker().spawn(async move { ... })` so they show up
    /// in `drain()`'s count.
    #[must_use]
    pub const fn tracker(&self) -> &TaskTracker {
        &self.tracker
    }

    /// Derive a per-listener / per-subsystem child handle that shares
    /// the same drain orchestration. Today a clone — kept as a
    /// distinct method so Wave 2 can switch to a true `child_token()`
    /// derivation without an API break.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
            tracker: self.tracker.clone(),
        }
    }

    /// Drain: refuse new spawns, signal cancel, wait for outstanding
    /// tasks up to `deadline`. Returns [`DrainOutcome`] reporting
    /// whether all tasks finished cleanly or how many remained at the
    /// deadline.
    ///
    /// Consumes `self` because no further spawns should occur on a
    /// drained handle. Wave 2 may instead loosen this to `&self` if
    /// the call-site shape forces it (the underlying tokio types
    /// support it); the conservative signature is the audit default.
    pub async fn drain(self, deadline: Duration) -> DrainOutcome {
        // 1) Refuse new spawns on the tracker.
        self.tracker.close();
        // 2) Wake cooperative `select!` arms.
        self.token.cancel();
        // 3) Bounded wait.
        match tokio::time::timeout(deadline, self.tracker.wait()).await {
            Ok(()) => DrainOutcome::Clean,
            Err(_) => DrainOutcome::TimedOut {
                remaining: self.tracker.len(),
            },
        }
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of [`Shutdown::drain`].
#[derive(Debug, PartialEq, Eq)]
pub enum DrainOutcome {
    /// All tracked tasks exited within the deadline.
    Clean,
    /// The deadline elapsed with `remaining` tasks still live. Wave 2
    /// SIGTERM orchestration translates this into a warn-level log
    /// followed by best-effort abort of the stragglers.
    TimedOut {
        /// Number of tracker-bound tasks still live when the deadline
        /// elapsed.
        remaining: usize,
    },
}
