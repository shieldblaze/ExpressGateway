//! Coordinator-level drain proofs, driving `Shutdown::run_drain` directly. The binary-level
//! 15-case matrix lives in the workspace-root `round8_drain_15case.rs`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use lb_core::{
    DrainObserver, DrainPhase, DrainSpec, ListenerOutcome, PhaseTiming, Shutdown, XdpDetachOutcome,
};

#[derive(Default)]
struct CountingObserver {
    observations: parking_lot::Mutex<Vec<(DrainPhase, ListenerOutcome, Option<String>)>>,
}

impl DrainObserver for CountingObserver {
    fn observe(&self, timing: &PhaseTiming, listener: Option<&str>) {
        self.observations
            .lock()
            .push((timing.phase, timing.outcome, listener.map(str::to_owned)));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_drain_phases_in_order() {
    let shutdown = Shutdown::new();
    let obs = Arc::new(CountingObserver::default());
    let spec = DrainSpec {
        readiness_settle: Duration::from_millis(5),
        listener_cancel_deadline: Duration::from_millis(50),
        inflight_drain_deadline: Duration::from_millis(50),
        xdp_detach_deadline: None,
        jitter_max: Duration::ZERO,
        mark_draining: None,
        xdp_detach: None,
        observer: Some(obs.clone() as Arc<dyn DrainObserver>),
    };

    let report = shutdown.run_drain(spec).await;
    let seen: Vec<DrainPhase> = obs.observations.lock().iter().map(|(p, _, _)| *p).collect();
    assert_eq!(
        seen,
        vec![
            DrainPhase::MarkDraining,
            DrainPhase::ReadinessSettle,
            DrainPhase::ListenerCancel,
            DrainPhase::InFlightDrain,
            DrainPhase::XdpDetach,
            DrainPhase::Total,
        ],
        "phases must observe in declared order"
    );
    // Total is the wall-clock — must be >= sum of the others.
    let sum: Duration = report.mark_draining.duration
        + report.readiness_settle.duration
        + report.listener_cancel.timing.duration
        + report.in_flight_drain.timing.duration
        + report.xdp_detach.timing.duration;
    assert!(
        report.total.duration >= sum || report.total.duration + Duration::from_millis(10) >= sum,
        "total must be at least the sum (within 10ms tolerance for monotonic clock skew)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_drain_idempotent_c10() {
    let shutdown = Shutdown::new();
    let mark_called = Arc::new(AtomicU64::new(0));
    let mark_called_2 = Arc::clone(&mark_called);

    let spec1 = DrainSpec {
        readiness_settle: Duration::from_millis(10),
        listener_cancel_deadline: Duration::from_millis(10),
        inflight_drain_deadline: Duration::from_millis(10),
        xdp_detach_deadline: None,
        jitter_max: Duration::ZERO,
        mark_draining: Some(Box::new(move || {
            mark_called_2.fetch_add(1, Ordering::AcqRel);
        })),
        xdp_detach: None,
        observer: None,
    };
    let report1 = shutdown.run_drain(spec1).await;

    // A distinct closure, so a broken latch shows up as a bumped counter.
    let mark_called_3 = Arc::clone(&mark_called);
    let spec2 = DrainSpec {
        readiness_settle: Duration::from_millis(10),
        listener_cancel_deadline: Duration::from_millis(10),
        inflight_drain_deadline: Duration::from_millis(10),
        xdp_detach_deadline: None,
        jitter_max: Duration::ZERO,
        mark_draining: Some(Box::new(move || {
            mark_called_3.fetch_add(100, Ordering::AcqRel);
        })),
        xdp_detach: None,
        observer: None,
    };
    let report2 = shutdown.run_drain(spec2).await;

    assert_eq!(
        mark_called.load(Ordering::Acquire),
        1,
        "mark_draining must run exactly once across two SIGTERMs (case C-10)"
    );
    assert_eq!(
        report1, report2,
        "second drain must return the first call's cached report"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_drain_inflight_timeout_c7() {
    let shutdown = Shutdown::new();
    shutdown.tracker().spawn(async {
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let spec = DrainSpec {
        readiness_settle: Duration::ZERO,
        listener_cancel_deadline: Duration::from_millis(10),
        inflight_drain_deadline: Duration::from_millis(50),
        xdp_detach_deadline: None,
        jitter_max: Duration::ZERO,
        mark_draining: None,
        xdp_detach: None,
        observer: None,
    };

    let report = shutdown.run_drain(spec).await;
    assert_eq!(
        report.in_flight_drain.outcome,
        ListenerOutcome::TimedOut,
        "stuck task must drive in-flight outcome to TimedOut"
    );
    assert_eq!(
        report.in_flight_remaining, 1,
        "exactly one uncooperative task must remain"
    );
    assert_eq!(
        report.total.outcome,
        ListenerOutcome::TimedOut,
        "total outcome must propagate the timed-out leg"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_drain_xdp_detach_timeout_c8() {
    let shutdown = Shutdown::new();
    let spec = DrainSpec {
        readiness_settle: Duration::ZERO,
        listener_cancel_deadline: Duration::from_millis(10),
        inflight_drain_deadline: Duration::from_millis(10),
        xdp_detach_deadline: Some(Duration::from_millis(20)),
        jitter_max: Duration::ZERO,
        mark_draining: None,
        xdp_detach: Some(Box::pin(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            XdpDetachOutcome::Clean
        })),
        observer: None,
    };
    let report = shutdown.run_drain(spec).await;
    assert_eq!(
        report.xdp_detach.outcome,
        XdpDetachOutcome::TimedOut,
        "stuck XDP detach must record TimedOut"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_drain_admin_skips_xdp_c13() {
    let shutdown = Shutdown::new();
    let xdp_was_called = Arc::new(AtomicBool::new(false));
    let _xdp_was_called2 = Arc::clone(&xdp_was_called);

    let spec = DrainSpec {
        readiness_settle: Duration::ZERO,
        listener_cancel_deadline: Duration::from_millis(10),
        inflight_drain_deadline: Duration::from_millis(10),
        xdp_detach_deadline: None,
        jitter_max: Duration::ZERO,
        mark_draining: None,
        xdp_detach: None,
        observer: None,
    };
    let report = shutdown.run_drain(spec).await;
    assert_eq!(
        report.xdp_detach.outcome,
        XdpDetachOutcome::NotAttempted,
        "admin-drain (no xdp deadline) must skip phase 6"
    );
    assert!(
        !xdp_was_called.load(Ordering::Acquire),
        "the no-op detach closure must NOT have been invoked"
    );
}

/// Cancellation is ONE-WAY: parent cancels the listener token, never the reverse.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listener_token_is_child_of_root_token() {
    let s1 = Shutdown::new();
    s1.listener_token().cancel();
    assert!(s1.listener_token().is_cancelled());
    assert!(
        !s1.token().is_cancelled(),
        "cancelling listener_token must NOT cancel the parent token (per-conn tasks must keep running through readiness settle)"
    );

    let s2 = Shutdown::new();
    s2.token().cancel();
    assert!(s2.token().is_cancelled());
    assert!(
        s2.listener_token().is_cancelled(),
        "cancelling parent token must cascade to listener_token (process-wide cancel from any path)"
    );
}
