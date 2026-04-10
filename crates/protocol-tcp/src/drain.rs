//! Graceful drain support for the TCP proxy.
//!
//! When draining, the proxy:
//! 1. Stops accepting new connections.
//! 2. Marks all active connections as draining.
//! 3. Waits for existing connections to complete, up to a configurable timeout.
//! 4. Forcibly closes any remaining connections after the timeout.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Notify;
use tracing::{info, warn};

use crate::connection::ConnectionTracker;

/// Manages the graceful drain lifecycle.
pub struct DrainHandle {
    /// Whether draining has been initiated.
    draining: AtomicBool,
    /// Notified when all connections have finished draining.
    all_drained: Arc<Notify>,
    /// Drain timeout.
    timeout: Duration,
}

impl DrainHandle {
    /// Create a new drain handle with the given timeout.
    pub fn new(timeout: Duration) -> Self {
        Self {
            draining: AtomicBool::new(false),
            all_drained: Arc::new(Notify::new()),
            timeout,
        }
    }

    /// Whether draining has been initiated.
    #[inline]
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    /// Initiate draining: mark all connections as draining and stop accepting new ones.
    ///
    /// Returns `false` if already draining.
    pub fn start_drain(&self, tracker: &ConnectionTracker) -> bool {
        if self
            .draining
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return false;
        }

        info!("TCP proxy entering drain mode, timeout={:?}", self.timeout);
        tracker.drain_all();
        true
    }

    /// Wait for all connections to drain, up to the configured timeout.
    ///
    /// Returns `true` if all connections drained before the timeout,
    /// `false` if the timeout was reached.
    pub async fn wait_for_drain(&self, tracker: &ConnectionTracker) -> bool {
        let notifier = self.all_drained.clone();

        // Poll the connection count at a reasonable interval.
        let poll_interval = Duration::from_millis(100);
        let check = async {
            loop {
                if tracker.active_count() == 0 {
                    notifier.notify_one();
                    return;
                }
                tokio::time::sleep(poll_interval).await;
            }
        };

        tokio::select! {
            _ = check => {
                info!("All connections drained successfully");
                true
            }
            _ = tokio::time::sleep(self.timeout) => {
                let remaining = tracker.active_count();
                warn!(
                    remaining,
                    "Drain timeout reached ({:?}), forcibly closing remaining connections",
                    self.timeout,
                );
                false
            }
        }
    }

    /// Convenience: start drain and wait for completion.
    pub async fn drain(&self, tracker: &ConnectionTracker) -> bool {
        self.start_drain(tracker);
        self.wait_for_drain(tracker).await
    }

    /// Get the drain timeout.
    #[inline]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{ConnectionTracker, TcpConnectionState};
    use expressgateway_core::FourTuple;

    #[test]
    fn test_start_drain_idempotent() {
        let handle = DrainHandle::new(Duration::from_secs(30));
        let tracker = ConnectionTracker::new(100);

        assert!(!handle.is_draining());
        assert!(handle.start_drain(&tracker));
        assert!(handle.is_draining());
        // Second call returns false
        assert!(!handle.start_drain(&tracker));
    }

    #[tokio::test]
    async fn test_drain_completes_when_empty() {
        let handle = DrainHandle::new(Duration::from_secs(5));
        let tracker = ConnectionTracker::new(100);

        // No connections: drain should complete immediately.
        let result = handle.drain(&tracker).await;
        assert!(result);
    }

    #[tokio::test]
    async fn test_drain_waits_for_connections() {
        let handle = DrainHandle::new(Duration::from_secs(5));
        let tracker = Arc::new(ConnectionTracker::new(100));

        let ft = FourTuple {
            src_addr: "10.0.0.1:1234".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
        };
        let conn = tracker
            .try_add(ft, "10.0.0.3:8080".parse().unwrap())
            .unwrap();
        conn.transition(TcpConnectionState::Active).unwrap();

        handle.start_drain(&tracker);
        assert_eq!(conn.state(), TcpConnectionState::Draining);

        // Remove the connection after a short delay.
        let tracker_clone = tracker.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            tracker_clone.remove(&ft);
        });

        let result = handle.wait_for_drain(&tracker).await;
        assert!(result);
        assert_eq!(tracker.active_count(), 0);
    }

    #[tokio::test]
    async fn test_drain_timeout() {
        let handle = DrainHandle::new(Duration::from_millis(200));
        let tracker = ConnectionTracker::new(100);

        let ft = FourTuple {
            src_addr: "10.0.0.1:1234".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
        };
        let _conn = tracker
            .try_add(ft, "10.0.0.3:8080".parse().unwrap())
            .unwrap();

        // Connection never gets removed, so drain should time out.
        let result = handle.drain(&tracker).await;
        assert!(!result);
        assert_eq!(tracker.active_count(), 1);
    }
}
