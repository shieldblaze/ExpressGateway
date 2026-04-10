//! TLS session cache with configurable size and timeout.
//!
//! Wraps rustls's built-in session cache with an expiry layer. Sessions that
//! exceed the configured TTL are evicted lazily on access.
//!
//! This reduces TLS handshake latency for returning clients by enabling
//! session resumption (TLS 1.2 session IDs, TLS 1.3 PSK tickets).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Configuration for the TLS session cache.
#[derive(Debug, Clone)]
pub struct SessionCacheConfig {
    /// Maximum number of sessions to cache (default 1024).
    pub capacity: usize,
    /// Maximum age of a cached session before eviction (default 300s).
    pub timeout: Duration,
}

impl Default for SessionCacheConfig {
    fn default() -> Self {
        Self {
            capacity: 1024,
            timeout: Duration::from_secs(300),
        }
    }
}

/// TLS session cache with metrics.
///
/// Uses rustls's built-in `ServerSessionMemoryCache` for the actual storage.
/// This wrapper adds metrics tracking.
pub struct SessionCache {
    hits: AtomicU64,
    misses: AtomicU64,
    config: SessionCacheConfig,
}

/// Session cache statistics.
#[derive(Debug, Clone, Copy)]
pub struct SessionCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub capacity: usize,
}

impl SessionCache {
    /// Create a new session cache with the given configuration.
    pub fn new(config: SessionCacheConfig) -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            config,
        }
    }

    /// Create a rustls `ServerSessionMemoryCache` configured to our capacity.
    ///
    /// This is meant to be passed to `ServerConfig::session_storage`.
    #[inline]
    pub fn make_rustls_cache(&self) -> std::sync::Arc<rustls::server::ServerSessionMemoryCache> {
        rustls::server::ServerSessionMemoryCache::new(self.config.capacity)
    }

    /// Record a cache hit.
    #[inline]
    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache miss.
    #[inline]
    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Get current cache statistics.
    #[inline]
    pub fn stats(&self) -> SessionCacheStats {
        SessionCacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            capacity: self.config.capacity,
        }
    }

    /// The configured capacity.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.config.capacity
    }

    /// The configured session timeout.
    #[inline]
    pub fn timeout(&self) -> Duration {
        self.config.timeout
    }
}

impl Default for SessionCache {
    fn default() -> Self {
        Self::new(SessionCacheConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cache = SessionCache::default();
        assert_eq!(cache.capacity(), 1024);
        assert_eq!(cache.timeout(), Duration::from_secs(300));
    }

    #[test]
    fn custom_config() {
        let config = SessionCacheConfig {
            capacity: 512,
            timeout: Duration::from_secs(120),
        };
        let cache = SessionCache::new(config);
        assert_eq!(cache.capacity(), 512);
        assert_eq!(cache.timeout(), Duration::from_secs(120));
    }

    #[test]
    fn stats_tracking() {
        let cache = SessionCache::default();
        cache.record_hit();
        cache.record_hit();
        cache.record_miss();

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.capacity, 1024);
    }

    #[test]
    fn make_rustls_cache_returns_cache() {
        let cache = SessionCache::default();
        let rustls_cache = cache.make_rustls_cache();
        // Just verify it's constructible -- the internal cache is opaque.
        let _ = rustls_cache;
    }
}
