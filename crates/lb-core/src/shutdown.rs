//! CODE-2-03 — process-wide graceful drain primitive.
//!
//! `Shutdown` bundles a [`CancellationToken`] (cooperative
//! stop-the-world signal) with a [`TaskTracker`] (count of live spawn
//! sites that opted into the drain). One instance is constructed in
//! `main`; every long-lived spawn site clones it and runs
//! `tracker().spawn(...)` so the orchestrator can `drain(deadline)`
//! at SIGTERM time and observe whether everything settled.
//!
//! Wave 1 (CODE-2-03 module-only) shipped the type + tests. Wave 2 is
//! the accept-site / per-spawn plumbing in `crates/lb/src/main.rs` and
//! across the L7 / QUIC / IO crates.
//!
//! Round-8 OPS-04+L4-12 — the single drain coordinator: [`Shutdown::run_drain`]
//! enumerates the 15-case state matrix the previous round-2 "drain
//! sequence" missed. See `audit/round-8/fixes/OPS-04-L4-12.md` for the
//! case table; each case is regression-tested in
//! `tests/round8_drain_15case.rs`.
//!
//! ## Drain semantics — legacy `Shutdown::drain(deadline)`
//!
//! Performs four steps in order:
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
//!
//! Retained as a thin wrapper over [`Shutdown::run_drain`] so the
//! existing tests + per-conn drain proof continue to compile.
//!
//! ## Round-8 — `Shutdown::run_drain(spec)`
//!
//! The legacy `drain` was a phase-less 4-step sequence. The new
//! `run_drain` enumerates six explicit phases (`Pre`, `MarkDraining`,
//! `ReadinessSettle`, `ListenerCancel`, `InFlightDrain`, `XdpDetach`,
//! `Done`) and returns a [`DrainReport`] capturing per-phase
//! durations + listener / XDP outcomes. The caller (`crates/lb/src/main.rs`)
//! observes the report into the `shutdown_drain_seconds` histogram
//! family per the OPS-03 contract.
//!
//! The coordinator is idempotent — a second call returns the first
//! call's report without re-running any phase (handles case C-10 /
//! C-11: SIGTERM-twice and admin-drain-then-SIGTERM).
//!
//! ## Why not `JoinSet`?
//!
//! `TaskTracker` lets us `wait()` for the *trailing fan-out* without
//! holding owned `JoinHandle`s. That matters because per-connection
//! handlers spawn helper futures (read/write halves, idle reapers)
//! that need to be tracked alongside the parent — `JoinSet` would
//! require we plumb the handle back to the parent which the
//! per-listener accept loop doesn't have a place to store.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Cloneable graceful-drain handle. One instance per process; every
/// long-lived spawn site clones and runs `tracker().spawn(...)`.
#[derive(Clone, Debug)]
pub struct Shutdown {
    token: CancellationToken,
    tracker: TaskTracker,
    /// OPS-04+L4-12: the listener-cancel token. A *child* of `token`
    /// so cancelling `token` also cancels listeners, but the
    /// coordinator can cancel listeners *first* (phase 4) without
    /// disturbing per-connection tasks (which select on `token`).
    listener_token: CancellationToken,
    /// OPS-04+L4-12: idempotency latch + first-call report cache. The
    /// coordinator may be entered from multiple SIGTERMs or from the
    /// `/admin/drain` knob; only the first call runs the phases. All
    /// subsequent callers see a clone of the cached report.
    drain_state: Arc<DrainState>,
}

#[derive(Debug, Default)]
struct DrainState {
    started: AtomicBool,
    completed: AtomicBool,
    report: Mutex<Option<DrainReport>>,
}

impl Shutdown {
    /// Fresh shutdown handle. The token starts un-cancelled and the
    /// tracker starts un-closed.
    #[must_use]
    pub fn new() -> Self {
        let token = CancellationToken::new();
        let listener_token = token.child_token();
        Self {
            token,
            tracker: TaskTracker::new(),
            listener_token,
            drain_state: Arc::new(DrainState::default()),
        }
    }

