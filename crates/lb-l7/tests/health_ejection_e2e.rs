//! G5 negative controls at the WIRE level: a real `H1Proxy` over three real TCP backends, driven
//! by real requests, asserting that traffic actually moves.
//!
//! The policy-level controls live in `crates/lb-health/tests/ejection_controls.rs`; these prove the
//! WIRING — that `HealthFilteredPicker` is consulted on the request path and that the proxy feeds
//! outcomes back. Each test names the pre-fix behaviour it catches.
//!
//! Signal: per-backend ACCEPT counters. Asserting on which backend served is more direct than
//! parsing bodies, and it also catches the case where a backend is dialed but its response is
//! discarded.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use lb_health::{AdmissionGate, EjectionPolicy, HealthRegistry};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts};
use lb_l7::upstream::{
    BackendInfoPicker, HealthFilteredPicker, RoundRobinUpstreams, UpstreamBackend,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Short enough to keep the suite fast, long enough that several requests land inside the window.
const BASE_EJECTION: Duration = Duration::from_millis(600);

struct Backend {
    addr: SocketAddr,
    /// Accepts seen. THE traffic signal.
    accepts: Arc<AtomicUsize>,
    /// Flip to make the backend start or stop serving.
    healthy: Arc<AtomicBool>,
}

/// A backend that serves `200` while `healthy`, and otherwise accepts and immediately drops the
/// connection — a transport fault the proxy surfaces as `ProxyErr::Upstream`, i.e. a health
/// FAILURE, without waiting for any timeout.
async fn spawn_backend(healthy: bool) -> Backend {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepts = Arc::new(AtomicUsize::new(0));
    let healthy_flag = Arc::new(AtomicBool::new(healthy));

    let accepts_task = Arc::clone(&accepts);
    let healthy_task = Arc::clone(&healthy_flag);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            accepts_task.fetch_add(1, Ordering::SeqCst);
            let serving = healthy_task.load(Ordering::SeqCst);
            tokio::spawn(async move {
                if !serving {
                    // Drop WITHOUT responding: the gateway's upstream leg errors immediately.
                    return;
                }
                let mut buf = [0u8; 4096];
                loop {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    if sock
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });

    Backend {
        addr,
        accepts,
        healthy: healthy_flag,
    }
}

fn policy() -> EjectionPolicy {
    EjectionPolicy {
        base_ejection: BASE_EJECTION,
        max_ejection: Duration::from_secs(5),
        ..EjectionPolicy::default()
    }
}

/// Build the gateway exactly the way the binary does: ONE registry backing both the picker's
/// admission gate and the proxy's outcome sink.
async fn spawn_gateway(
    backends: &[&Backend],
    policy: EjectionPolicy,
) -> (SocketAddr, Arc<HealthRegistry>) {
    let addrs: Vec<SocketAddr> = backends.iter().map(|b| b.addr).collect();
    let health = Arc::new(HealthRegistry::new(policy, &addrs));

    let inner: Arc<dyn BackendInfoPicker> = Arc::new(
        RoundRobinUpstreams::new(addrs.iter().copied().map(UpstreamBackend::h1).collect()).unwrap(),
    );
    let picker: Arc<dyn BackendInfoPicker> = Arc::new(HealthFilteredPicker::new(
        inner,
        Arc::clone(&health) as Arc<dyn AdmissionGate>,
        addrs.len(),
    ));

    let pool = TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts::default(),
        lb_io::Runtime::new(),
    );
    let proxy = Arc::new(
        H1Proxy::with_multi_proto(
            pool,
            picker,
            None,
            HttpTimeouts {
                header: Duration::from_secs(2),
                body: Duration::from_secs(2),
                total: Duration::from_secs(5),
                head: Duration::from_secs(5),
            },
            false,
        )
        .with_health(Arc::clone(&health)),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let p = Arc::clone(&proxy);
            tokio::spawn(async move {
                let _ = p.serve_connection(sock, peer).await;
            });
        }
    });
    (gw_addr, health)
}

/// One request on a fresh connection; returns the status code. A fresh connection per request keeps
/// response framing trivial and matches how the ejection decision is per-REQUEST.
async fn request(gw: SocketAddr) -> u16 {
    let Ok(mut client) = TcpStream::connect(gw).await else {
        return 0;
    };
    if client
        .write_all(b"GET /probe HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .is_err()
    {
        return 0;
    }
    let mut buf = [0u8; 512];
    let Ok(Ok(n)) = tokio::time::timeout(Duration::from_secs(3), client.read(&mut buf)).await
    else {
        return 0;
    };
    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
    head.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

async fn drive(gw: SocketAddr, count: usize) -> Vec<u16> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(request(gw).await);
    }
    out
}

