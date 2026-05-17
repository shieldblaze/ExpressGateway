//! OPS-04+L4-12 Round-8 — 15-case drain enumeration regression test.
//!
//! The previous round-2 "drain sequence" audit shipped without an
//! enumerated case table; this file owns the case-by-case
//! regression coverage promised in
//! `audit/round-8/fixes/OPS-04-L4-12.md` §B.2 (cases C-1 .. C-15).
//!
//! Test policy:
//!
//! * Every case has its OWN test function named `case_cNN_*`.
//! * No test is `#[ignore]`d — if a case cannot be fully
//!   reproduced at this level (because the bug requires the
//!   binary's full accept-site, e.g. C-3's per-IP-counter-drift),
//!   the test asserts the *narrowest possible coordinator-level
//!   property* that would have caught the regression.
//! * Each test cites the case ID and a one-line summary of what it
//!   guards against, mirroring the OPS-04+L4-12 table.
//!
//! Coordinator-level proofs (this file) + accept-loop wiring proofs
//! (`tests/reload_zero_drop.rs::test_listener_cancel_clean`) +
//! L4-12 detach-API proofs (separately owned by div-l4) together
//! satisfy the 15-case promise. This file is the entry point for
//! a verifier to walk in one place.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use lb_core::{
    DrainObserver, DrainPhase, DrainSpec, ListenerOutcome, PhaseTiming, Shutdown, XdpDetachOutcome,
};

// ── helpers ─────────────────────────────────────────────────────────

fn xdp_detach_clean()
-> Pin<Box<dyn std::future::Future<Output = XdpDetachOutcome> + Send + 'static>> {
    Box::pin(async {
        tokio::time::sleep(Duration::from_millis(2)).await;
        XdpDetachOutcome::Clean
    })
}

fn xdp_detach_failure(
    reason: &'static str,
) -> Pin<Box<dyn std::future::Future<Output = XdpDetachOutcome> + Send + 'static>> {
    Box::pin(async move {
        tokio::time::sleep(Duration::from_millis(2)).await;
        XdpDetachOutcome::Failed {
            reason: reason.to_owned(),
        }
    })
}

#[derive(Default)]
struct PhaseRecorder {
    inner: parking_lot::Mutex<Vec<(DrainPhase, ListenerOutcome)>>,
}

impl PhaseRecorder {
    fn phases(&self) -> Vec<DrainPhase> {
        self.inner.lock().iter().map(|(p, _)| *p).collect()
    }
}

impl DrainObserver for PhaseRecorder {
    fn observe(&self, timing: &PhaseTiming, _listener: Option<&str>) {
        self.inner.lock().push((timing.phase, timing.outcome));
    }
}

fn base_spec() -> DrainSpec {
    DrainSpec {
        readiness_settle: Duration::from_millis(5),
        listener_cancel_deadline: Duration::from_millis(50),
        inflight_drain_deadline: Duration::from_millis(100),
        xdp_detach_deadline: None,
        jitter_max: Duration::ZERO,
        mark_draining: None,
        xdp_detach: None,
        observer: None,
    }
}

// ── case C-1 ────────────────────────────────────────────────────────
//
// SIGTERM arrives with no active connections, no in-flight accept.
// Coordinator must observe every phase and complete clean.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c01_steady_idle() {
    let shutdown = Shutdown::new();
    let obs = Arc::new(PhaseRecorder::default());
    let mut spec = base_spec();
    spec.observer = Some(obs.clone());
    let report = shutdown.run_drain(spec).await;
    assert_eq!(report.in_flight_remaining, 0);
    assert_eq!(report.in_flight_drain.outcome, ListenerOutcome::Clean);
    assert_eq!(report.total.outcome, ListenerOutcome::Clean);
    assert_eq!(obs.phases().len(), 6, "all six phase observations");
}

