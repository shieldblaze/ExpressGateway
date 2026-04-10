//! UDP session management.
//!
//! Maps client addresses to backend addresses with expiry-based lifecycle.
//! Uses `DashMap` for lock-free concurrent access and a background task
//! for periodic session cleanup.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{debug, info};

/// A single UDP session mapping a client to a backend.
#[derive(Debug, Clone)]
pub struct UdpSession {
    /// The backend address this client is mapped to.
    pub backend_addr: SocketAddr,
    /// Timestamp of last activity (send or receive).
    pub last_seen: Instant,
}

/// Rate limiter state for a single source IP.
#[derive(Debug)]
struct RateLimitEntry {
    /// Packet count in the current window.
    count: AtomicU64,
    /// Start of the current 1-second window.
    window_start: parking_lot::Mutex<Instant>,
}

/// Manages UDP sessions with expiry, rate limiting, and background cleanup.
pub struct SessionManager {
    /// Active sessions: client_addr -> session.
    sessions: DashMap<SocketAddr, UdpSession>,
    /// Rate limit state per source IP.
    rate_limits: DashMap<SocketAddr, Arc<RateLimitEntry>>,
    /// Session timeout duration.
    session_timeout: Duration,
    /// Maximum number of sessions.
    max_sessions: usize,
    /// Packets per second limit per source IP. `None` means unlimited.
    rate_limit_pps: Option<u64>,
    /// Whether the cleanup task is running.
    cleanup_running: AtomicBool,
}

impl SessionManager {
    /// Create a new session manager.
    pub fn new(
        session_timeout: Duration,
        max_sessions: usize,
        rate_limit_pps: Option<u64>,
    ) -> Self {
        Self {
            sessions: DashMap::new(),
            rate_limits: DashMap::new(),
            session_timeout,
            max_sessions,
            rate_limit_pps,
            cleanup_running: AtomicBool::new(false),
        }
    }

    /// Get or create a session for the given client address.
    ///
    /// If a session exists and hasn't expired, returns the backend address and
    /// updates `last_seen`. If it has expired or doesn't exist, returns `None`
    /// so the caller can select a new backend.
    pub fn get_session(&self, client_addr: &SocketAddr) -> Option<SocketAddr> {
        if let Some(mut entry) = self.sessions.get_mut(client_addr) {
            if entry.last_seen.elapsed() > self.session_timeout {
                // Expired: remove it
                drop(entry);
                self.sessions.remove(client_addr);
                return None;
            }
            entry.last_seen = Instant::now();
            Some(entry.backend_addr)
        } else {
            None
        }
    }

    /// Create a new session mapping client_addr to backend_addr.
    ///
    /// Returns `true` if the session was created, `false` if the session
    /// limit has been reached.
    pub fn create_session(&self, client_addr: SocketAddr, backend_addr: SocketAddr) -> bool {
        if self.sessions.len() >= self.max_sessions {
            return false;
        }
        self.sessions.insert(
            client_addr,
            UdpSession {
                backend_addr,
                last_seen: Instant::now(),
            },
        );
        true
    }

    /// Remove a session.
    pub fn remove_session(&self, client_addr: &SocketAddr) -> Option<UdpSession> {
        self.sessions.remove(client_addr).map(|(_, v)| v)
    }

    /// Number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Check whether a packet from the given source is allowed under the rate limit.
    ///
    /// Returns `true` if the packet is allowed, `false` if rate limited.
    /// If no rate limit is configured, always returns `true`.
    pub fn check_rate_limit(&self, client_addr: &SocketAddr) -> bool {
        let limit = match self.rate_limit_pps {
            Some(limit) => limit,
            None => return true,
        };

        let entry = self
            .rate_limits
            .entry(*client_addr)
            .or_insert_with(|| {
                Arc::new(RateLimitEntry {
                    count: AtomicU64::new(0),
                    window_start: parking_lot::Mutex::new(Instant::now()),
                })
            })
            .clone();

        let mut window_start = entry.window_start.lock();
        if window_start.elapsed() >= Duration::from_secs(1) {
            // New window
            *window_start = Instant::now();
            entry.count.store(1, Ordering::Relaxed);
            true
        } else {
            let count = entry.count.fetch_add(1, Ordering::Relaxed) + 1;
            count <= limit
        }
    }