/// CONTROL (i) — a failing backend IS ejected and traffic SHIFTS to its peers.
///
/// PRE-FIX BEHAVIOUR CAUGHT: `_health_seed` was bound-and-dropped and `record_failure` had no
/// production caller, so the round-robin picker kept handing 1-in-3 requests to the dead backend
/// forever. Pre-fix, `dead.accepts` keeps climbing in phase 2 and this fails.
#[tokio::test]
async fn failing_backend_is_ejected_and_traffic_shifts() {
    let good_a = spawn_backend(true).await;
    let good_b = spawn_backend(true).await;
    let dead = spawn_backend(false).await;
    let (gw, health) = spawn_gateway(&[&good_a, &good_b, &dead], policy()).await;

    // Phase 1 — enough rotations for the dead backend to cross the 5-failure threshold.
    let statuses = drive(gw, 18).await;

    // NON-VACUITY: the dead backend must actually have been picked, or "it stopped receiving
    // traffic" would be trivially true and this test would prove nothing.
    let dead_accepts_phase1 = dead.accepts.load(Ordering::SeqCst);
    assert!(
        dead_accepts_phase1 >= 5,
        "the failing backend must have been picked at least threshold times, saw {dead_accepts_phase1}"
    );
    assert!(
        statuses.contains(&502),
        "the failing backend must have produced 502s: {statuses:?}"
    );
    assert_eq!(health.ejected_count(), 1, "exactly one backend is ejected");

    // Phase 2 — after ejection, NOTHING may reach it.
    let before = dead.accepts.load(Ordering::SeqCst);
    let phase2 = drive(gw, 12).await;
    assert_eq!(
        dead.accepts.load(Ordering::SeqCst),
        before,
        "an ejected backend must receive ZERO further traffic"
    );
    assert!(
        phase2.iter().all(|s| *s == 200),
        "with the bad backend ejected every request must succeed: {phase2:?}"
    );
    assert!(good_a.accepts.load(Ordering::SeqCst) > 0 && good_b.accepts.load(Ordering::SeqCst) > 0);
}

/// CONTROL (ii) — a RECOVERED backend is re-admitted.
///
/// NAIVE-FIX BEHAVIOUR CAUGHT: an ejection with no re-admission path is permanent — the backend can
/// never receive the request that would prove it healthy, so one blip removes it for the process
/// lifetime. That is worse than not ejecting at all.
#[tokio::test]
async fn recovered_backend_is_readmitted() {
    let good_a = spawn_backend(true).await;
    let good_b = spawn_backend(true).await;
    let flaky = spawn_backend(false).await;
    let (gw, health) = spawn_gateway(&[&good_a, &good_b, &flaky], policy()).await;

    drive(gw, 18).await;
    assert_eq!(health.ejected_count(), 1, "precondition: it is ejected");

    // NON-VACUITY: inside the window it stays out even though it is now healthy — otherwise the
    // re-admission below could just be "the ejection never took".
    flaky.healthy.store(true, Ordering::SeqCst);
    let before = flaky.accepts.load(Ordering::SeqCst);
    drive(gw, 6).await;
    assert_eq!(
        flaky.accepts.load(Ordering::SeqCst),
        before,
        "a recovered backend must stay out until its ejection window elapses"
    );

    // Past the window: the half-open probe is admitted and its success clears the ejection.
    tokio::time::sleep(BASE_EJECTION + Duration::from_millis(200)).await;
    let statuses = drive(gw, 12).await;
    assert!(
        flaky.accepts.load(Ordering::SeqCst) > before,
        "a recovered backend MUST be re-admitted after the ejection window"
    );
    assert!(
        statuses.iter().all(|s| *s == 200),
        "the recovered backend serves normally: {statuses:?}"
    );
    assert!(health.readmissions_total() >= 1);
    assert_eq!(health.ejected_count(), 0);
}

