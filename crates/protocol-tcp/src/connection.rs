//! Connection tracking for the TCP proxy.
//!
//! Tracks active connections through a state machine:
//! `Connecting -> Active -> Draining -> Closed`
//!
//! Provides an atomic connection counter and connection reset propagation
//! via `SO_LINGER=0` on RST.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use dashmap::DashMap;
use expressgateway_core::FourTuple;
use parking_lot::Mutex;

/// Connection states for the TCP proxy state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TcpConnectionState {
    /// Backend connection is being established.
    Connecting,
    /// Connection is active: bidirectional data forwarding in progress.
    Active,
    /// Connection is draining: no new data accepted, finishing in-flight.
    Draining,
    /// Connection is closed.
    Closed,
}

impl TcpConnectionState {
    /// Returns true if a transition from `self` to `next` is valid.
    pub fn can_transition_to(self, next: TcpConnectionState) -> bool {
        matches!(
            (self, next),
            (TcpConnectionState::Connecting, TcpConnectionState::Active)
                | (TcpConnectionState::Connecting, TcpConnectionState::Closed)
                | (TcpConnectionState::Active, TcpConnectionState::Draining)
                | (TcpConnectionState::Active, TcpConnectionState::Closed)
                | (TcpConnectionState::Draining, TcpConnectionState::Closed)
        )
    }
}

impl std::fmt::Display for TcpConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TcpConnectionState::Connecting => write!(f, "CONNECTING"),
            TcpConnectionState::Active => write!(f, "ACTIVE"),
            TcpConnectionState::Draining => write!(f, "DRAINING"),
            TcpConnectionState::Closed => write!(f, "CLOSED"),
        }
    }
}

/// Metadata for a single tracked TCP connection.
#[derive(Debug)]
pub struct TrackedConnection {
    /// The four-tuple identifying this connection.
    pub four_tuple: FourTuple,
    /// The backend address selected by the load balancer.
    pub backend_addr: SocketAddr,
    /// Current state.
    state: Mutex<TcpConnectionState>,
    /// When the connection was created.
    pub created_at: Instant,
    /// When the last data was transferred (either direction).
    pub last_activity: Mutex<Instant>,
}

impl TrackedConnection {
    /// Create a new tracked connection in the `Connecting` state.
    pub fn new(four_tuple: FourTuple, backend_addr: SocketAddr) -> Self {
        let now = Instant::now();
        Self {
            four_tuple,
            backend_addr,
            state: Mutex::new(TcpConnectionState::Connecting),
            created_at: now,
            last_activity: Mutex::new(now),
        }
    }

    /// Get the current state.
    pub fn state(&self) -> TcpConnectionState {
        *self.state.lock()
    }

    /// Transition to a new state. Returns `Ok(())` if the transition is valid,
    /// or `Err` with the current state if the transition is not allowed.
    pub fn transition(&self, next: TcpConnectionState) -> Result<(), TcpConnectionState> {
        let mut state = self.state.lock();
        if state.can_transition_to(next) {
            *state = next;
            Ok(())
        } else {
            Err(*state)
        }
    }

    /// Update the last activity timestamp.
    pub fn touch(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    /// Duration since this connection was created.
    pub fn age(&self) -> std::time::Duration {
        self.created_at.elapsed()
    }

    /// Duration since last activity.
    pub fn idle_time(&self) -> std::time::Duration {
        self.last_activity.lock().elapsed()
    }
}

/// Tracks all active TCP connections with an atomic counter.
pub struct ConnectionTracker {
    /// Active connections keyed by four-tuple.
    connections: DashMap<FourTuple, Arc<TrackedConnection>>,
    /// Atomic counter for fast O(1) connection count.
    count: AtomicUsize,
    /// Maximum number of connections allowed.
    max_connections: usize,
    /// Total connections ever accepted (monotonic counter for metrics).
    total_accepted: AtomicU64,
}

impl ConnectionTracker {
    /// Create a new connection tracker with the given maximum.
    pub fn new(max_connections: usize) -> Self {
        Self {
            connections: DashMap::new(),
            count: AtomicUsize::new(0),
            max_connections,
            total_accepted: AtomicU64::new(0),
        }
    }

    /// Try to register a new connection. Returns `None` if the limit is reached.
    pub fn try_add(
        &self,
        four_tuple: FourTuple,
        backend_addr: SocketAddr,
    ) -> Option<Arc<TrackedConnection>> {
        // CAS loop to atomically check-and-increment
        loop {
            let current = self.count.load(Ordering::Acquire);
            if current >= self.max_connections {
                return None;
            }
            if self
                .count
                .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }

        let conn = Arc::new(TrackedConnection::new(four_tuple, backend_addr));
        self.connections.insert(four_tuple, conn.clone());
        self.total_accepted.fetch_add(1, Ordering::Relaxed);
        Some(conn)
    }

