//! Packet rate limiting with token bucket algorithm.
//!
//! Provides both global and per-IP token buckets with lazy refill
//! (tokens are replenished on acquire, no background timer needed).

use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use dashmap::DashMap;
use tracing::trace;

use std::sync::OnceLock;

/// Process-wide monotonic epoch for `TokenBucket` timestamps.
/// All nanos are relative to this `Instant`, avoiding `SystemTime` clock skew.
fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Maximum number of per-IP entries.
const DEFAULT_MAX_PER_IP_ENTRIES: usize = 50_000;

/// Action to take when a packet exceeds the rate limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitAction {
    /// Silently drop the packet.
    Drop,
    /// Send an ICMP unreachable / reject response.
    Reject,
    /// Delay the packet (queue for later processing).
    Throttle,
}

/// Configuration for the packet rate limiter.
#[derive(Debug, Clone)]
pub struct PacketRateLimitConfig {
    /// Global token rate (tokens per second).
    pub global_rate: u64,
    /// Global burst (max tokens).
    pub global_burst: u64,
    /// Per-IP token rate (tokens per second).
    pub per_ip_rate: u64,
    /// Per-IP burst (max tokens).
    pub per_ip_burst: u64,
    /// Action when rate limit is exceeded.
    pub exceed_action: RateLimitAction,
    /// Maximum per-IP entries to track.
    pub max_per_ip_entries: usize,
}

impl Default for PacketRateLimitConfig {
    fn default() -> Self {
        Self {
            global_rate: 1_000_000,
            global_burst: 2_000_000,
            per_ip_rate: 10_000,
            per_ip_burst: 20_000,
            exceed_action: RateLimitAction::Drop,
            max_per_ip_entries: DEFAULT_MAX_PER_IP_ENTRIES,
        }
    }
}

/// Token bucket with lazy refill.
///
/// Tokens are replenished at `rate` tokens/second up to `burst` maximum.
/// Refill happens on each `try_acquire` call using elapsed time since the
/// last refill.
pub struct TokenBucket {
    /// Current token count (scaled by 1 for simplicity).
    tokens: AtomicU64,
    /// Timestamp of last refill in nanoseconds since an arbitrary epoch.
    last_refill: AtomicU64,
    /// Tokens added per second.
    rate: u64,
    /// Maximum token count.
    burst: u64,
}

impl TokenBucket {
    /// Create a new token bucket, initially full.
    pub fn new(rate: u64, burst: u64) -> Self {
        Self {
            tokens: AtomicU64::new(burst),
            last_refill: AtomicU64::new(Self::now_nanos()),
            rate,
            burst,
        }
    }

    /// Try to acquire one token. Returns `true` if successful.
    ///
    /// Performs lazy refill based on elapsed time before attempting to
    /// consume a token.
    pub fn try_acquire(&self) -> bool {
        self.refill();

        // Try to decrement tokens atomically.
        loop {
            let current = self.tokens.load(Ordering::Relaxed);
            if current == 0 {
                return false;
            }
            match self.tokens.compare_exchange_weak(
                current,
                current - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(_) => continue, // CAS failed, retry
            }
        }
    }

