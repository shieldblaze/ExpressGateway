//! Shared eviction executor for all pool types.
//!
//! A single background tokio task sweeps all registered pools on a fixed
//! interval, preventing thread explosion (one task, not per-pool).

use std::sync::Arc;
use std::time::Duration;

/// Trait for pool types that support eviction of expired connections.
pub trait Evictable: Send + Sync {
    /// Evict expired/idle connections. Returns the number evicted.
    fn evict_expired(&self) -> usize;
}

/// Shared eviction executor that runs a single background task for all pools.
pub struct PoolEvictor {
    pools: Vec<Arc<dyn Evictable>>,
    interval: Duration,
}

impl PoolEvictor {
    /// Create a new evictor with a 30-second sweep interval.
    pub fn new() -> Self {
        Self {
            pools: Vec::new(),
            interval: Duration::from_secs(30),
        }
    }

    /// Create a new evictor with a custom sweep interval.
    pub fn with_interval(interval: Duration) -> Self {
        Self {
            pools: Vec::new(),
            interval,
        }
    }

    /// Register a pool for periodic eviction.
    pub fn register(&mut self, pool: Arc<dyn Evictable>) {
        self.pools.push(pool);
    }

    /// Number of registered pools.
    pub fn pool_count(&self) -> usize {
        self.pools.len()
    }

    /// Run a single eviction sweep across all registered pools.
    ///
    /// Returns the total number of connections evicted.
    pub fn sweep(&self) -> usize {
        let mut total = 0;
        for pool in &self.pools {
            let evicted = pool.evict_expired();
            if evicted > 0 {
                tracing::debug!(evicted, "pool eviction sweep");
            }
            total += evicted;
        }
        total
    }

    /// Run the evictor forever, sweeping every `interval`.
    ///
    /// This is intended to be spawned as a tokio task:
    /// ```ignore
    /// tokio::spawn(evictor.run());
    /// ```
    pub async fn run(self) {
        let mut interval = tokio::time::interval(self.interval);
        loop {
            interval.tick().await;
            let total = self.sweep();
            if total > 0 {
                tracing::info!(total, "evictor sweep completed");
            }
        }
    }
}

impl Default for PoolEvictor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakePool {
        evict_count: AtomicUsize,
    }

    impl FakePool {
        fn new() -> Self {
            Self {
                evict_count: AtomicUsize::new(0),
            }
        }

        fn evict_calls(&self) -> usize {
            self.evict_count.load(Ordering::Relaxed)
        }
    }

    impl Evictable for FakePool {
        fn evict_expired(&self) -> usize {
            self.evict_count.fetch_add(1, Ordering::Relaxed);
            5 // pretend we evicted 5
        }
    }

    #[test]
    fn register_and_sweep() {
        let mut evictor = PoolEvictor::new();
        let pool1 = Arc::new(FakePool::new());
        let pool2 = Arc::new(FakePool::new());

        evictor.register(pool1.clone());
        evictor.register(pool2.clone());

        assert_eq!(evictor.pool_count(), 2);

        let total = evictor.sweep();
        assert_eq!(total, 10); // 5 + 5
        assert_eq!(pool1.evict_calls(), 1);
        assert_eq!(pool2.evict_calls(), 1);

        // Sweep again.
        let total = evictor.sweep();
        assert_eq!(total, 10);
        assert_eq!(pool1.evict_calls(), 2);
        assert_eq!(pool2.evict_calls(), 2);
    }

    #[test]
    fn default_interval_is_30s() {
        let evictor = PoolEvictor::new();
        assert_eq!(evictor.interval, Duration::from_secs(30));
    }

    #[test]
    fn custom_interval() {
        let evictor = PoolEvictor::with_interval(Duration::from_secs(10));
        assert_eq!(evictor.interval, Duration::from_secs(10));
    }

    #[test]
    fn empty_sweep_returns_zero() {
        let evictor = PoolEvictor::new();
        assert_eq!(evictor.sweep(), 0);
    }

    #[tokio::test]
    async fn run_performs_sweeps() {
        let mut evictor = PoolEvictor::with_interval(Duration::from_millis(50));
        let pool = Arc::new(FakePool::new());
        evictor.register(pool.clone());

        // Run the evictor for a short duration.
        let handle = tokio::spawn(evictor.run());
        tokio::time::sleep(Duration::from_millis(160)).await;
        handle.abort();

        // Should have swept at least twice (50ms interval over 160ms).
        assert!(
            pool.evict_calls() >= 2,
            "expected >= 2 sweeps, got {}",
            pool.evict_calls()
        );
    }

    #[test]
    fn integration_with_h1_pool() {
        use crate::h1::{H1Pool, H1PoolConfig, PooledConnection};

        let config = H1PoolConfig {
            max_per_node: 64,
            idle_timeout: Duration::from_millis(0),
            max_age: Duration::from_secs(300),
        };
        let h1 = Arc::new(H1Pool::new(config));
        h1.release("node-a", PooledConnection::new(1));
        h1.release("node-a", PooledConnection::new(2));

        let mut evictor = PoolEvictor::new();
        evictor.register(h1.clone());

        let evicted = evictor.sweep();
        assert_eq!(evicted, 2);
        assert_eq!(h1.pooled_count("node-a"), 0);
    }
}
