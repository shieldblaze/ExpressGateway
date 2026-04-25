//! Simple round-robin load balancer.

use crate::{Backend, BalancerError, LoadBalancer};

/// Round-robin balancer: cycles through backends sequentially.
#[derive(Debug, Default)]
pub struct RoundRobin {
    counter: usize,
}

impl RoundRobin {
    /// Create a new round-robin balancer.
    #[must_use]
    pub const fn new() -> Self {
        Self { counter: 0 }
    }
}

impl LoadBalancer for RoundRobin {
    fn pick(&mut self, backends: &[Backend]) -> Result<usize, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }
        let idx = self.counter % backends.len();
        self.counter = self.counter.wrapping_add(1);
        Ok(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cycles_through_all() {
        let mut rr = RoundRobin::new();
        let backends: Vec<Backend> = (0..3).map(|i| Backend::new(format!("b{i}"), 1)).collect();
        for round in 0..10 {
            let idx = rr.pick(&backends).unwrap();
            assert_eq!(idx, round % 3);
        }
    }

    #[test]
    fn test_empty_backends() {
        let mut rr = RoundRobin::new();
        let result = rr.pick(&[]);
        assert!(result.is_err());
    }
}
