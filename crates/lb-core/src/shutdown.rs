//! Process-wide graceful drain: a [`CancellationToken`] plus a [`TaskTracker`] of every opted-in
//! spawn site. `TaskTracker` NOT `JoinSet`: per-connection handlers spawn their own helper futures
//! that must be tracked alongside the parent, with no accept loop to hold the handles.
//! [`Shutdown::run_drain`] is the IDEMPOTENT coordinator (C-10 / C-11); `drain` is a legacy shim.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Cloneable graceful-drain handle; every long-lived spawn site clones it and uses `tracker()`.
#[derive(Clone, Debug)]
pub struct Shutdown {
    token: CancellationToken,
    tracker: TaskTracker,
    /// Listener-cancel token, a CHILD of `token`, so listeners stop without cancelling per-conn tasks.
    listener_token: CancellationToken,
    /// Idempotency latch + first-call report cache; only the first caller runs the phases.
    drain_state: Arc<DrainState>,
}

#[derive(Debug, Default)]
struct DrainState {
    started: AtomicBool,
    completed: AtomicBool,
    report: Mutex<Option<DrainReport>>,
}

impl Shutdown {
    /// Fresh handle; token un-cancelled, tracker un-closed.
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

    /// The cancellation token; poll it FIRST in a `biased;` select arm.
    #[must_use]
    pub const fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Listener-cancel token. Accept loops MUST select on this, not [`Self::token`], or stopping
    /// accepts also cancels in-flight connections.
    #[must_use]
    pub const fn listener_token(&self) -> &CancellationToken {
        &self.listener_token
    }

    /// Spawn long-lived tasks through this or they are invisible to the drain.
    #[must_use]
    pub const fn tracker(&self) -> &TaskTracker {
        &self.tracker
    }

    /// Per-subsystem child handle: own per-conn token, SHARED tracker, listener token and state.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
            tracker: self.tracker.clone(),
            listener_token: self.listener_token.clone(),
            drain_state: Arc::clone(&self.drain_state),
        }
    }

    /// Legacy drain shim over [`Self::run_drain`]: NO readiness settle, listener-cancel or XDP phase.
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

    /// The drain coordinator; returns per-phase durations and listener/XDP outcomes. IDEMPOTENT —
    /// a second call returns the cached report (C-10 two SIGTERMs, C-11 admin-drain-then-SIGTERM).
    pub async fn run_drain(&self, mut spec: DrainSpec) -> DrainReport {
        // CAS + sleep-loop rather than a notifier: expected concurrency here is at most 2.
        if self
            .drain_state
            .started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            while !self.drain_state.completed.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // `completed` is set only AFTER `report` is populated, so the None arm is unreachable;
            // it stubs rather than panics because the crate denies `expect_used`.
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

        // C-12 CONTRACT: a panic mid-drain must still detach XDP. The closure cannot cross the
        // panic boundary, so the CALL SITE must scopeguard `run_drain` with its own detach.

        let t = Instant::now();
        if let Some(mark) = spec.mark_draining.take() {
            (mark)();
        }
        report.mark_draining = PhaseTiming::clean(DrainPhase::MarkDraining, t.elapsed());
        if let Some(obs) = &spec.observer {
            obs.observe(&report.mark_draining, None);
        }

        let t = Instant::now();
        if spec.readiness_settle > Duration::ZERO {
            tokio::time::sleep(spec.readiness_settle).await;
        }
        report.readiness_settle = PhaseTiming::clean(DrainPhase::ReadinessSettle, t.elapsed());
        if let Some(obs) = &spec.observer {
            obs.observe(&report.readiness_settle, None);
        }

        // Signal only; the bounded wait is phase 5. A `Duration::ZERO` deadline disables it.
        let t = Instant::now();
        let listener_outcome = if spec.listener_cancel_deadline > Duration::ZERO {
            self.listener_token.cancel();
            // An OBSERVATION, not a forced abort — this coordinator owns no JoinHandles.
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

        // Jitter BEFORE cancelling, or every replica cancels at the same wall-clock instant.
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

        // A detach timeout PROCEEDS: stale-self recovery picks the lingering program up next boot.
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

        // Publish BEFORE marking complete — the latch above depends on that order.
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

/// Outcome of the legacy [`Shutdown::drain`]; the coordinator returns [`DrainReport`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainOutcome {
    /// All tracked tasks exited within the deadline.
    Clean,
    /// Deadline elapsed with `remaining` tasks live; the caller warns and best-effort aborts.
    TimedOut {
        /// Tracker-bound tasks still live at the deadline.
        remaining: usize,
    },
}

/// Per-phase outcome; XDP detach uses the richer [`XdpDetachOutcome`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListenerOutcome {
    /// Phase completed within its deadline (or had no deadline).
    Clean,
    /// Deadline elapsed; the coordinator fell through to the next phase.
    TimedOut,
}

impl ListenerOutcome {
    /// Label value for `shutdown_drain_seconds{outcome}`.
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::TimedOut => "timed_out",
        }
    }
}

/// XDP detach outcome; operators must distinguish timeout, kernel error and dirty post-query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XdpDetachOutcome {
    /// Detach succeeded; post-query confirmed no program is attached.
    Clean,
    /// `xdp_detach_deadline` elapsed before detach returned.
    TimedOut,
    /// Kernel error, carrying the `xdp_detach_total{result}` label value.
    Failed {
        /// The `xdp_detach_total{result}` label value.
        reason: String,
    },
    /// No loader supplied; the phase is skipped.
    NotAttempted,
}

impl XdpDetachOutcome {
    /// Label value for `xdp_detach_total{result}`.
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

/// Phases of [`Shutdown::run_drain`]; each is a `shutdown_drain_seconds{phase}` label value.
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
    /// Label value for `shutdown_drain_seconds{phase}`.
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

/// One phase's timing and outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseTiming {
    /// Which phase this records.
    pub phase: DrainPhase,
    /// Wall-clock duration of the phase.
    pub duration: Duration,
    /// Clean / TimedOut; XDP carries [`XdpDetachOutcome`] alongside.
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
    /// Cooperative-cancel outcome; abort fallbacks are counted at the call site.
    pub outcome: ListenerOutcome,
}

/// Phase 5 — in-flight drain outcome bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlightDrainPhase {
    /// Timing for the histogram emit.
    pub timing: PhaseTiming,
    /// Whether every tracker task exited inside the deadline.
    pub outcome: ListenerOutcome,
}

/// Phase 6 — XDP detach outcome bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XdpDetachPhase {
    /// Timing for the histogram emit.
    pub timing: PhaseTiming,
    /// Detach outcome.
    pub outcome: XdpDetachOutcome,
}

/// Coordinator output, cached and cloned for idempotent re-entry.
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
    /// Tracker-bound tasks still live after phase 5.
    pub in_flight_remaining: usize,
}

/// XDP detach closure, boxed so differing future types fit one field.
pub type XdpDetachFuture = Pin<Box<dyn Future<Output = XdpDetachOutcome> + Send + 'static>>;

/// `MarkDraining` closure; flips /readyz to 503.
pub type MarkDrainingFn = Box<dyn FnOnce() + Send + 'static>;

/// Per-phase emit hook. Boxed so lb-core takes no dependency on lb-observability.
pub trait DrainObserver: Send + Sync + 'static {
    /// Observe a phase; `listener` is `Some` only for listener-scoped phases.
    fn observe(&self, timing: &PhaseTiming, listener: Option<&str>);
}

/// Drain coordinator inputs.
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

/// Random jitter in `0..max` ms; `RandomState` not `rand` keeps lb-core near-zero-dep.
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