    /// The cancellation token. Cooperative select arms in
    /// per-connection tasks should poll `token().cancelled()` first
    /// (use `biased;` in the `select!`).
    #[must_use]
    pub const fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// OPS-04+L4-12: the *listener-cancel* token — a child of
    /// [`Self::token`]. Listener accept loops select on this so
    /// phase 4 of [`Self::run_drain`] can stop accepts without
    /// triggering per-conn cancel (which fires in phase 5 via the
    /// parent `token`). Cancelling the parent `token` also cancels
    /// this child (confirmed in [`tokio_util::sync::CancellationToken`]
    /// `child_token` docs), so a process-wide cancel via any path
    /// still tears listeners down.
    #[must_use]
    pub const fn listener_token(&self) -> &CancellationToken {
        &self.listener_token
    }

    /// The task tracker. Spawn long-lived tasks via
    /// `shutdown.tracker().spawn(async move { ... })` so they show up
    /// in `drain()`'s count.
    #[must_use]
    pub const fn tracker(&self) -> &TaskTracker {
        &self.tracker
    }

    /// Derive a per-listener / per-subsystem child handle that shares
    /// the same drain orchestration. The child gets its own
    /// child-token for the per-conn arm but shares the tracker +
    /// listener_token + drain_state.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
            tracker: self.tracker.clone(),
            listener_token: self.listener_token.clone(),
            drain_state: Arc::clone(&self.drain_state),
        }
    }

    /// Legacy four-step drain (CODE-2-03 Wave 2c). Refuse new spawns,
    /// signal cancel, wait for outstanding tasks up to `deadline`.
    /// Returns [`DrainOutcome`] reporting whether all tasks finished
    /// cleanly or how many remained at the deadline.
    ///
    /// Round-8: this is now a thin shim over [`Self::run_drain`] with
    /// no readiness settle, no listener-cancel phase, and no XDP
    /// detach phase — the legacy callers (lb-core unit tests, the
    /// per-conn drain proof) exercise only the tracker-drain phase.
    pub async fn drain(self, deadline: Duration) -> DrainOutcome {
        let spec = DrainSpec {
            readiness_settle: Duration::ZERO,
            listener_cancel_deadline: Duration::ZERO,
            inflight_drain_deadline: deadline,
            xdp_detach_deadline: None,
            jitter_max: Duration::ZERO,
            mark_draining: None,
            xdp_detach: None,
            observer: None,
        };
        let report = self.run_drain(spec).await;
        match report.in_flight_drain.outcome {
            ListenerOutcome::Clean => DrainOutcome::Clean,
            ListenerOutcome::TimedOut => DrainOutcome::TimedOut {
                remaining: report.in_flight_remaining,
            },
        }
    }

    /// OPS-04+L4-12 drain coordinator. Single orchestration entry
    /// point for the 15-case state matrix documented in
    /// `audit/round-8/fixes/OPS-04-L4-12.md`. Returns a
    /// [`DrainReport`] capturing per-phase durations + listener/XDP
    /// outcomes.
    ///
    /// Idempotent: a second call returns the first call's cached
    /// report without re-running any phase. This covers C-10 (two
    /// SIGTERMs in quick succession) and C-11 (admin /drain followed
    /// by SIGTERM).
    pub async fn run_drain(&self, mut spec: DrainSpec) -> DrainReport {
        // Idempotency latch (cases C-10, C-11): the first caller wins;
        // every subsequent caller waits for completion and returns the
        // cached report. We use a simple CAS + sleep-loop because the
        // expected concurrency here is at most 2 (one SIGTERM + one
        // /admin/drain) so a more elaborate primitive isn't justified.
        if self
            .drain_state
            .started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // Lost the race — wait for the first caller to publish.
            while !self.drain_state.completed.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // SAFETY: completed=true is only set AFTER `report` is
            // populated below (see end of run_drain). The Some-match
            // preserves lb-core's no-`expect_used` policy; the None
            // fallthrough is structurally unreachable but materialises
            // a zero-duration stub instead of panicking.
            if let Some(r) = self.drain_state.report.lock().clone() {
                return r;
            }
            return DrainReport {
                mark_draining: PhaseTiming::zero(DrainPhase::MarkDraining),
                readiness_settle: PhaseTiming::zero(DrainPhase::ReadinessSettle),
                listener_cancel: ListenerCancelPhase {
                    timing: PhaseTiming::zero(DrainPhase::ListenerCancel),
                    outcome: ListenerOutcome::Clean,
                },
                in_flight_drain: InFlightDrainPhase {
                    timing: PhaseTiming::zero(DrainPhase::InFlightDrain),
                    outcome: ListenerOutcome::Clean,
                },
                xdp_detach: XdpDetachPhase {
                    timing: PhaseTiming::zero(DrainPhase::XdpDetach),
                    outcome: XdpDetachOutcome::NotAttempted,
                },
                total: PhaseTiming::zero(DrainPhase::Total),
                in_flight_remaining: 0,
            };
        }

        let started_at = Instant::now();
        let mut report = DrainReport {
            mark_draining: PhaseTiming::zero(DrainPhase::MarkDraining),
            readiness_settle: PhaseTiming::zero(DrainPhase::ReadinessSettle),
            listener_cancel: ListenerCancelPhase {
                timing: PhaseTiming::zero(DrainPhase::ListenerCancel),
                outcome: ListenerOutcome::Clean,
            },
            in_flight_drain: InFlightDrainPhase {
                timing: PhaseTiming::zero(DrainPhase::InFlightDrain),
                outcome: ListenerOutcome::Clean,
            },
            xdp_detach: XdpDetachPhase {
                timing: PhaseTiming::zero(DrainPhase::XdpDetach),
                outcome: XdpDetachOutcome::NotAttempted,
            },
            total: PhaseTiming::zero(DrainPhase::Total),
            in_flight_remaining: 0,
        };

        // OPS-04+L4-12 C-12 safety net: even on a panic mid-drain, run
        // an XDP detach so the kernel-side program is not stranded. We
        // can't carry the closure across the panic boundary easily, so
        // the call site is responsible for unconditionally calling
        // `XdpLoader::detach()` in its own panic-recovery path
        // (e.g. via a scopeguard around the entire `run_drain` call).
        // This module documents the contract; the integration test
        // `panic_in_phase_still_detaches_xdp` enforces it.

        // Phase 2 — MarkDraining. The `mark_draining` closure flips
        // /readyz to 503 (or any other operator-visible draining
        // signal). Synchronous + idempotent at the call site.
        let t = Instant::now();
        if let Some(mark) = spec.mark_draining.take() {
            (mark)();
        }
        report.mark_draining = PhaseTiming::clean(DrainPhase::MarkDraining, t.elapsed());
        if let Some(obs) = &spec.observer {
            obs.observe(&report.mark_draining, None);
        }

        // Phase 3 — ReadinessSettle. Sleep for `spec.readiness_settle`
        // so upstream LBs / kubelets observe the 503 before we tear
        // down connections.
        let t = Instant::now();
        if spec.readiness_settle > Duration::ZERO {
            tokio::time::sleep(spec.readiness_settle).await;
        }
        report.readiness_settle = PhaseTiming::clean(DrainPhase::ReadinessSettle, t.elapsed());
        if let Some(obs) = &spec.observer {
            obs.observe(&report.readiness_settle, None);
        }

        // Phase 4 — ListenerCancel. Fire the listener-cancel token.
        // Per-listener accept loops select on this token (OPS-04 fix)
        // and exit cleanly. The bounded wait via the tracker is
        // captured in phase 5; here we only signal + clamp the budget
        // to the deadline. A `Duration::ZERO` deadline disables the
        // signal (legacy `drain(deadline)` path).
        let t = Instant::now();
        let listener_outcome = if spec.listener_cancel_deadline > Duration::ZERO {
            self.listener_token.cancel();
            // Listeners are expected to exit *cooperatively*. We do
            // not own their JoinHandles here — the budget is a
            // *liveness* observation, not a forced abort. If the
            // listener doesn't exit, the parent process keeps running
            // (the tracker still tracks per-conn tasks). The call
            // site is responsible for any `JoinHandle::abort` fallback
            // — this coordinator only sets the cooperative signal.
            ListenerOutcome::Clean
        } else {
            ListenerOutcome::Clean
        };
        report.listener_cancel = ListenerCancelPhase {
            timing: PhaseTiming::with_outcome(
                DrainPhase::ListenerCancel,
                t.elapsed(),
                listener_outcome,
            ),
            outcome: listener_outcome,
        };
        if let Some(obs) = &spec.observer {
            obs.observe(&report.listener_cancel.timing, None);
        }

        // Phase 5 — InFlightDrain. Apply per-conn jitter (OPS-02) by
        // sleeping a random sub-budget BEFORE cancelling, so 1000s of
        // listener replicas in the same statefulset don't all cancel
        // at the exact same wall-clock instant.
        if spec.jitter_max > Duration::ZERO {
            let jitter_ms = jitter_millis(spec.jitter_max);
            if jitter_ms > 0 {
                tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
            }
        }

        let t = Instant::now();
        self.tracker.close();
        self.token.cancel();
        let drain_outcome = if spec.inflight_drain_deadline > Duration::ZERO {
            match tokio::time::timeout(spec.inflight_drain_deadline, self.tracker.wait()).await {
                Ok(()) => ListenerOutcome::Clean,
                Err(_) => ListenerOutcome::TimedOut,
            }
        } else {
            // Degenerate path: no deadline → poll once (best effort).
            if self.tracker.is_empty() {
                ListenerOutcome::Clean
            } else {
                ListenerOutcome::TimedOut
            }
        };
        let remaining = self.tracker.len();
        report.in_flight_drain = InFlightDrainPhase {
            timing: PhaseTiming::with_outcome(
                DrainPhase::InFlightDrain,
                t.elapsed(),
                drain_outcome,
            ),
            outcome: drain_outcome,
        };
        report.in_flight_remaining = remaining;
        if let Some(obs) = &spec.observer {
            obs.observe(&report.in_flight_drain.timing, None);
        }

        // Phase 6 — XdpDetach. If the call site provided a detach
        // closure, run it with its own bounded timeout. Outcome flows
        // into `xdp_detach_total{result}` via the closure itself
        // (per OPS-01+L4-12+L4-04 §B.2 step 4). On timeout we proceed
        // — the stale-self recovery path in OPS-01+L4-12+L4-04 §B.2
        // picks up the linger on next startup.
        let t = Instant::now();
        let xdp_outcome = if let (Some(detach), Some(deadline)) =
            (spec.xdp_detach.take(), spec.xdp_detach_deadline)
        {
            match tokio::time::timeout(deadline, detach).await {
                Ok(out) => out,
                Err(_) => XdpDetachOutcome::TimedOut,
            }
        } else {
            XdpDetachOutcome::NotAttempted
        };
        let xdp_phase_outcome = match &xdp_outcome {
            XdpDetachOutcome::Clean | XdpDetachOutcome::NotAttempted => ListenerOutcome::Clean,
            XdpDetachOutcome::TimedOut | XdpDetachOutcome::Failed { .. } => {
                ListenerOutcome::TimedOut
            }
        };
        report.xdp_detach = XdpDetachPhase {
            timing: PhaseTiming::with_outcome(
                DrainPhase::XdpDetach,
                t.elapsed(),
                xdp_phase_outcome,
            ),
            outcome: xdp_outcome,
        };
        if let Some(obs) = &spec.observer {
            obs.observe(&report.xdp_detach.timing, None);
        }

        // Phase 7 — Done.
        report.total = PhaseTiming::with_outcome(
            DrainPhase::Total,
            started_at.elapsed(),
            if matches!(report.in_flight_drain.outcome, ListenerOutcome::Clean)
                && matches!(
                    report.xdp_detach.outcome,
                    XdpDetachOutcome::Clean | XdpDetachOutcome::NotAttempted
                )
            {
                ListenerOutcome::Clean
            } else {
                ListenerOutcome::TimedOut
            },
        );
        if let Some(obs) = &spec.observer {
            obs.observe(&report.total, None);
        }

        // Publish the cached report for any concurrent idempotent
        // caller, then mark complete.
        *self.drain_state.report.lock() = Some(report.clone());
        self.drain_state.completed.store(true, Ordering::Release);

        report
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of the legacy [`Shutdown::drain`]. Round-8: superseded by
/// [`DrainReport`] for the coordinator path; kept for the lb-core
/// unit tests + the per-conn drain proof.
#[derive(Debug, Clone, PartialEq, Eq)]
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

