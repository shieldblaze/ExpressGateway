//! Slowloris / slow-POST connection watchdog (SEC-2-03).
//!
//! Provides a stable accept-time API that the Wave-2b call sites in
//! `crates/lb-l7/src/h{1,2}_proxy.rs` (and Wave-2c's
//! `crates/lb/src/main.rs` accept loop) drive without depending on
//! the existing per-connection [`SlowlorisDetector`] /
//! [`SlowPostDetector`] state-machine internals.
//!
//! Shape
//! -----
//!
//! ```ignore
//! let wd = Watchdog::new(WatchdogConfig::default());
//! let conn_id = ConnId::new(socket.peer_addr()?, fd);
//! wd.register(conn_id, Instant::now() + Duration::from_secs(5));
//! // ... on every read, on every parsed body frame, etc.
//! wd.progress(conn_id, bytes_read_cumulative)?;
//! // ... when the request is done:
//! wd.deregister(conn_id);
//! ```
//!
//! `progress` returns `Err(WatchdogError::Evicted)` when the
//! connection has exceeded its deadline (slow handshake / slowloris
//! header phase) **or** its observed rate has dropped below the
//! configured minimum (slow-POST body phase). The hot-path caller is
//! expected to map that to either a 408 Request Timeout (if a
//! response can still be written) or RST (if not).
//!
//! Eviction is **passive** — `progress` checks both conditions on
//! every call. A separate sweeper task can be spawned via
//! [`Watchdog::spawn_sweeper`] which calls
//! [`Watchdog::sweep_expired`] on a tick; the sweeper emits
//! eviction events through a [`tokio::sync::mpsc`] channel so the
//! listener loop can close the offending socket even while it's
//! parked in a slow-read.
//!
//! Sweeper lifecycle is owned by Wave-2c's `lb_core::Shutdown`
//! token; this crate exposes the entry points and a stable
//! `Watchdog::shutdown()` for the call site to invoke.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;

/// Identifier for a watched connection.
///
/// The watchdog itself is opaque about how the caller assigns IDs —
/// the common pattern is `(peer_ip, accept_seqno)` so two
/// simultaneous connections from the same NAT egress IP are
/// distinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnId {
    /// Peer IP. Stored for eviction logging only — not used for
    /// hashing equality is composed of `(ip, seq)`.
    pub peer: IpAddr,
    /// Caller-assigned sequence number. Unique within the listener
    /// across the lifetime of the watchdog.
    pub seq: u64,
}

impl ConnId {
    /// Construct from peer + seq.
    #[must_use]
    pub const fn new(peer: IpAddr, seq: u64) -> Self {
        Self { peer, seq }
    }
}

/// Outcome the [`Watchdog::progress`] call returns to the hot path.
#[derive(Debug, thiserror::Error)]
pub enum WatchdogError {
    /// Connection passed its registered deadline without the caller
    /// invoking [`Watchdog::deregister`]. Map to 408 / RST.
    #[error("watchdog evicted conn {0:?}: deadline exceeded")]
    Deadline(ConnId),

    /// Observed byte-rate over the most recent window dropped below
    /// `min_rate_bps`. Map to 408 / RST.
    #[error("watchdog evicted conn {conn:?}: rate {observed_bps} B/s below floor {floor_bps} B/s")]
    SlowRate {
        /// The evicted connection.
        conn: ConnId,
        /// Observed rate over the most recent window.
        observed_bps: u64,
        /// Configured floor.
        floor_bps: u64,
    },

    /// `progress` called for a connection that was never
    /// [`register`](Watchdog::register)ed. Surface to the caller as a
    /// programming error — the hot path should never silence this.
    #[error("watchdog: unknown connection {0:?}")]
    Unknown(ConnId),
}

