//! Metrics, tracing, and logging for the load balancer.
//!
//! Provides a `MetricsRegistry` for registering and looking up named counters,
//! and utility types for observability integration.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![warn(clippy::pedantic, clippy::nursery)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;

/// A thread-safe in-process metrics registry.
///
/// Maps metric names to `AtomicU64` counter values. All methods take `&self`,
/// making the registry safe to share across threads without external
/// synchronization.
#[derive(Debug, Default)]
pub struct MetricsRegistry {
    counters: DashMap<String, AtomicU64>,
}

impl MetricsRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment a named counter by the given amount. Creates the counter
    /// if it does not yet exist.
    pub fn increment(&self, name: &str, value: u64) {
        if let Some(entry) = self.counters.get(name) {
            entry.value().fetch_add(value, Ordering::Relaxed);
        } else {
            self.counters
                .entry(name.to_owned())
                .or_insert_with(|| AtomicU64::new(0))
                .fetch_add(value, Ordering::Relaxed);
        }
    }

    /// Read the current value of a named counter.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<u64> {
        self.counters
            .get(name)
            .map(|entry| entry.value().load(Ordering::Relaxed))
    }

    /// Number of registered counters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.counters.len()
    }

    /// Whether the registry has any counters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.counters.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_and_read() {
        let reg = MetricsRegistry::new();
        assert!(reg.is_empty());

        reg.increment("requests", 1);
        reg.increment("requests", 1);
        assert_eq!(reg.get("requests"), Some(2));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn missing_counter_returns_none() {
        let reg = MetricsRegistry::new();
        assert_eq!(reg.get("nonexistent"), None);
    }

    #[test]
    fn thread_safe_increment() {
        use std::sync::Arc;

        let reg = Arc::new(MetricsRegistry::new());
        let mut handles = Vec::new();

        for _ in 0..4 {
            let r = Arc::clone(&reg);
            handles.push(std::thread::spawn(move || {
                for _ in 0..1000 {
                    r.increment("concurrent", 1);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(reg.get("concurrent"), Some(4000));
    }
}