    /// Remove a connection from the tracker.
    pub fn remove(&self, four_tuple: &FourTuple) -> Option<Arc<TrackedConnection>> {
        if let Some((_, conn)) = self.connections.remove(four_tuple) {
            self.count.fetch_sub(1, Ordering::AcqRel);
            Some(conn)
        } else {
            None
        }
    }

    /// Get a connection by its four-tuple.
    pub fn get(&self, four_tuple: &FourTuple) -> Option<Arc<TrackedConnection>> {
        self.connections.get(four_tuple).map(|r| r.value().clone())
    }

    /// Current number of active connections.
    pub fn active_count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Total connections ever accepted.
    pub fn total_accepted(&self) -> u64 {
        self.total_accepted.load(Ordering::Relaxed)
    }

    /// Maximum allowed connections.
    pub fn max_connections(&self) -> usize {
        self.max_connections
    }

    /// Iterate over all connections and collect those matching a predicate.
    pub fn connections_in_state(&self, state: TcpConnectionState) -> Vec<Arc<TrackedConnection>> {
        self.connections
            .iter()
            .filter(|entry| entry.value().state() == state)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Mark all active connections as draining.
    pub fn drain_all(&self) {
        for entry in self.connections.iter() {
            let conn = entry.value();
            // Best-effort transition; already-draining or closed connections are fine.
            let _ = conn.transition(TcpConnectionState::Draining);
        }
    }
}

/// Configure a TCP socket for RST propagation (SO_LINGER with timeout 0).
///
/// When a connection is reset (rather than gracefully closed), setting
/// SO_LINGER to 0 causes the kernel to send a RST instead of FIN,
/// propagating the reset to the peer.
pub fn set_rst_linger(socket: &tokio::net::TcpStream) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = socket.as_raw_fd();