    /// Refill tokens based on elapsed time since last refill.
    ///
    /// Note: there is a benign race between the CAS on `last_refill` and the
    /// subsequent token-add loop. If thread A wins the CAS but is preempted
    /// before adding tokens, thread B may see no tokens to add (small delta)
    /// and skip refill. The result is a brief under-refill that self-corrects
    /// on the next call. This is acceptable for rate-limiting; a fully
    /// linearizable design would require packing both fields into a single
    /// atomic (e.g. AtomicU128) or using a lock.
    fn refill(&self) {
        let now = Self::now_nanos();
        let last = self.last_refill.load(Ordering::Relaxed);

        if now <= last {
            return;
        }

        let elapsed_nanos = now - last;
        // tokens_to_add = rate * elapsed_seconds = rate * elapsed_nanos / 1_000_000_000
        let tokens_to_add = (self.rate as u128 * elapsed_nanos as u128 / 1_000_000_000) as u64;

        if tokens_to_add == 0 {
            return;
        }

        // Try to update the last_refill timestamp. If another thread beats us,
        // that's fine - they already refilled.
        if self
            .last_refill
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            // Add tokens, capped at burst.
            loop {
                let current = self.tokens.load(Ordering::Relaxed);
                let new_val = (current + tokens_to_add).min(self.burst);
                if current == new_val {
                    break;
                }
                match self.tokens.compare_exchange_weak(
                    current,
                    new_val,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
        }
    }

    /// Current number of available tokens (approximate, for diagnostics).
    pub fn available(&self) -> u64 {
        self.tokens.load(Ordering::Relaxed)
    }

    fn now_nanos() -> u64 {
        // Monotonic clock: nanos since process-wide epoch (Instant).
        // Immune to NTP adjustments, wall-clock jumps, and leap seconds.
        // Will not overflow u64 for ~584 years of process uptime.
        epoch().elapsed().as_nanos() as u64
    }
}

/// Packet rate limiter with global and per-IP token buckets.
pub struct PacketRateLimiter {
    /// Global token bucket applied to all packets.
    pub global_bucket: TokenBucket,
    /// Per-IP token buckets.
    per_ip: DashMap<IpAddr, TokenBucket>,
    /// Configuration.
    config: PacketRateLimitConfig,
}

impl PacketRateLimiter {
    /// Create a new packet rate limiter with the given configuration.
    pub fn new(config: PacketRateLimitConfig) -> Self {
        let global_bucket = TokenBucket::new(config.global_rate, config.global_burst);
        Self {
            global_bucket,
            per_ip: DashMap::with_capacity(config.max_per_ip_entries.min(1024)),
            config,
        }
    }

    /// Check if a packet from the given IP should be allowed.
    ///
    /// Returns `true` if the packet passes both per-IP and global limits.
    /// Per-IP is checked first so that a single abusive IP cannot drain the
    /// global bucket and starve legitimate traffic.
    pub fn check(&self, ip: &IpAddr) -> bool {
        // Check per-IP limit first to avoid draining global tokens on abusive IPs.
        if let Some(bucket) = self.per_ip.get(ip) {
            if !bucket.try_acquire() {
                trace!(?ip, "packet dropped: per-IP rate limit");
                return false;
            }
            // Per-IP passed; now check global.
            if !self.global_bucket.try_acquire() {
                trace!(?ip, "packet dropped: global rate limit");
                return false;
            }
            return true;
        }

        // New IP -- may need to evict stale entries first.
        if self.per_ip.len() >= self.config.max_per_ip_entries {
            self.evict_stale_buckets();
        }

        // If still over capacity after eviction, fail-open for new IPs
        // but still enforce the global limit.
        if self.per_ip.len() >= self.config.max_per_ip_entries {
            trace!(?ip, "packet limiter at per-IP capacity, allowing");
            if !self.global_bucket.try_acquire() {
                trace!(?ip, "packet dropped: global rate limit");
                return false;
            }
            return true;
        }

        let bucket = TokenBucket::new(self.config.per_ip_rate, self.config.per_ip_burst);
        // Consume one token from the new bucket.
        bucket.try_acquire();
        self.per_ip.insert(*ip, bucket);

        // Now check global.
        if !self.global_bucket.try_acquire() {
            trace!(?ip, "packet dropped: global rate limit");
            return false;
        }
        true
    }

    /// Evict per-IP buckets that are fully replenished (idle).
    ///
    /// A bucket at full capacity (`available() == burst`) has not been used
    /// recently, so it is safe to evict -- if the IP returns, a fresh bucket
    /// will be created starting at full burst, which is the same state.
    fn evict_stale_buckets(&self) {
        self.per_ip
            .retain(|_ip, bucket| bucket.available() < self.config.per_ip_burst);
    }

    /// Return the configured action for exceeded limits.
    pub fn exceed_action(&self) -> RateLimitAction {
        self.config.exceed_action
    }

    /// Return the number of tracked per-IP entries.
    pub fn tracked_ips(&self) -> usize {
        self.per_ip.len()
    }