/// Static configuration for a watchdog instance.
#[derive(Debug, Clone, Copy)]
pub struct WatchdogConfig {
    /// Minimum bytes-per-second over the most-recent window. `0`
    /// disables the rate check (deadline-only mode).
    pub min_rate_bps: u64,
    /// Window length over which the rate is computed. Must be
    /// non-zero.
    pub rate_window: Duration,
    /// Maximum number of concurrent registered connections. The
    /// watchdog's per-entry overhead is a `DashMap` slot
    /// (~64 bytes); a 100 000-conn ceiling is ~6 MB.
    pub max_registered: usize,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        // Defaults align with SEC-2-03 plan §Approach:
        //   header phase: 5 s total cap, 64 B/s min
        //   slow POST   : 10 s window, 256 B/s min
        // Pick the lower (header) bound here; the caller picks the
        // deadline per `register` call.
        Self {
            min_rate_bps: 64,
            rate_window: Duration::from_secs(1),
            max_registered: 100_000,
        }
    }
}

struct Entry {
    deadline: Instant,
    bytes_at_window_start: u64,
    window_started_at: Instant,
    last_bytes: u64,
    last_seen: Instant,
}

/// Per-connection slowloris / slow-POST watchdog.
///
/// Cheap to clone (`Arc` newtype). The Wave-2b call sites are
/// expected to hold a single watchdog per listener and clone it into
/// the per-task state.
#[derive(Clone)]
pub struct Watchdog {
    inner: Arc<WatchdogInner>,
}

struct WatchdogInner {
    config: WatchdogConfig,
    table: DashMap<ConnId, Entry>,
}

impl Watchdog {
    /// Build a new watchdog.
    #[must_use]
    pub fn new(config: WatchdogConfig) -> Self {
        Self {
            inner: Arc::new(WatchdogInner {
                config,
                table: DashMap::new(),
            }),
        }
    }

    /// Static configuration.
    #[must_use]
    pub fn config(&self) -> &WatchdogConfig {
        &self.inner.config
    }