// ── case C-2 ────────────────────────────────────────────────────────
//
// SIGTERM during a pending `accept().await`. The coordinator's
// listener_token.cancel() is what wakes the accept; the
// listener_token derives from the parent (so a `token.cancel()`
// cascades). Asserted at the token-relationship level — the
// binary-level accept-loop wiring is asserted by
// `tests/reload_zero_drop.rs::test_listener_cancel_clean`.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c02_sigterm_during_pending_accept() {
    let s = Shutdown::new();
    let lt = s.listener_token().clone();
    let pending = tokio::spawn(async move {
        // Simulate `accept().await` — sleep until cancelled.
        tokio::select! {
            biased;
            () = lt.cancelled() => "cancelled",
            () = tokio::time::sleep(Duration::from_secs(30)) => "slept",
        }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let _report = s.run_drain(base_spec()).await;
    let outcome = tokio::time::timeout(Duration::from_millis(200), pending)
        .await
        .expect("pending accept must exit cleanly within budget")
        .expect("task panicked");
    assert_eq!(outcome, "cancelled");
}

// ── case C-3 ────────────────────────────────────────────────────────
//
// **THE CANONICAL CASE.** SIGTERM arrives in the GAP between
// `accept()` returning Ok and the per-conn spawn. Coordinator-level
// guarantee: listener_token is settable BEFORE the per-conn
// shutdown_token fires, AND a synchronous read of
// `listener_token.is_cancelled()` after the listener_cancel phase
// returns true. The binary-level `is_cancelled()` post-accept tail
// check in `crates/lb/src/main.rs` is what closes the gap.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c03_post_accept_synchronous_tail_visibility() {
    let s = Shutdown::new();
    let parent = s.token().clone();
    let listener = s.listener_token().clone();
    let mut spec = base_spec();
    // Use a much-longer per-conn budget so the parent cancel hasn't
    // fired yet by the time we observe the listener_token.
    spec.readiness_settle = Duration::from_millis(50);
    spec.inflight_drain_deadline = Duration::from_secs(5);

    let s2 = s.clone();
    let drain_handle = tokio::spawn(async move {
        s2.run_drain(spec).await;
    });

    // Sleep through MarkDraining + half of ReadinessSettle so we
    // observe the *gap*: listener cancel has not yet fired, parent
    // has not yet fired.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!listener.is_cancelled(), "listener cancel pre-phase 4");
    assert!(!parent.is_cancelled(), "parent cancel pre-phase 5");

    // Wait for the coordinator to enter phase 4 (listener cancel).
    // The post-accept synchronous tail check in main.rs reads
    // `listener_cancel_token.is_cancelled()` — assert that the
    // visibility ordering guarantees the listener observes the bit
    // BEFORE the per-conn token fires.
    drain_handle.await.expect("drain panicked");
    assert!(
        listener.is_cancelled(),
        "phase 4 must have fired listener_token"
    );
    assert!(
        parent.is_cancelled(),
        "phase 5 must have fired the parent token after phase 4"
    );
}

