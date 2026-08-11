//! Slowloris / slow-POST connection watchdog (SEC-2-03).
//! SCOPE (F-RES-5, S38): this DETECTS, it does not ENFORCE. `progress` is called once per request,
//! never per body frame, so `SlowRate` is dormant by design, and the sweeper logs rather than
//! closing (closing would race the drain coordinator). The bounds that actually close a stalled
//! connection live in the timeout stack: hyper `header_read_timeout` (F-RES-1), `idle_bounded_send`
//! Phase-A + `HttpTimeouts::total`, the H2 keepalive PING, and QUIC `set_max_idle_timeout`.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;

/// Watched-connection id; `(peer_ip, accept_seqno)` keeps two conns behind one NAT IP distinct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnId {
    /// Peer IP, for eviction logging.
    pub peer: IpAddr,
    /// Caller-assigned sequence number, unique within the listener.
    pub seq: u64,
}

impl ConnId {
    /// Construct from peer + seq.
    #[must_use]
    pub const fn new(peer: IpAddr, seq: u64) -> Self {
        Self { peer, seq }
    }
}

/// Outcome the [`Watchdog::progress`] call returns to the hot path; map either eviction to 408 / RST.
#[derive(Debug, thiserror::Error)]
pub enum WatchdogError {
    /// Registered deadline elapsed without a [`Watchdog::deregister`].
    #[error("watchdog evicted conn {0:?}: deadline exceeded")]
    Deadline(ConnId),

    /// Byte-rate over the most recent window dropped below `min_rate_bps`.
    #[error("watchdog evicted conn {conn:?}: rate {observed_bps} B/s below floor {floor_bps} B/s")]
    SlowRate {
        /// The evicted connection.
        conn: ConnId,
        /// Observed rate over the most recent window.
        observed_bps: u64,
        /// Configured floor.
        floor_bps: u64,
    },

    /// `progress` on an unregistered id — a caller bug; never silence it on the hot path.
    #[error("watchdog: unknown connection {0:?}")]
    Unknown(ConnId),
}

/// Static configuration for a watchdog instance.
#[derive(Debug, Clone, Copy)]
pub struct WatchdogConfig {
    /// Minimum bytes-per-second over the most-recent window; `0` disables the rate check.
    pub min_rate_bps: u64,
    /// Window over which the rate is computed. Must be non-zero.
    pub rate_window: Duration,
    /// Concurrent-registration ceiling (~64 B per `DashMap` slot, so 100 000 conns ≈ 6 MB).
    pub max_registered: usize,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        // SEC-2-03 header-phase bound; the caller sets the per-`register` deadline.
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

/// Per-connection slowloris / slow-POST watchdog; cheap to clone (`Arc` newtype), one per listener.
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

    /// Register a connection; `false` means the table is at `max_registered` and the caller must reject.
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

    /// Record cumulative bytes read and evaluate the eviction rules.
    pub fn progress(&self, id: ConnId, bytes_read: u64) -> Result<(), WatchdogError> {
        let now = Instant::now();
        // Decide under the bucket lock, mutate the table only after releasing it.
        let mut evict_reason: Option<WatchdogError> = None;
        {
            let mut entry = match self.inner.table.get_mut(&id) {
                Some(e) => e,
                None => return Err(WatchdogError::Unknown(id)),
            };
            if now > entry.deadline {
                evict_reason = Some(WatchdogError::Deadline(id));
            }
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
                    entry.bytes_at_window_start = bytes_read;
                    entry.window_started_at = now;
                }
            }
            entry.last_bytes = bytes_read;
            entry.last_seen = now;
        }

        if let Some(reason) = evict_reason {
            self.inner.table.remove(&id);
            return Err(reason);
        }
        Ok(())
    }

    /// Remove a connection (clean shutdown path); `true` if the entry existed.
    pub fn deregister(&self, id: ConnId) -> bool {
        self.inner.table.remove(&id).is_some()
    }

    /// Drop and return every entry past its deadline — a fully stalled conn never calls `progress`.
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