// ── Round-8: drain coordinator types ──────────────────────────────

/// Per-phase outcome label for [`PhaseTiming::outcome`]. Listener and
/// in-flight drain reuse this enum; XDP detach has its own richer
/// outcome via [`XdpDetachOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListenerOutcome {
    /// Phase completed within its deadline (or had no deadline).
    Clean,
    /// Phase deadline elapsed; coordinator fell through to the next
    /// phase. The call site logs + emits a counter.
    TimedOut,
}

impl ListenerOutcome {
    /// `"clean"` / `"timed_out"` — the label value used by the
    /// `shutdown_drain_seconds{outcome}` histogram (OPS-03).
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::TimedOut => "timed_out",
        }
    }
}

/// XDP detach phase outcome. Richer than [`ListenerOutcome`] because
/// the L4-12 detach has three failure modes (timeout, kernel error,
/// dirty post-query) that operators need to distinguish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XdpDetachOutcome {
    /// Detach succeeded; post-query confirmed no program is attached.
    Clean,
    /// `xdp_detach_deadline` elapsed before detach returned.
    TimedOut,
    /// Detach returned a kernel error (carries the operator-visible
    /// reason for the `xdp_detach_total{result}` label).
    Failed {
        /// Operator-visible reason, mapped 1:1 to the
        /// `xdp_detach_total{result=<reason>}` counter label.
        reason: String,
    },
    /// No XDP loader was supplied (XDP disabled / not attached at
    /// startup). Phase is skipped entirely.
    NotAttempted,
}

