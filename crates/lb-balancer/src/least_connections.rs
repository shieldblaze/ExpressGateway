//! Least-connections load balancer.

use crate::{Backend, BalancerError, LoadBalancer};

/// Fewest active connections; ties go to the lower index.
#[derive(Debug, Default)]
pub struct LeastConnections;

impl LeastConnections {
    /// Create a new least-connections balancer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl LoadBalancer for LeastConnections {
    fn pick(&mut self, backends: &[Backend]) -> Result<usize, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }

        let mut best_idx = 0;
        let mut best_conns = u64::MAX;

        for (i, backend) in backends.iter().enumerate() {
            if backend.active_connections < best_conns {
                best_conns = backend.active_connections;
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
        let mut lb = LeastConnections::new();
        let mut backends = vec![
            Backend::new("a", 1),
            Backend::new("b", 1),
            Backend::new("c", 1),
        ];
        backends.get_mut(0).unwrap().active_connections = 10;
        backends.get_mut(1).unwrap().active_connections = 5;
        backends.get_mut(2).unwrap().active_connections = 3;
        assert_eq!(lb.pick(&backends).unwrap(), 2);
    }

    #[test]
    fn test_tie_breaks_by_index() {
        let mut lb = LeastConnections::new();
        let backends = vec![
            Backend::new("a", 1),
            Backend::new("b", 1),
            Backend::new("c", 1),
        ];
        assert_eq!(lb.pick(&backends).unwrap(), 0);
    }
}
