//! Health check result caching with time-based expiration.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use expressgateway_core::Health;
use parking_lot::RwLock;

/// Default cache TTL of 5 seconds.
const DEFAULT_TTL: Duration = Duration::from_secs(5);

/// Cached health check value with double-check pattern for refresh.
///
/// The `last_refresh` timestamp is behind a `RwLock` so that concurrent
/// `get()` calls (the hot path) take only a shared read lock, while the
/// infrequent `update()`/`set()`/`invalidate()` calls take the write lock.
pub struct HealthCache {
    /// Cached health value stored atomically.
    health: AtomicU8,
    /// Last refresh time, protected by RwLock for concurrent reads.
    last_refresh: RwLock<Option<Instant>>,
    /// Time-to-live for cached values.
    ttl: Duration,
}

impl HealthCache {
    /// Create a new health cache with the default 5-second TTL.
    pub fn new() -> Self {
        Self {
            health: AtomicU8::new(health_to_u8(Health::Unknown)),
            last_refresh: RwLock::new(None),
            ttl: DEFAULT_TTL,
        }
    }

    /// Create a new health cache with a custom TTL.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            health: AtomicU8::new(health_to_u8(Health::Unknown)),
            last_refresh: RwLock::new(None),
            ttl,
        }
    }

    /// Get the cached health value, or `None` if expired.
    pub fn get(&self) -> Option<Health> {
        let last = self.last_refresh.read();
        match *last {
            Some(instant) if instant.elapsed() < self.ttl => {
                Some(u8_to_health(self.health.load(Ordering::Acquire)))
            }
            _ => None,
        }
    }

    /// Update the cached health value using double-check pattern.
    ///
    /// If the cache is still valid (not expired), this is a no-op and returns
    /// the existing cached value. Otherwise, stores the new value.
    pub fn update(&self, health: Health) -> Health {
        let mut last = self.last_refresh.write();

        // Double-check: another thread may have refreshed while we waited.
        if let Some(instant) = *last
            && instant.elapsed() < self.ttl
        {
            return u8_to_health(self.health.load(Ordering::Acquire));
        }

        self.health.store(health_to_u8(health), Ordering::Release);
        *last = Some(Instant::now());
        health
    }

    /// Force-set the cached value regardless of TTL.
    pub fn set(&self, health: Health) {
        let mut last = self.last_refresh.write();
        self.health.store(health_to_u8(health), Ordering::Release);
        *last = Some(Instant::now());
    }

    /// Invalidate the cache.
    pub fn invalidate(&self) {
        let mut last = self.last_refresh.write();
        *last = None;
    }
}

impl Default for HealthCache {
    fn default() -> Self {
        Self::new()
    }
}

fn health_to_u8(h: Health) -> u8 {
    match h {
        Health::Good => 0,
        Health::Medium => 1,
        Health::Bad => 2,
        Health::Unknown => 3,
    }
}

fn u8_to_health(v: u8) -> Health {
    match v {
        0 => Health::Good,
        1 => Health::Medium,
        2 => Health::Bad,
        _ => Health::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_cache_is_unknown() {
        let cache = HealthCache::new();
        assert_eq!(cache.get(), None);
    }

    #[test]
    fn test_set_and_get() {
        let cache = HealthCache::new();
        cache.set(Health::Good);
        assert_eq!(cache.get(), Some(Health::Good));
    }

    #[test]
    fn test_update_within_ttl_returns_cached() {
        let cache = HealthCache::new();
        cache.set(Health::Good);
        // Update within TTL should return the cached value, not the new one.
        let result = cache.update(Health::Bad);
        assert_eq!(result, Health::Good);
    }

    #[test]
    fn test_expired_cache_returns_none() {
        let cache = HealthCache::with_ttl(Duration::from_millis(1));
        cache.set(Health::Good);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(cache.get(), None);
    }

    #[test]
    fn test_invalidate() {
        let cache = HealthCache::new();
        cache.set(Health::Good);
        assert_eq!(cache.get(), Some(Health::Good));

        cache.invalidate();
        assert_eq!(cache.get(), None);
    }

    #[test]
    fn test_update_after_expiry() {
        let cache = HealthCache::with_ttl(Duration::from_millis(1));
        cache.set(Health::Good);
        std::thread::sleep(Duration::from_millis(5));

        let result = cache.update(Health::Bad);
        assert_eq!(result, Health::Bad);
        assert_eq!(cache.get(), Some(Health::Bad));
    }
}