    /// Clean up expired sessions. Returns the number of sessions removed.
    pub fn cleanup_expired(&self) -> usize {
        let now = Instant::now();
        let timeout = self.session_timeout;
        let mut removed = 0;

        self.sessions.retain(|_, session| {
            let keep = now.duration_since(session.last_seen) < timeout;
            if !keep {
                removed += 1;
            }
            keep
        });

        // Also clean up stale rate limit entries (older than 2x session timeout)
        let rate_limit_expiry = timeout * 2;
        self.rate_limits
            .retain(|_, entry| entry.window_start.lock().elapsed() < rate_limit_expiry);

        removed
    }

    /// Start a background task that periodically cleans up expired sessions.
    ///
    /// The cleanup interval is half the session timeout to ensure timely
    /// expiration. The task runs until the returned `CleanupHandle` is dropped.
    pub fn start_cleanup_task(self: &Arc<Self>) -> CleanupHandle {
        let running = Arc::new(AtomicBool::new(true));
        let handle_running = running.clone();
        let manager = Arc::clone(self);
        let interval = self.session_timeout / 2;

        self.cleanup_running.store(true, Ordering::Release);

        let join = tokio::spawn(async move {
            info!(
                interval_ms = interval.as_millis(),
                "UDP session cleanup task started"
            );
            while handle_running.load(Ordering::Relaxed) {
                tokio::time::sleep(interval).await;
                if !handle_running.load(Ordering::Relaxed) {
                    break;
                }
                let removed = manager.cleanup_expired();
                if removed > 0 {
                    debug!(
                        removed,
                        remaining = manager.session_count(),
                        "Cleaned up expired UDP sessions"
                    );
                }
            }
            manager.cleanup_running.store(false, Ordering::Release);
            info!("UDP session cleanup task stopped");
        });

        CleanupHandle {
            running,
            _join: join,
        }
    }

    /// Whether the background cleanup task is running.
    pub fn is_cleanup_running(&self) -> bool {
        self.cleanup_running.load(Ordering::Relaxed)
    }
}

/// Handle for the background cleanup task. When dropped, the task is stopped.
pub struct CleanupHandle {
    running: Arc<AtomicBool>,
    _join: tokio::task::JoinHandle<()>,
}

impl CleanupHandle {
    /// Signal the cleanup task to stop.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
    }
}

impl Drop for CleanupHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_session() {
        let mgr = SessionManager::new(Duration::from_secs(30), 1000, None);

        let client: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let backend: SocketAddr = "10.0.0.2:8080".parse().unwrap();