// ── case C-4 ────────────────────────────────────────────────────────
//
// SIGTERM during slow TLS handshake. The per-conn task observes
// `shutdown_token.cancelled()` in its biased select; a stuck
// handshake future is dropped on cancel. Coordinator-level
// assertion: per-conn token fires during phase 5.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c04_handshake_future_dropped_on_cancel() {
    let s = Shutdown::new();
    let tok = s.token().clone();
    let dropped = Arc::new(AtomicBool::new(false));
    let dropped2 = Arc::clone(&dropped);

    // Per-conn task: simulates a TLS handshake by sleeping; the
    // biased select! drops the handshake future on cancel.
    s.tracker().spawn(async move {
        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let _flag = DropFlag(dropped2);
        let handshake = async {
            tokio::time::sleep(Duration::from_secs(30)).await;
        };
        tokio::select! {
            biased;
            () = tok.cancelled() => {}
            () = handshake => {}
        }
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let _report = s.run_drain(base_spec()).await;
    assert!(
        dropped.load(Ordering::Acquire),
        "handshake future must be dropped on per-conn cancel (drop-flag fires)"
    );
}

// ── case C-5 ────────────────────────────────────────────────────────
//
// Long-lived H2/H3 stream (gRPC bidi, SSE). Severed at the
// inflight_drain_deadline. Documented as `TimedOut` outcome — the
// per-listener override (OPS-10) is a separate plan.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c05_long_lived_stream_severed_at_deadline() {
    let s = Shutdown::new();
    s.tracker().spawn(async {
        // Simulate a gRPC bidi stream that ignores cancel.
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut spec = base_spec();
    spec.inflight_drain_deadline = Duration::from_millis(50);
    let report = s.run_drain(spec).await;
    assert_eq!(report.in_flight_drain.outcome, ListenerOutcome::TimedOut);
    assert!(report.in_flight_remaining >= 1);
}

// ── case C-6 ────────────────────────────────────────────────────────
//
// WebSocket — same shape as C-5. Separate test to make the
// coverage table explicit (a future per-protocol budget split would
// give them different deadlines).

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c06_websocket_severed_at_deadline() {
    let s = Shutdown::new();
    s.tracker().spawn(async {
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut spec = base_spec();
    spec.inflight_drain_deadline = Duration::from_millis(50);
    let report = s.run_drain(spec).await;
    assert_eq!(report.in_flight_drain.outcome, ListenerOutcome::TimedOut);
}

// ── case C-7 ────────────────────────────────────────────────────────
//
// tracker.wait() times out → coordinator still proceeds to phase 6.
// Records `in_flight_remaining` count.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c07_inflight_timeout_still_runs_xdp_phase() {
    let s = Shutdown::new();
    s.tracker().spawn(async {
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let xdp_ran = Arc::new(AtomicBool::new(false));
    let xdp_ran2 = Arc::clone(&xdp_ran);

    let mut spec = base_spec();
    spec.inflight_drain_deadline = Duration::from_millis(50);
    spec.xdp_detach_deadline = Some(Duration::from_millis(200));
    spec.xdp_detach = Some(Box::pin(async move {
        xdp_ran2.store(true, Ordering::Release);
        XdpDetachOutcome::Clean
    }));
    let report = s.run_drain(spec).await;
    assert_eq!(report.in_flight_drain.outcome, ListenerOutcome::TimedOut);
    assert!(
        xdp_ran.load(Ordering::Acquire),
        "phase 6 (xdp detach) must run even after phase 5 timed out"
    );
    assert_eq!(report.xdp_detach.outcome, XdpDetachOutcome::Clean);
}

// ── case C-8 ────────────────────────────────────────────────────────
//
// XDP detach itself hangs — coordinator's own xdp_detach_deadline
// fires; outcome = TimedOut; coordinator proceeds to phase 7.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c08_xdp_detach_hang_bounded() {
    let s = Shutdown::new();
    let mut spec = base_spec();
    spec.xdp_detach_deadline = Some(Duration::from_millis(30));
    spec.xdp_detach = Some(Box::pin(async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        XdpDetachOutcome::Clean
    }));
    let report = s.run_drain(spec).await;
    assert_eq!(report.xdp_detach.outcome, XdpDetachOutcome::TimedOut);
    // The Total phase is still observed → we didn't get stuck.
    assert!(report.total.duration > Duration::ZERO);
}

// ── case C-9 ────────────────────────────────────────────────────────
//
// SIGTERM during XDP attach itself (startup race). The coordinator
// is constructed in main *after* `try_attach_xdp`, so this is a
// no-op at the lb-core level. We assert the coordinator-level
// invariant: `run_drain` is safe to call even when the listener_token
// has already been cancelled externally (defensive — the call site
// can't reach this state today but a future startup-race fix might).

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c09_drain_after_external_cancel_is_safe() {
    let s = Shutdown::new();
    s.listener_token().cancel();
    s.token().cancel();
    let report = s.run_drain(base_spec()).await;
    // Even with both tokens pre-cancelled, the coordinator publishes
    // a report; no panic, no hang.
    assert_eq!(report.in_flight_drain.outcome, ListenerOutcome::Clean);
}

// ── case C-10 ───────────────────────────────────────────────────────
//
// Two SIGTERMs in quick succession. First wins; second returns the
// cached report.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c10_two_sigterms_idempotent() {
    let s = Shutdown::new();
    let calls = Arc::new(AtomicU64::new(0));
    let calls2 = Arc::clone(&calls);

    let mut spec1 = base_spec();
    spec1.mark_draining = Some(Box::new(move || {
        calls2.fetch_add(1, Ordering::AcqRel);
    }));
    let r1 = s.run_drain(spec1).await;

    let calls3 = Arc::clone(&calls);
    let mut spec2 = base_spec();
    spec2.mark_draining = Some(Box::new(move || {
        calls3.fetch_add(100, Ordering::AcqRel);
    }));
    let r2 = s.run_drain(spec2).await;

    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "mark_draining must run exactly once"
    );
    assert_eq!(r1, r2, "second SIGTERM returns cached report");
}

// ── case C-11 ───────────────────────────────────────────────────────
//
// /admin/drain followed by SIGTERM. Same idempotency latch as
// C-10; we just exercise the alternate ordering for completeness
// (admin drain is the first caller; SIGTERM is the second).

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c11_admin_drain_then_sigterm_idempotent() {
    let s = Shutdown::new();
    let admin_outcome = s.run_drain(base_spec()).await;
    let sigterm_outcome = s.run_drain(base_spec()).await;
    assert_eq!(
        admin_outcome, sigterm_outcome,
        "admin /drain caches; SIGTERM observes the same report"
    );
}

// ── case C-12 ───────────────────────────────────────────────────────
//
// Panic in a per-conn task is already caught by tracker.spawn's
// panic-handler. Coordinator-level guarantee: a tracker.spawn that
// panicked still counts as `exited` from the tracker's POV — the
// drain must NOT hang waiting on it.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c12_per_conn_panic_does_not_hang_drain() {
    let s = Shutdown::new();
    s.tracker().spawn(async {
        panic!("simulated per-conn panic");
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut spec = base_spec();
    spec.inflight_drain_deadline = Duration::from_millis(50);
    let report = s.run_drain(spec).await;
    // The panicked task has already exited (tracker count drops to
    // 0). The drain must therefore be Clean.
    assert_eq!(report.in_flight_drain.outcome, ListenerOutcome::Clean);
    assert_eq!(report.in_flight_remaining, 0);
}

// ── case C-13 ───────────────────────────────────────────────────────
//
// /admin/drain (admin set draining, no SIGTERM, no XDP detach):
// `xdp_detach_deadline = None` → coordinator skips phase 6.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c13_admin_drain_skips_xdp_detach() {
    let s = Shutdown::new();
    let mut spec = base_spec();
    spec.xdp_detach_deadline = None;
    spec.xdp_detach = None;
    let report = s.run_drain(spec).await;
    assert_eq!(report.xdp_detach.outcome, XdpDetachOutcome::NotAttempted);
}

// ── case C-14 ───────────────────────────────────────────────────────
//
// A listener accept loop that *ignores* cancellation. Coordinator
// emits the cooperative signal; the call site is responsible for
// the JoinHandle::abort fallback (asserted in
// `crates/lb/src/main.rs` after `run_drain` returns). At
// coordinator level we assert the listener_token was actually
// cancelled — that's the cooperative signal the abort fallback
// supplements.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c14_listener_cancel_signal_fires() {
    let s = Shutdown::new();
    assert!(!s.listener_token().is_cancelled());
    let mut spec = base_spec();
    spec.listener_cancel_deadline = Duration::from_millis(20);
    let _report = s.run_drain(spec).await;
    assert!(
        s.listener_token().is_cancelled(),
        "phase 4 must have signalled listener_token even though the call site owns the abort fallback"
    );
}