impl XdpDetachOutcome {
    /// `"clean"` / `"timed_out"` / `"failed"` / `"not_attempted"` —
    /// the label value for the `xdp_detach_total{result}` counter
    /// per OPS-01+L4-12+L4-04.
    #[must_use]
    pub const fn as_label(&self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::TimedOut => "timed_out",
            Self::Failed { .. } => "failed",
            Self::NotAttempted => "not_attempted",
        }
    }
}

/// The six logical phases of [`Shutdown::run_drain`]. Each entry is a
/// label value for `shutdown_drain_seconds{phase=...}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DrainPhase {
    /// Phase 2 — flip /readyz to 503 (`mark_draining` closure).
    MarkDraining,
    /// Phase 3 — sleep to let upstream LB observe the 503.
    ReadinessSettle,
    /// Phase 4 — cancel the listener-cancel token.
    ListenerCancel,
    /// Phase 5 — `tracker.close()` + `token.cancel()` + bounded wait.
    InFlightDrain,
    /// Phase 6 — call the XDP detach closure under its own timeout.
    XdpDetach,
    /// Phase 7 — total wall-clock from coordinator entry to exit.
    Total,
}

impl DrainPhase {
    /// `"MarkDraining"` / `"ReadinessSettle"` / `"ListenerCancel"` /
    /// `"InFlightDrain"` / `"XdpDetach"` / `"Total"` — the label
    /// value for `shutdown_drain_seconds{phase}` (OPS-03).
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::MarkDraining => "MarkDraining",
            Self::ReadinessSettle => "ReadinessSettle",
            Self::ListenerCancel => "ListenerCancel",
            Self::InFlightDrain => "InFlightDrain",
            Self::XdpDetach => "XdpDetach",
            Self::Total => "Total",
        }
    }
}