        assert!(mgr.get_session(&client).is_none());
        assert!(mgr.create_session(client, backend));
        assert_eq!(mgr.get_session(&client), Some(backend));
        assert_eq!(mgr.session_count(), 1);
    }

    #[test]
    fn test_session_expiry() {
        // Use a very short timeout so we can test expiry
        let mgr = SessionManager::new(Duration::from_millis(50), 1000, None);

        let client: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let backend: SocketAddr = "10.0.0.2:8080".parse().unwrap();

        assert!(mgr.create_session(client, backend));
        assert_eq!(mgr.get_session(&client), Some(backend));

        // Wait for expiry
        std::thread::sleep(Duration::from_millis(100));

        // Session should be expired
        assert!(mgr.get_session(&client).is_none());
        assert_eq!(mgr.session_count(), 0);
    }

    #[test]
    fn test_session_limit() {
        let mgr = SessionManager::new(Duration::from_secs(30), 2, None);

        let c1: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let c2: SocketAddr = "10.0.0.1:5001".parse().unwrap();
        let c3: SocketAddr = "10.0.0.1:5002".parse().unwrap();
        let backend: SocketAddr = "10.0.0.2:8080".parse().unwrap();

        assert!(mgr.create_session(c1, backend));
        assert!(mgr.create_session(c2, backend));
        // Third should fail
        assert!(!mgr.create_session(c3, backend));
    }

    #[test]
    fn test_remove_session() {
        let mgr = SessionManager::new(Duration::from_secs(30), 1000, None);

        let client: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let backend: SocketAddr = "10.0.0.2:8080".parse().unwrap();

        mgr.create_session(client, backend);
        assert_eq!(mgr.session_count(), 1);

        let removed = mgr.remove_session(&client);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().backend_addr, backend);
        assert_eq!(mgr.session_count(), 0);
    }

    #[test]
    fn test_cleanup_expired() {
        let mgr = SessionManager::new(Duration::from_millis(50), 1000, None);

        let c1: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let c2: SocketAddr = "10.0.0.1:5001".parse().unwrap();
        let backend: SocketAddr = "10.0.0.2:8080".parse().unwrap();

        mgr.create_session(c1, backend);
        mgr.create_session(c2, backend);
        assert_eq!(mgr.session_count(), 2);

        // Wait for expiry
        std::thread::sleep(Duration::from_millis(100));

        let removed = mgr.cleanup_expired();
        assert_eq!(removed, 2);
        assert_eq!(mgr.session_count(), 0);
    }

    #[test]
    fn test_rate_limit_allows_within_limit() {
        let mgr = SessionManager::new(Duration::from_secs(30), 1000, Some(10));
        let client: SocketAddr = "10.0.0.1:5000".parse().unwrap();

        for _ in 0..10 {
            assert!(mgr.check_rate_limit(&client));
        }
        // 11th should be rejected
        assert!(!mgr.check_rate_limit(&client));
    }

    #[test]
    fn test_rate_limit_no_limit() {
        let mgr = SessionManager::new(Duration::from_secs(30), 1000, None);
        let client: SocketAddr = "10.0.0.1:5000".parse().unwrap();

        // Should always be allowed
        for _ in 0..1000 {
            assert!(mgr.check_rate_limit(&client));
        }
    }

    #[test]
    fn test_rate_limit_window_reset() {
        let mgr = SessionManager::new(Duration::from_secs(30), 1000, Some(5));
        let client: SocketAddr = "10.0.0.1:5000".parse().unwrap();

        // Exhaust the limit
        for _ in 0..5 {
            assert!(mgr.check_rate_limit(&client));
        }
        assert!(!mgr.check_rate_limit(&client));

        // Wait for window to reset
        std::thread::sleep(Duration::from_secs(1));

        // Should be allowed again
        assert!(mgr.check_rate_limit(&client));
    }

    #[tokio::test]
    async fn test_cleanup_task() {
        let mgr = Arc::new(SessionManager::new(Duration::from_millis(50), 1000, None));

        let client: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let backend: SocketAddr = "10.0.0.2:8080".parse().unwrap();
        mgr.create_session(client, backend);
        assert_eq!(mgr.session_count(), 1);

        let handle = mgr.start_cleanup_task();
        assert!(mgr.is_cleanup_running());

        // Wait long enough for the session to expire and cleanup to run.
        // Cleanup interval = timeout/2 = 25ms, so wait 150ms to be safe.
        tokio::time::sleep(Duration::from_millis(150)).await;

        assert_eq!(mgr.session_count(), 0);

        handle.stop();
        // Give the task time to notice the stop signal.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[test]
    fn test_session_touch_extends_lifetime() {
        let mgr = SessionManager::new(Duration::from_millis(100), 1000, None);

        let client: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let backend: SocketAddr = "10.0.0.2:8080".parse().unwrap();

        mgr.create_session(client, backend);

        // Access the session at 50ms intervals, before the 100ms timeout
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(
                mgr.get_session(&client),
                Some(backend),
                "session should still be alive after touch"
            );
        }

        // Total elapsed ~150ms, but session should still be alive because
        // each get_session() refreshes last_seen.
        assert_eq!(mgr.get_session(&client), Some(backend));
    }
}
