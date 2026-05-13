//! Proof for the slowloris / slow-POST watchdog (SEC-2-03 API half).
//!
//! Wave-2b's `crates/lb-l7/src/h{1,2}_proxy.rs` and Wave-2c's
//! `crates/lb/src/main.rs` accept loop drive this watchdog. This test
//! file exercises the public API surface end-to-end so the call-site
//! insertion is a pure plumbing change.

use std::net::Ipv4Addr;
use std::thread::sleep;
use std::time::{Duration, Instant};

use lb_security::{ConnId, Watchdog, WatchdogConfig, WatchdogError};

fn conn(seq: u64) -> ConnId {
    ConnId::new(Ipv4Addr::LOCALHOST.into(), seq)
}

#[test]
fn test_slow_progress_fires_eviction() {
    // Headline test the plan calls out by name. A connection registers
    // with a tight rate floor; we feed it `progress` calls at a rate
    // below the floor and assert eviction fires with WatchdogError::SlowRate.
    let wd = Watchdog::new(WatchdogConfig {
        // 10 kB/s floor — well above the trickle we'll feed.
        min_rate_bps: 10_000,
        rate_window: Duration::from_millis(50),
        max_registered: 8,
    });
    let id = conn(1);
    let deadline = Instant::now() + Duration::from_secs(60);
    assert!(wd.register(id, deadline));

    // First progress sample establishes the window. Push only a
    // single byte cumulatively.
    wd.progress(id, 1).unwrap();
    // Sleep past the window so the next progress call triggers the
    // rate evaluation.
    sleep(Duration::from_millis(80));
    // Second sample: still only 2 bytes total — observed rate is
    // roughly 12.5 B/s, far below the 10 000 B/s floor.
    let err = wd.progress(id, 2).unwrap_err();
    match err {
        WatchdogError::SlowRate {
            conn,
            observed_bps,
            floor_bps,
        } => {
            assert_eq!(conn, id);
            assert!(observed_bps < floor_bps);
            assert_eq!(floor_bps, 10_000);
        }
        other => panic!("expected SlowRate, got {other:?}"),
    }
    // The watchdog must evict the entry on the same call so a
    // subsequent progress lookup fails as Unknown.
    assert!(matches!(
        wd.progress(id, 3).unwrap_err(),
        WatchdogError::Unknown(_)
    ));
}

#[test]
fn fast_progress_passes() {
    // Regression guard: a connection feeding bytes at a rate above
    // the floor must not be evicted.
    let wd = Watchdog::new(WatchdogConfig {
        min_rate_bps: 100,
        rate_window: Duration::from_millis(20),
        max_registered: 8,
    });
    let id = conn(2);
    wd.register(id, Instant::now() + Duration::from_secs(60));
    // Repeatedly feed bytes well above the 100 B/s floor for ~120ms.
    // Each iteration pushes 100_000 bytes over a 10ms tick =
    // 10 MB/s — orders of magnitude above the floor — so no window
    // can fall below 100 B/s.
    let mut cumulative: u64 = 0;
    for _ in 0..12 {
        cumulative += 100_000;
        wd.progress(id, cumulative).unwrap();
        sleep(Duration::from_millis(10));
    }
}

#[test]
fn deadline_evicts() {
    let wd = Watchdog::new(WatchdogConfig {
        min_rate_bps: 0,
        rate_window: Duration::from_secs(1),
        max_registered: 8,
    });
    let id = conn(3);
    wd.register(id, Instant::now() + Duration::from_millis(10));
    sleep(Duration::from_millis(25));
    let err = wd.progress(id, 0).unwrap_err();
    assert!(matches!(err, WatchdogError::Deadline(_)));
}

#[test]
fn sweep_expired_evicts_stalled() {
    // Exercises the sweeper entry point for connections that are
    // completely stalled (no progress calls).
    let wd = Watchdog::new(WatchdogConfig {
        min_rate_bps: 0,
        rate_window: Duration::from_secs(1),
        max_registered: 8,
    });
    let stalled = conn(10);
    let active = conn(11);
    wd.register(stalled, Instant::now() + Duration::from_millis(5));
    wd.register(active, Instant::now() + Duration::from_secs(60));
    sleep(Duration::from_millis(15));
    let evicted = wd.sweep_expired();
    assert_eq!(evicted, vec![stalled]);
    assert_eq!(wd.len(), 1);
}

#[test]
fn max_registered_caps_the_table() {
    let wd = Watchdog::new(WatchdogConfig {
        min_rate_bps: 0,
        rate_window: Duration::from_secs(1),
        max_registered: 2,
    });
    assert!(wd.register(conn(1), Instant::now() + Duration::from_secs(60)));
    assert!(wd.register(conn(2), Instant::now() + Duration::from_secs(60)));
    // Third registration must be refused (structural cap; the
    // listener should treat refusal as a connection cap exhaustion
    // and RST the socket — same disposition as ConnGate overflow).
    assert!(!wd.register(conn(3), Instant::now() + Duration::from_secs(60)));
}

#[test]
fn deregister_returns_existence() {
    let wd = Watchdog::new(WatchdogConfig::default());
    let id = conn(42);
    wd.register(id, Instant::now() + Duration::from_secs(60));
    assert!(wd.deregister(id));
    assert!(!wd.deregister(id));
}