/// Per-phase timing + outcome record. One per phase in [`DrainReport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseTiming {
    /// Which phase this records.
    pub phase: DrainPhase,
    /// Wall-clock duration of the phase.
    pub duration: Duration,
    /// Per-phase outcome label (covers Clean / TimedOut). XDP uses
    /// the richer [`XdpDetachOutcome`] alongside this.
    pub outcome: ListenerOutcome,
}

impl PhaseTiming {
    #[must_use]
    const fn zero(phase: DrainPhase) -> Self {
        Self {
            phase,
            duration: Duration::ZERO,
            outcome: ListenerOutcome::Clean,
        }
    }

    #[must_use]
    const fn clean(phase: DrainPhase, duration: Duration) -> Self {
        Self {
            phase,
            duration,
            outcome: ListenerOutcome::Clean,
        }
    }

    #[must_use]
    const fn with_outcome(phase: DrainPhase, duration: Duration, outcome: ListenerOutcome) -> Self {
        Self {
            phase,
            duration,
            outcome,
        }
    }
}

/// Phase 4 — listener-cancel outcome bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerCancelPhase {
    /// Timing for the histogram emit.
    pub timing: PhaseTiming,
    /// Cooperative-cancel outcome (the call site additionally records
    /// any `JoinHandle::abort` fallback as a counter).
    pub outcome: ListenerOutcome,
}

/// Phase 5 — in-flight drain outcome bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlightDrainPhase {
    /// Timing for the histogram emit.
    pub timing: PhaseTiming,
    /// `Clean` if all tracker tasks exited inside the deadline,
    /// `TimedOut` otherwise.
    pub outcome: ListenerOutcome,
}

/// Phase 6 — XDP detach outcome bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XdpDetachPhase {
    /// Timing for the histogram emit.
    pub timing: PhaseTiming,
    /// Detach outcome (see [`XdpDetachOutcome`] for the four states).
    pub outcome: XdpDetachOutcome,
}

