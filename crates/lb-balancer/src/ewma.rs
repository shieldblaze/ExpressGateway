//! EWMA (Exponentially Weighted Moving Average) latency-based load balancer.
//!
//! Picks the backend with the lowest score, where:
//!   `score = latency_ewma_ns * (active_connections + 1)`
//!
//! This accounts for both latency and current load, preferring fast backends
//! that are not overloaded.

use crate::{Backend, BalancerError, LoadBalancer};

/// EWMA-based load balancer: routes traffic to the backend with the lowest
/// latency-load product.
#[derive(Debug, Default)]
pub struct Ewma;

impl Ewma {
    /// Create a new EWMA balancer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl LoadBalancer for Ewma {
    fn pick(&mut self, backends: &[Backend]) -> Result<usize, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }

        // Compute a penalty latency for backends that have no measurements yet
        // (latency_ewma_ns == 0). Use the maximum observed latency among peers
        // that *have* been measured. If no backend has been measured, fall back
        // to 1 so all backends score equally.
        let max_observed_latency: u64 = backends
            .iter()
            .map(|b| b.latency_ewma_ns)
            .max()
            .unwrap_or(0);
        let cold_start_latency = u128::from(if max_observed_latency > 0 {
            max_observed_latency
        } else {
            1
        });

        let mut best_idx = 0;
        let mut best_score = u128::MAX;

        for (i, backend) in backends.iter().enumerate() {
            // Use u128 to avoid overflow on the multiply.
            let latency = if backend.latency_ewma_ns == 0 {
                // Unknown latency: apply penalty to avoid thundering herd.
                cold_start_latency
            } else {
                u128::from(backend.latency_ewma_ns)
            };
            let load = u128::from(backend.active_connections) + 1;
            let score = latency.saturating_mul(load);

            if score < best_score {
                best_score = score;
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
    fn test_prefers_fast() {
        let mut lb = Ewma::new();
        let mut backends = vec![Backend::new("fast", 1), Backend::new("slow", 1)];
        backends.get_mut(0).unwrap().latency_ewma_ns = 10_000_000; // 10ms
        backends.get_mut(1).unwrap().latency_ewma_ns = 100_000_000; // 100ms

        assert_eq!(lb.pick(&backends).unwrap(), 0);
    }

    #[test]
    fn test_cold_start_no_thundering_herd() {
        let mut lb = Ewma::new();
        let mut backends = vec![
            Backend::new("measured-a", 1),
            Backend::new("measured-b", 1),
            Backend::new("new-cold", 1),
        ];
        // Two measured backends with moderate latency.
        backends.get_mut(0).unwrap().latency_ewma_ns = 10_000_000; // 10ms
        backends.get_mut(1).unwrap().latency_ewma_ns = 20_000_000; // 20ms
        // new-cold has latency_ewma_ns == 0 (no measurements yet).

        // The cold backend should NOT win every pick. With the penalty it
        // receives max_observed (20ms), so its score is 20ms * 1 = 20ms,
        // same as measured-b. measured-a at 10ms wins.
        let idx = lb.pick(&backends).unwrap();
        assert_eq!(idx, 0, "measured-a (10ms) should win, not the cold backend");

        // Even if we run many picks, the cold backend must not dominate.
        let mut cold_count = 0u32;
        for _ in 0..100 {
            let picked = lb.pick(&backends).unwrap();
            if picked == 2 {
                cold_count += 1;
            }
        }
        assert!(
            cold_count < 50,
            "cold backend got {cold_count}/100 picks, thundering herd detected"
        );
    }

    #[test]
    fn test_accounts_for_load() {
        let mut lb = Ewma::new();
        let mut backends = vec![Backend::new("fast-loaded", 1), Backend::new("slow-idle", 1)];
        // Fast but very loaded: score = 1ms * 101 = 101ms
        backends.get_mut(0).unwrap().latency_ewma_ns = 1_000_000;
        backends.get_mut(0).unwrap().active_connections = 100;
        // Slow but idle: score = 50ms * 1 = 50ms
        backends.get_mut(1).unwrap().latency_ewma_ns = 50_000_000;
        backends.get_mut(1).unwrap().active_connections = 0;

        assert_eq!(lb.pick(&backends).unwrap(), 1);
    }
}
