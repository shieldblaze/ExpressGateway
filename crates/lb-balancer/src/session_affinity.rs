//! Session affinity (sticky sessions) load balancer.
//!
//! Uses a hash-based approach: the same session key always maps to the same
//! backend, as long as the backend set does not change.

use crate::{Backend, BalancerError, KeyedLoadBalancer};

/// Hash-based session affinity balancer.
///
/// For a given key, consistently returns the same backend index. This uses
/// a simple modulo approach with a mixing function on the key to avoid
/// patterns in sequential keys.
#[derive(Debug, Default)]
pub struct SessionAffinity;

impl SessionAffinity {
    /// Create a new session affinity balancer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Mix a key to distribute it uniformly.
    const fn mix(key: u64) -> u64 {
        let mut h = key;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^= h >> 33;
        h
    }
}

impl KeyedLoadBalancer for SessionAffinity {
    #[allow(clippy::cast_possible_truncation)]
    fn pick_with_key(&self, backends: &[Backend], key: u64) -> Result<usize, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }

        let h = Self::mix(key);
        let idx = (h as usize) % backends.len();
        Ok(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sticky() {
        let lb = SessionAffinity::new();
        let backends: Vec<Backend> = (0..5).map(|i| Backend::new(format!("b{i}"), 1)).collect();

        for key in [42u64, 100, 999, 0, u64::MAX] {
            let first = lb.pick_with_key(&backends, key).unwrap();
            for _ in 0..100 {
                let again = lb.pick_with_key(&backends, key).unwrap();
                assert_eq!(first, again, "same key must always map to same backend");
            }
        }
    }

    #[test]
    fn test_different_keys_spread() {
        let lb = SessionAffinity::new();
        let backends: Vec<Backend> = (0..4).map(|i| Backend::new(format!("b{i}"), 1)).collect();

        let mut counts = [0u32; 4];
        for key in 0..10_000u64 {
            let idx = lb.pick_with_key(&backends, key).unwrap();
            if let Some(c) = counts.get_mut(idx) {
                *c += 1;
            }
        }
        for (i, count) in counts.iter().enumerate() {
            assert!(
                *count > 1500 && *count < 3500,
                "backend {i} got {count}, expected ~2500"
            );
        }
    }
}
