//! Uniform random load balancer.

use rand::Rng;

use crate::{Backend, BalancerError, LoadBalancer};

/// Uniform random balancer: picks a backend at random with equal probability.
#[derive(Debug)]
pub struct Random<R: Rng> {
    rng: R,
}

impl<R: Rng> Random<R> {
    /// Create a new random balancer with the given RNG.
    pub const fn new(rng: R) -> Self {
        Self { rng }
    }
}

impl<R: Rng + Send + Sync> LoadBalancer for Random<R> {
    fn pick(&mut self, backends: &[Backend]) -> Result<usize, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }
        let idx = self.rng.gen_range(0..backends.len());
        Ok(idx)
    }
}

/// Create a random balancer using the thread-local RNG.
#[must_use]
pub fn with_thread_rng() -> Random<rand::rngs::ThreadRng> {
    Random::new(rand::thread_rng())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn test_uniform_distribution() {
        let rng = StdRng::seed_from_u64(42);
        let mut lb = Random::new(rng);
        let backends: Vec<Backend> = (0..4).map(|i| Backend::new(format!("b{i}"), 1)).collect();
        let mut counts = [0u32; 4];
        for _ in 0..10_000 {
            let idx = lb.pick(&backends).unwrap();
            if let Some(c) = counts.get_mut(idx) {
                *c += 1;
            }
        }
        for (i, count) in counts.iter().enumerate() {
            assert!(
                *count > 2000 && *count < 3000,
                "backend {i}: count {count} out of range"
            );
        }
    }
}