    let linger = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    let ret = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            &linger as *const libc::linger as *const libc::c_void,
            std::mem::size_of::<libc::linger>() as libc::socklen_t,
        )
    };
    if ret != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions() {
        let conn = TrackedConnection::new(
            FourTuple {
                src_addr: "10.0.0.1:1234".parse().unwrap(),
                dst_addr: "10.0.0.2:80".parse().unwrap(),
            },
            "10.0.0.3:8080".parse().unwrap(),
        );

        assert_eq!(conn.state(), TcpConnectionState::Connecting);

        // Connecting -> Active: valid
        assert!(conn.transition(TcpConnectionState::Active).is_ok());
        assert_eq!(conn.state(), TcpConnectionState::Active);

        // Active -> Draining: valid
        assert!(conn.transition(TcpConnectionState::Draining).is_ok());
        assert_eq!(conn.state(), TcpConnectionState::Draining);

        // Draining -> Closed: valid
        assert!(conn.transition(TcpConnectionState::Closed).is_ok());
        assert_eq!(conn.state(), TcpConnectionState::Closed);
    }

    #[test]
    fn test_invalid_transitions() {
        let conn = TrackedConnection::new(
            FourTuple {
                src_addr: "10.0.0.1:1234".parse().unwrap(),
                dst_addr: "10.0.0.2:80".parse().unwrap(),
            },
            "10.0.0.3:8080".parse().unwrap(),
        );

        // Connecting -> Draining: invalid
        assert!(conn.transition(TcpConnectionState::Draining).is_err());

        // Connecting -> Connecting: invalid
        assert!(conn.transition(TcpConnectionState::Connecting).is_err());
    }

    #[test]
    fn test_connecting_to_closed() {
        // Backend connection failed immediately
        let conn = TrackedConnection::new(
            FourTuple {
                src_addr: "10.0.0.1:1234".parse().unwrap(),
                dst_addr: "10.0.0.2:80".parse().unwrap(),
            },
            "10.0.0.3:8080".parse().unwrap(),
        );

        assert!(conn.transition(TcpConnectionState::Closed).is_ok());
        assert_eq!(conn.state(), TcpConnectionState::Closed);
    }

    #[test]
    fn test_active_to_closed() {
        // Abrupt close (e.g., RST) skipping drain
        let conn = TrackedConnection::new(
            FourTuple {
                src_addr: "10.0.0.1:1234".parse().unwrap(),
                dst_addr: "10.0.0.2:80".parse().unwrap(),
            },
            "10.0.0.3:8080".parse().unwrap(),
        );

        assert!(conn.transition(TcpConnectionState::Active).is_ok());
        assert!(conn.transition(TcpConnectionState::Closed).is_ok());
    }

    #[test]
    fn test_connection_tracker_basic() {
        let tracker = ConnectionTracker::new(100);
        let ft = FourTuple {
            src_addr: "10.0.0.1:1234".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
        };

        let conn = tracker
            .try_add(ft, "10.0.0.3:8080".parse().unwrap())
            .expect("should succeed");
        assert_eq!(tracker.active_count(), 1);
        assert_eq!(tracker.total_accepted(), 1);
        assert_eq!(conn.state(), TcpConnectionState::Connecting);

        // Get by four-tuple
        assert!(tracker.get(&ft).is_some());

        // Remove
        let removed = tracker.remove(&ft);
        assert!(removed.is_some());
        assert_eq!(tracker.active_count(), 0);
        assert_eq!(tracker.total_accepted(), 1); // monotonic
    }

    #[test]
    fn test_connection_tracker_limit() {
        let tracker = ConnectionTracker::new(2);
        let ft1 = FourTuple {
            src_addr: "10.0.0.1:1000".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
        };
        let ft2 = FourTuple {
            src_addr: "10.0.0.1:1001".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
        };
        let ft3 = FourTuple {
            src_addr: "10.0.0.1:1002".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
        };

        assert!(
            tracker
                .try_add(ft1, "10.0.0.3:8080".parse().unwrap())
                .is_some()
        );
        assert!(
            tracker
                .try_add(ft2, "10.0.0.3:8080".parse().unwrap())
                .is_some()
        );
        // Third should be rejected
        assert!(
            tracker
                .try_add(ft3, "10.0.0.3:8080".parse().unwrap())
                .is_none()
        );
        assert_eq!(tracker.active_count(), 2);

        // Remove one, then third should succeed
        tracker.remove(&ft1);
        assert!(
            tracker
                .try_add(ft3, "10.0.0.3:8080".parse().unwrap())
                .is_some()
        );
    }

    #[test]
    fn test_drain_all() {
        let tracker = ConnectionTracker::new(100);
        let ft1 = FourTuple {
            src_addr: "10.0.0.1:1000".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
        };
        let ft2 = FourTuple {
            src_addr: "10.0.0.1:1001".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
        };

        let c1 = tracker
            .try_add(ft1, "10.0.0.3:8080".parse().unwrap())
            .unwrap();
        let c2 = tracker
            .try_add(ft2, "10.0.0.3:8080".parse().unwrap())
            .unwrap();

        // Move to Active first
        c1.transition(TcpConnectionState::Active).unwrap();
        c2.transition(TcpConnectionState::Active).unwrap();

        tracker.drain_all();

        assert_eq!(c1.state(), TcpConnectionState::Draining);
        assert_eq!(c2.state(), TcpConnectionState::Draining);
    }

    #[test]
    fn test_connections_in_state() {
        let tracker = ConnectionTracker::new(100);
        let ft1 = FourTuple {
            src_addr: "10.0.0.1:1000".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
        };
        let ft2 = FourTuple {
            src_addr: "10.0.0.1:1001".parse().unwrap(),
            dst_addr: "10.0.0.2:80".parse().unwrap(),
        };

        let c1 = tracker
            .try_add(ft1, "10.0.0.3:8080".parse().unwrap())
            .unwrap();
        let _c2 = tracker
            .try_add(ft2, "10.0.0.3:8080".parse().unwrap())
            .unwrap();

        c1.transition(TcpConnectionState::Active).unwrap();

        let connecting = tracker.connections_in_state(TcpConnectionState::Connecting);
        assert_eq!(connecting.len(), 1);

        let active = tracker.connections_in_state(TcpConnectionState::Active);
        assert_eq!(active.len(), 1);
    }

    #[test]
    fn test_touch_updates_activity() {
        let conn = TrackedConnection::new(
            FourTuple {
                src_addr: "10.0.0.1:1234".parse().unwrap(),
                dst_addr: "10.0.0.2:80".parse().unwrap(),
            },
            "10.0.0.3:8080".parse().unwrap(),
        );

        let idle_before = conn.idle_time();
        // Spin briefly to let some time pass
        std::thread::sleep(std::time::Duration::from_millis(10));
        conn.touch();
        let idle_after = conn.idle_time();

        assert!(idle_after < idle_before + std::time::Duration::from_millis(50));
    }
}
