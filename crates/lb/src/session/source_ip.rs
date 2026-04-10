//! Source IP session persistence.
//!
//! Maps client IP addresses (masked to /24 for IPv4, /48 for IPv6) to backend
//! node IDs. Entries expire after a configurable TTL and the store enforces a
//! maximum capacity with batch eviction.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use expressgateway_core::session::SessionPersistence;

/// Default TTL for source IP sessions: 1 hour.
const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// Default maximum number of entries.
const DEFAULT_MAX_ENTRIES: usize = 100_000;

/// Fraction of entries to evict when at capacity (10%).
const EVICTION_FRACTION: f64 = 0.10;

/// An entry in the source IP session store.
struct Entry {
    value: String,
    expires_at: Instant,
}

/// Source IP session persistence.
///
/// Client IPs are masked to /24 (IPv4) or /48 (IPv6) so that clients behind
/// the same subnet are routed to the same backend. Entries have a TTL and the
/// store enforces a maximum capacity, batch-evicting ~10% of entries when full.
pub struct SourceIpPersistence {
    store: DashMap<String, Entry>,
    ttl: Duration,
    max_entries: usize,
}

impl SourceIpPersistence {
    /// Create a new source IP persistence store with default settings.
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

impl Default for SourceIpPersistence {
    fn default() -> Self {
        Self::new()
    }
}

/// Mask an IP address to its subnet prefix.
///
/// IPv4: /24 (255.255.255.0)
/// IPv6: /48 (first 48 bits)
pub fn mask_ip(ip: &IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2])
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            // /48 = first 3 segments (48 bits).
            format!("{:x}:{:x}:{:x}::/48", segments[0], segments[1], segments[2])
        }
    }
}

impl SessionPersistence<IpAddr, String> for SourceIpPersistence {
    fn get(&self, key: &IpAddr) -> Option<String> {
        let masked = mask_ip(key);
        let entry = self.store.get(&masked)?;
        if Instant::now() >= entry.expires_at {
            drop(entry);
            self.store.remove(&masked);
            return None;
        }
        Some(entry.value.clone())
    }

    fn put(&self, key: IpAddr, value: String) {
        let masked = mask_ip(&key);

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

            // Second pass: if not enough evicted, remove oldest.
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
            masked,
            Entry {
                value,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    fn remove(&self, key: &IpAddr) {
        let masked = mask_ip(key);
        self.store.remove(&masked);
    }

    fn len(&self) -> usize {
        self.store.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_put_get() {
        let store = SourceIpPersistence::new();
        let ip: IpAddr = "192.168.1.100".parse().unwrap();
        store.put(ip, "node-1".into());
        assert_eq!(store.get(&ip), Some("node-1".into()));
    }

    #[test]
    fn same_subnet_maps_to_same_entry() {
        let store = SourceIpPersistence::new();
        let ip1: IpAddr = "192.168.1.100".parse().unwrap();
        let ip2: IpAddr = "192.168.1.200".parse().unwrap();

        store.put(ip1, "node-1".into());
        // ip2 is in the same /24, so should map to the same entry.
        assert_eq!(store.get(&ip2), Some("node-1".into()));
    }

    #[test]
    fn different_subnets_are_independent() {
        let store = SourceIpPersistence::new();
        let ip1: IpAddr = "192.168.1.100".parse().unwrap();
        let ip2: IpAddr = "192.168.2.100".parse().unwrap();

        store.put(ip1, "node-1".into());
        store.put(ip2, "node-2".into());

        assert_eq!(store.get(&ip1), Some("node-1".into()));
        assert_eq!(store.get(&ip2), Some("node-2".into()));
    }

    #[test]
    fn ipv6_masking() {
        let masked = mask_ip(&"2001:db8:abcd:1234::1".parse().unwrap());
        assert_eq!(masked, "2001:db8:abcd::/48");
    }

    #[test]
    fn ttl_expiry() {
        let store = SourceIpPersistence::with_config(Duration::from_millis(1), 100_000);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        store.put(ip, "node-1".into());

        // Wait for expiry.
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(store.get(&ip), None);
    }

    #[test]
    fn capacity_eviction() {
        let max = 100;
        let store = SourceIpPersistence::with_config(Duration::from_secs(3600), max);

        // Fill to capacity.
        for i in 0..max {
            let ip: IpAddr = format!("10.{}.{}.1", i / 256, i % 256).parse().unwrap();
            store.put(ip, format!("node-{}", i));
        }
        assert_eq!(store.len(), max);

        // Add one more; should trigger eviction.
        let new_ip: IpAddr = "172.16.0.1".parse().unwrap();
        store.put(new_ip, "node-new".into());

        // After eviction, size should be below max + 1.
        assert!(
            store.len() <= max,
            "Size should be <= max after eviction: {}",
            store.len()
        );
        // New entry should be present.
        assert_eq!(store.get(&new_ip), Some("node-new".into()));
    }

    #[test]
    fn remove_entry() {
        let store = SourceIpPersistence::new();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        store.put(ip, "node-1".into());
        assert_eq!(store.len(), 1);

        store.remove(&ip);
        assert_eq!(store.len(), 0);
        assert_eq!(store.get(&ip), None);
    }

    #[test]
    fn is_empty() {
        let store = SourceIpPersistence::new();
        assert!(store.is_empty());
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        store.put(ip, "node-1".into());
        assert!(!store.is_empty());
    }
}