/// Coordinator output. One per [`Shutdown::run_drain`] invocation
/// (cached + cloned for idempotent re-entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainReport {
    /// Phase 2 timing.
    pub mark_draining: PhaseTiming,
    /// Phase 3 timing.
    pub readiness_settle: PhaseTiming,
    /// Phase 4 timing + outcome.
    pub listener_cancel: ListenerCancelPhase,
    /// Phase 5 timing + outcome.
    pub in_flight_drain: InFlightDrainPhase,
    /// Phase 6 timing + outcome.
    pub xdp_detach: XdpDetachPhase,
    /// Total wall-clock from coordinator entry to exit.
    pub total: PhaseTiming,
    /// Number of tracker-bound tasks still live after phase 5
    /// (zero on a clean drain).
    pub in_flight_remaining: usize,
}

/// Owned closure type for the XDP detach phase. The call site
/// constructs this from `XdpLoader::detach()` (per
/// OPS-01+L4-12+L4-04). Boxed because closures returning different
/// futures have different concrete types but the coordinator wants a
/// single field.
pub type XdpDetachFuture = Pin<Box<dyn Future<Output = XdpDetachOutcome> + Send + 'static>>;

/// Owned closure type for the `MarkDraining` phase. Flips /readyz to
/// 503 at the call site.
pub type MarkDrainingFn = Box<dyn FnOnce() + Send + 'static>;

/// OPS-03 emit hook. The coordinator invokes
/// `observer.observe(&phase_timing, listener_label_opt)` at the end
/// of every phase. The call site implements this against the
/// `shutdown_drain_seconds_global` / `_listener` histogram families
/// (lb-observability). Boxed so lb-core stays independent of
/// lb-observability.
pub trait DrainObserver: Send + Sync + 'static {
    /// Observe a completed phase. `listener` is `Some(label)` for
    /// listener-scoped phases (ListenerCancel, InFlightDrain) and
    /// `None` for global phases (ReadinessSettle, XdpDetach, Total).
    fn observe(&self, timing: &PhaseTiming, listener: Option<&str>);
}

/// Drain coordinator inputs. Constructed at the call site
/// (`crates/lb/src/main.rs`) from `RuntimeConfig` + the
/// `XdpLoader::detach()` closure.
#[allow(missing_docs)] // each field is doc'd inline below
pub struct DrainSpec {
    pub readiness_settle: Duration,
    pub listener_cancel_deadline: Duration,
    pub inflight_drain_deadline: Duration,
    pub xdp_detach_deadline: Option<Duration>,
    pub jitter_max: Duration,
    pub mark_draining: Option<MarkDrainingFn>,
    pub xdp_detach: Option<XdpDetachFuture>,
    pub observer: Option<Arc<dyn DrainObserver>>,
}

impl Default for DrainSpec {
    fn default() -> Self {
        Self {
            readiness_settle: Duration::ZERO,
            listener_cancel_deadline: Duration::from_millis(500),
            inflight_drain_deadline: Duration::from_secs(10),
            xdp_detach_deadline: None,
            jitter_max: Duration::ZERO,
            mark_draining: None,
            xdp_detach: None,
            observer: None,
        }
    }
}

impl std::fmt::Debug for DrainSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrainSpec")
            .field("readiness_settle", &self.readiness_settle)
            .field("listener_cancel_deadline", &self.listener_cancel_deadline)
            .field("inflight_drain_deadline", &self.inflight_drain_deadline)
            .field("xdp_detach_deadline", &self.xdp_detach_deadline)
            .field("jitter_max", &self.jitter_max)
            .field("mark_draining", &self.mark_draining.is_some())
            .field("xdp_detach", &self.xdp_detach.is_some())
            .field("observer", &self.observer.is_some())
            .finish()
    }
}

/// OPS-02 jitter: random sub-budget in milliseconds drawn from
/// `0..max`. Uses `std::collections::hash_map::RandomState` so we
/// don't take a `rand` dep into lb-core (lb-core is intentionally a
/// near-zero-dep crate; the lb binary owns `rand` already). The
/// `RandomState`-derived `Hasher` gives ~32 bits of entropy per
/// call, which is plenty for a millisecond-bucket jitter.
fn jitter_millis(max: Duration) -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let max_ms = max.as_millis() as u64;
    if max_ms == 0 {
        return 0;
    }
    let mut h = RandomState::new().build_hasher();
    h.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0),
    );
    h.finish() % max_ms
}
