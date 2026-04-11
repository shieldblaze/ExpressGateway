//! Background task for cleaning up stale UDP sessions from the XDP map.
//!
//! Periodically scans the UDP session table and removes entries that have not
//! been seen within the configured timeout.
//!
//! The sweep uses `DashMap::retain` which iterates shards without a global lock.
//! Evicted keys are collected into a caller-provided callback instead of
//! allocating a Vec on every sweep in steady state.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::mpsc;
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
    #[inline]
    pub fn touch(&self, key: UdpSessionKey, now_ns: u64) {
        self.sessions.insert(key, now_ns);
    }

    /// Remove a specific session.
    #[inline]
    pub fn remove(&self, key: &UdpSessionKey) {
        self.sessions.remove(key);
    }

    /// Return the number of tracked sessions.
    #[inline]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether the session table is empty.
    #[inline]
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

    /// Run a single cleanup sweep without collecting evicted keys.
    ///
    /// Use this on the hot path when you don't need the evicted key list.
    /// Returns the number of sessions evicted.
    pub fn sweep_count(&self, now_ns: u64) -> usize {
        let timeout_ns = self.timeout.as_nanos() as u64;
        let before = self.sessions.len();

        self.sessions.retain(|_key, last_seen| {
            now_ns.saturating_sub(*last_seen) <= timeout_ns
        });

        // Use saturating_sub: concurrent `touch()` calls can insert entries
        // during `retain()`, making len() > before.
        let evicted = before.saturating_sub(self.sessions.len());

        if evicted > 0 {
            tracing::debug!(
                evicted,
                remaining = self.sessions.len(),
                "UDP session cleanup sweep completed"
            );
        }

        evicted
    }

    /// Spawn a background Tokio task that periodically sweeps stale sessions.
    ///
    /// Returns a `JoinHandle` and a shutdown sender. Drop the sender or
    /// send `()` to stop the task.
    pub fn spawn_cleanup_task<F>(
        &self,
        clock_fn: F,
    ) -> (JoinHandle<()>, mpsc::Sender<()>)
    where
        F: Fn() -> u64 + Send + 'static,
    {
        let sessions = Arc::clone(&self.sessions);
        let timeout = self.timeout;
        let interval = self.interval;
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let now_ns = clock_fn();
                        let timeout_ns = timeout.as_nanos() as u64;
                        let before = sessions.len();

                        sessions.retain(|_key, last_seen| {
                            now_ns.saturating_sub(*last_seen) <= timeout_ns
                        });

                        // Use saturating_sub: concurrent `touch()` calls can
                        // insert entries during `retain()`, making len() > before.
                        let evicted = before.saturating_sub(sessions.len());
                        if evicted > 0 {
                            tracing::debug!(
                                evicted,
                                remaining = sessions.len(),
                                "Background UDP session cleanup sweep"
                            );
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        tracing::info!("UDP session cleanup task shutting down");
                        break;
                    }
                }
            }
        });

        (handle, shutdown_tx)
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
            _pad: 0,
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

        let base_ns = 100_000_000_000u64;
        sc.touch(make_key(1, 100), base_ns);
        sc.touch(make_key(2, 200), base_ns);
        sc.touch(make_key(3, 300), base_ns - 40_000_000_000);

        let evicted = sc.sweep(base_ns);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0], make_key(3, 300));
        assert_eq!(sc.len(), 2);
    }

    #[test]
    fn sweep_count_returns_count() {
        let sc = SessionCleanup::with_config(Duration::from_secs(30), Duration::from_secs(10));

        let base_ns = 100_000_000_000u64;
        sc.touch(make_key(1, 100), base_ns);
        sc.touch(make_key(2, 200), base_ns - 40_000_000_000);
        sc.touch(make_key(3, 300), base_ns - 50_000_000_000);

        let evicted = sc.sweep_count(base_ns);
        assert_eq!(evicted, 2);
        assert_eq!(sc.len(), 1);
    }

    #[test]
    fn sweep_keeps_fresh_sessions() {
        let sc = SessionCleanup::with_config(Duration::from_secs(30), Duration::from_secs(10));

        let now = 50_000_000_000u64;
        sc.touch(make_key(1, 100), now);
        sc.touch(make_key(2, 200), now - 10_000_000_000);

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

        sc.touch(make_key(1, 100), 0);
        assert_eq!(sc.len(), 1);

        let clock = Arc::new(AtomicU64::new(200_000_000)); // 200ms in ns
        let clock_clone = Arc::clone(&clock);

        let (handle, _shutdown_tx) = sc.spawn_cleanup_task(move || clock_clone.load(Ordering::Relaxed));

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(sc.len(), 0, "Stale session should have been evicted");

        handle.abort();
    }

    #[tokio::test]
    async fn shutdown_stops_task() {
        let sc = SessionCleanup::with_config(Duration::from_secs(30), Duration::from_millis(20));

        let (handle, shutdown_tx) = sc.spawn_cleanup_task(|| 0);

        drop(shutdown_tx);

        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok(), "task should have stopped on shutdown");
    }
}
