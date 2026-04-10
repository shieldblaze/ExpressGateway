//! Four-tuple session persistence.
//!
//! Maps a [`FourTuple`] (source addr + destination addr) to a backend node ID.
//! Entries expire after a configurable TTL and the store enforces a maximum
//! capacity with batch eviction.

use std::time::{Duration, Instant};

use dashmap::DashMap;
use expressgateway_core::session::SessionPersistence;
use expressgateway_core::types::FourTuple;

/// Default TTL for four-tuple sessions: 1 hour.
const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// Default maximum number of entries.
const DEFAULT_MAX_ENTRIES: usize = 500_000;

/// Fraction of entries to evict when at capacity (10%).
const EVICTION_FRACTION: f64 = 0.10;

/// An entry in the four-tuple session store.
struct Entry {
    value: String,
    expires_at: Instant,
}

/// Four-tuple session persistence.
///
/// Provides session affinity based on the full connection four-tuple
/// (src addr, dst addr). Entries have a TTL and batch eviction under
/// capacity pressure.
pub struct FourTuplePersistence {
    store: DashMap<FourTuple, Entry>,
    ttl: Duration,
    max_entries: usize,
}

impl FourTuplePersistence {
    /// Create a new four-tuple persistence store with default settings.
    pub fn new() -> Self {
        Self {
            store: DashMap::new(),
            ttl: DEFAULT_TTL,
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }

    /// Create with custom TTL and capacity.
    pub fn with_config(ttl: Duration, max_entries: usize) -> Self {
        Self {
            store: DashMap::new(),
            ttl,
            max_entries,
        }
    }
}

impl Default for FourTuplePersistence {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionPersistence<FourTuple, String> for FourTuplePersistence {
    fn get(&self, key: &FourTuple) -> Option<String> {
        let entry = self.store.get(key)?;
        if Instant::now() >= entry.expires_at {
            drop(entry);
            self.store.remove(key);
            return None;
        }
        Some(entry.value.clone())
    }

    fn put(&self, key: FourTuple, value: String) {
        // Evict if at capacity.
        if self.store.len() >= self.max_entries {
            let now = Instant::now();
            let to_evict = (self.max_entries as f64 * EVICTION_FRACTION) as usize;
            let mut evicted = 0;

            // First pass: evict expired entries.
            self.store.retain(|_, entry| {
                if evicted >= to_evict {
                    return true;
                }
                if now >= entry.expires_at {
                    evicted += 1;
                    return false;
                }
                true
            });

            // Second pass: if not enough evicted, remove arbitrary entries.
            if evicted < to_evict {
                let remaining = to_evict - evicted;
                let mut count = 0;
                self.store.retain(|_, _| {
                    if count < remaining {
                        count += 1;
                        false
                    } else {
                        true
                    }
                });
            }
        }

        self.store.insert(
            key,
            Entry {
                value,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    fn remove(&self, key: &FourTuple) {
        self.store.remove(key);
    }

    fn len(&self) -> usize {
        self.store.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tuple(src_port: u16, dst_port: u16) -> FourTuple {
        FourTuple {
            src_addr: ([10, 0, 0, 1], src_port).into(),
            dst_addr: ([192, 168, 1, 1], dst_port).into(),
        }
    }

    #[test]
    fn basic_put_get() {
        let store = FourTuplePersistence::new();
        let key = make_tuple(5000, 80);
        store.put(key, "node-1".into());
        assert_eq!(store.get(&key), Some("node-1".into()));
    }

    #[test]
    fn different_tuples_independent() {
        let store = FourTuplePersistence::new();
        let k1 = make_tuple(5000, 80);
        let k2 = make_tuple(5001, 80);

        store.put(k1, "node-1".into());
        store.put(k2, "node-2".into());

        assert_eq!(store.get(&k1), Some("node-1".into()));
        assert_eq!(store.get(&k2), Some("node-2".into()));
    }

    #[test]
    fn ttl_expiry() {
        let store = FourTuplePersistence::with_config(Duration::from_millis(1), 500_000);
        let key = make_tuple(5000, 80);
        store.put(key, "node-1".into());

        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(store.get(&key), None);
    }

    #[test]
    fn capacity_eviction() {
        let max = 100;
        let store = FourTuplePersistence::with_config(Duration::from_secs(3600), max);

        for i in 0..max as u16 {
            let key = make_tuple(5000 + i, 80);
            store.put(key, format!("node-{}", i));
        }
        assert_eq!(store.len(), max);

        // Add one more to trigger eviction.
        let new_key = make_tuple(9999, 443);
        store.put(new_key, "node-new".into());

        assert!(store.len() <= max, "Size after eviction: {}", store.len());
        assert_eq!(store.get(&new_key), Some("node-new".into()));
    }

    #[test]
    fn remove_entry() {
        let store = FourTuplePersistence::new();
        let key = make_tuple(5000, 80);
        store.put(key, "node-1".into());
        store.remove(&key);
        assert_eq!(store.get(&key), None);
        assert!(store.is_empty());
    }
}
