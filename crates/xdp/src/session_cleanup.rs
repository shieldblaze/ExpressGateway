//! Background task for cleaning up stale UDP sessions from the XDP map.
//!
//! Periodically scans the UDP session table and removes entries that have not
//! been seen within the configured timeout.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::task::JoinHandle;

use crate::maps::UdpSessionKey;

/// Default session timeout: 30 seconds.
pub const DEFAULT_SESSION_TIMEOUT: Duration = Duration::from_secs(30);

/// Default cleanup interval: 10 seconds.
pub const DEFAULT_CLEANUP_INTERVAL: Duration = Duration::from_secs(10);

/// Tracks UDP session timestamps in userspace for cleanup decisions.
///
/// The actual BPF map lives in the kernel; this mirror tracks `last_seen`
/// timestamps so we know which entries to evict.
pub struct SessionCleanup {
    /// Userspace mirror: key -> last_seen (nanoseconds since boot).
    sessions: Arc<DashMap<UdpSessionKey, u64>>,
    /// Sessions older than this are evicted.
    timeout: Duration,
    /// How often to run the cleanup sweep.
    interval: Duration,
}

impl SessionCleanup {
    /// Create a new session cleanup tracker with default timeout (30s) and
    /// interval (10s).
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            timeout: DEFAULT_SESSION_TIMEOUT,
            interval: DEFAULT_CLEANUP_INTERVAL,
        }
    }

    /// Create a session cleanup tracker with custom timeout and interval.
    pub fn with_config(timeout: Duration, interval: Duration) -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            timeout,
            interval,
        }
    }

    /// Record a session as recently seen.
    ///
    /// `now_ns` should be the current time in nanoseconds (matching
    /// `bpf_ktime_get_ns` semantics).
    pub fn touch(&self, key: UdpSessionKey, now_ns: u64) {
        self.sessions.insert(key, now_ns);
    }

    /// Remove a specific session.
    pub fn remove(&self, key: &UdpSessionKey) {
        self.sessions.remove(key);
    }

    /// Return the number of tracked sessions.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether the session table is empty.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Run a single cleanup sweep, evicting entries older than `timeout`
    /// relative to `now_ns`.
    ///
    /// Returns the keys that were evicted.
    pub fn sweep(&self, now_ns: u64) -> Vec<UdpSessionKey> {
        let timeout_ns = self.timeout.as_nanos() as u64;
        let mut evicted = Vec::new();

        self.sessions.retain(|key, last_seen| {
            if now_ns.saturating_sub(*last_seen) > timeout_ns {
                evicted.push(*key);
                false
            } else {
                true
            }
        });

        if !evicted.is_empty() {
            tracing::debug!(
                evicted = evicted.len(),
                remaining = self.sessions.len(),
                "UDP session cleanup sweep completed"
            );
        }

        evicted
    }

    /// Spawn a background Tokio task that periodically sweeps stale sessions.
    ///
    /// The task runs until the returned `JoinHandle` is aborted or the runtime
    /// shuts down. The `clock_fn` provides the current time in nanoseconds
    /// (allows testing with a fake clock).
    pub fn spawn_cleanup_task<F>(&self, clock_fn: F) -> JoinHandle<()>
    where
        F: Fn() -> u64 + Send + 'static,
    {
        let sessions = Arc::clone(&self.sessions);
        let timeout = self.timeout;
        let interval = self.interval;

        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;

                let now_ns = clock_fn();
                let timeout_ns = timeout.as_nanos() as u64;
                let mut evicted = 0usize;

                sessions.retain(|_key, last_seen| {
                    if now_ns.saturating_sub(*last_seen) > timeout_ns {
                        evicted += 1;
                        false
                    } else {
                        true
                    }
                });

                if evicted > 0 {
                    tracing::debug!(
                        evicted,
                        remaining = sessions.len(),
                        "Background UDP session cleanup sweep"
                    );
                }
            }
        })
    }
}

impl Default for SessionCleanup {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(ip: u32, port: u16) -> UdpSessionKey {
        UdpSessionKey {
            src_ip: ip,
            src_port: port,
        }
    }

    #[test]
    fn touch_and_len() {
        let sc = SessionCleanup::new();
        assert!(sc.is_empty());

        sc.touch(make_key(1, 100), 1_000_000_000);
        sc.touch(make_key(2, 200), 1_000_000_000);
        assert_eq!(sc.len(), 2);
    }

    #[test]
    fn remove_session() {
        let sc = SessionCleanup::new();
        let key = make_key(1, 100);
        sc.touch(key, 1_000_000_000);
        assert_eq!(sc.len(), 1);

        sc.remove(&key);
        assert!(sc.is_empty());
    }

    #[test]
    fn sweep_evicts_old_sessions() {
        let timeout = Duration::from_secs(30);
        let sc = SessionCleanup::with_config(timeout, Duration::from_secs(10));

        let base_ns = 100_000_000_000u64; // 100s since boot
        sc.touch(make_key(1, 100), base_ns);
        sc.touch(make_key(2, 200), base_ns);
        sc.touch(make_key(3, 300), base_ns - 40_000_000_000); // 40s ago -> stale

        // Sweep at base_ns: session 3 is 40s old (> 30s timeout) -> evicted
        let evicted = sc.sweep(base_ns);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0], make_key(3, 300));
        assert_eq!(sc.len(), 2);
    }

    #[test]
    fn sweep_keeps_fresh_sessions() {
        let sc = SessionCleanup::with_config(Duration::from_secs(30), Duration::from_secs(10));

        let now = 50_000_000_000u64;
        sc.touch(make_key(1, 100), now);
        sc.touch(make_key(2, 200), now - 10_000_000_000); // 10s ago -> fresh

        let evicted = sc.sweep(now);
        assert!(evicted.is_empty());
        assert_eq!(sc.len(), 2);
    }

    #[test]
    fn sweep_handles_empty_table() {
        let sc = SessionCleanup::new();
        let evicted = sc.sweep(1_000_000_000);
        assert!(evicted.is_empty());
    }

    #[test]
    fn default_config_values() {
        let sc = SessionCleanup::default();
        assert_eq!(sc.timeout, DEFAULT_SESSION_TIMEOUT);
        assert_eq!(sc.interval, DEFAULT_CLEANUP_INTERVAL);
    }

    #[tokio::test]
    async fn background_task_evicts_sessions() {
        use std::sync::atomic::{AtomicU64, Ordering};

        let sc = SessionCleanup::with_config(Duration::from_millis(50), Duration::from_millis(20));

        // Add a session that will be stale by the time the sweep runs.
        sc.touch(make_key(1, 100), 0);
        assert_eq!(sc.len(), 1);

        let clock = Arc::new(AtomicU64::new(200_000_000)); // 200ms in ns
        let clock_clone = Arc::clone(&clock);

        let handle = sc.spawn_cleanup_task(move || clock_clone.load(Ordering::Relaxed));

        // Wait for at least one sweep cycle.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The session with last_seen=0 is far older than 50ms timeout.
        assert_eq!(sc.len(), 0, "Stale session should have been evicted");

        handle.abort();
    }
}
