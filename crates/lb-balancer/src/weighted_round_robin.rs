//! Smooth weighted round-robin load balancer (Nginx-style).

use crate::{Backend, BalancerError, LoadBalancer};

/// Smooth weighted round-robin: produces an even interleaving of backends
/// proportional to their weights, avoiding bursts.
///
/// Algorithm (per Nginx upstream):
/// 1. For each backend, add its effective weight to its current weight.
/// 2. Pick the backend with the highest current weight.
/// 3. Subtract the total weight from the chosen backend's current weight.
#[derive(Debug)]
pub struct WeightedRoundRobin {
    /// Current weights, one per backend. Grows/shrinks as the backend list changes.
    current_weights: Vec<i64>,
    /// Backend IDs corresponding to `current_weights`. Used to detect topology
    /// changes (reorder, replacement) that a length check alone would miss.
    backend_ids: Vec<String>,
}

impl WeightedRoundRobin {
    /// Create a new weighted round-robin balancer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            current_weights: Vec::new(),
            backend_ids: Vec::new(),
        }
    }

    /// Ensure the internal weight vector matches the backend set by identity,
    /// not just by length. Any topology change resets accumulated WRR state.
    fn sync_weights(&mut self, backends: &[Backend]) {
        let ids_match = self.backend_ids.len() == backends.len()
            && self
                .backend_ids
                .iter()
                .zip(backends.iter())
                .all(|(stored, b)| *stored == b.id);

        if !ids_match {
            self.current_weights = vec![0; backends.len()];
            self.backend_ids = backends.iter().map(|b| b.id.clone()).collect();
        }
    }
}

impl Default for WeightedRoundRobin {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer for WeightedRoundRobin {
    fn pick(&mut self, backends: &[Backend]) -> Result<usize, BalancerError> {
        if backends.is_empty() {
            return Err(BalancerError::NoBackends);
        }

        self.sync_weights(backends);

        let total_weight: i64 = backends.iter().map(|b| i64::from(b.weight)).sum();
        if total_weight == 0 {
            return Err(BalancerError::AllZeroWeight);
        }

        // Step 1: add effective_weight to current_weight for each backend
        let mut best_idx = 0;
        let mut best_weight = i64::MIN;

        for (i, backend) in backends.iter().enumerate() {
            let ew = i64::from(backend.weight);
            if let Some(cw) = self.current_weights.get_mut(i) {
                *cw += ew;
                if *cw > best_weight {
                    best_weight = *cw;
                    best_idx = i;
                }
            }
        }

        // Step 2: subtract total_weight from the chosen backend
        if let Some(cw) = self.current_weights.get_mut(best_idx) {
            *cw -= total_weight;
        }

        Ok(best_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weight_proportional() {
        let mut wrr = WeightedRoundRobin::new();
        let backends = vec![
            Backend::new("a", 5),
            Backend::new("b", 3),
            Backend::new("c", 2),
        ];
        let mut counts = [0u32; 3];
        for _ in 0..100 {
            let idx = wrr.pick(&backends).unwrap();
            if let Some(c) = counts.get_mut(idx) {
                *c += 1;
            }
        }
        assert_eq!(counts.first().copied().unwrap_or(0), 50);
        assert_eq!(counts.get(1).copied().unwrap_or(0), 30);
        assert_eq!(counts.get(2).copied().unwrap_or(0), 20);
    }

    #[test]
    fn test_all_zero_weight() {
        let mut wrr = WeightedRoundRobin::new();
        let backends = vec![Backend::new("a", 0), Backend::new("b", 0)];
        assert!(wrr.pick(&backends).is_err());
    }

    #[test]
    #[allow(clippy::similar_names)]
    fn test_backend_removal_resets_state() {
        let mut wrr = WeightedRoundRobin::new();

        // Phase 1: backends [A, B, C] with weights [5, 1, 1]
        let backends_abc = vec![
            Backend::new("A", 5),
            Backend::new("B", 1),
            Backend::new("C", 1),
        ];
        for _ in 0..20 {
            let idx = wrr.pick(&backends_abc).unwrap();
            assert!(idx < 3);
        }

        // Phase 2: remove B, now [A, C] with weights [5, 1]
        let backends_ac = vec![Backend::new("A", 5), Backend::new("C", 1)];
        let mut counts = [0u32; 2];
        for _ in 0..60 {
            let idx = wrr.pick(&backends_ac).unwrap();
            assert!(idx < 2, "index {idx} out of range for 2 backends");
            if let Some(c) = counts.get_mut(idx) {
                *c += 1;
            }
        }

        // With weights [5, 1], A should get 50/60 and C should get 10/60.
        assert_eq!(
            counts.first().copied().unwrap_or(0),
            50,
            "A (weight 5) should get 50 of 60 picks"
        );
        assert_eq!(
            counts.get(1).copied().unwrap_or(0),
            10,
            "C (weight 1) should get 10 of 60 picks"
        );
    }
}
