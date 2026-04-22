//! Weighted random load balancer: probability proportional to weight.

use rand::Rng;

use crate::{Backend, BalancerError, LoadBalancer};

/// Weighted random balancer: picks a backend with probability proportional
/// to its weight.
#[derive(Debug)]
pub struct WeightedRandom<R: Rng> {
    rng: R,
}

impl<R: Rng> WeightedRandom<R> {
    /// Create a new weighted random balancer with the given RNG.
    pub const fn new(rng: R) -> Self {
        Self { rng }
    }
}

impl<R: Rng + Send + Sync> LoadBalancer for WeightedRandom<R> {
    fn pick(&mut self, backends: &[Backend]) -> Result<usize, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }

        let total_weight: u64 = backends.iter().map(|b| u64::from(b.weight)).sum();
        if total_weight == 0 {
            return Err(BalancerError::AllZeroWeight);
        }

        let mut dart = self.rng.gen_range(0..total_weight);
        for (i, backend) in backends.iter().enumerate() {
            let w = u64::from(backend.weight);
            if dart < w {
                return Ok(i);
            }
            dart -= w;
        }

        // Fallback: should not be reached if weights are correct, but we never
        // panic in non-test code. Return last backend.
        Ok(backends.len().saturating_sub(1))
    }
}

/// Create a weighted random balancer using the thread-local RNG.
#[must_use]
pub fn with_thread_rng() -> WeightedRandom<rand::rngs::ThreadRng> {
    WeightedRandom::new(rand::thread_rng())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn test_weighted_distribution() {
        let rng = StdRng::seed_from_u64(99);
        let mut lb = WeightedRandom::new(rng);
        let backends = vec![
            Backend::new("a", 7),
            Backend::new("b", 2),
            Backend::new("c", 1),
        ];
        let mut counts = [0u32; 3];
        for _ in 0..10_000 {
            let idx = lb.pick(&backends).unwrap();
            if let Some(c) = counts.get_mut(idx) {
                *c += 1;
            }
        }
        // Expected: ~7000, ~2000, ~1000
        assert!(counts.first().copied().unwrap_or(0) > 6000);
        assert!(counts.get(1).copied().unwrap_or(0) > 1200);
        assert!(counts.get(2).copied().unwrap_or(0) > 400);
    }
}
