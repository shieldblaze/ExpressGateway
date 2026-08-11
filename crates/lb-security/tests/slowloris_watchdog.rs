//! Proof for the slowloris / slow-POST watchdog (SEC-2-03).

use std::net::Ipv4Addr;
use std::thread::sleep;
use std::time::{Duration, Instant};

use lb_security::{ConnId, Watchdog, WatchdogConfig, WatchdogError};

fn conn(seq: u64) -> ConnId {
    ConnId::new(Ipv4Addr::LOCALHOST.into(), seq)
}

#[test]
fn test_slow_progress_fires_eviction() {
    let wd = Watchdog::new(WatchdogConfig {
        min_rate_bps: 10_000,
        rate_window: Duration::from_millis(50),
        max_registered: 8,
    });
    let id = conn(1);
    let deadline = Instant::now() + Duration::from_secs(60);
    assert!(wd.register(id, deadline));

    // First sample establishes the window.
    wd.progress(id, 1).unwrap();
    // Sleep past the window so the next call evaluates the rate.
    sleep(Duration::from_millis(80));
    // 2 bytes total ≈ 12.5 B/s, far below the floor.
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
    // Eviction happens on the same call, so the next lookup must be Unknown.
    assert!(matches!(
        wd.progress(id, 3).unwrap_err(),
        WatchdogError::Unknown(_)
    ));
}

#[test]
fn fast_progress_passes() {
    // Regression guard: an above-floor connection must never be evicted.
    let wd = Watchdog::new(WatchdogConfig {
        min_rate_bps: 100,
        rate_window: Duration::from_millis(20),
        max_registered: 8,
    });
    let id = conn(2);
    wd.register(id, Instant::now() + Duration::from_secs(60));
    // 10 MB/s sustained — no window can dip below the 100 B/s floor.
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
    // The sweeper is the only thing that catches a fully stalled connection.
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
    // Refusal is a cap exhaustion: the listener RSTs, same as a ConnGate overflow.
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