    /// Current number of registered connections (snapshot).
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.table.len()
    }

    /// `true` if no connections are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.table.is_empty()
    }

    /// Register a new connection.
    ///
    /// Returns `false` if the table is already at
    /// `max_registered` capacity; the caller should reject the
    /// connection in that case (it is structurally equivalent to a
    /// listener-cap exhaustion).
    pub fn register(&self, id: ConnId, deadline: Instant) -> bool {
        if self.inner.table.len() >= self.inner.config.max_registered {
            return false;
        }
        let now = Instant::now();
        let entry = Entry {
            deadline,
            bytes_at_window_start: 0,
            window_started_at: now,
            last_bytes: 0,
            last_seen: now,
        };
        self.inner.table.insert(id, entry);
        true
    }

    /// Record progress (cumulative bytes read) for a connection and
    /// evaluate eviction rules.
    ///
    /// # Errors
    ///
    /// * [`WatchdogError::Deadline`] — registered deadline elapsed.
    /// * [`WatchdogError::SlowRate`] — rate over the most-recent
    ///   window is below the configured floor.
    /// * [`WatchdogError::Unknown`] — caller passed an unregistered id.
    pub fn progress(&self, id: ConnId, bytes_read: u64) -> Result<(), WatchdogError> {
        let now = Instant::now();
        // Snapshot the eviction decision under the bucket lock, then
        // release the lock before mutating the table.
        let mut evict_reason: Option<WatchdogError> = None;
        {
            let mut entry = match self.inner.table.get_mut(&id) {
                Some(e) => e,
                None => return Err(WatchdogError::Unknown(id)),
            };
            // Deadline check.
            if now > entry.deadline {
                evict_reason = Some(WatchdogError::Deadline(id));
            }
            // Rate check (only if no deadline trip and rate enabled).
            if evict_reason.is_none() && self.inner.config.min_rate_bps > 0 {
                let window_elapsed = now.saturating_duration_since(entry.window_started_at);
                if window_elapsed >= self.inner.config.rate_window {
                    let window_bytes = bytes_read.saturating_sub(entry.bytes_at_window_start);
                    let window_ms_total = window_elapsed.as_millis();
                    let window_ms = u64::try_from(window_ms_total).unwrap_or(u64::MAX);
                    if let Some(observed_bps) =
                        window_bytes.saturating_mul(1000).checked_div(window_ms)
                    {
                        if observed_bps < self.inner.config.min_rate_bps {
                            evict_reason = Some(WatchdogError::SlowRate {
                                conn: id,
                                observed_bps,
                                floor_bps: self.inner.config.min_rate_bps,
                            });
                        }
                    }
                    // Roll the window forward regardless of outcome.
                    entry.bytes_at_window_start = bytes_read;
                    entry.window_started_at = now;
                }
            }
            // Update the checkpoint.
            entry.last_bytes = bytes_read;
            entry.last_seen = now;
        }

        if let Some(reason) = evict_reason {
            // Evict from the table — the caller is closing the
            // socket on this error.
            self.inner.table.remove(&id);
            return Err(reason);
        }
        Ok(())
    }

    /// Remove a connection from the watchdog (clean shutdown path).
    ///
    /// Returns `true` if the entry existed.
    pub fn deregister(&self, id: ConnId) -> bool {
        self.inner.table.remove(&id).is_some()
    }

    /// Sweep all entries and remove any whose deadline has elapsed.
    ///
    /// Returns the set of evicted connection ids. Intended for a
    /// periodic sweeper task driven by `tokio::time::interval`;
    /// inline `progress` calls already cover the common path where
    /// the connection is making any progress at all. The sweeper
    /// closes the gap for connections that are completely stalled
    /// (no bytes received → no `progress` calls → deadline trip is
    /// only observed when the next byte arrives or when this sweeper
    /// runs).
    pub fn sweep_expired(&self) -> Vec<ConnId> {
        let now = Instant::now();
        let mut evicted = Vec::new();
        self.inner.table.retain(|id, entry| {
            if now > entry.deadline {
                evicted.push(*id);
                false
            } else {
                true
            }
        });
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::thread::sleep;

    fn conn(seq: u64) -> ConnId {
        ConnId::new(Ipv4Addr::LOCALHOST.into(), seq)
    }

    #[test]
    fn register_progress_deregister_roundtrip() {
        let wd = Watchdog::new(WatchdogConfig {
            min_rate_bps: 0,
            rate_window: Duration::from_secs(1),
            max_registered: 16,
        });
        let id = conn(1);
        assert!(wd.register(id, Instant::now() + Duration::from_secs(60)));
        assert_eq!(wd.len(), 1);
        wd.progress(id, 100).unwrap();
        wd.progress(id, 200).unwrap();
        assert!(wd.deregister(id));
        assert_eq!(wd.len(), 0);
    }

    #[test]
    fn deadline_evicts_via_progress() {
        let wd = Watchdog::new(WatchdogConfig {
            min_rate_bps: 0,
            rate_window: Duration::from_secs(1),
            max_registered: 16,
        });
        let id = conn(2);
        wd.register(id, Instant::now() + Duration::from_millis(10));
        sleep(Duration::from_millis(20));
        let err = wd.progress(id, 1).unwrap_err();
        assert!(matches!(err, WatchdogError::Deadline(_)));
        // Evicted from table.
        assert!(matches!(
            wd.progress(id, 2).unwrap_err(),
            WatchdogError::Unknown(_)
        ));
    }

    #[test]
    fn unknown_id_errs() {
        let wd = Watchdog::new(WatchdogConfig::default());
        assert!(matches!(
            wd.progress(conn(99), 0).unwrap_err(),
            WatchdogError::Unknown(_)
        ));
    }

    #[test]
    fn sweep_evicts_stalled_connections() {
        let wd = Watchdog::new(WatchdogConfig {
            min_rate_bps: 0,
            rate_window: Duration::from_secs(1),
            max_registered: 16,
        });
        let id_a = conn(10);
        let id_b = conn(11);
        wd.register(id_a, Instant::now() + Duration::from_millis(5));
        wd.register(id_b, Instant::now() + Duration::from_secs(60));
        sleep(Duration::from_millis(20));
        let evicted = wd.sweep_expired();
        assert_eq!(evicted, vec![id_a]);
        assert_eq!(wd.len(), 1);
    }

    #[test]
    fn max_registered_enforced() {
        let wd = Watchdog::new(WatchdogConfig {
            min_rate_bps: 0,
            rate_window: Duration::from_secs(1),
            max_registered: 2,
        });
        assert!(wd.register(conn(1), Instant::now() + Duration::from_secs(60)));
        assert!(wd.register(conn(2), Instant::now() + Duration::from_secs(60)));
        assert!(!wd.register(conn(3), Instant::now() + Duration::from_secs(60)));
    }
}