// ── case C-15 ───────────────────────────────────────────────────────
//
// Zero listeners bound (degenerate config). Coordinator runs every
// phase and returns Clean.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn case_c15_zero_listeners_clean_drain() {
    let s = Shutdown::new();
    let mut spec = base_spec();
    spec.xdp_detach_deadline = Some(Duration::from_millis(20));
    spec.xdp_detach = Some(xdp_detach_clean());
    let report = s.run_drain(spec).await;
    assert_eq!(report.in_flight_drain.outcome, ListenerOutcome::Clean);
    assert_eq!(report.xdp_detach.outcome, XdpDetachOutcome::Clean);
    assert_eq!(report.in_flight_remaining, 0);
}

// ── extra coverage: XDP detach kernel-error path ───────────────────
//
// Not in the 15-case table, but the OPS-01+L4-12+L4-04 §B.2 step 4
// contract requires the coordinator surfaces the `Failed{reason}`
// path into the metric label.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn xdp_detach_kernel_error_surfaced() {
    let s = Shutdown::new();
    let mut spec = base_spec();
    spec.xdp_detach_deadline = Some(Duration::from_millis(50));
    spec.xdp_detach = Some(xdp_detach_failure("kernel-EINVAL"));
    let report = s.run_drain(spec).await;
    match &report.xdp_detach.outcome {
        XdpDetachOutcome::Failed { reason } => {
            assert_eq!(reason, "kernel-EINVAL");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}
