//! Least-request load balancer.

use crate::{Backend, BalancerError, LoadBalancer};

/// Picks the backend with the fewest active (in-flight) requests.
/// Ties are broken by index (lower index wins).
#[derive(Debug, Default)]
pub struct LeastRequest;

impl LeastRequest {
    /// Create a new least-request balancer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl LoadBalancer for LeastRequest {
    fn pick(&mut self, backends: &[Backend]) -> Result<usize, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }

        let mut best_idx = 0;
        let mut best_reqs = u64::MAX;

        for (i, backend) in backends.iter().enumerate() {
            if backend.active_requests < best_reqs {
                best_reqs = backend.active_requests;
                best_idx = i;
            }
        }

        Ok(best_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_picks_minimum() {
        let mut lb = LeastRequest::new();
        let mut backends = vec![
            Backend::new("a", 1),
            Backend::new("b", 1),
            Backend::new("c", 1),
        ];
        backends.get_mut(0).unwrap().active_requests = 10;
        backends.get_mut(1).unwrap().active_requests = 2;
        backends.get_mut(2).unwrap().active_requests = 8;
        assert_eq!(lb.pick(&backends).unwrap(), 1);
    }
}
