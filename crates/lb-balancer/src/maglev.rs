//! Maglev consistent hashing (Google, 2016).

use crate::{Backend, BalancerError, KeyedLoadBalancer, backend_identity_hash};

/// Table size. MUST stay prime — the skip-coprimality argument below depends on it.
const TABLE_SIZE: usize = 65537;

/// Maglev balancer; the table is precomputed so a pick is O(1).
#[derive(Debug)]
pub struct Maglev {
    /// Lookup table: each entry is a backend index.
    table: Vec<usize>,
    /// Number of backends the table was built for.
    num_backends: usize,
    /// Identity hash of the backend set — catches a same-count swap a length check would miss.
    identity_hash: u64,
}

impl Maglev {
    /// Build the lookup table.
    pub fn new(backends: &[Backend]) -> Result<Self, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }

        let table = Self::populate(backends);
        Ok(Self {
            table,
            num_backends: backends.len(),
            identity_hash: backend_identity_hash(backends),
        })
    }

    /// Compute offset and skip for a backend based on its id hash.
    #[allow(clippy::cast_possible_truncation)]
    fn permutation(id: &str) -> (usize, usize) {
        let h1 = Self::hash_str(id, 0);
        let h2 = Self::hash_str(id, 0x9e37_79b9_7f4a_7c15);

        let offset = (h1 as usize) % TABLE_SIZE;
        // `skip` must be coprime with TABLE_SIZE; primality makes every value in range valid.
        let skip = ((h2 as usize) % (TABLE_SIZE - 1)) + 1;
        (offset, skip)
    }

    /// Simple multiply-shift string hash.
    fn hash_str(s: &str, seed: u64) -> u64 {
        let mut h = seed;
        for byte in s.bytes() {
            h = h
                .wrapping_mul(0x517c_c1b7_2722_0a95)
                .wrapping_add(u64::from(byte));
        }
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^= h >> 33;
        h
    }

    /// Populate the Maglev lookup table using the permutation algorithm.
    fn populate(backends: &[Backend]) -> Vec<usize> {
        let n = backends.len();

        let perms: Vec<(usize, usize)> =
            backends.iter().map(|b| Self::permutation(&b.id)).collect();

        let mut next = vec![0usize; n];
        let mut table = vec![usize::MAX; TABLE_SIZE];
        let mut filled = 0usize;

        while filled < TABLE_SIZE {
            for i in 0..n {
                let (offset, skip) = match perms.get(i) {
                    Some(p) => *p,
                    None => continue,
                };

                let mut c = match next.get(i) {
                    Some(v) => *v,
                    None => continue,
                };
                let mut slot = (offset + c * skip) % TABLE_SIZE;
                while table.get(slot).copied() != Some(usize::MAX) {
                    c += 1;
                    slot = (offset + c * skip) % TABLE_SIZE;
                }

                if let Some(entry) = table.get_mut(slot) {
                    *entry = i;
                }
                if let Some(n_i) = next.get_mut(i) {
                    *n_i = c + 1;
                }
                filled += 1;

                if filled >= TABLE_SIZE {
                    break;
                }
            }
        }

        table
    }
}

impl KeyedLoadBalancer for Maglev {
    #[allow(clippy::cast_possible_truncation)]
    fn pick_with_key(&self, backends: &[Backend], key: u64) -> Result<usize, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }
        if backends.len() != self.num_backends
            || backend_identity_hash(backends) != self.identity_hash
        {
            return Err(BalancerError::TableStale);
        }

        let slot = (key as usize) % TABLE_SIZE;
        let idx = self
            .table
            .get(slot)
            .copied()
            .ok_or_else(|| BalancerError::Internal("table lookup out of range".to_string()))?;

        if idx >= backends.len() {
            return Err(BalancerError::Internal(
                "table entry exceeds backend count".to_string(),
            ));
        }

        Ok(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_backends(n: usize) -> Vec<Backend> {
        (0..n)
            .map(|i| Backend::new(format!("backend-{i}"), 1))
            .collect()
    }

    #[test]
    fn test_consistent_mapping() {
        let backends = make_backends(5);
        let maglev = Maglev::new(&backends).unwrap();

        for key in 0..1000u64 {
            let a = maglev.pick_with_key(&backends, key).unwrap();
            let b = maglev.pick_with_key(&backends, key).unwrap();
            assert_eq!(a, b, "same key should always map to same backend");
        }
    }

    #[test]
    fn test_detects_stale_identity() {
        let backends_orig = vec![
            Backend::new("backend-0", 1),
            Backend::new("backend-1", 1),
            Backend::new("backend-2", 1),
        ];
        let maglev = Maglev::new(&backends_orig).unwrap();

        // Same count, different identity — the case a length check misses.
        let backends_swapped = vec![
            Backend::new("backend-0", 1),
            Backend::new("backend-X", 1),
            Backend::new("backend-2", 1),
        ];

        let result = maglev.pick_with_key(&backends_swapped, 42);
        assert!(
            matches!(result, Err(BalancerError::TableStale)),
            "expected TableStale, got {result:?}"
        );
    }

    #[test]
    fn test_minimal_disruption() {
        let backends_5 = make_backends(5);
        let backends_4: Vec<Backend> = backends_5.iter().take(4).cloned().collect();

        let maglev_5 = Maglev::new(&backends_5).unwrap();
        let maglev_4 = Maglev::new(&backends_4).unwrap();

        let mut same = 0u32;
        let total = 10_000u32;
        for key in 0..u64::from(total) {
            let idx_5 = maglev_5.pick_with_key(&backends_5, key).unwrap();
            let idx_4 = maglev_4.pick_with_key(&backends_4, key).unwrap();
            if idx_5 < 4 && idx_5 == idx_4 {
                same += 1;
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        let mapped_to_remaining: u32 = (0..u64::from(total))
            .filter(|k| maglev_5.pick_with_key(&backends_5, *k).unwrap() < 4)
            .count() as u32;
        let pct = if mapped_to_remaining > 0 {
            f64::from(same) / f64::from(mapped_to_remaining) * 100.0
        } else {
            0.0
        };
        assert!(pct > 60.0, "disruption too high: only {pct:.1}% stable");
    }
}
