//! Proof for the LRU-backed 0-RTT replay guard (SEC-2-05).
//!
//! Pins:
//!   * eviction targets the least-recently-used digest, not the
//!     oldest-by-insertion (FIFO regression guard)
//!   * the configurable window size honours the
//!     `[security].zero_rtt_replay_window_size` knob (default
//!     `DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE = 65_536`)
//!   * a replay hit promotes the matching entry to MRU so a
//!     sustained spray cannot push the replayee out

use lb_security::{DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE, SecurityError, ZeroRttReplayGuard};

#[test]
fn test_lru_evicts_oldest() {
    // Plan-named headline test. Insert capacity tokens, then push
    // one more — the FIRST token (which is the LRU under the
    // never-touched policy) must be the eviction victim.
    let mut guard = ZeroRttReplayGuard::new(3);
    assert!(guard.check_and_record(b"a").is_ok());
    assert!(guard.check_and_record(b"b").is_ok());
    assert!(guard.check_and_record(b"c").is_ok());
    assert_eq!(guard.len(), 3);

    // Fourth insertion evicts the LRU (`a`).
    assert!(guard.check_and_record(b"d").is_ok());
    assert_eq!(guard.len(), 3);

    // `a` is gone — re-recording is accepted (the original was
    // evicted, not a replay).
    assert!(guard.check_and_record(b"a").is_ok());
    // Now `b` is the LRU and must have been evicted by the previous
    // re-record of `a`.
    assert!(guard.check_and_record(b"b").is_ok());
}

#[test]
fn replay_hit_promotes_to_mru() {
    // The crucial LRU-vs-FIFO distinction. Under a FIFO, `a` would
    // age out after `b`, `c` insert. Under LRU, touching `a` (here:
    // via a replay attempt) refreshes it so it stays.
    let mut guard = ZeroRttReplayGuard::new(3);
    assert!(guard.check_and_record(b"a").is_ok());
    assert!(guard.check_and_record(b"b").is_ok());
    assert!(guard.check_and_record(b"c").is_ok());

    // Touch `a` via a replay attempt — must return ZeroRttReplay
    // AND promote `a` to MRU.
    assert!(matches!(
        guard.check_and_record(b"a"),
        Err(SecurityError::ZeroRttReplay)
    ));

    // Insert `d` — eviction victim should be `b` (LRU after `a`'s
    // promotion), not `a`.
    assert!(guard.check_and_record(b"d").is_ok());

    // `a` must still be in the window (returning Err on replay) ...
    assert!(matches!(
        guard.check_and_record(b"a"),
        Err(SecurityError::ZeroRttReplay)
    ));
    // ... while `b` must have been evicted (re-record now Ok).
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
    // Same token: replay.
    assert!(matches!(
        guard.check_and_record(b"a"),
        Err(SecurityError::ZeroRttReplay)
    ));
    // Different token evicts `a`.
    assert!(guard.check_and_record(b"b").is_ok());
    // `a` re-record now accepted.
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
    // Pin the SEC-2-05 default — Wave-2c's
    // `[security].zero_rtt_replay_window_size` knob defaults to this
    // value. Bumping it must be a deliberate edit, not a silent drift.
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
    // 10x capacity worth of unique tokens. After this loop, only the
    // last `cap` tokens should still be in the window — the rest
    // have aged out via LRU eviction.
    let cap = 64;
    let mut guard = ZeroRttReplayGuard::new(cap);
    let total = cap * 10;
    for i in 0..total {
        let tok = format!("tok-{i}");
        assert!(guard.check_and_record(tok.as_bytes()).is_ok());
    }
    assert_eq!(guard.len(), cap);

    // The most-recent `cap` tokens are still flagged as replays.
    for i in (total - cap)..total {
        let tok = format!("tok-{i}");
        assert!(matches!(
            guard.check_and_record(tok.as_bytes()),
            Err(SecurityError::ZeroRttReplay)
        ));
    }
    // A token older than the LRU window is accepted again.
    assert!(guard.check_and_record(b"tok-0").is_ok());
}

#[test]
fn arena_reuses_freed_slots() {
    // The internal arena uses a free-list so heap growth is bounded
    // by the capacity, not the lifetime insert count. We can't probe
    // the arena directly through the public API, but we can assert
    // that millions of churned inserts under a small cap stay
    // bounded in size (no panic, len() stays at cap, etc.).
    let mut guard = ZeroRttReplayGuard::new(8);
    for i in 0..10_000 {
        let tok = format!("t-{i}");
        let _ = guard.check_and_record(tok.as_bytes());
    }
    assert_eq!(guard.len(), 8);
}
