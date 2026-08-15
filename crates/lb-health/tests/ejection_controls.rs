//! G5 negative controls at the POLICY level: threshold, floor, re-admission.
//!
//! Every test here names, in a comment, the pre-fix (or naive-fix) behaviour it catches. A control
//! that cannot fail is worthless, so where a case would pass trivially against the pre-fix build it
//! says so and states which WRONG implementation it is aimed at instead.
//!
//! Timings use a deliberately short `base_ejection` (50 ms) rather than an injected clock: the
//! ejection window IS the knob, so driving it through the real policy exercises the production
//! path instead of a test-only seam.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::time::Duration;

use lb_health::ejection::AdmissionGate;
use lb_health::{AttemptOutcome, EjectionPolicy, HealthRegistry, HealthStatus, UpstreamErrorClass};

const BASE_EJECTION: Duration = Duration::from_millis(50);

fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// Production defaults except for a short ejection window, so re-admission is testable in-process.
fn policy() -> EjectionPolicy {
    EjectionPolicy {
        base_ejection: BASE_EJECTION,
        max_ejection: Duration::from_millis(400),
        ..EjectionPolicy::default()
    }
}

fn fail_n(reg: &HealthRegistry, a: SocketAddr, n: u32) {
    for _ in 0..n {
        reg.record(a, UpstreamErrorClass::Transport.outcome());
    }
}

/// CONTROL (i) — a consistently failing backend IS ejected.
///
/// PRE-FIX BEHAVIOUR CAUGHT: `record_failure` had no production caller and no admission gate
/// existed, so nothing was ever removed from rotation. `admits()` would have been unconditionally
/// true.
#[test]
fn failing_backend_is_ejected() {
    let p = policy();
    let reg = HealthRegistry::new(p, &[addr(1), addr(2), addr(3)]);

    fail_n(&reg, addr(1), p.consecutive_failures);

    assert!(!reg.admits(addr(1)), "the failing backend must be ejected");
    assert!(
        reg.admits(addr(2)) && reg.admits(addr(3)),
        "peers unaffected"
    );
    assert_eq!(reg.status(addr(1)), HealthStatus::Unhealthy);
    assert_eq!(reg.ejections_total(), 1);
    assert_eq!(reg.ejected_count(), 1);
}

/// CONTROL (ii) — a recovered backend IS re-admitted, through the half-open probe.
///
/// PRE-FIX BEHAVIOUR CAUGHT: no ejection at all. NAIVE-FIX BEHAVIOUR CAUGHT: an ejection with no
/// timer and no success path is PERMANENT — once a backend is filtered out it can never receive
/// the request that would prove it healthy again, so a transient blip kills it forever. That is
/// strictly worse than no ejection, and the second half of this test is what detects it.
#[test]
fn recovered_backend_is_readmitted() {
    let p = policy();
    let reg = HealthRegistry::new(p, &[addr(1), addr(2), addr(3)]);
    fail_n(&reg, addr(1), p.consecutive_failures);
    assert!(!reg.admits(addr(1)));

    // NON-VACUITY: before the deadline it must still be OUT, or "re-admission" below would just be
    // "ejection never worked".
    std::thread::sleep(BASE_EJECTION / 5);
    assert!(
        !reg.admits(addr(1)),
        "must stay ejected until the window elapses"
    );

    // Past the window it is admitted again as a half-open probe...
    std::thread::sleep(BASE_EJECTION);
    assert!(reg.admits(addr(1)), "half-open probe must be admitted");
    assert_eq!(
        reg.ejected_count(),
        0,
        "a half-open backend is receiving traffic, so it is not counted as ejected"
    );

    // ...and one success clears the ejection outright.
    reg.record(addr(1), AttemptOutcome::Success);
    assert!(reg.admits(addr(1)));
    assert_eq!(reg.status(addr(1)), HealthStatus::Healthy);
    assert_eq!(reg.readmissions_total(), 1);
}

/// A failed half-open probe must re-eject with a LONGER window, not flap at the base interval.
#[test]
fn failed_half_open_probe_backs_off() {
    let p = policy();
    let reg = HealthRegistry::new(p, &[addr(1), addr(2), addr(3)]);
    fail_n(&reg, addr(1), p.consecutive_failures);
    std::thread::sleep(BASE_EJECTION * 2);
    assert!(reg.admits(addr(1)), "half-open");

    reg.record(addr(1), UpstreamErrorClass::Timeout.outcome());
    assert!(!reg.admits(addr(1)), "a failed probe re-ejects");
    assert_eq!(reg.ejections_total(), 2);

    // The second window is 2x the base, so sleeping one base interval is NOT enough.
    std::thread::sleep(BASE_EJECTION + BASE_EJECTION / 4);
    assert!(
        !reg.admits(addr(1)),
        "the backoff must exceed the base window"
    );
}

/// CONTROL (iii) — all backends failing does NOT eject everything; the floor holds.
///
/// PRE-FIX BEHAVIOUR CAUGHT: none — pre-fix nothing ejects, so this passes trivially there.
/// It exists to catch the NAIVE FIX: per-backend ejection with no floor, which turns a correlated
/// outage (bad deploy, shared database down, network partition) into a listener that ejects every
/// backend and serves nothing. Serving degraded beats serving nothing.
#[test]
fn all_backends_failing_does_not_eject_everything() {
    let p = policy();
    let backends = [addr(1), addr(2), addr(3)];
    let reg = HealthRegistry::new(p, &backends);

    for _ in 0..20 {
        for b in &backends {
            reg.record(*b, UpstreamErrorClass::Transport.outcome());
        }
    }

    // 3 backends at min_healthy_percent=50 → floor(3 * 50 / 100) = 1 may be ejected.
    assert_eq!(
        reg.ejected_count(),
        1,
        "the floor caps ejections at 50% of 3 backends"
    );
    let admitted = backends.iter().filter(|b| reg.admits(**b)).count();
    assert_eq!(admitted, 2, "two backends must remain in rotation");
    assert!(
        reg.ejections_suppressed_total() > 0,
        "a suppressed ejection must be COUNTED, never silent"
    );
}

