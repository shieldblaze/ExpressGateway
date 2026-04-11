//! Connection rate limiting with sliding window counters per IP.
//!
//! Uses a `DashMap` for lock-free concurrent reads and bounded memory
//! through periodic eviction of expired entries.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::trace;

/// Per-IP rate limiting entry.
struct RateLimitEntry {
    /// Number of connections in the current window.
    count: u32,
    /// Start of the current window.
    window_start: Instant,
}

/// Connection rate limiter using sliding window counters.
///
/// Each IP address gets an independent window. When an IP exceeds
/// `max_per_ip` connections within `window`, further connections are
/// rejected until the window resets.
///
/// Memory is bounded by `max_tracked` entries; when this limit is reached,
/// expired entries are evicted. If the map is still full after eviction,
/// new IPs are allowed through (fail-open).
pub struct ConnectionRateLimiter {
    limits: DashMap<IpAddr, RateLimitEntry>,
    max_per_ip: u32,
    window: Duration,
    max_tracked: usize,
}

impl ConnectionRateLimiter {
    /// Create a new rate limiter.
    ///
    /// - `max_per_ip`: maximum connections per IP within the window.
    /// - `window`: duration of the sliding window.
    /// - `max_tracked`: maximum number of IPs to track (default 100,000).
    pub fn new(max_per_ip: u32, window: Duration, max_tracked: usize) -> Self {
        Self {
            limits: DashMap::with_capacity(max_tracked.min(1024)),
            max_per_ip,
            window,
            max_tracked,
        }
    }

    /// Check if the given IP is allowed and increment its counter.
    ///
    /// Returns `true` if the connection is allowed, `false` if the rate
    /// limit has been exceeded.
    ///
    /// Note: there is a benign TOCTOU race between `get_mut` returning `None`
    /// and `insert` for the same IP from two threads. The worst case is that
    /// the second `insert` overwrites the first, resetting the counter once.
    /// This is acceptable for rate limiting -- the window is one connection's
    /// worth of slack on first appearance of a new IP under contention. Using
    /// `DashMap::entry` would deadlock because we also need `len()` and
    /// `retain()`, which acquire shard locks.
    pub fn check_and_increment(&self, ip: &IpAddr) -> bool {
        let now = Instant::now();

        // Fast path: existing entry.
        if let Some(mut entry) = self.limits.get_mut(ip) {
            let elapsed = now.duration_since(entry.window_start);
            if elapsed >= self.window {
                entry.count = 1;
                entry.window_start = now;
                trace!(?ip, "rate limit window reset");
                return true;
            }
            if entry.count >= self.max_per_ip {
                trace!(?ip, count = entry.count, "rate limit exceeded");
                return false;
            }
            entry.count += 1;
            return true;
        }

        // Slow path: new IP.
        if self.limits.len() >= self.max_tracked {
            self.evict_expired(now);
        }

        if self.limits.len() >= self.max_tracked {
            trace!(?ip, "rate limiter at capacity, allowing new IP");
            return true;
        }

        self.limits.insert(
            *ip,
            RateLimitEntry {
                count: 1,
                window_start: now,
            },
        );
        true
    }

    /// Return the number of currently tracked IPs.
    pub fn tracked_ips(&self) -> usize {
        self.limits.len()
    }

    /// Remove entries whose windows have fully expired (2x window duration).
    fn evict_expired(&self, now: Instant) {
        let ttl = self.window * 2;
        self.limits
            .retain(|_ip, entry| now.duration_since(entry.window_start) < ttl);
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_basic_rate_limiting() {
        let limiter = ConnectionRateLimiter::new(3, Duration::from_secs(60), 1000);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        assert!(limiter.check_and_increment(&ip)); // 1
        assert!(limiter.check_and_increment(&ip)); // 2
        assert!(limiter.check_and_increment(&ip)); // 3
        assert!(!limiter.check_and_increment(&ip)); // exceeded
    }

    #[test]
    fn test_different_ips_independent() {
        let limiter = ConnectionRateLimiter::new(2, Duration::from_secs(60), 1000);
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        assert!(limiter.check_and_increment(&ip1));
        assert!(limiter.check_and_increment(&ip1));
        assert!(!limiter.check_and_increment(&ip1));

        // ip2 should still be allowed
        assert!(limiter.check_and_increment(&ip2));
        assert!(limiter.check_and_increment(&ip2));
    }

    #[test]
    fn test_tracked_ips() {
        let limiter = ConnectionRateLimiter::new(10, Duration::from_secs(60), 1000);
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        assert_eq!(limiter.tracked_ips(), 0);
        limiter.check_and_increment(&ip1);
        assert_eq!(limiter.tracked_ips(), 1);
        limiter.check_and_increment(&ip2);
        assert_eq!(limiter.tracked_ips(), 2);
    }

    #[test]
    fn test_max_tracked_eviction_and_failopen() {
        let limiter = ConnectionRateLimiter::new(10, Duration::from_millis(1), 2);
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        let ip3 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));

        limiter.check_and_increment(&ip1);
        limiter.check_and_increment(&ip2);
        assert_eq!(limiter.tracked_ips(), 2);

        // Wait for entries to expire
        std::thread::sleep(Duration::from_millis(5));

        // ip3 should trigger eviction and succeed
        assert!(limiter.check_and_increment(&ip3));
    }

    #[test]
    fn test_window_reset() {
        let limiter = ConnectionRateLimiter::new(1, Duration::from_millis(1), 1000);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        assert!(limiter.check_and_increment(&ip));
        assert!(!limiter.check_and_increment(&ip));

        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(5));

        // Should be allowed again
        assert!(limiter.check_and_increment(&ip));
    }

    #[test]
    fn test_zero_limit() {
        let limiter = ConnectionRateLimiter::new(0, Duration::from_secs(60), 1000);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        // With max 0, the first request inserts count=1, which is >= 0 on subsequent calls
        // but first call inserts then returns true (count goes to 1 which is > 0)
        // Actually: first call is a new IP path, inserts with count=1, returns true.
        // Second call: count(1) >= max(0), so denied.
        assert!(limiter.check_and_increment(&ip)); // first insert
        assert!(!limiter.check_and_increment(&ip)); // already at 1 >= 0
    }

    #[test]
    fn test_ipv6_rate_limiting() {
        let limiter = ConnectionRateLimiter::new(2, Duration::from_secs(60), 1000);
        let ip: IpAddr = "2001:db8::1".parse().unwrap();

        assert!(limiter.check_and_increment(&ip));
        assert!(limiter.check_and_increment(&ip));
        assert!(!limiter.check_and_increment(&ip));
    }
}