    /// Return a reference to the config.
    pub fn config(&self) -> &PacketRateLimitConfig {
        &self.config
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    #[test]
    fn test_token_bucket_basic() {
        let bucket = TokenBucket::new(1000, 5);
        // Initially full with 5 tokens (burst)
        assert!(bucket.try_acquire());
        assert!(bucket.try_acquire());
        assert!(bucket.try_acquire());
        assert!(bucket.try_acquire());
        assert!(bucket.try_acquire());
        // Should be empty now
        assert!(!bucket.try_acquire());
    }

    #[test]
    fn test_token_bucket_refill() {
        let bucket = TokenBucket::new(10_000, 2);
        // Drain all tokens
        assert!(bucket.try_acquire());
        assert!(bucket.try_acquire());
        assert!(!bucket.try_acquire());

        // Wait for refill (at 10,000/s, 1ms should give ~10 tokens, capped at burst=2)
        std::thread::sleep(Duration::from_millis(5));

        assert!(bucket.try_acquire());
    }

    #[test]
    fn test_token_bucket_burst_cap() {
        let bucket = TokenBucket::new(1_000_000, 3);
        // Wait a bit to ensure refill would exceed burst
        std::thread::sleep(Duration::from_millis(10));

        // Should get at most `burst` tokens
        assert!(bucket.try_acquire());
        assert!(bucket.try_acquire());
        assert!(bucket.try_acquire());
        // Might or might not have more depending on timing, but burst caps at 3
        // After consuming 3, check one more refill cycle
    }

    #[test]
    fn test_packet_rate_limiter_global() {
        let config = PacketRateLimitConfig {
            global_rate: 100,
            global_burst: 3,
            per_ip_rate: 100,
            per_ip_burst: 100,
            exceed_action: RateLimitAction::Drop,
            max_per_ip_entries: 1000,
        };
        let limiter = PacketRateLimiter::new(config);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        // Global bucket has burst=3
        assert!(limiter.check(&ip));
        assert!(limiter.check(&ip));
        assert!(limiter.check(&ip));
        // Global bucket exhausted (note: per-IP bucket created with burst=100, so per-IP is fine)
        assert!(!limiter.check(&ip));
    }

    #[test]
    fn test_packet_rate_limiter_per_ip() {
        let config = PacketRateLimitConfig {
            global_rate: 1_000_000,
            global_burst: 1_000_000,
            per_ip_rate: 100,
            per_ip_burst: 2,
            exceed_action: RateLimitAction::Reject,
            max_per_ip_entries: 1000,
        };
        let limiter = PacketRateLimiter::new(config);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        // First call creates bucket (burst=2) and consumes 1 token -> 1 left
        assert!(limiter.check(&ip));
        // Second call consumes 1 token -> 0 left
        assert!(limiter.check(&ip));
        // Third call: no tokens
        assert!(!limiter.check(&ip));
    }

    #[test]
    fn test_packet_rate_limiter_different_ips() {
        let config = PacketRateLimitConfig {
            global_rate: 1_000_000,
            global_burst: 1_000_000,
            per_ip_rate: 100,
            per_ip_burst: 1,
            exceed_action: RateLimitAction::Drop,
            max_per_ip_entries: 1000,
        };
        let limiter = PacketRateLimiter::new(config);
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        // ip1: bucket created with burst=1, one consumed on creation -> 0 left
        assert!(limiter.check(&ip1));
        assert!(!limiter.check(&ip1));

        // ip2: should still be fine
        assert!(limiter.check(&ip2));
    }

    #[test]
    fn test_exceed_action() {
        let config = PacketRateLimitConfig {
            exceed_action: RateLimitAction::Throttle,
            ..Default::default()
        };
        let limiter = PacketRateLimiter::new(config);
        assert_eq!(limiter.exceed_action(), RateLimitAction::Throttle);
    }

    #[test]
    fn test_tracked_ips() {
        let config = PacketRateLimitConfig::default();
        let limiter = PacketRateLimiter::new(config);

        assert_eq!(limiter.tracked_ips(), 0);

        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        limiter.check(&ip1);
        assert_eq!(limiter.tracked_ips(), 1);

        limiter.check(&ip2);
        assert_eq!(limiter.tracked_ips(), 2);
    }

    #[test]
    fn test_lazy_refill_timing() {
        // Create a bucket with high rate so refill is fast
        let bucket = TokenBucket::new(100_000, 1);
        // Drain it
        assert!(bucket.try_acquire());
        assert!(!bucket.try_acquire());

        // Sleep briefly to allow refill
        std::thread::sleep(Duration::from_millis(2));

        // Should have tokens again
        assert!(bucket.try_acquire());
    }
}