/// The absolute floor: a single-backend listener can never eject its only backend, whatever the
/// percentage says — including `min_healthy_percent = 0`.
#[test]
fn single_backend_listener_never_ejects() {
    for pct in [0_u8, 50, 100] {
        let p = EjectionPolicy {
            min_healthy_percent: pct,
            ..policy()
        };
        let reg = HealthRegistry::new(p, &[addr(1)]);
        fail_n(&reg, addr(1), 50);
        assert!(
            reg.admits(addr(1)),
            "min_healthy_percent={pct} must still not empty a 1-backend listener"
        );
        assert_eq!(reg.ejected_count(), 0);
    }
}

/// CONTROL (iv) — a single transient error does NOT eject; the threshold holds.
///
/// PRE-FIX BEHAVIOUR CAUGHT: none directly. This is aimed at the NAIVE FIX that ejects on the first
/// error — a single RST during a rolling backend restart would then pull a healthy backend out of
/// rotation. It pins the exact boundary rather than asserting "eventually ejects".
#[test]
fn single_transient_error_does_not_eject() {
    let p = policy();
    let reg = HealthRegistry::new(p, &[addr(1), addr(2), addr(3)]);

    reg.record(addr(1), UpstreamErrorClass::Transport.outcome());
    assert!(reg.admits(addr(1)), "one error must not eject");
    assert_ne!(reg.status(addr(1)), HealthStatus::Unhealthy);
    assert_eq!(reg.ejections_total(), 0);

    // Exactly one short of the threshold: still in.
    fail_n(&reg, addr(1), p.consecutive_failures - 2);
    assert!(
        reg.admits(addr(1)),
        "threshold-1 consecutive failures must not eject"
    );

    // The threshold-th failure is the one that ejects.
    reg.record(addr(1), UpstreamErrorClass::Transport.outcome());
    assert!(!reg.admits(addr(1)));
}

/// An interleaved success resets the streak, so intermittent errors below the rate never eject.
#[test]
fn interleaved_success_resets_the_streak() {
    let p = policy();
    let reg = HealthRegistry::new(p, &[addr(1), addr(2), addr(3)]);
    for _ in 0..10 {
        fail_n(&reg, addr(1), p.consecutive_failures - 1);
        reg.record(addr(1), AttemptOutcome::Success);
    }
    assert!(reg.admits(addr(1)));
    assert_eq!(reg.ejections_total(), 0);
}

/// A client-fault error must not count against the backend, and must not clear a real streak
/// either — it is discarded, not scored.
#[test]
fn client_faults_are_not_charged_to_the_backend() {
    let p = policy();
    let reg = HealthRegistry::new(p, &[addr(1), addr(2), addr(3)]);

    for _ in 0..50 {
        reg.record(addr(1), UpstreamErrorClass::ClientRequest.outcome());
        reg.record(addr(1), UpstreamErrorClass::Misconfigured.outcome());
    }
    assert!(
        reg.admits(addr(1)),
        "malformed client requests must never eject a backend"
    );
    assert_eq!(reg.ejections_total(), 0);
}

/// A burst of unattributable pooled-send failures must NOT eject.
///
/// NAIVE-FIX BEHAVIOUR CAUGHT: charging `Http2PoolError::Send` to the backend. `Http2Pool` hands out
/// a cached sender without saying it was reused, so a connection the peer closed while idle fails
/// exactly like a backend refusing work. `N` concurrent requests sharing one stale sender fail
/// TOGETHER — correlated, not independent — so the consecutive-failure threshold is no protection:
/// `N >= consecutive_failures` ejects a healthy backend for the full window on OUR race. That is the
/// "worse than nothing" mode the floor and threshold exist to prevent.
#[test]
fn unattributable_pooled_send_burst_does_not_eject() {
    let p = policy();
    let reg = HealthRegistry::new(p, &[addr(1), addr(2), addr(3)]);

    for _ in 0..50 {
        reg.record(addr(1), UpstreamErrorClass::Unattributable.outcome());
    }
    assert!(
        reg.admits(addr(1)),
        "a pooled-send burst must never eject: we cannot tell it from our own stale connection"
    );
    assert_eq!(reg.ejections_total(), 0);

    // It must be DISCARDED, not scored as a success: a success would clear a real streak, letting an
    // interleaved stale-send hold a genuinely dying backend in rotation forever.
    fail_n(&reg, addr(1), p.consecutive_failures - 1);
    reg.record(addr(1), UpstreamErrorClass::Unattributable.outcome());
    reg.record(addr(1), UpstreamErrorClass::Transport.outcome());
    assert!(
        !reg.admits(addr(1)),
        "an unattributable sample must not reset a genuine transport-failure streak"
    );
}

/// `enabled = false` must restore pre-G5 behaviour exactly: the gate is constant-true and nothing
/// is recorded.
#[test]
fn disabled_policy_is_inert() {
    let p = EjectionPolicy {
        enabled: false,
        ..policy()
    };
    let reg = HealthRegistry::new(p, &[addr(1), addr(2), addr(3)]);
    fail_n(&reg, addr(1), 100);
    assert!(reg.admits(addr(1)));
    assert_eq!(reg.ejections_total(), 0);
    assert_eq!(reg.ejected_count(), 0);
}
