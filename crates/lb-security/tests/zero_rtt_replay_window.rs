//! Proof for the LRU-backed 0-RTT replay guard (SEC-2-05): eviction is least-recently-USED, not
//! oldest-by-insertion, and a replay hit promotes to MRU so a spray cannot push the replayee out.

use lb_security::{DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE, SecurityError, ZeroRttReplayGuard};

#[test]
fn test_lru_evicts_oldest() {
    // With nothing touched, the first-inserted token is the LRU and must be the victim.
    let mut guard = ZeroRttReplayGuard::new(3);
    assert!(guard.check_and_record(b"a").is_ok());
    assert!(guard.check_and_record(b"b").is_ok());
    assert!(guard.check_and_record(b"c").is_ok());
    assert_eq!(guard.len(), 3);

    assert!(guard.check_and_record(b"d").is_ok());
    assert_eq!(guard.len(), 3);

    // `a` was evicted, so re-recording is a miss, not a replay.
    assert!(guard.check_and_record(b"a").is_ok());
    // `b` is now the LRU, evicted by the re-record of `a`.
    assert!(guard.check_and_record(b"b").is_ok());
}

#[test]
fn replay_hit_promotes_to_mru() {
    // THE LRU-vs-FIFO distinction: under FIFO `a` ages out behind `b` and `c`; under LRU the
    // replay attempt itself refreshes `a` so it survives.
    let mut guard = ZeroRttReplayGuard::new(3);
    assert!(guard.check_and_record(b"a").is_ok());
    assert!(guard.check_and_record(b"b").is_ok());
    assert!(guard.check_and_record(b"c").is_ok());

    // Must both report the replay AND promote `a`.
    assert!(matches!(
        guard.check_and_record(b"a"),
        Err(SecurityError::ZeroRttReplay)
    ));

    // Victim must be `b` (LRU after `a`'s promotion), not `a`.
    assert!(guard.check_and_record(b"d").is_ok());

    assert!(matches!(
        guard.check_and_record(b"a"),
        Err(SecurityError::ZeroRttReplay)
    ));
    assert!(guard.check_and_record(b"b").is_ok());
}

#[test]
fn replay_detected_within_window() {
    let mut guard = ZeroRttReplayGuard::new(16);
    let tok = b"some-0rtt-token";
    assert!(guard.check_and_record(tok).is_ok());
    assert!(matches!(
        guard.check_and_record(tok),
        Err(SecurityError::ZeroRttReplay)
    ));
}

#[test]
fn capacity_one_still_detects_replay_of_last_token() {
    let mut guard = ZeroRttReplayGuard::new(1);
    assert!(guard.check_and_record(b"a").is_ok());
    assert!(matches!(
        guard.check_and_record(b"a"),
        Err(SecurityError::ZeroRttReplay)
    ));
    assert!(guard.check_and_record(b"b").is_ok());
    assert!(guard.check_and_record(b"a").is_ok());
}

#[test]
fn capacity_zero_coerced_to_one() {
    let mut guard = ZeroRttReplayGuard::new(0);
    assert_eq!(guard.capacity(), 1);
    assert!(guard.check_and_record(b"a").is_ok());
    assert!(matches!(
        guard.check_and_record(b"a"),
        Err(SecurityError::ZeroRttReplay)
    ));
}

#[test]
fn default_window_size_constant_is_65k() {
    // Pins the `[security].zero_rtt_replay_window_size` default against silent drift.
    assert_eq!(DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE, 65_536);
}

#[test]
fn with_default_window_sizes_correctly() {
    let guard = ZeroRttReplayGuard::with_default_window();
    assert_eq!(guard.capacity(), DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE);
    assert!(guard.is_empty());
}

#[test]
fn fills_and_evicts_under_unique_token_spray() {
    // 10x capacity of unique tokens: only the last `cap` survive.
    let cap = 64;
    let mut guard = ZeroRttReplayGuard::new(cap);
    let total = cap * 10;
    for i in 0..total {
        let tok = format!("tok-{i}");
        assert!(guard.check_and_record(tok.as_bytes()).is_ok());
    }
    assert_eq!(guard.len(), cap);

    for i in (total - cap)..total {
        let tok = format!("tok-{i}");
        assert!(matches!(
            guard.check_and_record(tok.as_bytes()),
            Err(SecurityError::ZeroRttReplay)
        ));
    }
    assert!(guard.check_and_record(b"tok-0").is_ok());
}

#[test]
fn arena_reuses_freed_slots() {
    // The free-list bounds heap growth by capacity, not lifetime inserts. The arena is not
    // reachable through the public API, so `len()` staying at `cap` across heavy churn is the
    // available proxy.
    let mut guard = ZeroRttReplayGuard::new(8);
    for i in 0..10_000 {
        let tok = format!("t-{i}");
        let _ = guard.check_and_record(tok.as_bytes());
    }
    assert_eq!(guard.len(), 8);
}
