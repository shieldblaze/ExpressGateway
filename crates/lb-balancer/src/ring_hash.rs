//! Ring-based consistent hashing with virtual nodes.
//!
//! Each backend is placed at multiple positions (virtual nodes) on a hash ring.
//! Keys are mapped to the ring and routed to the nearest backend clockwise.

use crate::{Backend, BalancerError, KeyedLoadBalancer, backend_identity_hash};

/// Number of virtual nodes per backend on the ring.
const VNODES_PER_BACKEND: u32 = 150;

/// A point on the hash ring: (hash value, backend index).
#[derive(Debug, Clone)]
struct RingPoint {
    hash: u64,
    backend_idx: usize,
}

/// Consistent hash ring load balancer.
#[derive(Debug)]
pub struct RingHash {
    ring: Vec<RingPoint>,
    num_backends: usize,
    /// Identity hash of the backend set used to build the ring.
    identity_hash: u64,
}

impl RingHash {
    /// Build a new hash ring for the given backends.
    ///
    /// # Errors
    ///
    /// Returns `BalancerError::NoBackends` if the backend slice is empty.
    pub fn new(backends: &[Backend]) -> Result<Self, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }

        let mut ring = Vec::with_capacity(backends.len() * VNODES_PER_BACKEND as usize);

        for (idx, backend) in backends.iter().enumerate() {
            for vnode in 0..VNODES_PER_BACKEND {
                let hash = Self::hash_vnode(&backend.id, vnode);
                ring.push(RingPoint {
                    hash,
                    backend_idx: idx,
                });
            }
        }

        ring.sort_by_key(|p| p.hash);

        Ok(Self {
            ring,
            num_backends: backends.len(),
            identity_hash: backend_identity_hash(backends),
        })
    }

    /// Hash a backend id with a virtual node index using multiply-shift.
    fn hash_vnode(id: &str, vnode: u32) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV offset basis
        for byte in id.bytes() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0100_0000_01b3); // FNV prime
        }
        // Mix in the vnode number.
        h ^= u64::from(vnode);
        h = h.wrapping_mul(0x517c_c1b7_2722_0a95);
        // Finalizer
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^= h >> 33;
        h
    }

    /// Hash a key value.
    const fn hash_key(key: u64) -> u64 {
        let mut h = key;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^= h >> 33;
        h
    }
}

impl KeyedLoadBalancer for RingHash {
    fn pick_with_key(&self, backends: &[Backend], key: u64) -> Result<usize, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }
        if backends.len() != self.num_backends
            || backend_identity_hash(backends) != self.identity_hash
        {
            return Err(BalancerError::TableStale);
        }
        if self.ring.is_empty() {
            return Err(BalancerError::Internal("ring is empty".to_string()));
        }

        let h = Self::hash_key(key);

        // Binary search for the first ring point >= h.
        let pos = match self.ring.binary_search_by_key(&h, |p| p.hash) {
            Ok(i) => i,
            Err(i) => {
                if i >= self.ring.len() {
                    0 // Wrap around to the first point.
                } else {
                    i
                }
            }
        };

        let point = self
            .ring
            .get(pos)
            .ok_or_else(|| BalancerError::Internal("ring position out of range".to_string()))?;

        if point.backend_idx >= backends.len() {
            return Err(BalancerError::Internal(
                "ring entry exceeds backend count".to_string(),
            ));
        }

        Ok(point.backend_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_backends(n: usize) -> Vec<Backend> {
        (0..n)
            .map(|i| Backend::new(format!("ring-backend-{i}"), 1))
            .collect()
    }

    #[test]
    fn test_consistent() {
        let backends = make_backends(5);
        let ring = RingHash::new(&backends).unwrap();

        for key in 0..1000u64 {
            let a = ring.pick_with_key(&backends, key).unwrap();
            let b = ring.pick_with_key(&backends, key).unwrap();
            assert_eq!(a, b);
        }
    }

    #[test]
    fn test_detects_stale_identity() {
        let backends_orig = vec![
            Backend::new("ring-backend-0", 1),
            Backend::new("ring-backend-1", 1),
            Backend::new("ring-backend-2", 1),
        ];
        let ring = RingHash::new(&backends_orig).unwrap();

        // Replace ring-backend-1 with ring-backend-X at the same index.
        let backends_swapped = vec![
            Backend::new("ring-backend-0", 1),
            Backend::new("ring-backend-X", 1),
            Backend::new("ring-backend-2", 1),
        ];

        let result = ring.pick_with_key(&backends_swapped, 42);
        assert!(
            matches!(result, Err(BalancerError::TableStale)),
            "expected TableStale, got {result:?}"
        );
    }

    #[test]
    fn test_minimal_disruption_on_removal() {
        let backends_5 = make_backends(5);
        let backends_4: Vec<Backend> = backends_5.iter().take(4).cloned().collect();

        let ring_5 = RingHash::new(&backends_5).unwrap();
        let ring_4 = RingHash::new(&backends_4).unwrap();

        let mut same = 0u32;
        let total = 10_000u32;
        for key in 0..u64::from(total) {
            let idx_5 = ring_5.pick_with_key(&backends_5, key).unwrap();
            let idx_4 = ring_4.pick_with_key(&backends_4, key).unwrap();
            if idx_5 < 4 && idx_5 == idx_4 {
                same += 1;
            }
        }

        #[allow(clippy::cast_possible_truncation)]
        let mapped_to_remaining: u32 = (0..u64::from(total))
            .filter(|k| ring_5.pick_with_key(&backends_5, *k).unwrap() < 4)
            .count() as u32;
        let pct = if mapped_to_remaining > 0 {
            f64::from(same) / f64::from(mapped_to_remaining) * 100.0
        } else {
            0.0
        };
        assert!(pct > 60.0, "disruption too high: only {pct:.1}% stable");
    }
}