/// CONTROL (iii) — with EVERY backend failing, the floor holds and the gateway keeps trying.
///
/// NAIVE-FIX BEHAVIOUR CAUGHT: floorless per-backend ejection. A correlated outage would eject all
/// three, the picker would have nothing to return, and the listener would stop dialing entirely —
/// turning a recoverable backend outage into a gateway that cannot recover when they come back.
#[tokio::test]
async fn all_backends_failing_keeps_serving_degraded() {
    let a = spawn_backend(false).await;
    let b = spawn_backend(false).await;
    let c = spawn_backend(false).await;
    let (gw, health) = spawn_gateway(&[&a, &b, &c], policy()).await;

    drive(gw, 24).await;

    // The floor caps ejections at 50% of 3 → 1.
    assert_eq!(
        health.ejected_count(),
        1,
        "the minimum-healthy floor must cap ejections"
    );
    assert!(
        health.ejections_suppressed_total() > 0,
        "suppressed ejections must be counted, never silent"
    );

    // And the gateway must STILL be attempting dials — recovery depends on it.
    let before = (
        a.accepts.load(Ordering::SeqCst),
        b.accepts.load(Ordering::SeqCst),
        c.accepts.load(Ordering::SeqCst),
    );
    drive(gw, 12).await;
    let after = (
        a.accepts.load(Ordering::SeqCst),
        b.accepts.load(Ordering::SeqCst),
        c.accepts.load(Ordering::SeqCst),
    );
    assert!(
        after.0 + after.1 + after.2 > before.0 + before.1 + before.2,
        "with everything down the gateway must keep dialing, not give up: {before:?} -> {after:?}"
    );

    // Now bring them all back: the un-ejected majority recovers immediately.
    a.healthy.store(true, Ordering::SeqCst);
    b.healthy.store(true, Ordering::SeqCst);
    c.healthy.store(true, Ordering::SeqCst);
    let statuses = drive(gw, 9).await;
    assert!(
        statuses.contains(&200),
        "recovery must be visible without waiting for any ejection window: {statuses:?}"
    );
}

/// CONTROL (iv) — a single transient error does NOT eject.
///
/// NAIVE-FIX BEHAVIOUR CAUGHT: eject-on-first-error. One RST during a rolling backend restart would
/// pull a healthy backend out of rotation for the full ejection window.
#[tokio::test]
async fn single_transient_error_does_not_eject() {
    let good_a = spawn_backend(true).await;
    let good_b = spawn_backend(true).await;
    let blip = spawn_backend(true).await;
    let (gw, health) = spawn_gateway(&[&good_a, &good_b, &blip], policy()).await;

    // Warm up, then inject EXACTLY one failure window on the third backend.
    drive(gw, 3).await;
    blip.healthy.store(false, Ordering::SeqCst);
    let before = blip.accepts.load(Ordering::SeqCst);
    // Three requests = one full rotation = exactly one attempt against `blip`.
    let statuses = drive(gw, 3).await;
    blip.healthy.store(true, Ordering::SeqCst);

    assert_eq!(
        blip.accepts.load(Ordering::SeqCst),
        before + 1,
        "the rig must produce exactly ONE failing attempt, or this proves nothing about the threshold"
    );
    assert_eq!(
        statuses.iter().filter(|s| **s == 502).count(),
        1,
        "exactly one request should have failed: {statuses:?}"
    );
    assert_eq!(
        health.ejected_count(),
        0,
        "a single transient error must NOT eject"
    );

    // And the backend keeps taking its share.
    let before = blip.accepts.load(Ordering::SeqCst);
    let statuses = drive(gw, 9).await;
    assert!(
        blip.accepts.load(Ordering::SeqCst) > before,
        "a backend that blipped once must stay in rotation"
    );
    assert!(
        statuses.iter().all(|s| *s == 200),
        "everything healthy again: {statuses:?}"
    );
    assert_eq!(health.ejections_total(), 0);
}

/// `enabled = false` must restore pre-G5 wire behaviour: the dead backend keeps its 1-in-3 share.
/// This is the escape hatch the config doc promises, asserted end to end.
#[tokio::test]
async fn disabled_policy_keeps_pre_g5_routing() {
    let good_a = spawn_backend(true).await;
    let good_b = spawn_backend(true).await;
    let dead = spawn_backend(false).await;
    let disabled = EjectionPolicy {
        enabled: false,
        ..policy()
    };
    let (gw, health) = spawn_gateway(&[&good_a, &good_b, &dead], disabled).await;

    drive(gw, 18).await;
    let mid = dead.accepts.load(Ordering::SeqCst);
    drive(gw, 12).await;

    assert!(
        dead.accepts.load(Ordering::SeqCst) > mid,
        "with ejection disabled the failing backend must keep receiving its share"
    );
    assert_eq!(health.ejected_count(), 0);
    assert_eq!(health.ejections_total(), 0);
}
