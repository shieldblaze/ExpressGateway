//! Power-of-two-choices (P2C) load balancer.

use rand::Rng;

use crate::{Backend, BalancerError, LoadBalancer};

/// Power-of-two-choices load balancer.
///
/// Picks two backends at random and chooses the one with fewer active
/// connections. This simple algorithm achieves exponentially better load
/// distribution than pure random (per Mitzenmacher, 2001).
#[derive(Debug)]
pub struct PowerOfTwoChoices<R: Rng> {
    rng: R,
}

impl<R: Rng> PowerOfTwoChoices<R> {
    /// Create a new P2C balancer with the given RNG.
    pub const fn new(rng: R) -> Self {
        Self { rng }
    }
}

impl<R: Rng + Send + Sync> LoadBalancer for PowerOfTwoChoices<R> {
    fn pick(&mut self, backends: &[Backend]) -> Result<usize, BalancerError> {
        let len = backends.len();
        if len == 0 {
            return Err(BalancerError::NoBackends);
        }
        if len == 1 {
            return Ok(0);
        }

        let a = self.rng.gen_range(0..len);
        let mut b = self.rng.gen_range(0..len.saturating_sub(1));
        if b >= a {
            b += 1;
        }

        let ba = backends.get(a);
        let bb = backends.get(b);

        match (ba, bb) {
            (Some(backend_a), Some(backend_b)) => {
                if backend_a.active_connections <= backend_b.active_connections {
                    Ok(a)
                } else {
                    Ok(b)
                }
            }
            // Should not happen given the range checks, but we never panic.
            (Some(_), None) => Ok(a),
            (None, Some(_)) => Ok(b),
            (None, None) => Err(BalancerError::Internal("index out of range".to_string())),
        }
    }
}

/// Create a P2C balancer using the thread-local RNG.
#[must_use]
pub fn with_thread_rng() -> PowerOfTwoChoices<rand::rngs::ThreadRng> {
    PowerOfTwoChoices::new(rand::thread_rng())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn test_avoids_heavily_loaded() {
        let rng = StdRng::seed_from_u64(42);
        let mut lb = PowerOfTwoChoices::new(rng);
        let mut backends: Vec<Backend> = (0..4).map(|i| Backend::new(format!("b{i}"), 1)).collect();
        backends.get_mut(0).unwrap().active_connections = 100;
        backends.get_mut(1).unwrap().active_connections = 100;
        backends.get_mut(2).unwrap().active_connections = 1;
        backends.get_mut(3).unwrap().active_connections = 1;

        let mut counts = [0u32; 4];
        for _ in 0..1_000 {
            let idx = lb.pick(&backends).unwrap();
            if let Some(c) = counts.get_mut(idx) {
                *c += 1;
            }
        }
        // Lightly loaded backends should get the vast majority.
        let light = counts.get(2).copied().unwrap_or(0) + counts.get(3).copied().unwrap_or(0);
        assert!(
            light > 700,
            "light backends got {light}/1000, expected > 700"
        );
    }
}
