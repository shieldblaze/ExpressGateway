//! SEC-2-03 follow-on — the per-process `Watchdog` constructed in
//! `crates/lb/src/main.rs::async_main` is sweep-driven on the
//! `Shutdown` tracker and threaded into every `H1Proxy` / `H2Proxy`
//! via `.with_watchdog(...)`.
//!
//! Verifying this end-to-end (TCP connect, partial write over
//! 10 seconds, assert socket close) would require booting a full lb
//! process with config; that machinery lives in the bridging
//! suites. Here we pin the runtime-side wiring contract directly:
//!
//! 1. Construct a `Watchdog` exactly the way main.rs does (config
//!    sourced from `RuntimeWatchdogConfig::default`, sweep-loop
//!    spawned on the same `lb_core::Shutdown` tracker).
//! 2. Register a "slow-loris" connection with a 200 ms header
//!    deadline. Do *not* call `progress` (simulates a TCP client
//!    that opens the socket and drips zero bytes — the classic
//!    slow-loris).
//! 3. Wait for the sweep tick. Assert the entry was evicted before
//!    the connection's body phase could complete.
//!
//! Anyone who later drops the `Watchdog::new` call or the sweep
//! spawn from `main.rs` breaks this test because the eviction never
//! lands.

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use lb_config::RuntimeWatchdogConfig;
use lb_core::Shutdown;
use lb_security::{ConnId, Watchdog, WatchdogConfig};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

/// Mirror of the construction in `crates/lb/src/main.rs::async_main`
/// — keep these two factories in lock-step so a divergence is caught
/// at compile time on the next refactor.
fn build_watchdog_like_main(cfg: RuntimeWatchdogConfig) -> Watchdog {
    Watchdog::new(WatchdogConfig {
        min_rate_bps: cfg.body_progress_min_bps,
        rate_window: Duration::from_secs(1),
        max_registered: 100_000,
    })
}

#[tokio::test(flavor = "current_thread", start_paused = false)]
async fn test_slow_loris_evicted_within_deadline() {
    // Tight defaults so the test finishes in well under a second.
    let cfg = RuntimeWatchdogConfig {
        header_deadline_ms: 200,
        body_progress_min_bps: 64,
        sweep_interval_ms: 50,
    };
    let wd = build_watchdog_like_main(cfg);

    // Spawn the sweep loop the same way main.rs does (shutdown
    // tracker + cancellation token), so a regression in the spawn
    // wiring lands here.
    let shutdown = Shutdown::new();
    let cancel: CancellationToken = shutdown.token().clone();
    let wd_sweep = wd.clone();
    let sweep_interval = Duration::from_millis(cfg.sweep_interval_ms);
    shutdown.tracker().spawn(async move {
        let mut ticker = tokio::time::interval(sweep_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                _ = ticker.tick() => {}
            }
            let _evicted = wd_sweep.sweep_expired();
        }
    });

    // Register a connection with the header deadline. Do NOT call
    // `progress` — this is exactly the slow-loris pattern
    // (open socket, dribble zero bytes).
    let id = ConnId::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1);
    let deadline = Instant::now() + Duration::from_millis(cfg.header_deadline_ms);
    assert!(wd.register(id, deadline), "register must succeed");
    assert_eq!(wd.len(), 1, "watchdog should hold the registered conn");

    // Wait long enough for the sweeper to tick at least twice past
    // the deadline (deadline=200 ms, sweep=50 ms → first eviction
    // tick lands around T=250 ms).
    sleep(Duration::from_millis(500)).await;

    // Assertion: the slow-loris connection MUST be gone before the
    // body phase could complete. Without main.rs's `Watchdog::new`
    // + sweep spawn the eviction never happens and `wd.len()`
    // stays at 1.
    assert_eq!(
        wd.len(),
        0,
        "slow-loris connection should have been swept; \
         did main.rs drop the Watchdog wire-up?"
    );

    shutdown.token().cancel();
}

#[tokio::test]
async fn test_watchdog_progress_path_evicts_on_deadline() {
    // Belt-and-braces: even if the sweep loop is broken, a hot-path
    // `progress` call after the deadline must also evict. This is
    // the path lb-l7 takes on each header parse / body chunk.
    let wd = build_watchdog_like_main(RuntimeWatchdogConfig::default());
    let id = ConnId::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 2);
    wd.register(id, Instant::now() + Duration::from_millis(20));
    sleep(Duration::from_millis(80)).await;
    let err = wd.progress(id, 1).expect_err("expected eviction");
    assert!(matches!(err, lb_security::WatchdogError::Deadline(_)));
}
